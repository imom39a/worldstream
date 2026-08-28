//! Protected Host authority startup binding for the local Studio Supervisor.

use std::{
    fs,
    io::{ErrorKind, Write as _},
    path::Path,
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use worldstream_runtime::{
    SecretSource, StorageProfile, create_owner_only_file, create_owner_only_renameable_file,
    prepare_data_directory, validate_owner_only_file,
};
use zeroize::Zeroizing;

use crate::secrets::{FileSecretVaultV1, SecretKindV1, SecretReferenceV1};

const BINDING_FILE: &str = "host-authority-reference.json";
const BINDING_SCHEMA: &str = "worldstream/studio-host-authority-binding/v1";

/// Returns a usable configured bootstrap source, generating one owner-only
/// local source only before daemon and Studio state exist.
///
/// # Errors
///
/// Returns a non-secret failure instead of creating or rotating authority when
/// the source is missing beside existing local state.
pub fn bootstrap_source_for_local_development(
    state_dir: &Path,
    daemon_data_dir: &Path,
    storage_profile: StorageProfile,
    configured_source: Option<&SecretSource>,
) -> Result<SecretSource, HostAuthorityStartupErrorV1> {
    let source = configured_source.ok_or(HostAuthorityStartupErrorV1::BootstrapUnavailable)?;
    let SecretSource::File(path) = source else {
        return Ok(source.clone());
    };
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(source.clone()),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            if storage_profile == StorageProfile::PostgresPrimary
                || !directory_is_empty_or_absent(state_dir)
                || !directory_is_empty_or_absent(daemon_data_dir)
            {
                return Err(HostAuthorityStartupErrorV1::BootstrapUnavailable);
            }
            let parent = path
                .parent()
                .ok_or(HostAuthorityStartupErrorV1::BootstrapUnavailable)?;
            prepare_data_directory(parent)
                .map_err(|_| HostAuthorityStartupErrorV1::BootstrapUnavailable)?;
            let mut secret = Zeroizing::new([0_u8; 32]);
            getrandom::fill(secret.as_mut())
                .map_err(|_| HostAuthorityStartupErrorV1::BootstrapUnavailable)?;
            publish_fresh_bootstrap_source(path, source, &secret)
        }
        Err(_) => Err(HostAuthorityStartupErrorV1::BootstrapUnavailable),
    }
}

/// Establishes the one retained Host authority reference used by Supervisor
/// daemon proxies.
///
/// The configured bootstrap source remains the independent authority check on
/// every start. Its raw bytes are imported only into the protected vault and
/// are never returned by this boundary.
///
/// # Errors
///
/// Returns an actionable, non-secret local-development failure when the
/// configured bootstrap authority, retained binding, or protected vault cannot
/// prove that they name the same exact 256-bit authority.
pub fn establish_host_authority_reference(
    state_dir: &Path,
    vault: &FileSecretVaultV1,
    bootstrap_source: Option<&SecretSource>,
    supplied_reference: Option<&SecretReferenceV1>,
) -> Result<SecretReferenceV1, HostAuthorityStartupErrorV1> {
    let bootstrap = Zeroizing::new(
        bootstrap_source
            .ok_or(HostAuthorityStartupErrorV1::BootstrapUnavailable)?
            .read_exact_256()
            .map_err(|_| HostAuthorityStartupErrorV1::BootstrapUnavailable)?,
    );
    let binding_path = state_dir.join(BINDING_FILE);

    if let Some(binding) = read_binding(&binding_path)? {
        if supplied_reference.is_some_and(|reference| reference != &binding.reference) {
            return Err(HostAuthorityStartupErrorV1::SuppliedReferenceMismatch);
        }
        verify_matches_bootstrap(vault, &binding.reference, &bootstrap)?;
        return Ok(binding.reference);
    }

    if let Some(reference) = supplied_reference {
        verify_matches_bootstrap(vault, reference, &bootstrap)?;
        persist_binding(&binding_path, reference)?;
        return Ok(reference.clone());
    }

    let references = vault
        .retained_references(SecretKindV1::HostAuthority)
        .map_err(|_| HostAuthorityStartupErrorV1::UnboundRetainedAuthority)?;
    if let [reference] = references.as_slice() {
        verify_matches_bootstrap(vault, reference, &bootstrap)?;
        persist_binding(&binding_path, reference)?;
        return Ok(reference.clone());
    }
    if !references.is_empty() {
        return Err(HostAuthorityStartupErrorV1::UnboundRetainedAuthority);
    }

    let reference = vault
        .store(SecretKindV1::HostAuthority, bootstrap.as_ref())
        .map_err(|_| HostAuthorityStartupErrorV1::RetainedAuthorityUnavailable)?;
    persist_binding(&binding_path, &reference)?;
    Ok(reference)
}

