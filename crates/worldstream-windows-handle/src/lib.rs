//! One narrowly audited safe wrapper around Windows retained-handle rename.
//!
//! Product crates keep `unsafe_code = "forbid"`. This platform shim denies
//! unsafe code everywhere except the single FFI implementation below, whose
//! safe API requires an already-open caller-owned [`std::fs::File`].

#![deny(unsafe_code)]

#[cfg(windows)]
mod windows {
    use std::{
        ffi::OsStr,
        fs::{self, File},
        io,
        mem::{MaybeUninit, offset_of, size_of},
        os::windows::{
            ffi::OsStrExt as _,
            fs::OpenOptionsExt as _,
            io::{AsRawHandle as _, FromRawHandle as _},
        },
        path::{Component, Path},
        ptr,
    };

    use windows_sys::Win32::{
        Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE, INVALID_HANDLE_VALUE},
        Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
            FILE_RENAME_INFO, FILE_SHARE_READ, FILE_SHARE_WRITE, FileRenameInfo,
            GetFileInformationByHandle, SetFileInformationByHandle,
        },
        System::{
            Console::{GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE},
            Threading::GetCurrentProcess,
        },
    };

    const MAX_EXTENDED_PATH_UTF16_UNITS: usize = 32_767;

    /// A process standard handle selected without exposing a raw Win32 value.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum StandardHandle {
        /// Standard input.
        Stdin,
        /// Standard output.
        Stdout,
        /// Standard error.
        Stderr,
    }

    /// Duplicates one inherited standard handle into an independently owned
    /// non-inheritable [`File`].
    ///
    /// `GetStdHandle` returns a borrowed process-table value that must never be
    /// closed by this wrapper. `DuplicateHandle` creates a new kernel handle;
    /// ownership of only that duplicate is transferred to `File`, whose drop
    /// closes it exactly once. Null and `INVALID_HANDLE_VALUE` are rejected
    /// before duplication.
    ///
    /// # Errors
    ///
    /// Returns `NotFound` when the selected process standard handle is null,
    /// or the exact Windows error when lookup or duplication fails.
    #[allow(unsafe_code)]
    pub fn duplicate_standard_handle(which: StandardHandle) -> io::Result<File> {
        let identifier = match which {
            StandardHandle::Stdin => STD_INPUT_HANDLE,
            StandardHandle::Stdout => STD_OUTPUT_HANDLE,
            StandardHandle::Stderr => STD_ERROR_HANDLE,
        };
        // SAFETY: `identifier` is one of the three documented STD_* constants.
        // The returned handle is borrowed from the process handle table and is
        // only inspected and passed as a source to DuplicateHandle below.
        let borrowed = unsafe { GetStdHandle(identifier) };
        if borrowed.is_null() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "requested standard handle is not installed",
            ));
        }
        if borrowed == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }

        let mut duplicate: HANDLE = ptr::null_mut();
        // SAFETY: GetCurrentProcess returns a process pseudo-handle valid in
        // this process. `borrowed` was checked above. `duplicate` points to live
        // writable storage for the synchronous call. Inheritance is disabled
        // and SAME_ACCESS asks the kernel to preserve the redirected handle's
        // exact access mask. On success, the new handle is uniquely owned here.
        if unsafe {
            let process = GetCurrentProcess();
            DuplicateHandle(
                process,
                borrowed,
                process,
                &raw mut duplicate,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        if duplicate.is_null() || duplicate == INVALID_HANDLE_VALUE {
            return Err(io::Error::other(
                "DuplicateHandle returned an invalid successful result",
            ));
        }
        // SAFETY: DuplicateHandle created `duplicate` for this caller and no
        // other Rust owner exists. File assumes exactly that ownership and will
        // close the kernel handle once when dropped.
        Ok(unsafe { File::from_raw_handle(duplicate) })
    }

    /// Atomically renames the exact file referenced by `file` and fails when
    /// `destination` already exists.
    ///
    /// The retained handle must have been opened with `DELETE` access. Its
    /// sharing policy remains in force for the entire kernel operation, so the
    /// source cannot be substituted between validation and rename. The classic
    /// `FileRenameInfo` structure uses `ReplaceIfExists = FALSE`, giving an
    /// atomic no-replace result on supported NTFS and `ReFS` volumes.
    ///
    /// # Errors
    ///
    /// Returns `InvalidInput` for a non-absolute, empty, NUL-containing, or
    /// overlong destination and otherwise returns the exact Windows I/O error.
    pub fn rename_noreplace(file: &File, destination: &Path) -> io::Result<()> {
        if !destination.is_absolute() || destination.as_os_str().is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "destination must be an absolute nonempty path",
            ));
        }
        let name = destination.as_os_str().encode_wide().collect::<Vec<_>>();
        if name.is_empty() || name.contains(&0) || name.len() > MAX_EXTENDED_PATH_UTF16_UNITS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "destination contains an invalid or overlong Windows path",
            ));
        }
        rename_noreplace_ffi(file, None, &name)
    }

    /// Opens and retains a directory authority that cannot be renamed or
    /// deleted while the returned handle is alive.
    ///
    /// The handle deliberately omits `FILE_SHARE_DELETE`. It also opens the
    /// named object itself rather than following a reparse point. Callers must
    /// still enforce their owner-only policy and bind the returned file ID to
    /// the admitted pathname.
    ///
    /// # Errors
    ///
    /// Returns the exact open or metadata error, and `InvalidInput` when the
    /// opened object is not a directory.
    pub fn open_pinned_directory(path: &Path) -> io::Result<File> {
        let directory = fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)?;
        if !directory.metadata()?.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "pinned publication parent is not a directory",
            ));
        }
        Ok(directory)
    }

    /// Returns the kernel-maintained hard-link count for an already-open file.
    ///
    /// This avoids the unstable `std::os::windows::fs::MetadataExt` link-count
    /// accessor while preserving the caller's retained-handle authority.
    ///
    /// # Errors
    ///
    /// Returns the exact Windows error when the handle cannot be queried.
    #[allow(unsafe_code)]
    pub fn hard_link_count(file: &File) -> io::Result<u32> {
        let mut information = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
        // SAFETY: `file` owns a valid live handle for the duration of this
        // synchronous call, and `information` provides correctly aligned,
        // writable storage for the documented output structure. The value is
        // read only after the API reports success and therefore initialization.
        if unsafe {
            GetFileInformationByHandle(file.as_raw_handle() as HANDLE, information.as_mut_ptr())
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the successful kernel call above initialized the complete
        // `BY_HANDLE_FILE_INFORMATION` output structure.
        Ok(unsafe { information.assume_init() }.nNumberOfLinks)
    }

    /// Atomically renames `file` beneath the exact retained `parent` and fails
    /// when `name` already exists.
    ///
    /// `name` must be one nonempty relative path component. The kernel resolves
    /// it from `parent` through `FILE_RENAME_INFO.RootDirectory`, so a mutable
    /// absolute parent pathname is never re-resolved during publication.
    ///
    /// # Errors
    ///
    /// Returns `InvalidInput` for an empty, nested, NUL-containing, or overlong
    /// name and otherwise returns the exact Windows rename error.
    pub fn rename_noreplace_at(file: &File, parent: &File, name: &OsStr) -> io::Result<()> {
        let mut components = Path::new(name).components();
        if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "destination name must be one relative path component",
            ));
        }
        let name = name.encode_wide().collect::<Vec<_>>();
        if name.is_empty() || name.contains(&0) || name.len() > MAX_EXTENDED_PATH_UTF16_UNITS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "destination contains an invalid or overlong Windows name",
            ));
        }
        rename_noreplace_ffi(file, Some(parent), &name)
    }

    #[allow(unsafe_code)]
    fn rename_noreplace_ffi(file: &File, parent: Option<&File>, name: &[u16]) -> io::Result<()> {
        let name_bytes = name
            .len()
            .checked_mul(size_of::<u16>())
            .and_then(|length| u32::try_from(length).ok())
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "destination path is too long")
            })?;
        let file_name_offset = offset_of!(FILE_RENAME_INFO, FileName);
        let buffer_bytes = file_name_offset
            .checked_add(name_bytes as usize)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "rename buffer is too large")
            })?;
        let word_bytes = size_of::<usize>();
        let word_count = buffer_bytes
            .checked_add(word_bytes - 1)
            .map(|bytes| bytes / word_bytes)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "rename buffer is too large")
            })?;
        let mut storage = vec![0_usize; word_count];
        let info = storage.as_mut_ptr().cast::<FILE_RENAME_INFO>();

        // SAFETY: `storage` is nonempty, usize-aligned, zero-initialized, and
        // sized for the fixed FILE_RENAME_INFO prefix plus every UTF-16 code
        // unit. `name` remains alive for the copy. RootDirectory is either null
        // for an absolute name or the live caller-owned directory handle for a
        // relative name. The byte length excludes a terminator as required and
        // the union is written as ReplaceIfExists=FALSE. Both borrowed files
        // own valid live Windows handles for the synchronous kernel call; no
        // raw handle escapes.
        unsafe {
            (*info).Anonymous.ReplaceIfExists = false;
            (*info).RootDirectory = parent.map_or(ptr::null_mut(), |directory| {
                directory.as_raw_handle() as HANDLE
            });
            (*info).FileNameLength = name_bytes;
            ptr::copy_nonoverlapping(
                name.as_ptr(),
                ptr::addr_of_mut!((*info).FileName).cast::<u16>(),
                name.len(),
            );
            let buffer_size = u32::try_from(buffer_bytes).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, "rename buffer is too large")
            })?;
            if SetFileInformationByHandle(
                file.as_raw_handle() as HANDLE,
                FileRenameInfo,
                info.cast(),
                buffer_size,
            ) == 0
            {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::{
            StandardHandle, duplicate_standard_handle, open_pinned_directory, rename_noreplace,
            rename_noreplace_at,
        };
        use std::{
            ffi::OsStr,
            fs,
            io::Write as _,
            os::windows::{fs::OpenOptionsExt as _, io::AsRawHandle as _},
        };
        use windows_sys::Win32::{
            Foundation::{GENERIC_READ, GENERIC_WRITE},
            Storage::FileSystem::{DELETE, FILE_SHARE_READ},
        };

        #[test]
        fn retained_handle_rename_is_no_replace_and_moves_exact_bytes() {
            let directory = tempfile::tempdir()
                .unwrap_or_else(|error| unreachable!("temporary directory: {error}"));
            let source = directory.path().join("source");
            let destination = directory.path().join("destination");
            let mut file = fs::OpenOptions::new()
                .access_mode(GENERIC_READ | GENERIC_WRITE | DELETE)
                .share_mode(FILE_SHARE_READ)
                .create_new(true)
                .open(&source)
                .unwrap_or_else(|error| unreachable!("source: {error}"));
            file.write_all(b"exact")
                .unwrap_or_else(|error| unreachable!("source bytes: {error}"));
            fs::write(&destination, b"sentinel")
                .unwrap_or_else(|error| unreachable!("sentinel: {error}"));
            assert!(rename_noreplace(&file, &destination).is_err());
            assert_eq!(
                fs::read(&destination)
                    .unwrap_or_else(|error| unreachable!("sentinel bytes: {error}")),
                b"sentinel"
            );
            fs::remove_file(&destination)
                .unwrap_or_else(|error| unreachable!("remove sentinel: {error}"));
            rename_noreplace(&file, &destination)
                .unwrap_or_else(|error| unreachable!("retained-handle rename: {error}"));
            assert!(!source.exists());
            assert_eq!(
                fs::read(&destination)
                    .unwrap_or_else(|error| unreachable!("renamed bytes: {error}")),
                b"exact"
            );
        }

        #[test]
        fn retained_handle_blocks_source_and_destination_substitution() {
            let directory = tempfile::tempdir()
                .unwrap_or_else(|error| unreachable!("temporary directory: {error}"));
            let source = directory.path().join("source");
            let moved_source = directory.path().join("moved-source");
            let destination = directory.path().join("destination");
            let moved_destination = directory.path().join("moved-destination");
            let mut file = fs::OpenOptions::new()
                .access_mode(GENERIC_READ | GENERIC_WRITE | DELETE)
                .share_mode(FILE_SHARE_READ)
                .create_new(true)
                .open(&source)
                .unwrap_or_else(|error| unreachable!("source: {error}"));
            file.write_all(b"exact")
                .unwrap_or_else(|error| unreachable!("source bytes: {error}"));

            assert!(fs::rename(&source, &moved_source).is_err());
            assert!(fs::remove_file(&source).is_err());
            assert!(fs::OpenOptions::new().write(true).open(&source).is_err());

            rename_noreplace(&file, &destination)
                .unwrap_or_else(|error| unreachable!("retained-handle rename: {error}"));
            assert!(!source.exists());
            assert!(fs::rename(&destination, &moved_destination).is_err());
            assert!(fs::remove_file(&destination).is_err());
            assert!(
                fs::OpenOptions::new()
                    .write(true)
                    .open(&destination)
                    .is_err()
            );

            drop(file);
            assert_eq!(
                fs::read(&destination)
                    .unwrap_or_else(|error| unreachable!("renamed bytes: {error}")),
                b"exact"
            );
        }

        #[test]
        fn overlong_destination_is_rejected_before_the_kernel_call() {
            let source = tempfile::tempfile()
                .unwrap_or_else(|error| unreachable!("temporary source: {error}"));
            let destination = std::path::PathBuf::from(format!(
                r"C:\{}",
                "x".repeat(super::MAX_EXTENDED_PATH_UTF16_UNITS)
            ));
            let error = match rename_noreplace(&source, &destination) {
                Ok(()) => unreachable!("an overlong extended Windows path was accepted"),
                Err(error) => error,
            };
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        }

        #[test]
        fn parent_relative_rename_pins_parent_and_preserves_no_clobber() {
            let root = tempfile::tempdir()
                .unwrap_or_else(|error| unreachable!("temporary directory: {error}"));
            let admitted = root.path().join("admitted");
            let held = root.path().join("held");
            fs::create_dir(&admitted)
                .unwrap_or_else(|error| unreachable!("create admitted parent: {error}"));
            let parent = open_pinned_directory(&admitted)
                .unwrap_or_else(|error| unreachable!("pin admitted parent: {error}"));
            assert!(fs::rename(&admitted, &held).is_err());

            let source_path = admitted.join("source");
            let mut staging = fs::OpenOptions::new()
                .access_mode(GENERIC_READ | GENERIC_WRITE)
                .share_mode(
                    FILE_SHARE_READ
                        | windows_sys::Win32::Storage::FileSystem::FILE_SHARE_WRITE
                        | windows_sys::Win32::Storage::FileSystem::FILE_SHARE_DELETE,
                )
                .create_new(true)
                .open(&source_path)
                .unwrap_or_else(|error| unreachable!("source: {error}"));
            staging
                .write_all(b"exact")
                .unwrap_or_else(|error| unreachable!("source bytes: {error}"));

            let sqlite_like = fs::OpenOptions::new()
                .access_mode(GENERIC_READ | GENERIC_WRITE)
                .share_mode(
                    FILE_SHARE_READ | windows_sys::Win32::Storage::FileSystem::FILE_SHARE_WRITE,
                )
                .open(&source_path)
                .unwrap_or_else(|error| unreachable!("compatible SQLite-like handle: {error}"));
            drop(sqlite_like);

            let source = fs::OpenOptions::new()
                .access_mode(GENERIC_READ | GENERIC_WRITE | DELETE)
                .share_mode(
                    FILE_SHARE_READ | windows_sys::Win32::Storage::FileSystem::FILE_SHARE_WRITE,
                )
                .open(&source_path)
                .unwrap_or_else(|error| unreachable!("publication handle: {error}"));
            drop(staging);

            fs::write(admitted.join("destination"), b"sentinel")
                .unwrap_or_else(|error| unreachable!("sentinel: {error}"));
            assert!(rename_noreplace_at(&source, &parent, OsStr::new("destination")).is_err());
            fs::remove_file(admitted.join("destination"))
                .unwrap_or_else(|error| unreachable!("remove sentinel: {error}"));
            rename_noreplace_at(&source, &parent, OsStr::new("destination"))
                .unwrap_or_else(|error| unreachable!("relative rename: {error}"));
            assert_eq!(
                fs::read(admitted.join("destination"))
                    .unwrap_or_else(|error| unreachable!("destination bytes: {error}")),
                b"exact"
            );
        }

        #[test]
        fn redirected_standard_handles_are_duplicated_into_independent_files() {
            let stdout = duplicate_standard_handle(StandardHandle::Stdout)
                .unwrap_or_else(|error| unreachable!("duplicate stdout: {error}"));
            let stderr = duplicate_standard_handle(StandardHandle::Stderr)
                .unwrap_or_else(|error| unreachable!("duplicate stderr: {error}"));
            assert_ne!(stdout.as_raw_handle(), stderr.as_raw_handle());
            drop(stdout);
            drop(stderr);
        }
    }
}

#[cfg(windows)]
pub use windows::{
    StandardHandle, duplicate_standard_handle, hard_link_count, open_pinned_directory,
    rename_noreplace, rename_noreplace_at,
};
