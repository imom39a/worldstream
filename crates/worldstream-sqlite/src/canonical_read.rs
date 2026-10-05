//! Bounded immutable lineage witnesses for current serving and captured cuts.

use rusqlite::{Connection, OptionalExtension, params};
use worldstream_core::{
    CompleteHeadV1, RoomId, RoomRecoveryErrorV1, RoomSequenceV1, VerifiedCanonicalGenesis,
    VerifiedCanonicalLineageRecord,
};

pub(super) fn load_genesis(
    connection: &Connection,
    room_id: &RoomId,
) -> Result<VerifiedCanonicalGenesis, RoomRecoveryErrorV1> {
    let bytes: Vec<u8> = connection
        .query_row(
            "SELECT genesis_bytes FROM room_genesis WHERE room_id = ?1",
            [room_id.as_str()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?
        .ok_or(RoomRecoveryErrorV1::Corrupt)?;
    let genesis = VerifiedCanonicalGenesis::from_canonical_bytes(&bytes)?;
    if genesis.record().room_id() != room_id {
        return Err(RoomRecoveryErrorV1::Corrupt);
    }
    Ok(genesis)
}

/// Verifies one stored record and all indexed commitment columns. This reads
/// no predecessor, complete state, historical reducer, or retained executor.
pub(super) fn load_record_head(
    connection: &Connection,
    genesis: &VerifiedCanonicalGenesis,
    sequence: RoomSequenceV1,
) -> Result<CompleteHeadV1, RoomRecoveryErrorV1> {
    if sequence.get() == 0 {
        return Ok(genesis.record().complete_head());
    }
    let sequence_sql = i64::try_from(sequence.get()).map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
    let row = connection
        .query_row(
            "SELECT transition_hash, previous_lineage_hash, core_schema_version, pack_digest, \
             core_state_hash, activity_state_hash, authoritative_state_hash, transition_bytes \
             FROM transitions WHERE room_id = ?1 AND room_seq = ?2",
            params![genesis.record().room_id().as_str(), sequence_sql],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, Vec<u8>>(7)?,
                ))
            },
        )
        .optional()
        .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?
        .ok_or(RoomRecoveryErrorV1::Corrupt)?;
    let record = genesis
        .record()
        .decode_transition(&row.7)
        .map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
    let head = record.complete_head();
    VerifiedCanonicalLineageRecord::verify_for_storage(genesis, &head, &row.7)?;
    if head.room_seq() != sequence
        || row.0 != head.genesis_or_transition_hash().to_string()
        || row.1 != record.previous_lineage_hash().to_string()
        || row.2 != head.core_schema_version()
        || row.3 != head.pack_digest().to_string()
        || row.4 != head.core_state_hash().to_string()
        || row.5 != head.activity_state_hash().to_string()
        || row.6 != head.authoritative_state_hash().to_string()
    {
        return Err(RoomRecoveryErrorV1::Corrupt);
    }
    Ok(head)
}
