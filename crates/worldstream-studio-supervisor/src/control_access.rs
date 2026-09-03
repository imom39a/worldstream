//! Installation-owner control access, separate from all Runtime and vault authority.

use std::{
    fmt, fs,
    io::{self, Write as _},
    path::{Path, PathBuf},
};

use axum::http::{HeaderMap, HeaderValue, header::AUTHORIZATION};
use thiserror::Error;
use worldstream_runtime::{
    FilesystemError, SecretSource, create_owner_only_file, prepare_data_directory,
    validate_data_directory, validate_owner_only_file,
};
use zeroize::Zeroizing;

use crate::protected_publication::{PublicationMode, publish};

const RECORD_FILE: &str = "control-access.v1";
const LOCK_FILE: &str = "control-access.lock";
const MAGIC: &[u8; 8] = b"WSCTL01\0";
const RECORD_BYTES: usize = 48;

/// Closed failures; no filesystem paths, headers, or credential bytes are retained.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ControlAccessError {
    #[error("local control access is not initialized")]
    NotInitialized,
    #[error("local control access is invalid or unsafe")]
    Invalid,
    #[error("local control access is unavailable")]
    Unavailable,
    #[error("control access publication is uncertain; inspect the retained installation")]
    PublicationUncertain,
}

/// A protected installation location, never a cached or serializable credential.
pub struct ControlAccess {
    state: PathBuf,
}

impl fmt::Debug for ControlAccess {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ControlAccess([REDACTED])")
    }
}

impl ControlAccess {
    /// Explicitly installs control access if absent. Existing bytes are preserved.
    /// The caller must validate/bootstrap the installation before this mutation.
    ///
    /// # Errors
    /// Fails closed for unsafe state, invalid existing access, or uncertain publication.
    pub fn initialize(state: &Path) -> Result<Self, ControlAccessError> {
        let access = Self {
            state: prepare_data_directory(state).map_err(map_filesystem_error)?,
        };
        match access.read_record() {
            Ok(_) | Err(ControlAccessError::NotInitialized) => {}
            Err(error) => return Err(error),
        }
        let _lock = access.lock_mutation()?;
        let (record, mode) = match access.read_record() {
            // Republish the same bytes under the mutation lock. This establishes
            // a completed publication even after a prior process died between
            // replacing the record and reporting durable success. It does not
            // rotate access or change the generation.
            Ok(record) => (record, PublicationMode::Replace),
            Err(ControlAccessError::NotInitialized) => {
                (fresh_record(1)?, PublicationMode::CreateNew)
            }
            Err(error) => return Err(error),
        };
        access.publish_record(&record, mode)?;
        Self::open(&access.state)
    }

    /// Opens existing access without creating files, directories, or credentials.
    ///
    /// # Errors
    /// Missing or unsafe retained access requires explicit initialization or restoration.
    pub fn open(state: &Path) -> Result<Self, ControlAccessError> {
        let access = Self {
            state: validate_data_directory(state).map_err(map_filesystem_error)?,
        };
        let _record = access.read_record()?;
        Ok(access)
    }

    /// Loads the current credential for one local control request. Debug output is redacted.
    ///
    /// # Errors
    /// Fails without exposing material when the protected record cannot be read.
    pub fn authorization_header(&self) -> Result<HeaderValue, ControlAccessError> {
        let record = self.read_record()?;
        let mut encoded = Zeroizing::new(String::from("Bearer "));
        append_hex(&mut encoded, &record[16..]);
        let mut header =
            HeaderValue::from_str(&encoded).map_err(|_| ControlAccessError::Invalid)?;
        header.set_sensitive(true);
        Ok(header)
    }

    /// Replaces only installation-owner control access. Existing observers load
    /// the new generation on their next request; no other authority is changed.
    ///
    /// # Errors
    /// Unsafe state, a concurrent rotation, or uncertain durable publication
    /// fails closed. An uncertain publication must not be reported as a rollback.
    pub fn rotate(&self) -> Result<(), ControlAccessError> {
        let _current = self.read_record()?;
        let _lock = self.lock_mutation()?;
        let current = self.read_record()?;
        let next = generation(&current)?
            .checked_add(1)
            .ok_or(ControlAccessError::Invalid)?;
        let replacement = fresh_record(next)?;
        self.publish_record(&replacement, PublicationMode::Replace)
    }

    /// Checks exactly one canonical Bearer header against current protected access.
    /// Cookie, query, body and Origin values are not installation authentication.
    ///
    /// # Errors
    /// Unreadable/unsafe control state never falls back to another authority kind.
    pub fn authenticate(&self, headers: &HeaderMap) -> Result<bool, ControlAccessError> {
        let mut supplied = headers.get_all(AUTHORIZATION).iter();
        let Some(header) = supplied.next() else {
            return Ok(false);
        };
        if supplied.next().is_some() {
            return Ok(false);
        }
        let bytes = header.as_bytes();
        if bytes.len() != 71 || !bytes.starts_with(b"Bearer ") {
            return Ok(false);
        }
        let mut candidate = Zeroizing::new([0_u8; 32]);
        for (index, pair) in bytes[7..].chunks_exact(2).enumerate() {
            let (Some(high), Some(low)) = (hex_digit(pair[0]), hex_digit(pair[1])) else {
                return Ok(false);
            };
            candidate[index] = (high << 4) | low;
        }
        let record = self.read_record()?;
        let difference = candidate
            .iter()
            .zip(&record[16..])
            .fold(0_u8, |diff, (a, b)| diff | (a ^ b));
        Ok(difference == 0)
    }