fn directory_is_empty_or_absent(path: &Path) -> bool {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == ErrorKind::NotFound => true,
        Ok(metadata) if metadata.file_type().is_dir() => match fs::read_dir(path) {
            Ok(mut entries) => entries.next().is_none(),
            Err(_) => false,
        },
        Ok(_) | Err(_) => false,
    }
}

fn publish_fresh_bootstrap_source(
    path: &Path,
    source: &SecretSource,
    secret: &[u8; 32],
) -> Result<SecretSource, HostAuthorityStartupErrorV1> {
    let temporary = temporary_bootstrap_path(path)?;
    let mut file = create_owner_only_file(&temporary)
        .map_err(|_| HostAuthorityStartupErrorV1::BootstrapUnavailable)?;
    let written = file.write_all(secret).and_then(|()| file.sync_all());
    drop(file);
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
        return Err(HostAuthorityStartupErrorV1::BootstrapUnavailable);
    }

    let parent = path
        .parent()
        .ok_or(HostAuthorityStartupErrorV1::BootstrapUnavailable)?;
    match fs::hard_link(&temporary, path) {
        Ok(()) => {
            let publication = sync_directory(parent);
            let _ = fs::remove_file(&temporary);
            let cleanup = sync_directory(parent);
            publication
                .and(cleanup)
                .map_err(|_| HostAuthorityStartupErrorV1::BootstrapUnavailable)?;
            Ok(source.clone())
        }
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            let _ = fs::remove_file(&temporary);
            let _verified = Zeroizing::new(
                source
                    .read_exact_256()
                    .map_err(|_| HostAuthorityStartupErrorV1::BootstrapUnavailable)?,
            );
            Ok(source.clone())
        }
        Err(_) => {
            let _ = fs::remove_file(&temporary);
            Err(HostAuthorityStartupErrorV1::BootstrapUnavailable)
        }
    }
}

fn verify_matches_bootstrap(
    vault: &FileSecretVaultV1,
    reference: &SecretReferenceV1,
    bootstrap: &[u8; 32],
) -> Result<(), HostAuthorityStartupErrorV1> {
    let retained = vault
        .resolve(SecretKindV1::HostAuthority, reference)
        .map_err(|_| HostAuthorityStartupErrorV1::RetainedAuthorityUnavailable)?;
    if retained.as_bytes().len() != bootstrap.len() {
        return Err(HostAuthorityStartupErrorV1::RetainedAuthorityUnavailable);
    }
    if exact_bytes_match(retained.as_bytes(), bootstrap) {
        Ok(())
    } else {
        Err(HostAuthorityStartupErrorV1::RetainedAuthorityMismatch)
    }
}

fn exact_bytes_match(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0_u8;
    for (left, right) in left.iter().zip(right) {
        difference |= left ^ right;
    }
    difference == 0
}

fn read_binding(
    path: &Path,
) -> Result<Option<HostAuthorityBindingV1>, HostAuthorityStartupErrorV1> {
    match fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(HostAuthorityStartupErrorV1::BindingUnavailable),
    }
    validate_owner_only_file(path).map_err(|_| HostAuthorityStartupErrorV1::BindingUnavailable)?;
    let bytes = fs::read(path).map_err(|_| HostAuthorityStartupErrorV1::BindingUnavailable)?;
    let binding: HostAuthorityBindingV1 =
        serde_json::from_slice(&bytes).map_err(|_| HostAuthorityStartupErrorV1::BindingInvalid)?;
    if binding.schema != BINDING_SCHEMA {
        return Err(HostAuthorityStartupErrorV1::BindingInvalid);
    }
    Ok(Some(binding))
}

