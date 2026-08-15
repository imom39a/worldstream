use std::{
    fs, io,
    path::{Path, PathBuf},
};

use thiserror::Error;

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

    let metadata = fs::symlink_metadata(path).map_err(|source| FilesystemError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_symlink() {
        return Err(FilesystemError::Symlink(path.to_path_buf()));
    }
    if !metadata.is_file() {
        return Err(FilesystemError::WrongType {
            path: path.to_path_buf(),
            expected: "regular file",
        });
    }
    validate_unix_owner(path, metadata.uid())?;
    let mode = metadata.mode() & 0o777;
    if mode & 0o077 != 0 || mode & 0o400 == 0 {
        return Err(FilesystemError::Permissions {
            path: path.to_path_buf(),
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
            Ok(_) => {
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
    let metadata = fs::symlink_metadata(path).map_err(|source| FilesystemError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    validate_windows_path_type(path, &metadata, false)?;
    let current_sid = windows_current_identity_sid(None)?;
    validate_windows_owner_only_acl(path, &current_sid, false)
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
    /// No filesystem policy exists for this target.
    #[error("filesystem permission validation is unsupported on this platform")]
    UnsupportedPlatform,
}

#[cfg(all(test, unix))]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt};

    use tempfile::tempdir;

    use super::{FilesystemError, prepare_data_directory, validate_owner_only_file};

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
}

#[cfg(all(test, windows))]
mod windows_tests {
    use std::{fs, path::Path};

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
        FilesystemError, apply_windows_owner_only_acl, prepare_data_directory,
        validate_owner_only_file, validate_windows_local_data_path,
        validate_windows_resolved_local_data_path, windows_current_identity_sid,
        windows_path_kind_matches,
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
