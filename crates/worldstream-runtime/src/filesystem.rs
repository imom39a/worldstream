use std::{
    fs, io,
    path::{Path, PathBuf},
};

use thiserror::Error;

#[cfg(unix)]
type OwnerOnlyFileIdentity = (u64, u64);

#[cfg(windows)]
type OwnerOnlyFileIdentity = fs_id::FileID;

#[cfg(unix)]
fn owner_only_file_identity(file: &fs::File) -> io::Result<OwnerOnlyFileIdentity> {
    use std::os::unix::fs::MetadataExt as _;

    file.metadata()
        .map(|metadata| (metadata.dev(), metadata.ino()))
}

#[cfg(windows)]
fn owner_only_file_identity(file: &fs::File) -> io::Result<OwnerOnlyFileIdentity> {
    fs_id::FileID::new(file)
}

fn cleanup_failed_owner_only_file(
    file: fs::File,
    _identity: Option<&OwnerOnlyFileIdentity>,
    _path: &Path,
) {
    // Scrub through the retained handle before releasing its pathname lock.
    // Intentionally leave the resulting empty, protected placeholder on every
    // platform: compare-then-unlink is not atomic and could delete a
    // same-service replacement after the identity check.
    let _ = file.set_len(0);
    let _ = file.sync_all();
    drop(file);
}

/// Creates (when absent) and validates the WorldStream-owned data directory.
///
/// POSIX validation requires the effective user to own a non-symlink directory
/// with mode 0700. Windows creates a protected DACL and verifies that only the
/// current owner/service identity, SYSTEM, and Administrators have access.
///
/// # Errors
///
/// Returns an error for an unsafe target, wrong type, symlink, wrong owner or
/// permissions, unsupported platform verification, or filesystem failure.
pub fn prepare_data_directory(path: &Path) -> Result<PathBuf, FilesystemError> {
    if path.as_os_str().is_empty() {
        return Err(FilesystemError::UnsafePath {
            path: path.to_path_buf(),
            reason: "path is empty",
        });
    }

    #[cfg(unix)]
    {
        prepare_data_directory_unix(path)
    }

    #[cfg(windows)]
    {
        prepare_data_directory_windows(path)
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        Err(FilesystemError::UnsupportedPlatform)
    }
}

/// Prepares the one canonical Studio live-backup root associated with a
/// daemon data directory. Both the daemon and Supervisor must use this exact
/// helper so the artifact owner and durable operation owner cannot diverge.
///
/// # Errors
///
/// Returns an error when the data directory has no parent or the derived
/// owner-only backup root cannot be securely prepared.
pub fn prepare_live_backup_root(data_directory: &Path) -> Result<PathBuf, FilesystemError> {
    let data_directory = prepare_data_directory(data_directory)?;
    let parent = data_directory
        .parent()
        .ok_or_else(|| FilesystemError::UnsafePath {
            path: data_directory.clone(),
            reason: "data directory has no parent",
        })?;
    prepare_data_directory(&parent.join("studio/backups"))
}

/// Validates a secret file without reading or logging its contents.
///
/// # Errors
///
/// Returns an error unless the path is a non-symlink regular file owned and
/// readable only by the effective user, or when safe platform checks are absent.
pub fn validate_owner_only_file(path: &Path) -> Result<(), FilesystemError> {
    if path.as_os_str().is_empty() {
        return Err(FilesystemError::UnsafePath {
            path: path.to_path_buf(),
            reason: "path is empty",
        });
    }

    #[cfg(unix)]
    {
        validate_owner_only_file_unix(path)
    }

    #[cfg(windows)]
    {
        validate_owner_only_file_windows(path)
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        Err(FilesystemError::UnsupportedPlatform)
    }
}

/// Exclusively creates a new owner-only file inside a prepared data directory.
///
/// The parent must already satisfy [`prepare_data_directory`]. On Windows the
/// protected parent DACL constrains access at `CREATE_NEW`; the new file is
/// then normalized to, and verified against, the exact protected owner,
/// SYSTEM, and Administrators DACL. POSIX creates the file with mode 0600 and
/// verifies ownership and permissions before returning it.
///
/// # Errors
///
/// Returns an error when the parent is unsafe, the path already exists, the
/// file cannot be created exclusively, or the resulting owner/permissions do
/// not match the platform policy.
pub fn create_owner_only_file(path: &Path) -> Result<fs::File, FilesystemError> {
    create_owner_only_file_with_policy(path, false)
}

/// Exclusively creates an owner-only file whose Windows handle has the DELETE
/// access required for an exact retained-handle rename. The handle still
/// shares reads only, so other processes cannot rename, delete, replace, or
/// open the file for writes. POSIX behavior is identical to
/// [`create_owner_only_file`].
///
/// # Errors
///
/// Returns the same closed filesystem errors as [`create_owner_only_file`].
pub fn create_owner_only_renameable_file(path: &Path) -> Result<fs::File, FilesystemError> {
    create_owner_only_file_with_policy(path, true)
}

fn create_owner_only_file_with_policy(
    path: &Path,
    windows_delete_access: bool,
) -> Result<fs::File, FilesystemError> {
    #[cfg(not(windows))]
    let _ = windows_delete_access;
    if path.as_os_str().is_empty() {
        return Err(FilesystemError::UnsafePath {
            path: path.to_path_buf(),
            reason: "path is empty",
        });
    }
    let parent = path.parent().ok_or_else(|| FilesystemError::UnsafePath {
        path: path.to_path_buf(),
        reason: "owner-only file must have a parent directory",
    })?;
    let prepared_parent = prepare_data_directory(parent)?;
    let file_name = path
        .file_name()
        .ok_or_else(|| FilesystemError::UnsafePath {
            path: path.to_path_buf(),
            reason: "owner-only file must have a file name",
        })?;
    let protected_path = prepared_parent.join(file_name);

    #[cfg(unix)]
    let file = {
        use std::os::unix::fs::OpenOptionsExt as _;

        fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&protected_path)
            .map_err(|source| FilesystemError::Io {
                path: protected_path.clone(),
                source,
            })?
    };

    #[cfg(windows)]
    let file = {
        use std::os::windows::fs::OpenOptionsExt as _;
        use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE};
        use windows_sys::Win32::Storage::FileSystem::{DELETE, FILE_SHARE_READ};

        // Denying delete sharing binds this pathname to the returned handle
        // for its lifetime. Provider children may reopen it for reads, but a
        // concurrent rename/delete/replacement is rejected by the kernel.
        let mut options = fs::OpenOptions::new();
        options.create_new(true).share_mode(FILE_SHARE_READ);
        if windows_delete_access {
            options.access_mode(GENERIC_READ | GENERIC_WRITE | DELETE);
        } else {
            options.read(true).write(true);
        }
        options
            .open(&protected_path)
            .map_err(|source| FilesystemError::Io {
                path: protected_path.clone(),
                source,
            })?
    };

    #[cfg(not(any(unix, windows)))]
    {
        let _ = protected_path;
        return Err(FilesystemError::UnsupportedPlatform);
    }

    let created_identity = match owner_only_file_identity(&file) {
        Ok(identity) => identity,
        Err(source) => {
            let error = FilesystemError::Io {
                path: protected_path.clone(),
                source,
            };
            cleanup_failed_owner_only_file(file, None, &protected_path);
            return Err(error);
        }
    };

    #[cfg(windows)]
    {
        let secure_result =
            windows_current_identity_sid(Some(&prepared_parent)).and_then(|current_sid| {
                apply_windows_owner_only_acl(&protected_path, &current_sid, false)
                    .and_then(|()| validate_owner_only_file(&protected_path))
            });
        if let Err(error) = secure_result {
            cleanup_failed_owner_only_file(file, Some(&created_identity), &protected_path);
            return Err(error);
        }
    }

    #[cfg(unix)]
    if let Err(error) = validate_owner_only_file(&protected_path) {
        cleanup_failed_owner_only_file(file, Some(&created_identity), &protected_path);
        return Err(error);
    }

    Ok(file)
}

