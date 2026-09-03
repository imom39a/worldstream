//! Platform publication for caller-owned protected staged files.
//! Windows write-through publication still requires native release qualification.

use std::{fs, io, path::Path};

#[derive(Clone, Copy)]
pub(crate) enum PublicationMode {
    CreateNew,
    Replace,
}

/// Flush and close a protected staged file before publishing its path.
///
/// The caller supplies the matching file/path within a trusted protected directory
/// and owns cleanup of any remaining source. A publication error can occur after
/// the target changes; callers must not interpret every error as a rollback.
pub(crate) fn publish(
    file: fs::File,
    source: &Path,
    target: &Path,
    mode: PublicationMode,
) -> io::Result<()> {
    file.sync_all()?;
    drop(file);
    publish_closed(source, target, mode)
}

#[cfg(unix)]
fn publish_closed(source: &Path, target: &Path, mode: PublicationMode) -> io::Result<()> {
    let parent = target
        .parent()
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
    match mode {
        PublicationMode::CreateNew => fs::hard_link(source, target)?,
        PublicationMode::Replace => fs::rename(source, target)?,
    }
    fs::File::open(parent)?.sync_all()
}

#[cfg(windows)]
fn publish_closed(source: &Path, target: &Path, mode: PublicationMode) -> io::Result<()> {
    native_path(source)?;
    native_path(target)?;
    // These safe wrappers use MoveFileExW with WRITE_THROUGH, adding
    // REPLACE_EXISTING only for replacement, without COPY_ALLOWED:
    // https://docs.rs/crate/atomicwrites/0.4.4/source/src/lib.rs
    match mode {
        PublicationMode::CreateNew => atomicwrites::move_atomic(source, target),
        PublicationMode::Replace => atomicwrites::replace_atomic(source, target),
    }
}

#[cfg(windows)]
fn native_path(path: &Path) -> io::Result<&str> {
    path.to_str()
        .filter(|value| !value.contains('\0'))
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))
}

#[cfg(test)]
mod tests {
    use super::{PublicationMode, publish};
    use std::{
        fs,
        io::{ErrorKind, Write as _},
    };
    use worldstream_runtime::{create_owner_only_file, prepare_data_directory};

    #[test]
    fn protected_publication_has_explicit_create_new_and_replace_semantics()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempfile::tempdir()?;
        let directory = prepare_data_directory(&temporary.path().join("protected"))?;
        let first_source = directory.join("first.staged");
        let target = directory.join("record");
        let mut first = create_owner_only_file(&first_source)?;
        first.write_all(b"original record")?;
        publish(first, &first_source, &target, PublicationMode::CreateNew)?;
        assert_eq!(fs::read(&target)?, b"original record");
        #[cfg(unix)]
        assert!(
            first_source.exists(),
            "hard-link publication leaves source cleanup to caller"
        );
        #[cfg(windows)]
        assert!(
            !first_source.exists(),
            "Windows publication moves the staged file"
        );

        let second_source = directory.join("second.staged");
        let mut second = create_owner_only_file(&second_source)?;
        second.write_all(b"replacement record")?;
        let rejected = publish(second, &second_source, &target, PublicationMode::CreateNew)
            .err()
            .ok_or("create-new unexpectedly replaced an existing target")?;
        assert_eq!(rejected.kind(), ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&target)?, b"original record");
        assert!(
            second_source.exists(),
            "failed publication must retain caller-owned source"
        );

        let replacement = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&second_source)?;
        publish(
            replacement,
            &second_source,
            &target,
            PublicationMode::Replace,
        )?;
        assert_eq!(fs::read(&target)?, b"replacement record");
        assert!(
            !second_source.exists(),
            "replacement consumes the staged path"
        );
        Ok(())
    }
}