    fn read_record(&self) -> Result<Zeroizing<Vec<u8>>, ControlAccessError> {
        validate_data_directory(&self.state).map_err(map_filesystem_error)?;
        let path = self.state.join(RECORD_FILE);
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(ControlAccessError::NotInitialized);
            }
            Err(_) => return Err(ControlAccessError::Unavailable),
            Ok(_) => {}
        }
        let record = Zeroizing::new(
            SecretSource::File(path)
                .read_bounded(RECORD_BYTES)
                .map_err(|_| ControlAccessError::Invalid)?,
        );
        if record.len() != RECORD_BYTES || &record[..8] != MAGIC || generation(&record)? == 0 {
            return Err(ControlAccessError::Invalid);
        }
        Ok(record)
    }

    fn temporary_path(&self) -> Result<PathBuf, ControlAccessError> {
        let mut nonce = [0_u8; 16];
        getrandom::fill(&mut nonce).map_err(|_| ControlAccessError::Unavailable)?;
        let mut name = String::from(".control-access-");
        append_hex(&mut name, &nonce);
        name.push_str(".tmp");
        Ok(self.state.join(name))
    }

    fn publish_record(
        &self,
        record: &[u8],
        mode: PublicationMode,
    ) -> Result<(), ControlAccessError> {
        let temporary = self.temporary_path()?;
        let mut file = create_owner_only_file(&temporary).map_err(map_filesystem_error)?;
        let publication = match file.write_all(record) {
            Ok(()) => publish(file, &temporary, &self.state.join(RECORD_FILE), mode),
            Err(error) => {
                drop(file);
                Err(error)
            }
        };
        let _ = fs::remove_file(&temporary);
        publication.map_err(|_| ControlAccessError::PublicationUncertain)
    }

    fn lock_mutation(&self) -> Result<fs::File, ControlAccessError> {
        let path = self.state.join(LOCK_FILE);
        match fs::symlink_metadata(&path) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let temporary = self.temporary_path()?;
                let file = create_owner_only_file(&temporary).map_err(map_filesystem_error)?;
                let publication = publish(file, &temporary, &path, PublicationMode::CreateNew);
                let _ = fs::remove_file(&temporary);
                match publication {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(_) => return Err(ControlAccessError::Unavailable),
                }
            }
            Err(_) => return Err(ControlAccessError::Unavailable),
        }
        validate_owner_only_file(&path).map_err(map_filesystem_error)?;
        let mut options = fs::OpenOptions::new();
        options.read(true).write(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt as _;
            // Retain one lock inode while contenders can open and lock it.
            options.share_mode(0x0000_0001 | 0x0000_0002);
        }
        let lock = options
            .open(&path)
            .map_err(|_| ControlAccessError::Unavailable)?;
        validate_owner_only_file(&path).map_err(map_filesystem_error)?;
        lock.try_lock()
            .map_err(|_| ControlAccessError::Unavailable)?;
        Ok(lock)
    }
}

fn fresh_record(generation: u64) -> Result<Zeroizing<Vec<u8>>, ControlAccessError> {
    let mut record = Zeroizing::new(vec![0_u8; RECORD_BYTES]);
    record[..8].copy_from_slice(MAGIC);
    record[8..16].copy_from_slice(&generation.to_le_bytes());
    getrandom::fill(&mut record[16..]).map_err(|_| ControlAccessError::Unavailable)?;
    Ok(record)
}

fn generation(record: &[u8]) -> Result<u64, ControlAccessError> {
    let bytes = record.get(8..16).ok_or(ControlAccessError::Invalid)?;
    Ok(u64::from_le_bytes(
        bytes.try_into().map_err(|_| ControlAccessError::Invalid)?,
    ))
}

fn append_hex(target: &mut String, bytes: &[u8]) {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    for byte in bytes {
        target.push(char::from(DIGITS[usize::from(byte >> 4)]));
        target.push(char::from(DIGITS[usize::from(byte & 15)]));
    }
}

fn hex_digit(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        _ => None,
    }
}

fn map_filesystem_error(error: FilesystemError) -> ControlAccessError {
    match error {
        FilesystemError::Io { source, .. } if source.kind() == io::ErrorKind::NotFound => {
            ControlAccessError::NotInitialized
        }
        FilesystemError::Io { .. } => ControlAccessError::Unavailable,
        _ => ControlAccessError::Invalid,
    }
}