/// Verifies that a prepared `SQLite` data directory uses the platform's
/// frozen local durable-filesystem allowlist.
///
/// Linux accepts only an exact ext4 or XFS mount identity, Windows accepts
/// only a fixed NTFS or `ReFS` volume, and the macOS source-development path
/// accepts only APFS. Unknown or unobservable identities fail closed.
///
/// # Errors
///
/// Returns an error when the directory cannot be resolved, its filesystem
/// identity cannot be observed, or that identity is outside the allowlist.
pub fn validate_sqlite_data_filesystem(path: &Path) -> Result<(), FilesystemError> {
    let canonical = fs::canonicalize(path).map_err(|source| FilesystemError::Io {
        path: path.to_path_buf(),
        source,
    })?;

    #[cfg(target_os = "linux")]
    {
        validate_linux_sqlite_filesystem(&canonical)
    }

    #[cfg(target_os = "macos")]
    {
        validate_macos_sqlite_filesystem(&canonical)
    }

    #[cfg(windows)]
    {
        validate_windows_sqlite_filesystem(&canonical)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        let _ = canonical;
        Err(FilesystemError::UnsupportedPlatform)
    }
}

#[cfg(target_os = "linux")]
fn validate_linux_sqlite_filesystem(path: &Path) -> Result<(), FilesystemError> {
    const MOUNTINFO: &str = "/proc/self/mountinfo";
    const MAX_MOUNTINFO_BYTES: u64 = 4 * 1024 * 1024;

    let metadata =
        fs::metadata(MOUNTINFO).map_err(|_| FilesystemError::FilesystemIdentityUnavailable {
            path: path.to_path_buf(),
        })?;
    if !metadata.is_file() || metadata.len() > MAX_MOUNTINFO_BYTES {
        return Err(FilesystemError::FilesystemIdentityUnavailable {
            path: path.to_path_buf(),
        });
    }
    let mountinfo = fs::read_to_string(MOUNTINFO).map_err(|_| {
        FilesystemError::FilesystemIdentityUnavailable {
            path: path.to_path_buf(),
        }
    })?;
    let filesystem = linux_mount_filesystem(path, &mountinfo).ok_or_else(|| {
        FilesystemError::FilesystemIdentityUnavailable {
            path: path.to_path_buf(),
        }
    })?;
    if matches!(filesystem.as_str(), "ext4" | "xfs") {
        Ok(())
    } else {
        Err(FilesystemError::UnsupportedFilesystem {
            path: path.to_path_buf(),
            actual: filesystem,
            expected: "ext4 or xfs",
        })
    }
}

#[cfg(target_os = "linux")]
fn linux_mount_filesystem(path: &Path, mountinfo: &str) -> Option<String> {
    use std::os::unix::ffi::OsStringExt;

    fn decode(value: &str) -> Option<PathBuf> {
        let source = value.as_bytes();
        let mut decoded = Vec::with_capacity(source.len());
        let mut index = 0;
        while index < source.len() {
            if source[index] == b'\\' {
                if index + 3 >= source.len()
                    || !source[index + 1..=index + 3].iter().all(u8::is_ascii_digit)
                {
                    return None;
                }
                let octal =
                    source[index + 1..=index + 3]
                        .iter()
                        .try_fold(0_u8, |value, digit| {
                            if *digit > b'7' {
                                None
                            } else {
                                value.checked_mul(8)?.checked_add(*digit - b'0')
                            }
                        })?;
                decoded.push(octal);
                index += 4;
            } else {
                decoded.push(source[index]);
                index += 1;
            }
        }
        Some(PathBuf::from(std::ffi::OsString::from_vec(decoded)))
    }

    let mut selected: Option<(usize, String)> = None;
    for line in mountinfo.lines() {
        let (left, right) = line.split_once(" - ")?;
        let left_fields = left.split_ascii_whitespace().collect::<Vec<_>>();
        let right_fields = right.split_ascii_whitespace().collect::<Vec<_>>();
        if left_fields.len() < 6 || right_fields.len() < 3 {
            return None;
        }
        let mount_point = decode(left_fields[4])?;
        if !path.starts_with(&mount_point) {
            continue;
        }
        let specificity = mount_point.components().count();
        if selected
            .as_ref()
            .is_none_or(|(current, _)| specificity > *current)
        {
            selected = Some((specificity, right_fields[0].to_owned()));
        }
    }
    selected.map(|(_, filesystem)| filesystem)
}

#[cfg(target_os = "macos")]
fn validate_macos_sqlite_filesystem(path: &Path) -> Result<(), FilesystemError> {
    const MAX_MOUNT_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
    let completed = std::process::Command::new("/sbin/mount")
        .output()
        .map_err(|_| FilesystemError::FilesystemIdentityUnavailable {
            path: path.to_path_buf(),
        })?;
    let output = String::from_utf8(completed.stdout)
        .ok()
        .filter(|value| {
            completed.status.success() && !value.is_empty() && value.len() <= MAX_MOUNT_OUTPUT_BYTES
        })
        .ok_or_else(|| FilesystemError::FilesystemIdentityUnavailable {
            path: path.to_path_buf(),
        })?;
    let filesystem = macos_mount_filesystem(path, &output).ok_or_else(|| {
        FilesystemError::FilesystemIdentityUnavailable {
            path: path.to_path_buf(),
        }
    })?;
    if filesystem == "apfs" {
        Ok(())
    } else {
        Err(FilesystemError::UnsupportedFilesystem {
            path: path.to_path_buf(),
            actual: filesystem,
            expected: "apfs (macOS source-development only)",
        })
    }
}