fn persist_binding(
    path: &Path,
    reference: &SecretReferenceV1,
) -> Result<(), HostAuthorityStartupErrorV1> {
    let binding = HostAuthorityBindingV1 {
        schema: BINDING_SCHEMA.to_owned(),
        reference: reference.clone(),
    };
    let encoded = serde_json::to_vec(&binding)
        .map_err(|_| HostAuthorityStartupErrorV1::BindingUnavailable)?;
    let temporary = temporary_binding_path(path)?;
    let mut file = create_owner_only_renameable_file(&temporary)
        .map_err(|_| HostAuthorityStartupErrorV1::BindingUnavailable)?;
    let result = file
        .write_all(&encoded)
        .and_then(|()| file.sync_all())
        .and_then(|()| replace_binding(&temporary, path))
        .and_then(|()| {
            path.parent()
                .ok_or_else(|| std::io::Error::other("binding has no parent"))
                .and_then(sync_directory)
        })
        .map_err(|_| HostAuthorityStartupErrorV1::BindingUnavailable);
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn temporary_binding_path(path: &Path) -> Result<std::path::PathBuf, HostAuthorityStartupErrorV1> {
    let parent = path
        .parent()
        .ok_or(HostAuthorityStartupErrorV1::BindingUnavailable)?;
    let mut random = [0_u8; 16];
    getrandom::fill(&mut random).map_err(|_| HostAuthorityStartupErrorV1::BindingUnavailable)?;
    Ok(parent.join(format!(".host-authority-reference-{}.tmp", hex(&random))))
}

fn temporary_bootstrap_path(
    path: &Path,
) -> Result<std::path::PathBuf, HostAuthorityStartupErrorV1> {
    let parent = path
        .parent()
        .ok_or(HostAuthorityStartupErrorV1::BootstrapUnavailable)?;
    let mut random = [0_u8; 16];
    getrandom::fill(&mut random).map_err(|_| HostAuthorityStartupErrorV1::BootstrapUnavailable)?;
    Ok(parent.join(format!(".authority-bootstrap-{}.tmp", hex(&random))))
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(DIGITS[usize::from(byte >> 4)]));
        encoded.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    encoded
}

#[cfg(unix)]
fn replace_binding(source: &Path, target: &Path) -> std::io::Result<()> {
    fs::rename(source, target)
}

#[cfg(windows)]
fn replace_binding(source: &Path, target: &Path) -> std::io::Result<()> {
    let source = source.to_str().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "source path is not Unicode",
        )
    })?;
    let target = target.to_str().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "target path is not Unicode",
        )
    })?;
    winsafe::MoveFileEx(
        source,
        Some(target),
        winsafe::co::MOVEFILE::REPLACE_EXISTING,
    )
    .map_err(|error| std::io::Error::other(error.to_string()))
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> std::io::Result<()> {
    fs::File::open(path)?.sync_all()
}

#[cfg(windows)]
fn sync_directory(path: &Path) -> std::io::Result<()> {
    use std::{fs::OpenOptions, os::windows::fs::OpenOptionsExt as _};

    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?
        .sync_all()
}

#[derive(Deserialize, Serialize)]
struct HostAuthorityBindingV1 {
    schema: String,
    reference: SecretReferenceV1,
}

/// Non-secret startup failure for the local Host authority binding.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum HostAuthorityStartupErrorV1 {
    /// No usable, configured bootstrap source is available to authenticate the
    /// local daemon authority.
    #[error(
        "Studio local development requires the configured bootstrap authority; restore its owner-only 32-byte secret source and restart"
    )]
    BootstrapUnavailable,
    /// The protected persisted binding cannot be read safely.
    #[error(
        "Studio retained Host authority binding is unavailable; restore protected local Studio state access and restart"
    )]
    BindingUnavailable,
    /// The persisted binding has no accepted non-secret shape.
    #[error(
        "Studio retained Host authority binding is malformed; restore the protected local Studio state from a known-good copy and restart"
    )]
    BindingInvalid,
    /// A caller tried to select a reference other than the retained one.
    #[error("the supplied Host authority reference does not match the retained local binding")]
    SuppliedReferenceMismatch,
    /// Existing authority vault material cannot be safely associated with a
    /// single retained binding.
    #[error(
        "Studio has retained Host authority material without one protected binding; restore the matching local Studio state and restart"
    )]
    UnboundRetainedAuthority,
    /// The retained authority cannot be resolved or has the wrong length.
    #[error(
        "the retained Host authority is unavailable or malformed; restore protected local Studio state access and restart"
    )]
    RetainedAuthorityUnavailable,
    /// The retained authority does not authenticate the configured daemon.
    #[error(
        "the retained Host authority does not match the configured bootstrap authority; restore matching protected local development state and restart"
    )]
    RetainedAuthorityMismatch,
}
