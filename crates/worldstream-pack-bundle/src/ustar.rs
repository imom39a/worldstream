use std::{collections::BTreeMap, ops::Range, str, sync::Arc};

use crate::PackBundleErrorV1;

pub const MAX_BUNDLE_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_COMPONENT_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_JSON_MEMBER_BYTES: usize = 1024 * 1024;
pub const MAX_STATIC_MEMBER_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_MEMBER_COUNT: usize = 256;

const BLOCK_BYTES: usize = 512;
const TRAILER_BYTES: usize = BLOCK_BYTES * 2;

/// One validated canonical uncompressed ustar archive.
#[derive(Clone)]
pub(crate) struct CanonicalUstarV1 {
    bytes: Arc<[u8]>,
    members: BTreeMap<String, Range<usize>>,
}

impl CanonicalUstarV1 {
    pub(crate) fn parse(bytes: Arc<[u8]>) -> Result<Self, PackBundleErrorV1> {
        if bytes.len() > MAX_BUNDLE_BYTES
            || bytes.len() < TRAILER_BYTES
            || !bytes.len().is_multiple_of(BLOCK_BYTES)
        {
            return Err(PackBundleErrorV1::LimitExceeded);
        }

        let mut offset = 0_usize;
        let mut members = BTreeMap::new();
        let mut previous_name: Option<String> = None;
        loop {
            let header_end = offset
                .checked_add(BLOCK_BYTES)
                .ok_or(PackBundleErrorV1::LimitExceeded)?;
            let header = bytes
                .get(offset..header_end)
                .ok_or(PackBundleErrorV1::ArchiveNotCanonical)?;
            if header.iter().all(|byte| *byte == 0) {
                let trailer_end = offset
                    .checked_add(TRAILER_BYTES)
                    .ok_or(PackBundleErrorV1::LimitExceeded)?;
                let trailer = bytes
                    .get(offset..trailer_end)
                    .ok_or(PackBundleErrorV1::ArchiveNotCanonical)?;
                if trailer_end != bytes.len() || trailer.iter().any(|byte| *byte != 0) {
                    return Err(PackBundleErrorV1::ArchiveNotCanonical);
                }
                break;
            }

            if members.len() >= MAX_MEMBER_COUNT {
                return Err(PackBundleErrorV1::LimitExceeded);
            }
            let name = parse_name(header)?;
            validate_member_name(&name)?;
            if members.contains_key(&name) {
                return Err(PackBundleErrorV1::DuplicateMember(name));
            }
            if previous_name
                .as_ref()
                .is_some_and(|previous| previous >= &name)
            {
                return Err(PackBundleErrorV1::ArchiveNotCanonical);
            }
            let size = parse_size(header)?;
            let canonical_header = canonical_header(&name, size)?;
            if header != canonical_header {
                return Err(PackBundleErrorV1::ArchiveMetadataInvalid);
            }

            let data_start = header_end;
            let data_end = data_start
                .checked_add(size)
                .ok_or(PackBundleErrorV1::LimitExceeded)?;
            let padded_size = size
                .checked_add(BLOCK_BYTES - 1)
                .ok_or(PackBundleErrorV1::LimitExceeded)?
                / BLOCK_BYTES
                * BLOCK_BYTES;
            let next_offset = data_start
                .checked_add(padded_size)
                .ok_or(PackBundleErrorV1::LimitExceeded)?;
            bytes
                .get(data_start..data_end)
                .ok_or(PackBundleErrorV1::ArchiveNotCanonical)?;
            let padding = bytes
                .get(data_end..next_offset)
                .ok_or(PackBundleErrorV1::ArchiveNotCanonical)?;
            if padding.iter().any(|byte| *byte != 0) {
                return Err(PackBundleErrorV1::ArchiveNotCanonical);
            }
            members.insert(name.clone(), data_start..data_end);
            previous_name = Some(name);
            offset = next_offset;
        }
        if members.is_empty() {
            return Err(PackBundleErrorV1::ArchiveNotCanonical);
        }
        Ok(Self { bytes, members })
    }

    pub(crate) fn member(&self, name: &str) -> Option<&[u8]> {
        self.members
            .get(name)
            .and_then(|range| self.bytes.get(range.clone()))
    }

    pub(crate) fn member_names(&self) -> impl ExactSizeIterator<Item = &str> {
        self.members.keys().map(String::as_str)
    }

    pub(crate) fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// Deterministic writer used by Pack Author tooling and byte-stability tests.
pub struct PackBundleWriterV1;

impl PackBundleWriterV1 {
    /// Emits one canonical uncompressed ustar archive from already named bytes.
    ///
    /// # Errors
    ///
    /// Returns an error for unsafe names, member-count or byte limits, or
    /// values that cannot be represented by the frozen ustar profile.
    pub fn write(members: &BTreeMap<String, Vec<u8>>) -> Result<Vec<u8>, PackBundleErrorV1> {
        if members.is_empty() || members.len() > MAX_MEMBER_COUNT {
            return Err(PackBundleErrorV1::LimitExceeded);
        }
        let mut output = Vec::new();
        for (name, bytes) in members {
            validate_member_name(name)?;
            let header = canonical_header(name, bytes.len())?;
            output.extend_from_slice(&header);
            output.extend_from_slice(bytes);
            let padding = (BLOCK_BYTES - bytes.len() % BLOCK_BYTES) % BLOCK_BYTES;
            let new_length = output
                .len()
                .checked_add(padding)
                .ok_or(PackBundleErrorV1::LimitExceeded)?;
            output.resize(new_length, 0);
            if output.len() > MAX_BUNDLE_BYTES.saturating_sub(TRAILER_BYTES) {
                return Err(PackBundleErrorV1::LimitExceeded);
            }
        }
        output.resize(output.len() + TRAILER_BYTES, 0);
        if output.len() > MAX_BUNDLE_BYTES {
            return Err(PackBundleErrorV1::LimitExceeded);
        }
        Ok(output)
    }
}

pub(crate) fn validate_member_name(name: &str) -> Result<(), PackBundleErrorV1> {
    if name.is_empty()
        || name.len() > 100
        || name.starts_with('/')
        || name.ends_with('/')
        || name.contains('\0')
        || name.contains('\\')
        || name
            .split('/')
            .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return Err(PackBundleErrorV1::UnsafeMemberPath(name.to_owned()));
    }
    Ok(())
}