#[cfg(target_os = "macos")]
fn macos_mount_filesystem(path: &Path, mount_output: &str) -> Option<String> {
    let mut selected: Option<(usize, String)> = None;
    for line in mount_output.lines() {
        let (_, mounted) = line.rsplit_once(" on ")?;
        let (mount_point, details) = mounted.split_once(" (")?;
        let details = details.strip_suffix(')')?;
        let filesystem = details.split(',').next()?.trim().to_ascii_lowercase();
        if filesystem.is_empty() {
            return None;
        }
        let mount_point = Path::new(mount_point);
        if !path.starts_with(mount_point) {
            continue;
        }
        let specificity = mount_point.components().count();
        if selected
            .as_ref()
            .is_none_or(|(current, _)| specificity > *current)
        {
            selected = Some((specificity, filesystem));
        }
    }
    selected.map(|(_, filesystem)| filesystem)
}

#[cfg(windows)]
fn validate_windows_sqlite_filesystem(path: &Path) -> Result<(), FilesystemError> {
    validate_windows_resolved_local_data_path(path)?;
    let path_text =
        path.to_str()
            .ok_or_else(|| FilesystemError::FilesystemIdentityUnavailable {
                path: path.to_path_buf(),
            })?;
    let volume = winsafe::GetVolumePathName(path_text).map_err(|_| {
        FilesystemError::FilesystemIdentityUnavailable {
            path: path.to_path_buf(),
        }
    })?;
    let mut filesystem = String::new();
    winsafe::GetVolumeInformation(Some(&volume), None, None, None, None, Some(&mut filesystem))
        .map_err(|_| FilesystemError::FilesystemIdentityUnavailable {
            path: path.to_path_buf(),
        })?;
    if windows_sqlite_filesystem_is_supported(&filesystem) {
        Ok(())
    } else {
        Err(FilesystemError::UnsupportedFilesystem {
            path: path.to_path_buf(),
            actual: filesystem.to_ascii_lowercase(),
            expected: "ntfs or refs",
        })
    }
}

#[cfg(windows)]
fn windows_sqlite_filesystem_is_supported(value: &str) -> bool {
    value.eq_ignore_ascii_case("ntfs") || value.eq_ignore_ascii_case("refs")
}

#[cfg(unix)]
fn prepare_data_directory_unix(path: &Path) -> Result<PathBuf, FilesystemError> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

    let absolute = absolute_path(path)?;
    if absolute.parent().is_none() {
        return Err(FilesystemError::UnsafePath {
            path: absolute,
            reason: "filesystem root cannot be a WorldStream data directory",
        });
    }
    validate_unix_path_components(&absolute)?;

    match fs::symlink_metadata(&absolute) {
        Ok(metadata) => {
            validate_unix_directory_metadata(&absolute, &metadata)?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let mut builder = fs::DirBuilder::new();
            builder.recursive(true).mode(0o700);
            builder
                .create(&absolute)
                .map_err(|source| FilesystemError::Io {
                    path: absolute.clone(),
                    source,
                })?;
            fs::set_permissions(&absolute, fs::Permissions::from_mode(0o700)).map_err(
                |source| FilesystemError::Io {
                    path: absolute.clone(),
                    source,
                },
            )?;
            let metadata =
                fs::symlink_metadata(&absolute).map_err(|source| FilesystemError::Io {
                    path: absolute.clone(),
                    source,
                })?;
            validate_unix_directory_metadata(&absolute, &metadata)?;
        }
        Err(source) => {
            return Err(FilesystemError::Io {
                path: absolute,
                source,
            });
        }
    }
    validate_unix_path_components(&absolute)?;

    let canonical = fs::canonicalize(&absolute).map_err(|source| FilesystemError::Io {
        path: absolute,
        source,
    })?;
    let metadata = fs::symlink_metadata(&canonical).map_err(|source| FilesystemError::Io {
        path: canonical.clone(),
        source,
    })?;
    validate_unix_directory_metadata(&canonical, &metadata)?;
    Ok(canonical)
}

#[cfg(unix)]
fn validate_unix_path_components(path: &Path) -> Result<(), FilesystemError> {
    let mut components = path.ancestors().collect::<Vec<_>>();
    components.reverse();
    for component in components {
        match fs::symlink_metadata(component) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink()
                    && !trusted_macos_system_alias(component, &metadata)
                {
                    return Err(FilesystemError::Symlink(component.to_path_buf()));
                }
                if component != path && !metadata.is_dir() && !metadata.file_type().is_symlink() {
                    return Err(FilesystemError::WrongType {
                        path: component.to_path_buf(),
                        expected: "directory",
                    });
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(FilesystemError::Io {
                    path: component.to_path_buf(),
                    source,
                });
            }
        }
    }
    Ok(())
}

#[cfg(all(unix, target_os = "macos"))]
fn trusted_macos_system_alias(path: &Path, metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;

    let expected = match path.to_str() {
        Some("/etc") => Path::new("private/etc"),
        Some("/tmp") => Path::new("private/tmp"),
        Some("/var") => Path::new("private/var"),
        _ => return false,
    };
    metadata.uid() == 0 && fs::read_link(path).is_ok_and(|target| target == expected)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn trusted_macos_system_alias(_path: &Path, _metadata: &fs::Metadata) -> bool {
    false
}

#[cfg(unix)]
fn validate_unix_directory_metadata(
    path: &Path,
    metadata: &fs::Metadata,
) -> Result<(), FilesystemError> {
    use std::os::unix::fs::MetadataExt;

    if metadata.file_type().is_symlink() {
        return Err(FilesystemError::Symlink(path.to_path_buf()));
    }
    if !metadata.is_dir() {
        return Err(FilesystemError::WrongType {
            path: path.to_path_buf(),
            expected: "directory",
        });
    }
    validate_unix_owner(path, metadata.uid())?;
    let mode = metadata.mode() & 0o777;
    if mode != 0o700 {
        return Err(FilesystemError::Permissions {
            path: path.to_path_buf(),
            expected: "0700",
            actual: mode,
        });
    }
    Ok(())
}

#[cfg(unix)]
fn validate_owner_only_file_unix(path: &Path) -> Result<(), FilesystemError> {
    use std::os::unix::fs::MetadataExt;

    let absolute = absolute_path(path)?;
    validate_unix_path_components(&absolute)?;
    let metadata = fs::symlink_metadata(&absolute).map_err(|source| FilesystemError::Io {
        path: absolute.clone(),
        source,
    })?;
    if metadata.file_type().is_symlink() {
        return Err(FilesystemError::Symlink(absolute));
    }
    if !metadata.is_file() {
        return Err(FilesystemError::WrongType {
            path: absolute,
            expected: "regular file",
        });
    }
    validate_unix_owner(&absolute, metadata.uid())?;
    let mode = metadata.mode() & 0o777;
    if mode & 0o077 != 0 || mode & 0o400 == 0 {
        return Err(FilesystemError::Permissions {
            path: absolute,
            expected: "owner-readable with no group/other permissions",
            actual: mode,
        });
    }
    Ok(())
}

#[cfg(unix)]
fn validate_unix_owner(path: &Path, actual: u32) -> Result<(), FilesystemError> {
    let expected = rustix::process::geteuid().as_raw();
    if actual != expected {
        return Err(FilesystemError::Owner {
            path: path.to_path_buf(),
            expected,
            actual,
        });
    }
    Ok(())
}

#[cfg(windows)]
fn prepare_data_directory_windows(path: &Path) -> Result<PathBuf, FilesystemError> {
    validate_windows_local_data_path(path)?;
    let absolute = absolute_path(path)?;
    if absolute.parent().is_none() {
        return Err(FilesystemError::UnsafePath {
            path: absolute,
            reason: "filesystem root cannot be a WorldStream data directory",
        });
    }
    validate_windows_path_components(&absolute)?;

    let newly_created = match fs::symlink_metadata(&absolute) {
        Ok(metadata) => {
            validate_windows_path_type(&absolute, &metadata, true)?;
            false
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            validate_windows_resolved_existing_ancestor(&absolute)?;
            if let Some(parent) = absolute.parent() {
                fs::create_dir_all(parent).map_err(|source| FilesystemError::Io {
                    path: parent.to_path_buf(),
                    source,
                })?;
            }
            match fs::create_dir(&absolute) {
                Ok(()) => true,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => false,
                Err(source) => {
                    return Err(FilesystemError::Io {
                        path: absolute,
                        source,
                    });
                }
            }
        }
        Err(source) => {
            return Err(FilesystemError::Io {
                path: absolute,
                source,
            });
        }
    };

    let metadata = fs::symlink_metadata(&absolute).map_err(|source| FilesystemError::Io {
        path: absolute.clone(),
        source,
    })?;
    validate_windows_path_components(&absolute)?;
    validate_windows_path_type(&absolute, &metadata, true)?;
    let canonical = fs::canonicalize(&absolute).map_err(|source| FilesystemError::Io {
        path: absolute,
        source,
    })?;
    validate_windows_resolved_local_data_path(&canonical)?;
    let metadata = fs::symlink_metadata(&canonical).map_err(|source| FilesystemError::Io {
        path: canonical.clone(),
        source,
    })?;
    validate_windows_path_type(&canonical, &metadata, true)?;
    let current_sid = windows_current_identity_sid(Some(&canonical))?;
    if newly_created {
        apply_windows_owner_only_acl(&canonical, &current_sid, true)?;
    }
    validate_windows_owner_only_acl(&canonical, &current_sid, true)?;
    Ok(canonical)
}

#[cfg(windows)]
fn validate_windows_local_data_path(path: &Path) -> Result<(), FilesystemError> {
    validate_windows_drive_path(path, false)
}

#[cfg(windows)]
fn validate_windows_resolved_local_data_path(path: &Path) -> Result<(), FilesystemError> {
    validate_windows_drive_path(path, true)
}

#[cfg(windows)]
fn validate_windows_drive_path(
    path: &Path,
    allow_canonical_verbatim_disk: bool,
) -> Result<(), FilesystemError> {
    use std::path::{Component, Prefix};

    let mut components = path.components();
    let drive = match (components.next(), components.next()) {
        (Some(Component::Prefix(prefix)), Some(Component::RootDir)) => match prefix.kind() {
            Prefix::Disk(drive) => drive,
            Prefix::VerbatimDisk(drive) if allow_canonical_verbatim_disk => drive,
            Prefix::UNC(_, _)
            | Prefix::VerbatimUNC(_, _)
            | Prefix::VerbatimDisk(_)
            | Prefix::Verbatim(_)
            | Prefix::DeviceNS(_) => {
                return Err(FilesystemError::UnsafePath {
                    path: path.to_path_buf(),
                    reason: "UNC, network, and device-prefixed data paths are forbidden",
                });
            }
        },
        _ => {
            return Err(FilesystemError::UnsafePath {
                path: path.to_path_buf(),
                reason: "Windows data directory must be an absolute local drive path",
            });
        }
    };

    let drive_root = format!("{}:\\", char::from(drive).to_ascii_uppercase());
    match winsafe::GetDriveType(Some(&drive_root)) {
        winsafe::co::DRIVE::FIXED => Ok(()),
        winsafe::co::DRIVE::REMOTE => Err(FilesystemError::UnsafePath {
            path: path.to_path_buf(),
            reason: "network-backed Windows data drives are forbidden",
        }),
        _ => Err(FilesystemError::UnsafePath {
            path: path.to_path_buf(),
            reason: "Windows data directory must use a mounted local fixed drive",
        }),
    }
}

#[cfg(windows)]
fn validate_windows_resolved_existing_ancestor(path: &Path) -> Result<(), FilesystemError> {
    let mut candidate = path.parent();
    while let Some(ancestor) = candidate {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) => {
                validate_windows_path_type(ancestor, &metadata, true)?;
                let canonical =
                    fs::canonicalize(ancestor).map_err(|source| FilesystemError::Io {
                        path: ancestor.to_path_buf(),
                        source,
                    })?;
                return validate_windows_resolved_local_data_path(&canonical);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                candidate = ancestor.parent();
            }
            Err(source) => {
                return Err(FilesystemError::Io {
                    path: ancestor.to_path_buf(),
                    source,
                });
            }
        }
    }

    Err(FilesystemError::UnsafePath {
        path: path.to_path_buf(),
        reason: "Windows data directory has no resolvable local ancestor",
    })
}

#[cfg(windows)]
fn validate_owner_only_file_windows(path: &Path) -> Result<(), FilesystemError> {
    let absolute = absolute_path(path)?;
    validate_windows_path_components(&absolute)?;
    let metadata = fs::symlink_metadata(&absolute).map_err(|source| FilesystemError::Io {
        path: absolute.clone(),
        source,
    })?;
    validate_windows_path_type(&absolute, &metadata, false)?;
    let current_sid = windows_current_identity_sid(None)?;
    validate_windows_owner_only_acl(&absolute, &current_sid, false)
}