fn parse_name(header: &[u8]) -> Result<String, PackBundleErrorV1> {
    let field = header
        .get(..100)
        .ok_or(PackBundleErrorV1::ArchiveNotCanonical)?;
    let end = field
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(field.len());
    if field[end..].iter().any(|byte| *byte != 0) {
        return Err(PackBundleErrorV1::ArchiveMetadataInvalid);
    }
    str::from_utf8(&field[..end])
        .map(str::to_owned)
        .map_err(|_| PackBundleErrorV1::ArchiveMetadataInvalid)
}

fn parse_size(header: &[u8]) -> Result<usize, PackBundleErrorV1> {
    let field = header
        .get(124..136)
        .ok_or(PackBundleErrorV1::ArchiveNotCanonical)?;
    let (digits, terminator) = field.split_at(11);
    if terminator != [0] || !digits.iter().all(u8::is_ascii_digit) {
        return Err(PackBundleErrorV1::ArchiveMetadataInvalid);
    }
    if digits.iter().any(|byte| *byte > b'7') {
        return Err(PackBundleErrorV1::ArchiveMetadataInvalid);
    }
    let text = str::from_utf8(digits).map_err(|_| PackBundleErrorV1::ArchiveMetadataInvalid)?;
    usize::from_str_radix(text, 8).map_err(|_| PackBundleErrorV1::ArchiveMetadataInvalid)
}

fn canonical_header(name: &str, size: usize) -> Result<[u8; BLOCK_BYTES], PackBundleErrorV1> {
    validate_member_name(name)?;
    let mut header = [0_u8; BLOCK_BYTES];
    header[..name.len()].copy_from_slice(name.as_bytes());
    write_octal(&mut header[100..108], 0o644)?;
    write_octal(&mut header[108..116], 0)?;
    write_octal(&mut header[116..124], 0)?;
    write_octal(&mut header[124..136], size)?;
    write_octal(&mut header[136..148], 0)?;
    header[148..156].fill(b' ');
    header[156] = b'0';
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");
    write_octal(&mut header[329..337], 0)?;
    write_octal(&mut header[337..345], 0)?;
    let checksum: usize = header.iter().map(|byte| usize::from(*byte)).sum();
    let checksum_text = format!("{checksum:06o}\0 ");
    if checksum_text.len() != 8 {
        return Err(PackBundleErrorV1::LimitExceeded);
    }
    header[148..156].copy_from_slice(checksum_text.as_bytes());
    Ok(header)
}

fn write_octal(field: &mut [u8], value: usize) -> Result<(), PackBundleErrorV1> {
    let digits = field
        .len()
        .checked_sub(1)
        .ok_or(PackBundleErrorV1::ArchiveMetadataInvalid)?;
    let text = format!("{value:0digits$o}");
    if text.len() != digits {
        return Err(PackBundleErrorV1::LimitExceeded);
    }
    field[..digits].copy_from_slice(text.as_bytes());
    field[digits] = 0;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use std::{collections::BTreeMap, sync::Arc};

    use super::{BLOCK_BYTES, CanonicalUstarV1, PackBundleWriterV1, canonical_header};
    use crate::PackBundleErrorV1;

    fn one_member_block(name: &str, bytes: &[u8]) -> Vec<u8> {
        let mut output = canonical_header(name, bytes.len())
            .unwrap_or_else(|error| panic!("header: {error}"))
            .to_vec();
        output.extend_from_slice(bytes);
        output.resize(output.len().div_ceil(BLOCK_BYTES) * BLOCK_BYTES, 0);
        output
    }

    #[test]
    fn duplicate_member_fails_closed() {
        let member = one_member_block("same.bin", b"one");
        let mut archive = member.clone();
        archive.extend_from_slice(&member);
        archive.resize(archive.len() + BLOCK_BYTES * 2, 0);
        assert!(matches!(
            CanonicalUstarV1::parse(Arc::from(archive)),
            Err(PackBundleErrorV1::DuplicateMember(name)) if name == "same.bin"
        ));
    }

    #[test]
    fn trailing_blocks_and_nonzero_padding_fail_closed() {
        let members = BTreeMap::from([("one.bin".to_owned(), vec![1])]);
        let canonical =
            PackBundleWriterV1::write(&members).unwrap_or_else(|error| panic!("archive: {error}"));
        let mut trailing = canonical.clone();
        trailing.resize(trailing.len() + BLOCK_BYTES, 0);
        assert!(matches!(
            CanonicalUstarV1::parse(Arc::from(trailing)),
            Err(PackBundleErrorV1::ArchiveNotCanonical)
        ));

        let mut nonzero_padding = canonical;
        nonzero_padding[BLOCK_BYTES + 1] = 1;
        assert!(matches!(
            CanonicalUstarV1::parse(Arc::from(nonzero_padding)),
            Err(PackBundleErrorV1::ArchiveNotCanonical)
        ));
    }
}