#[cfg(windows)]
fn validate_windows_path_components(path: &Path) -> Result<(), FilesystemError> {
    for component in path.ancestors().collect::<Vec<_>>().into_iter().rev() {
        match fs::symlink_metadata(component) {
            Ok(metadata) => {
                use std::os::windows::fs::MetadataExt;
                use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

                if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                    return Err(FilesystemError::ReparsePoint(component.to_path_buf()));
                }
                if component != path && !metadata.is_dir() {
                    return Err(FilesystemError::WrongType {
                        path: component.to_path_buf(),
                        expected: "directory",
                    });
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(FilesystemError::Io {
                    path: component.to_path_buf(),
                    source,
                });
            }
        }
    }
    Ok(())
}

#[cfg(windows)]
fn validate_windows_path_type(
    path: &Path,
    metadata: &fs::Metadata,
    expect_directory: bool,
) -> Result<(), FilesystemError> {
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    let attributes = metadata.file_attributes();
    if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(FilesystemError::ReparsePoint(path.to_path_buf()));
    }
    if !windows_path_kind_matches(
        attributes,
        metadata.is_file(),
        metadata.is_dir(),
        expect_directory,
    ) {
        return Err(FilesystemError::WrongType {
            path: path.to_path_buf(),
            expected: if expect_directory {
                "directory"
            } else {
                "regular file"
            },
        });
    }
    Ok(())
}

#[cfg(windows)]
fn windows_path_kind_matches(
    attributes: u32,
    is_file: bool,
    is_directory: bool,
    expect_directory: bool,
) -> bool {
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_DEVICE, FILE_ATTRIBUTE_DIRECTORY,
    };

    if attributes & FILE_ATTRIBUTE_DEVICE != 0 {
        return false;
    }
    if expect_directory {
        is_directory && attributes & FILE_ATTRIBUTE_DIRECTORY != 0
    } else {
        is_file && !is_directory && attributes & FILE_ATTRIBUTE_DIRECTORY == 0
    }
}

#[cfg(windows)]
fn windows_current_identity_sid(
    probe_directory: Option<&Path>,
) -> Result<windows_permissions::LocalBox<windows_permissions::Sid>, FilesystemError> {
    use windows_permissions::{WindowsSecure, constants::SecurityInformation, wrappers};

    let probe = match probe_directory {
        Some(directory) => tempfile::NamedTempFile::new_in(directory),
        None => tempfile::NamedTempFile::new(),
    }
    .map_err(|source| FilesystemError::Io {
        path: probe_directory.map_or_else(std::env::temp_dir, Path::to_path_buf),
        source,
    })?;
    let descriptor = probe
        .as_file()
        .security_descriptor(SecurityInformation::Owner)
        .map_err(|source| FilesystemError::Io {
            path: probe.path().to_path_buf(),
            source,
        })?;
    let owner = descriptor
        .owner()
        .ok_or_else(|| FilesystemError::WindowsAcl {
            path: probe.path().to_path_buf(),
            reason: "temporary identity probe has no owner SID",
        })?;
    wrappers::CopySid(owner).map_err(|source| FilesystemError::Io {
        path: probe.path().to_path_buf(),
        source,
    })
}

#[cfg(windows)]
fn apply_windows_owner_only_acl(
    path: &Path,
    owner: &windows_permissions::Sid,
    directory: bool,
) -> Result<(), FilesystemError> {
    use windows_permissions::{
        LocalBox, SecurityDescriptor,
        constants::{SeObjectType, SecurityInformation},
        wrappers,
    };

    let inheritance = if directory { "OICI" } else { "" };
    let sddl = format!(
        "D:P(A;{inheritance};FA;;;{owner})(A;{inheritance};FA;;;SY)(A;{inheritance};FA;;;BA)"
    );
    let descriptor: LocalBox<SecurityDescriptor> =
        sddl.parse().map_err(|source| FilesystemError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    let dacl = descriptor
        .dacl()
        .ok_or_else(|| FilesystemError::WindowsAcl {
            path: path.to_path_buf(),
            reason: "constructed owner-only descriptor has no DACL",
        })?;
    wrappers::SetNamedSecurityInfo(
        path,
        SeObjectType::SE_FILE_OBJECT,
        SecurityInformation::Dacl | SecurityInformation::ProtectedDacl,
        None,
        None,
        Some(dacl),
        None,
    )
    .map_err(|source| FilesystemError::Io {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(windows)]
fn validate_windows_owner_only_acl(
    path: &Path,
    expected_owner: &windows_permissions::Sid,
    directory: bool,
) -> Result<(), FilesystemError> {
    use windows_permissions::{
        constants::{SeObjectType, SecurityInformation},
        wrappers,
    };

    let descriptor = wrappers::GetNamedSecurityInfo(
        path,
        SeObjectType::SE_FILE_OBJECT,
        SecurityInformation::Owner | SecurityInformation::Dacl,
    )
    .map_err(|source| FilesystemError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let owner = descriptor
        .owner()
        .ok_or_else(|| FilesystemError::WindowsAcl {
            path: path.to_path_buf(),
            reason: "security descriptor has no owner SID",
        })?;
    if owner != expected_owner {
        return Err(FilesystemError::WindowsAcl {
            path: path.to_path_buf(),
            reason: "owner SID differs from the current process/service identity",
        });
    }

    let sddl = wrappers::ConvertSecurityDescriptorToStringSecurityDescriptor(
        &descriptor,
        SecurityInformation::Dacl,
    )
    .map_err(|source| FilesystemError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if !sddl.to_string_lossy().starts_with("D:P") {
        return Err(FilesystemError::WindowsAcl {
            path: path.to_path_buf(),
            reason: "DACL inheritance is not protected",
        });
    }

    let dacl = descriptor
        .dacl()
        .ok_or_else(|| FilesystemError::WindowsAcl {
            path: path.to_path_buf(),
            reason: "security descriptor has no DACL",
        })?;
    if dacl.len() != 3 {
        return Err(FilesystemError::WindowsAcl {
            path: path.to_path_buf(),
            reason: "DACL must contain exactly owner, SYSTEM, and Administrators entries",
        });
    }

    validate_windows_dacl_entries(path, dacl, expected_owner, directory)
}

#[cfg(windows)]
fn validate_windows_dacl_entries(
    path: &Path,
    dacl: &windows_permissions::Acl,
    expected_owner: &windows_permissions::Sid,
    directory: bool,
) -> Result<(), FilesystemError> {
    use windows_permissions::{
        LocalBox, Sid,
        constants::{AccessRights, AceFlags, AceType},
    };

    let system: LocalBox<Sid> = "SY".parse().map_err(|source| FilesystemError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let administrators: LocalBox<Sid> = "BA".parse().map_err(|source| FilesystemError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let expected_flags = if directory {
        AceFlags::ObjectInherit | AceFlags::ContainerInherit
    } else {
        AceFlags::empty()
    };
    let mut owner_seen = false;
    let mut system_seen = false;
    let mut administrators_seen = false;

    for index in 0..dacl.len() {
        let ace = dacl
            .get_ace(index)
            .ok_or_else(|| FilesystemError::WindowsAcl {
                path: path.to_path_buf(),
                reason: "DACL entry could not be read",
            })?;
        if ace.ace_type() != AceType::ACCESS_ALLOWED_ACE_TYPE
            || ace.mask() != AccessRights::FileAllAccess
            || ace.flags() != expected_flags
        {
            return Err(FilesystemError::WindowsAcl {
                path: path.to_path_buf(),
                reason: "DACL entry is not the exact required full-access grant",
            });
        }
        let sid = ace.sid().ok_or_else(|| FilesystemError::WindowsAcl {
            path: path.to_path_buf(),
            reason: "DACL entry has no SID",
        })?;
        if sid == expected_owner && !owner_seen {
            owner_seen = true;
        } else if sid == &*system && !system_seen {
            system_seen = true;
        } else if sid == &*administrators && !administrators_seen {
            administrators_seen = true;
        } else {
            return Err(FilesystemError::WindowsAcl {
                path: path.to_path_buf(),
                reason: "DACL grants an unexpected or duplicate principal",
            });
        }
    }

    if !(owner_seen && system_seen && administrators_seen) {
        return Err(FilesystemError::WindowsAcl {
            path: path.to_path_buf(),
            reason: "DACL is missing owner, SYSTEM, or Administrators",
        });
    }
    Ok(())
}

fn absolute_path(path: &Path) -> Result<PathBuf, FilesystemError> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        std::env::current_dir()
            .map(|current| current.join(path))
            .map_err(|source| FilesystemError::Io {
                path: path.to_path_buf(),
                source,
            })
    }
}

/// Fail-closed local path and permission diagnostics.
#[derive(Debug, Error)]
pub enum FilesystemError {
    /// A filesystem operation failed.
    #[error("filesystem operation failed for {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// A symlink is unsafe at a protected boundary.
    #[error("protected path must not be a symlink: {0}")]
    Symlink(PathBuf),
    /// A Windows reparse point can redirect a protected path.
    #[error("protected path must not be a Windows reparse point: {0}")]
    ReparsePoint(PathBuf),
    /// A file/directory boundary has the wrong kind.
    #[error("protected path {path} must be a {expected}")]
    WrongType {
        path: PathBuf,
        expected: &'static str,
    },
    /// POSIX owner does not match the process effective user.
    #[error("protected path {path} owner is {actual}, expected effective uid {expected}")]
    Owner {
        path: PathBuf,
        expected: u32,
        actual: u32,
    },
    /// POSIX mode is broader or narrower than the required safe mode.
    #[error("protected path {path} mode {actual:o} does not satisfy {expected}")]
    Permissions {
        path: PathBuf,
        expected: &'static str,
        actual: u32,
    },
    /// Empty or root-level target is too broad.
    #[error("unsafe protected path {path}: {reason}")]
    UnsafePath { path: PathBuf, reason: &'static str },
    /// A Windows owner/DACL does not match the frozen allowlist.
    #[error("protected path {path} has an unsafe Windows DACL: {reason}")]
    WindowsAcl { path: PathBuf, reason: &'static str },
    /// The host could not provide a trustworthy filesystem identity.
    #[error("protected path filesystem identity is unavailable: {path}")]
    FilesystemIdentityUnavailable { path: PathBuf },
    /// A `SQLite` data path uses a filesystem outside the frozen allowlist.
    #[error("protected path {path} uses unsupported filesystem {actual}; expected {expected}")]
    UnsupportedFilesystem {
        path: PathBuf,
        actual: String,
        expected: &'static str,
    },
    /// No filesystem policy exists for this target.
    #[error("filesystem permission validation is unsupported on this platform")]
    UnsupportedPlatform,
}

#[cfg(all(test, unix))]
mod tests {
    use std::{fs, io::Write as _, os::unix::fs::PermissionsExt};

    use tempfile::tempdir;

    use super::{
        FilesystemError, create_owner_only_file, prepare_data_directory, prepare_live_backup_root,
        validate_owner_only_file,
    };

    #[test]
    fn creates_owner_only_data_directory() {
        let parent = tempdir().unwrap_or_else(|error| unreachable!("temp dir: {error}"));
        let path = parent.path().join("data");
        let canonical = prepare_data_directory(&path)
            .unwrap_or_else(|error| unreachable!("prepare data dir: {error}"));
        let mode = fs::metadata(canonical)
            .unwrap_or_else(|error| unreachable!("metadata: {error}"))
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o700);
    }

    #[test]
    fn live_backup_root_is_canonical_and_shared_beside_the_data_directory() {
        let parent = tempdir().unwrap_or_else(|error| unreachable!("temp dir: {error}"));
        let root = prepare_live_backup_root(&parent.path().join("data"))
            .unwrap_or_else(|error| unreachable!("backup root: {error}"));
        assert_eq!(root, parent.path().join("studio/backups"));
        assert_eq!(
            prepare_live_backup_root(&parent.path().join("data"))
                .unwrap_or_else(|error| unreachable!("reopen root: {error}")),
            root
        );
    }

    #[test]
    fn exclusively_creates_owner_only_file_in_prepared_directory() {
        let parent = tempdir().unwrap_or_else(|error| unreachable!("temp dir: {error}"));
        let directory = parent.path().join("data");
        prepare_data_directory(&directory)
            .unwrap_or_else(|error| unreachable!("prepare data dir: {error}"));
        let path = directory.join("secret");
        let file = create_owner_only_file(&path)
            .unwrap_or_else(|error| unreachable!("create protected file: {error}"));
        drop(file);
        assert!(validate_owner_only_file(&path).is_ok());
        assert!(create_owner_only_file(&path).is_err());
    }

    #[test]
    fn failed_file_cleanup_scrubs_without_pathname_unlink() {
        let parent = tempdir().unwrap_or_else(|error| unreachable!("temp dir: {error}"));
        let directory = parent.path().join("data");
        prepare_data_directory(&directory)
            .unwrap_or_else(|error| unreachable!("prepare data dir: {error}"));
        let path = directory.join("failed-secret");
        let mut file = create_owner_only_file(&path)
            .unwrap_or_else(|error| unreachable!("create protected file: {error}"));
        file.write_all(b"secret")
            .unwrap_or_else(|error| unreachable!("write fixture: {error}"));
        let identity = super::owner_only_file_identity(&file)
            .unwrap_or_else(|error| unreachable!("file identity: {error}"));

        super::cleanup_failed_owner_only_file(file, Some(&identity), &path);
        assert!(path.exists());
        assert_eq!(
            fs::metadata(&path)
                .unwrap_or_else(|error| unreachable!("scrubbed metadata: {error}"))
                .len(),
            0
        );
        assert!(validate_owner_only_file(&path).is_ok());
    }

    #[test]
    fn rejects_broad_secret_file_permissions() {
        let parent = tempdir().unwrap_or_else(|error| unreachable!("temp dir: {error}"));
        let path = parent.path().join("dsn");
        fs::write(&path, "not-a-real-dsn")
            .unwrap_or_else(|error| unreachable!("write fixture: {error}"));
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644))
            .unwrap_or_else(|error| unreachable!("permissions: {error}"));
        assert!(matches!(
            validate_owner_only_file(&path),
            Err(FilesystemError::Permissions { .. })
        ));
    }

    #[test]
    fn rejects_symlink_secret_file() {
        use std::os::unix::fs::symlink;

        let parent = tempdir().unwrap_or_else(|error| unreachable!("temp dir: {error}"));
        let target = parent.path().join("target");
        let link = parent.path().join("link");
        fs::write(&target, "not-a-real-dsn")
            .unwrap_or_else(|error| unreachable!("write fixture: {error}"));
        symlink(&target, &link).unwrap_or_else(|error| unreachable!("symlink fixture: {error}"));
        assert!(matches!(
            validate_owner_only_file(&link),
            Err(FilesystemError::Symlink(_))
        ));
    }

    #[test]
    fn rejects_symlink_ancestor_for_data_and_secret_paths() {
        use std::os::unix::fs::symlink;

        let parent = tempdir().unwrap_or_else(|error| unreachable!("temp dir: {error}"));
        let target = parent.path().join("target");
        let link = parent.path().join("redirect");
        fs::create_dir(&target).unwrap_or_else(|error| unreachable!("create target: {error}"));
        fs::set_permissions(&target, fs::Permissions::from_mode(0o700))
            .unwrap_or_else(|error| unreachable!("target permissions: {error}"));
        let secret = target.join("dsn");
        fs::write(&secret, "not-a-real-dsn")
            .unwrap_or_else(|error| unreachable!("write secret: {error}"));
        fs::set_permissions(&secret, fs::Permissions::from_mode(0o600))
            .unwrap_or_else(|error| unreachable!("secret permissions: {error}"));
        symlink(&target, &link).unwrap_or_else(|error| unreachable!("symlink fixture: {error}"));

        assert!(matches!(
            prepare_data_directory(&link.join("data")),
            Err(FilesystemError::Symlink(path)) if path == link
        ));
        assert!(matches!(
            validate_owner_only_file(&link.join("dsn")),
            Err(FilesystemError::Symlink(path)) if path == link
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn accepts_apfs_for_macos_source_development() {
        let parent = tempdir().unwrap_or_else(|error| unreachable!("temp dir: {error}"));
        super::validate_sqlite_data_filesystem(parent.path())
            .unwrap_or_else(|error| unreachable!("APFS development filesystem: {error}"));
    }
}

#[cfg(all(test, target_os = "linux"))]
mod linux_filesystem_tests {
    use std::path::Path;

    use super::linux_mount_filesystem;

    #[test]
    fn mountinfo_uses_the_most_specific_exact_filesystem_identity() {
        let mountinfo = concat!(
            "24 1 8:1 / / rw,relatime - ext4 /dev/root rw\n",
            "25 24 8:2 / /srv/world\\040stream rw,relatime - xfs /dev/data rw\n",
        );
        assert_eq!(
            linux_mount_filesystem(Path::new("/srv/world stream/data"), mountinfo).as_deref(),
            Some("xfs")
        );
        assert_eq!(
            linux_mount_filesystem(Path::new("/unrelated"), mountinfo).as_deref(),
            Some("ext4")
        );
    }

    #[test]
    fn mountinfo_preserves_unsupported_identity_for_closed_rejection() {
        let mountinfo = "24 1 0:42 / / rw,relatime - overlay overlay rw\n";
        assert_eq!(
            linux_mount_filesystem(Path::new("/var/lib/worldstream"), mountinfo).as_deref(),
            Some("overlay")
        );
        assert!(linux_mount_filesystem(Path::new("/data"), "malformed").is_none());
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use std::{fs, io::Write as _, path::Path, process::Command};

    use tempfile::tempdir;
    use windows_permissions::{
        LocalBox, SecurityDescriptor,
        constants::{SeObjectType, SecurityInformation},
        wrappers,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_DEVICE, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL,
    };

    use super::{
        FilesystemError, apply_windows_owner_only_acl, create_owner_only_file,
        prepare_data_directory, validate_owner_only_file, validate_sqlite_data_filesystem,
        validate_windows_local_data_path, validate_windows_resolved_local_data_path,
        windows_current_identity_sid, windows_path_kind_matches,
        windows_sqlite_filesystem_is_supported,
    };

    #[test]
    fn rejects_nonlocal_windows_data_path_prefixes_before_creation() {
        for path in [
            Path::new(r"\\server\share\worldstream"),
            Path::new(r"\\?\C:\worldstream"),
            Path::new(r"\\.\C:\worldstream"),
            Path::new(r"C:relative\worldstream"),
            Path::new(r"\rooted\worldstream"),
        ] {
            assert!(matches!(
                validate_windows_local_data_path(path),
                Err(FilesystemError::UnsafePath { .. })
            ));
        }
    }

    #[test]
    fn accepts_a_path_on_the_current_fixed_temporary_drive() {
        let parent = tempdir().unwrap_or_else(|error| unreachable!("temp dir: {error}"));
        assert!(validate_windows_local_data_path(parent.path()).is_ok());
        let canonical = fs::canonicalize(parent.path())
            .unwrap_or_else(|error| unreachable!("canonical temp dir: {error}"));
        assert!(validate_windows_resolved_local_data_path(&canonical).is_ok());
        assert!(validate_sqlite_data_filesystem(&canonical).is_ok());
        assert!(windows_sqlite_filesystem_is_supported("NTFS"));
        assert!(windows_sqlite_filesystem_is_supported("refs"));
        assert!(!windows_sqlite_filesystem_is_supported("FAT32"));
        assert!(!windows_sqlite_filesystem_is_supported("exFAT"));
    }

    #[test]
    fn rejects_resolved_network_paths() {
        for path in [
            Path::new(r"\\server\share\worldstream"),
            Path::new(r"\\?\UNC\server\share\worldstream"),
            Path::new(r"\\.\UNC\server\share\worldstream"),
        ] {
            assert!(matches!(
                validate_windows_resolved_local_data_path(path),
                Err(FilesystemError::UnsafePath { .. })
            ));
        }
    }

    #[test]
    fn regular_file_check_rejects_devices_and_ambiguous_types() {
        assert!(windows_path_kind_matches(
            FILE_ATTRIBUTE_NORMAL,
            true,
            false,
            false
        ));
        assert!(!windows_path_kind_matches(
            FILE_ATTRIBUTE_DEVICE,
            true,
            false,
            false
        ));
        assert!(!windows_path_kind_matches(
            FILE_ATTRIBUTE_NORMAL,
            false,
            false,
            false
        ));
        assert!(windows_path_kind_matches(
            FILE_ATTRIBUTE_DIRECTORY,
            false,
            true,
            true
        ));
    }

    #[test]
    fn creates_and_reopens_owner_only_data_directory() {
        let parent = tempdir().unwrap_or_else(|error| unreachable!("temp dir: {error}"));
        let path = parent.path().join("data");
        let first = prepare_data_directory(&path)
            .unwrap_or_else(|error| unreachable!("prepare data dir: {error}"));
        let second = prepare_data_directory(&path)
            .unwrap_or_else(|error| unreachable!("reopen data dir: {error}"));
        assert_eq!(first, second);
    }

    #[test]
    fn exclusively_creates_exact_owner_only_windows_file() {
        let parent = tempdir().unwrap_or_else(|error| unreachable!("temp dir: {error}"));
        let directory = parent.path().join("data");
        prepare_data_directory(&directory)
            .unwrap_or_else(|error| unreachable!("prepare data dir: {error}"));
        let path = directory.join("credential.pgpass");
        let file = create_owner_only_file(&path)
            .unwrap_or_else(|error| unreachable!("create protected file: {error}"));
        drop(file);
        assert!(validate_owner_only_file(&path).is_ok());
        assert!(create_owner_only_file(&path).is_err());
    }

    #[test]
    fn failed_windows_file_cleanup_scrubs_through_the_retained_handle() {
        let parent = tempdir().unwrap_or_else(|error| unreachable!("temp dir: {error}"));
        let directory = parent.path().join("data");
        prepare_data_directory(&directory)
            .unwrap_or_else(|error| unreachable!("prepare data dir: {error}"));
        let path = directory.join("failed-secret");
        let mut file = create_owner_only_file(&path)
            .unwrap_or_else(|error| unreachable!("create protected file: {error}"));
        file.write_all(b"secret")
            .unwrap_or_else(|error| unreachable!("write fixture: {error}"));
        let identity = super::owner_only_file_identity(&file)
            .unwrap_or_else(|error| unreachable!("file identity: {error}"));

        super::cleanup_failed_owner_only_file(file, Some(&identity), &path);
        assert!(path.exists());
        assert_eq!(
            fs::metadata(&path)
                .unwrap_or_else(|error| unreachable!("scrubbed metadata: {error}"))
                .len(),
            0
        );
        assert!(validate_owner_only_file(&path).is_ok());
    }

    #[test]
    fn rejects_broad_windows_directory_dacl() {
        let parent = tempdir().unwrap_or_else(|error| unreachable!("temp dir: {error}"));
        let path = parent.path().join("broad");
        fs::create_dir(&path).unwrap_or_else(|error| unreachable!("create fixture: {error}"));
        install_test_dacl(&path, "D:P(A;OICI;FA;;;WD)");
        assert!(matches!(
            prepare_data_directory(&path),
            Err(FilesystemError::WindowsAcl { .. })
        ));
    }

    #[test]
    fn validates_secret_file_and_rejects_broad_replacement_dacl() {
        let parent = tempdir().unwrap_or_else(|error| unreachable!("temp dir: {error}"));
        let path = parent.path().join("dsn");
        fs::write(&path, "not-a-real-dsn")
            .unwrap_or_else(|error| unreachable!("write fixture: {error}"));
        let owner = windows_current_identity_sid(None)
            .unwrap_or_else(|error| unreachable!("current SID: {error}"));
        apply_windows_owner_only_acl(&path, &owner, false)
            .unwrap_or_else(|error| unreachable!("secure fixture: {error}"));
        assert!(validate_owner_only_file(&path).is_ok());

        install_test_dacl(&path, "D:P(A;;FA;;;WD)");
        assert!(matches!(
            validate_owner_only_file(&path),
            Err(FilesystemError::WindowsAcl { .. })
        ));
    }

    #[test]
    fn rejects_junction_ancestor_for_data_and_secret_paths() {
        let parent = tempdir().unwrap_or_else(|error| unreachable!("temp dir: {error}"));
        let target = parent.path().join("target");
        let junction = parent.path().join("redirect");
        fs::create_dir(&target).unwrap_or_else(|error| unreachable!("create target: {error}"));
        let secret = target.join("dsn");
        fs::write(&secret, "not-a-real-dsn")
            .unwrap_or_else(|error| unreachable!("write secret: {error}"));
        let status = Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&junction)
            .arg(&target)
            .status()
            .unwrap_or_else(|error| unreachable!("create junction: {error}"));
        assert!(status.success(), "junction fixture creation failed");

        assert!(matches!(
            prepare_data_directory(&junction.join("data")),
            Err(FilesystemError::ReparsePoint(path)) if path == junction
        ));
        assert!(matches!(
            validate_owner_only_file(&junction.join("dsn")),
            Err(FilesystemError::ReparsePoint(path)) if path == junction
        ));
    }

    fn install_test_dacl(path: &Path, sddl: &str) {
        let descriptor: LocalBox<SecurityDescriptor> = sddl
            .parse()
            .unwrap_or_else(|error| unreachable!("test SDDL: {error}"));
        let dacl = descriptor
            .dacl()
            .unwrap_or_else(|| unreachable!("test SDDL contains DACL"));
        wrappers::SetNamedSecurityInfo(
            path,
            SeObjectType::SE_FILE_OBJECT,
            SecurityInformation::Dacl | SecurityInformation::ProtectedDacl,
            None,
            None,
            Some(dacl),
            None,
        )
        .unwrap_or_else(|error| unreachable!("install test DACL: {error}"));
    }
}
