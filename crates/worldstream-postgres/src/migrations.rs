//! PostgreSQL migration identity, verification, and provider-neutral fixtures.

use std::{fmt, sync::Mutex};

use thiserror::Error;
use worldstream_core::Blake3DigestV1;

/// Stable logical migration history shared by the storage profiles.
pub const LOGICAL_HISTORY_ID: &str = "worldstream-storage-v1";
/// Stable identifier for the current schema contract.
pub const SCHEMA_CONTRACT_ID: &str = "worldstream-postgresql-room-commit-v1";
/// The migration that created the original PostgreSQL room-commit schema.
pub const INITIAL_MIGRATION_ID: &str = "0001-initial-storage-schema";
/// The migration that made migration metadata and operational authority explicit.
pub const AUTHORITY_MIGRATION_ID: &str = "0002-operational-authority-v1";
/// The migration that adds the shared timer, delivery, receipt, Activation,
/// snapshot, and integrity witness rows used by the conformance boundary.
pub const KERNEL_CONFORMANCE_MIGRATION_ID: &str = "0003-kernel-conformance-v1";
/// The migration that closes the commit-time witness and receipt uniqueness
/// gaps in the kernel conformance schema.
pub const KERNEL_PARITY_MIGRATION_ID: &str = "0004-kernel-parity-witnesses-v1";
/// The migration that adds the byte-preserving transfer publication staging
/// rows. These rows are an import seam, not semantic Room truth.
pub const TRANSFER_PUBLICATION_MIGRATION_ID: &str = "0005-transfer-publication-v1";
/// The migration that adds one durable target-wide fence for transfer imports.
pub const TRANSFER_TARGET_FENCE_MIGRATION_ID: &str = "0006-transfer-target-fence-v1";
/// The migration that adds the target-wide canonical deployment identity row.
pub const DEPLOYMENT_METADATA_MIGRATION_ID: &str = "0007-deployment-metadata-v1";
/// The migration that persists the source-authoritative complete pack/resource
/// identity witness used by whole-deployment transfer.
pub const DEPLOYMENT_IDENTITY_MIGRATION_ID: &str = "0008-deployment-identities-v1";
/// The forward migration that installs the production Core authority facts.
pub const AUTHORITY_FACTS_MIGRATION_ID: &str = "0009-authority-facts-v1";
/// The forward migration that retains exact resource bytes and archived
/// source authority-fence evidence for complete transfer/restore parity.
pub const TRANSFER_RECOVERY_COMPLETENESS_MIGRATION_ID: &str =
    "0010-transfer-recovery-completeness-v1";
/// The forward migration that globally closes resource identity across kinds.
pub const TRANSFER_RESOURCE_IDENTITY_MIGRATION_ID: &str =
    "0011-transfer-lifecycle-and-resource-identity-v1";
/// The forward migration that durably retains the first sampled Semantic Time
/// for one exact ExternalInput operation identity and request hash.
pub const EXTERNAL_INPUT_PREPARATION_MIGRATION_ID: &str = "0013-external-input-preparations-v1";
/// The forward migration that adds an operational reset epoch. It distinguishes
/// a reset installed by one Session from a later reset at the same frame head.
pub const OBSERVATION_RESET_GENERATION_MIGRATION_ID: &str = "0014-observation-reset-generation-v1";
/// Adds operational retention age and bounded runtime prefix maintenance.
pub const OBSERVATION_RETENTION_MIGRATION_ID: &str = "0015-observation-retention-v1";
/// Reserves the shared logical snapshot-cadence step. PostgreSQL has no
/// backend-owned cadence schedule: its snapshots are produced by the same
/// bounded Room Commit path, so the backend-specific execution is intentionally
/// empty while the logical history remains aligned with SQLite.
pub const SNAPSHOT_CADENCE_MIGRATION_ID: &str = "0015-snapshot-cadence-v1";
/// Adds bounded refresh attention metadata and auditable supersession fields.
pub const ACTIVATION_BACKLOG_POLICY_MIGRATION_ID: &str = "0016-activation-backlog-policy-v1";
/// Adds the manifest-bound, resumable stream journal used by durable SQLite to
/// PostgreSQL transfer. The journal remains non-serving until semantic
/// verification succeeds behind the target authority fence.
pub const STREAM_TRANSFER_V2_MIGRATION_ID: &str = "0017-stream-transfer-v2";
/// Adds cut-consistent operational witnesses for bounded checkpoint recovery.
pub const CHECKPOINT_OPERATIONAL_WITNESS_MIGRATION_ID: &str =
    "0018-checkpoint-operational-witness-v1";
/// Adds incrementally maintained roots for guard-only checkpoint histories.
pub const OPERATIONAL_HISTORY_ROOTS_MIGRATION_ID: &str = "0019-operational-history-roots-v2";
/// Adds the bounded current Timer materialization used only by V2 checkpoint
/// recovery. The immutable Timer ledger remains the forensic authority.
pub const CURRENT_TIMERS_MIGRATION_ID: &str = "0020-current-timers-v2";
/// Adds the compact V2 checkpoint witness without altering the frozen V1
/// witness table or its canonical schema constraint.
pub const CHECKPOINT_OPERATIONAL_WITNESS_V2_MIGRATION_ID: &str =
    "0021-checkpoint-operational-witness-v2";

/// A migration body and its stable identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MigrationDescriptor {
    /// One-based, contiguous migration version.
    pub version: i32,
    /// Stable logical identifier.
    pub id: &'static str,
    /// Backend-specific SQL body. It is never edited after release.
    pub sql: &'static str,
}

impl MigrationDescriptor {
    /// Returns the checksum that must be stored with this migration.
    #[must_use]
    pub fn checksum(self) -> Blake3DigestV1 {
        Blake3DigestV1::hash(self.sql.as_bytes())
    }
}

/// The migration ledger row as read from PostgreSQL or the local fixture seam.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MigrationRecord {
    /// Applied migration version.
    pub version: i32,
    /// Stored migration identifier.
    pub migration_id: String,
    /// Stored SQL checksum.
    pub checksum: Vec<u8>,
    /// Logical history metadata introduced by migration 2.
    pub logical_history_id: Option<String>,
    /// Schema fingerprint metadata introduced by migration 2.
    pub schema_contract_fingerprint: Option<Vec<u8>>,
}

impl MigrationRecord {
    /// Builds a legacy migration row, useful for testing an interrupted upgrade.
    #[must_use]
    pub fn legacy(version: i32, migration_id: impl Into<String>, checksum: &[u8]) -> Self {
        Self {
            version,
            migration_id: migration_id.into(),
            checksum: checksum.to_vec(),
            logical_history_id: None,
            schema_contract_fingerprint: None,
        }
    }
}

/// The canonical schema fingerprint material. It is independent of PostgreSQL
/// OIDs, physical indexes, and provider-specific catalog details.
pub const SCHEMA_FINGERPRINT_MATERIAL: &str = concat!(
    "worldstream_schema_migrations(",
    "version:integer:NO,",
    "migration_id:text:NO,",
    "checksum:bytea:NO,",
    "logical_history_id:text:YES,",
    "schema_contract_fingerprint:bytea:YES);",
    "worldstream_operation_guards(",
    "identity_bytes:bytea:NO,request_hash:bytea:NO,room_id:text:YES,receipt_bytes:bytea:YES);",
    "worldstream_external_input_preparations(",
    "identity_bytes:bytea:NO,canonical_request_hash:bytea:NO,recorded_at:text:NO);",
    "worldstream_room_roots(",
    "room_id:text:NO,head_bytes:bytea:NO,integrity_generation:bigint:NO,integrity_status:text:NO);",
    "worldstream_genesis(",
    "room_id:text:NO,pack_revision_lock_bytes:bytea:NO,genesis_bytes:bytea:NO);",
    "worldstream_materializations(",
    "room_id:text:NO,core_state_bytes:bytea:NO,activity_state_bytes:bytea:NO);",
    "worldstream_members(",
    "room_id:text:NO,member_id:text:NO,membership_bytes:bytea:NO,frame_head:bigint:NO,",
    "membership_generation:bigint:NO,retained_frame_floor:bigint:NO,last_ack_frame_seq:bigint:YES,",
    "reset_required_through:bigint:YES,reset_generation:bigint:NO);",
    "worldstream_timers(",
    "room_id:text:NO,timer_id:text:NO,generation:bigint:NO,scheduled_for:text:NO,",
    "payload_bytes:bytea:NO,state:text:NO);",
    "worldstream_room_current_timers_v2(",
    "room_id:text:NO,timer_id:text:NO,generation:bigint:NO,scheduled_for:text:NO,",
    "payload_bytes:bytea:NO,state:text:NO);",
    "worldstream_room_operational_history_roots_v2(",
    "room_id:text:NO,domain:text:NO,entry_count:bigint:NO,root_hash:bytea:NO);",
    "worldstream_transitions(",
    "room_id:text:NO,room_seq:bigint:NO,transition_bytes:bytea:NO);",
    "worldstream_frames(",
    "room_id:text:NO,member_id:text:NO,frame_seq:bigint:NO,cause_room_seq:bigint:NO,",
    "payload_bytes:bytea:NO,payload_hash:bytea:NO,retained_at:text:NO);",
    "worldstream_observation_consequences(",
    "room_id:text:NO,member_id:text:NO,cause_room_seq:bigint:NO,consequence_kind:text:NO,",
    "payload_bytes:bytea:YES,projection_hash:bytea:YES);",
    "worldstream_activation_decisions(",
    "room_id:text:NO,cause_room_seq:bigint:NO,decision_id:text:NO,",
    "target_member_id:text:YES,decision_bytes:bytea:NO);",
    "worldstream_activation_intents(",
    "activation_id:text:NO,room_id:text:NO,cause_room_seq:bigint:NO,decision_id:text:NO,",
    "target_member_id:text:NO,reason_code:text:NO,deduplication_key:text:NO,priority:bigint:NO,",
    "semantic_deadline:text:YES,policy_revision:bigint:NO,state:text:NO,intent_generation:bigint:NO,",
    "lease_generation:bigint:NO,runner_id:text:YES,claim_id:text:YES,lease_until:text:YES,",
    "context_hash:bytea:YES,context_bytes:bytea:YES,context_retired:boolean:NO,",
    "created_at:text:NO,attention_bytes:bigint:NO,terminal_disposition:text:YES,",
    "superseded_by_activation_id:text:YES,terminal_at:text:YES);",
    "worldstream_activation_operation_receipts(",
    "room_id:text:NO,operation_id:text:NO,operation_kind:text:NO,canonical_request_hash:bytea:NO,",
    "activation_id:text:YES,result_code:text:NO,result_bytes:bytea:NO,context_hash:bytea:YES,",
    "context_bytes:bytea:YES);",
    "worldstream_room_snapshots(",
    "room_id:text:NO,room_seq:bigint:NO,snapshot_schema_version:text:NO,",
    "genesis_or_transition_hash:text:NO,core_schema_version:text:NO,pack_digest:text:NO,",
    "core_state_hash:text:NO,activity_state_hash:text:NO,authoritative_state_hash:text:NO,",
    "complete_head_bytes:bytea:NO,core_state_bytes:bytea:NO,activity_state_bytes:bytea:NO);",
    "worldstream_room_snapshot_operational_witnesses(",
    "room_id:text:NO,room_seq:bigint:NO,witness_schema_version:text:NO,",
    "witness_hash:bytea:NO,witness_bytes:bytea:NO);",
    "worldstream_room_snapshot_operational_witnesses_v2(",
    "room_id:text:NO,room_seq:bigint:NO,witness_schema_version:text:NO,",
    "witness_hash:bytea:NO,witness_bytes:bytea:NO);",
    "worldstream_room_snapshot_schedules(",
    "room_id:text:NO,last_snapshot_room_seq:bigint:NO,transitions_since_snapshot:bigint:NO,",
    "active_started_at:text:YES);",
    "worldstream_semantic_receipts(",
    "identity_bytes:bytea:NO,operation_kind:text:NO,canonical_request_hash:bytea:NO,",
    "basis_complete_head_bytes:bytea:YES,semantic_input_bytes:bytea:NO,semantic_time_bytes:bytea:NO,",
    "resolution_kind:text:NO,transition_seq:bigint:YES,receipt_bytes:bytea:NO,room_id:text:YES);",
    "worldstream_integrity_incidents(",
    "room_id:text:NO,incident_seq:bigint:NO,generation:bigint:NO,status:text:NO,reason_code:text:NO,",
    "details_bytes:bytea:YES);",
    "worldstream_authority_fences(",
    "witness_id:text:NO,authenticated_principal:text:NO,generation:bigint:NO,",
    "scope_revocation_hash:bytea:NO,active:boolean:NO);",
    "worldstream_authority_state(",
    "authority_id:boolean:NO);",
    "worldstream_authority_principals(",
    "principal_id:text:NO,principal_kind:text:NO,authority_status:text:NO,",
    "principal_generation:bigint:NO);",
    "worldstream_authority_runners(",
    "runner_id:text:NO,owner_principal_id:text:NO,authority_status:text:NO,",
    "runner_generation:bigint:NO);",
    "worldstream_authority_capabilities(",
    "capability_id:text:NO,token_hash:bytea:NO,principal_id:text:NO,profile_kind:text:NO,",
    "target_room_id:text:YES,target_member_id:text:YES,runner_id:text:YES,",
    "authority_generation:bigint:NO,expires_at:text:YES,revoked_at:text:YES);",
    "worldstream_authority_capability_scopes(",
    "capability_id:text:NO,scope:text:NO);",
    "worldstream_authority_runner_capability_memberships(",
    "capability_id:text:NO,room_id:text:NO,member_id:text:NO);",
    "worldstream_authority_change_receipts(",
    "change_id:text:NO,authenticated_principal:text:YES,request_hash:bytea:NO,",
    "result_kind:text:NO,target_kind:text:NO,target_id:text:NO,secondary_target_id:text:YES,",
    "resulting_generation:bigint:NO,checked_at:text:NO);",
    "worldstream_authority_audit(",
    "audit_seq:bigint:NO,change_id:text:NO,actor_principal_id:text:YES,target_kind:text:NO,",
    "target_id:text:NO,secondary_target_id:text:YES,change_kind:text:NO,",
    "prior_generation:bigint:YES,resulting_generation:bigint:NO,checked_at:text:NO,",
    "reason_code:text:YES,request_hash:bytea:NO);",
    "worldstream_transfer_imports(",
    "bundle_hash:bytea:NO,target_fingerprint:bytea:NO,state:text:NO);",
    "worldstream_transfer_chunks(",
    "bundle_hash:bytea:NO,chunk_start:bigint:NO,chunk_end:bigint:NO,",
    "chunk_digest:bytea:NO,records_bytes:bytea:NO);",
    "worldstream_transfer_target_fence(",
    "fence_id:boolean:NO,bundle_hash:bytea:NO,target_fingerprint:bytea:NO,state:text:NO);",
    "worldstream_transfer_stream_imports_v2(",
    "stream_header_digest:bytea:NO,target_fingerprint:bytea:NO,manifest_bytes:bytea:NO,",
    "manifest_digest:bytea:NO,state:text:NO,next_chunk:bigint:NO,next_ordinal:bigint:NO,",
    "footer_chunk_count:bigint:YES,footer_record_count:bigint:YES,footer_record_bytes:bigint:YES,",
    "footer_digest:bytea:YES);",
    "worldstream_transfer_stream_chunks_v2(",
    "stream_header_digest:bytea:NO,chunk_index:bigint:NO,chunk_start:bigint:NO,",
    "chunk_end:bigint:NO,chunk_digest:bytea:NO,records_bytes:bytea:NO);",
    "worldstream_transfer_stream_records_v2(",
    "stream_header_digest:bytea:NO,ordinal:bigint:NO,class_tag:smallint:NO,kind_tag:smallint:NO,",
    "identity:text:NO,record_bytes:bytea:NO,record_digest:bytea:NO);",
    "worldstream_deployment_metadata(",
    "target_id:boolean:NO,deployment_lineage_bytes:bytea:NO,storage_epoch_bytes:bytea:NO,",
    "storage_epoch:bigint:NO);",
    "worldstream_deployment_identity_metadata(",
    "target_id:boolean:NO,identity_digest:bytea:NO,pack_set_digest:bytea:NO,",
    "resource_set_digest:bytea:NO,canonical_bytes:bytea:NO);",
    "worldstream_deployment_pack_identities(",
    "pack_id:text:NO,revision:text:NO,pack_digest:bytea:NO);",
    "worldstream_deployment_resource_identities(",
    "resource_kind:text:NO,resource_identity:text:NO,size_bytes:bigint:NO,",
    "resource_digest:bytea:NO);",
    "worldstream_deployment_resource_blobs(",
    "resource_kind:text:NO,resource_identity:text:NO,resource_bytes:bytea:NO,",
    "resource_digest:bytea:NO);",
    "worldstream_retired_authority_fences_v1(",
    "witness_id:text:NO,authenticated_principal:text:NO,generation:bigint:NO,",
    "scope_revocation_bytes:bytea:NO,scope_revocation_hash:bytea:NO,active:boolean:NO);",
);

/// Computes the release-published schema fingerprint.
#[must_use]
pub fn schema_contract_fingerprint() -> Blake3DigestV1 {
    Blake3DigestV1::hash(SCHEMA_FINGERPRINT_MATERIAL.as_bytes())
}

/// The complete ordered migration history.
#[must_use]
pub fn migration_history() -> [MigrationDescriptor; 21] {
    [
        MigrationDescriptor {
            version: 1,
            id: INITIAL_MIGRATION_ID,
            sql: super::SCHEMA,
        },
        MigrationDescriptor {
            version: 2,
            id: AUTHORITY_MIGRATION_ID,
            sql: MIGRATION_0002_SQL,
        },
        MigrationDescriptor {
            version: 3,
            id: KERNEL_CONFORMANCE_MIGRATION_ID,
            sql: MIGRATION_0003_SQL,
        },
        MigrationDescriptor {
            version: 4,
            id: KERNEL_PARITY_MIGRATION_ID,
            sql: MIGRATION_0004_SQL,
        },
        MigrationDescriptor {
            version: 5,
            id: TRANSFER_PUBLICATION_MIGRATION_ID,
            sql: MIGRATION_0005_SQL,
        },
        MigrationDescriptor {
            version: 6,
            id: TRANSFER_TARGET_FENCE_MIGRATION_ID,
            sql: MIGRATION_0006_SQL,
        },
        MigrationDescriptor {
            version: 7,
            id: DEPLOYMENT_METADATA_MIGRATION_ID,
            sql: MIGRATION_0007_SQL,
        },
        MigrationDescriptor {
            version: 8,
            id: DEPLOYMENT_IDENTITY_MIGRATION_ID,
            sql: MIGRATION_0008_SQL,
        },
        MigrationDescriptor {
            version: 9,
            id: AUTHORITY_FACTS_MIGRATION_ID,
            sql: MIGRATION_0009_SQL,
        },
        MigrationDescriptor {
            version: 10,
            id: TRANSFER_RECOVERY_COMPLETENESS_MIGRATION_ID,
            sql: MIGRATION_0010_SQL,
        },
        MigrationDescriptor {
            version: 11,
            id: TRANSFER_RESOURCE_IDENTITY_MIGRATION_ID,
            sql: MIGRATION_0011_SQL,
        },
        MigrationDescriptor {
            version: 12,
            id: EXTERNAL_INPUT_PREPARATION_MIGRATION_ID,
            sql: MIGRATION_0012_SQL,
        },
        MigrationDescriptor {
            version: 13,
            id: OBSERVATION_RESET_GENERATION_MIGRATION_ID,
            sql: MIGRATION_0013_SQL,
        },
        MigrationDescriptor {
            version: 14,
            id: OBSERVATION_RETENTION_MIGRATION_ID,
            sql: MIGRATION_0014_SQL,
        },
        MigrationDescriptor {
            version: 15,
            id: SNAPSHOT_CADENCE_MIGRATION_ID,
            sql: MIGRATION_0015_SQL,
        },
        MigrationDescriptor {
            version: 16,
            id: ACTIVATION_BACKLOG_POLICY_MIGRATION_ID,
            sql: MIGRATION_0016_SQL,
        },
        MigrationDescriptor {
            version: 17,
            id: STREAM_TRANSFER_V2_MIGRATION_ID,
            sql: MIGRATION_0017_SQL,
        },
        MigrationDescriptor {
            version: 18,
            id: CHECKPOINT_OPERATIONAL_WITNESS_MIGRATION_ID,
            sql: MIGRATION_0018_SQL,
        },
        MigrationDescriptor {
            version: 19,
            id: OPERATIONAL_HISTORY_ROOTS_MIGRATION_ID,
            sql: MIGRATION_0019_SQL,
        },
        MigrationDescriptor {
            version: 20,
            id: CURRENT_TIMERS_MIGRATION_ID,
            sql: MIGRATION_0020_SQL,
        },
        MigrationDescriptor {
            version: 21,
            id: CHECKPOINT_OPERATIONAL_WITNESS_V2_MIGRATION_ID,
            sql: MIGRATION_0021_SQL,
        },
    ]
}

/// DDL and metadata-shape change for the second migration.
pub const MIGRATION_0002_SQL: &str = r"
ALTER TABLE worldstream_schema_migrations
    ADD COLUMN logical_history_id text;
ALTER TABLE worldstream_schema_migrations
    ADD COLUMN schema_contract_fingerprint bytea;
CREATE TABLE IF NOT EXISTS worldstream_authority_fences (
    witness_id text PRIMARY KEY,
    authenticated_principal text NOT NULL,
    generation bigint NOT NULL CHECK (generation > 0),
    scope_revocation_hash bytea NOT NULL,
    active boolean NOT NULL
);
";

/// Adds only storage witnesses whose bytes are already sealed by Core. The
/// migration does not introduce a second source of truth or any provider
/// callback surface.
pub const MIGRATION_0003_SQL: &str = r"
ALTER TABLE worldstream_members
    ADD COLUMN IF NOT EXISTS membership_generation bigint NOT NULL DEFAULT 1 CHECK (membership_generation > 0),
    ADD COLUMN IF NOT EXISTS retained_frame_floor bigint NOT NULL DEFAULT 1 CHECK (retained_frame_floor > 0),
    ADD COLUMN IF NOT EXISTS last_ack_frame_seq bigint CHECK (last_ack_frame_seq IS NULL OR last_ack_frame_seq > 0),
    ADD COLUMN IF NOT EXISTS reset_required_through bigint CHECK (reset_required_through IS NULL OR reset_required_through >= 0);
CREATE UNIQUE INDEX worldstream_one_scheduled_timer_generation
    ON worldstream_timers(room_id, timer_id) WHERE state = 'scheduled';
CREATE TABLE worldstream_observation_consequences (
    room_id text NOT NULL,
    member_id text NOT NULL,
    cause_room_seq bigint NOT NULL,
    consequence_kind text NOT NULL CHECK (consequence_kind IN ('reset_required', 'visibility_lost')),
    payload_bytes bytea,
    projection_hash bytea,
    PRIMARY KEY (room_id, member_id, cause_room_seq)
);
CREATE TABLE worldstream_activation_intents (
    activation_id text PRIMARY KEY,
    room_id text NOT NULL,
    cause_room_seq bigint NOT NULL CHECK (cause_room_seq > 0),
    decision_id text NOT NULL,
    target_member_id text NOT NULL,
    reason_code text NOT NULL,
    deduplication_key text NOT NULL,
    priority bigint NOT NULL CHECK (priority >= 0),
    semantic_deadline text,
    policy_revision bigint NOT NULL CHECK (policy_revision > 0),
    state text NOT NULL CHECK (state IN ('pending', 'leased', 'completed', 'expired', 'cancelled')),
    intent_generation bigint NOT NULL CHECK (intent_generation > 0),
    lease_generation bigint NOT NULL CHECK (lease_generation >= 0),
    runner_id text,
    claim_id text,
    lease_until text,
    context_hash bytea CHECK (context_hash IS NULL OR octet_length(context_hash) = 32),
    context_bytes bytea,
    context_retired boolean NOT NULL DEFAULT false,
    CHECK ((state = 'leased') = (runner_id IS NOT NULL AND claim_id IS NOT NULL AND lease_until IS NOT NULL)),
    CHECK ((context_hash IS NULL AND context_bytes IS NULL) OR (context_hash IS NOT NULL AND (context_bytes IS NOT NULL OR context_retired)))
);
CREATE UNIQUE INDEX worldstream_activation_dedup
    ON worldstream_activation_intents(room_id, cause_room_seq, target_member_id, deduplication_key);
CREATE UNIQUE INDEX worldstream_activation_one_live_lease
    ON worldstream_activation_intents(room_id, target_member_id) WHERE state = 'leased';
CREATE TABLE worldstream_activation_operation_receipts (
    room_id text NOT NULL,
    operation_id text NOT NULL,
    operation_kind text NOT NULL CHECK (operation_kind IN ('offer', 'claim', 'renew', 'complete', 'release')),
    canonical_request_hash bytea NOT NULL CHECK (octet_length(canonical_request_hash) = 32),
    activation_id text,
    result_code text NOT NULL,
    result_bytes bytea NOT NULL,
    context_hash bytea CHECK (context_hash IS NULL OR octet_length(context_hash) = 32),
    context_bytes bytea,
    PRIMARY KEY (room_id, operation_id)
);
CREATE TABLE worldstream_room_snapshots (
    room_id text NOT NULL,
    room_seq bigint NOT NULL CHECK (room_seq >= 0),
    snapshot_schema_version text NOT NULL CHECK (snapshot_schema_version = 'worldstream/paired-snapshot/v1'),
    genesis_or_transition_hash text NOT NULL,
    core_schema_version text NOT NULL,
    pack_digest text NOT NULL,
    core_state_hash text NOT NULL,
    activity_state_hash text NOT NULL,
    authoritative_state_hash text NOT NULL,
    complete_head_bytes bytea NOT NULL,
    core_state_bytes bytea NOT NULL,
    activity_state_bytes bytea NOT NULL,
    PRIMARY KEY (room_id, room_seq)
);
CREATE TABLE worldstream_semantic_receipts (
    identity_bytes bytea PRIMARY KEY,
    operation_kind text NOT NULL,
    canonical_request_hash bytea NOT NULL CHECK (octet_length(canonical_request_hash) = 32),
    basis_complete_head_bytes bytea,
    semantic_input_bytes bytea NOT NULL,
    semantic_time_bytes bytea NOT NULL,
    resolution_kind text NOT NULL,
    transition_seq bigint,
    receipt_bytes bytea NOT NULL
);
CREATE TABLE worldstream_integrity_incidents (
    room_id text NOT NULL,
    incident_seq bigint NOT NULL CHECK (incident_seq > 0),
    generation bigint NOT NULL CHECK (generation > 0),
    status text NOT NULL CHECK (status IN ('healthy', 'faulted', 'quarantined')),
    reason_code text NOT NULL,
    details_bytes bytea,
    PRIMARY KEY (room_id, incident_seq)
);
";

/// Adds provider-enforced uniqueness and the room discriminator required to
/// keep the operational semantic-receipt ledger aligned with SQLite. This is
/// deliberately a new forward migration: migration 0003 is immutable once a
/// database has recorded its checksum.
pub const MIGRATION_0004_SQL: &str = r"
ALTER TABLE worldstream_semantic_receipts
    ADD COLUMN room_id text;
CREATE UNIQUE INDEX worldstream_observation_frames_one_per_member_transition
    ON worldstream_frames(room_id, member_id, cause_room_seq);
CREATE UNIQUE INDEX worldstream_semantic_receipts_by_transition
    ON worldstream_semantic_receipts(room_id, transition_seq)
    WHERE transition_seq IS NOT NULL AND room_id IS NOT NULL;
CREATE UNIQUE INDEX worldstream_semantic_receipts_one_genesis_per_room
    ON worldstream_semantic_receipts(room_id)
    WHERE resolution_kind = 'genesis_created' AND room_id IS NOT NULL;
CREATE INDEX worldstream_activation_pending_by_member
    ON worldstream_activation_intents(room_id, target_member_id, state, priority DESC, cause_room_seq);
";

/// Adds the provider-facing, byte-preserving transfer publication seam.
///
/// The import and chunk rows are deliberately separate from Core semantic
/// tables. A successful transfer therefore proves exact staged-byte parity and
/// fencing, not that the target has been hydrated into a complete live Room.
pub const MIGRATION_0005_SQL: &str = r"
CREATE TABLE worldstream_transfer_imports (
    bundle_hash bytea PRIMARY KEY CHECK (octet_length(bundle_hash) = 32),
    target_fingerprint bytea NOT NULL CHECK (octet_length(target_fingerprint) = 32),
    state text NOT NULL CHECK (state IN ('pending', 'verified', 'finalized', 'authoritative'))
);
CREATE TABLE worldstream_transfer_chunks (
    bundle_hash bytea NOT NULL REFERENCES worldstream_transfer_imports(bundle_hash) ON DELETE CASCADE,
    chunk_start bigint NOT NULL CHECK (chunk_start >= 0),
    chunk_end bigint NOT NULL CHECK (chunk_end > chunk_start),
    chunk_digest bytea NOT NULL CHECK (octet_length(chunk_digest) = 32),
    records_bytes bytea NOT NULL,
    PRIMARY KEY (bundle_hash, chunk_start)
);
";

/// Adds a singleton row that serializes target admission even when the target
/// has no prior import rows. The fence is removed only by an abort of the
/// matching incomplete import, and survives finalization/authority.
pub const MIGRATION_0006_SQL: &str = r"
CREATE TABLE worldstream_transfer_target_fence (
    fence_id boolean PRIMARY KEY DEFAULT true CHECK (fence_id),
    bundle_hash bytea NOT NULL CHECK (octet_length(bundle_hash) = 32),
    target_fingerprint bytea NOT NULL CHECK (octet_length(target_fingerprint) = 32)
);
";

/// Adds one target-wide canonical deployment identity row. The bytes are
/// supplied by the deployment/transfer boundary and are never reconstructed
/// from Room state. Runtime processes only read this table; publication can
/// later write it in the same ordinary transaction as the canonical bundle.
pub const MIGRATION_0007_SQL: &str = r"
CREATE TABLE worldstream_deployment_metadata (
    target_id boolean PRIMARY KEY DEFAULT true CHECK (target_id),
    deployment_lineage_bytes bytea NOT NULL CHECK (octet_length(deployment_lineage_bytes) > 0),
    storage_epoch_bytes bytea NOT NULL CHECK (octet_length(storage_epoch_bytes) > 0),
    storage_epoch bigint NOT NULL CHECK (storage_epoch >= 0)
);
";

/// Adds the target-side projection of the source-authenticated deployment
/// identity. The canonical bytes are the completeness witness; normalized
/// rows make the exact pack/resource set queryable without becoming a second
/// source of truth.
pub const MIGRATION_0008_SQL: &str = r"
CREATE TABLE worldstream_deployment_identity_metadata (
    target_id boolean PRIMARY KEY DEFAULT true CHECK (target_id),
    identity_digest bytea NOT NULL CHECK (octet_length(identity_digest) = 32),
    pack_set_digest bytea NOT NULL CHECK (octet_length(pack_set_digest) = 32),
    resource_set_digest bytea NOT NULL CHECK (octet_length(resource_set_digest) = 32),
    canonical_bytes bytea NOT NULL CHECK (octet_length(canonical_bytes) > 0)
);
CREATE TABLE worldstream_deployment_pack_identities (
    pack_id text NOT NULL,
    revision text NOT NULL,
    pack_digest bytea NOT NULL CHECK (octet_length(pack_digest) = 32),
    PRIMARY KEY (pack_id, revision)
);
CREATE TABLE worldstream_deployment_resource_identities (
    resource_kind text NOT NULL CHECK (resource_kind IN ('artifact', 'codec', 'schema')),
    resource_identity text NOT NULL,
    size_bytes bigint NOT NULL CHECK (size_bytes >= 0),
    resource_digest bytea NOT NULL CHECK (octet_length(resource_digest) = 32),
    PRIMARY KEY (resource_kind, resource_identity)
);
";

/// Adds the durable operational authority facts consumed by Core's
/// `AuthorityStoreV1`. Existing migration bodies remain immutable; this
/// migration is intentionally additive and is safe to apply to a database
/// that has already recorded migration 0008.
pub const MIGRATION_0009_SQL: &str = r"
CREATE TABLE worldstream_authority_state (
    authority_id boolean PRIMARY KEY DEFAULT true CHECK (authority_id)
);
INSERT INTO worldstream_authority_state(authority_id) VALUES (true);

CREATE TABLE worldstream_authority_principals (
    principal_id text PRIMARY KEY,
    principal_kind text NOT NULL CHECK (principal_kind IN ('human', 'agent')),
    authority_status text NOT NULL CHECK (authority_status IN ('enabled', 'disabled')),
    principal_generation bigint NOT NULL CHECK (principal_generation BETWEEN 1 AND 9007199254740991)
);

CREATE TABLE worldstream_authority_runners (
    runner_id text PRIMARY KEY,
    owner_principal_id text NOT NULL REFERENCES worldstream_authority_principals(principal_id),
    authority_status text NOT NULL CHECK (authority_status IN ('enabled', 'revoked')),
    runner_generation bigint NOT NULL CHECK (runner_generation BETWEEN 1 AND 9007199254740991)
);

CREATE TABLE worldstream_authority_capabilities (
    capability_id text PRIMARY KEY,
    token_hash bytea NOT NULL UNIQUE CHECK (octet_length(token_hash) = 32),
    principal_id text NOT NULL REFERENCES worldstream_authority_principals(principal_id),
    profile_kind text NOT NULL CHECK (profile_kind IN ('room_member', 'host_operator', 'runner_control')),
    target_room_id text,
    target_member_id text,
    runner_id text REFERENCES worldstream_authority_runners(runner_id),
    authority_generation bigint NOT NULL CHECK (authority_generation BETWEEN 1 AND 9007199254740991),
    expires_at text,
    revoked_at text,
    CHECK (
        (profile_kind = 'room_member' AND target_room_id IS NOT NULL AND target_member_id IS NOT NULL AND runner_id IS NULL)
        OR (profile_kind = 'host_operator' AND target_member_id IS NULL AND runner_id IS NULL)
        OR (profile_kind = 'runner_control' AND target_room_id IS NULL AND target_member_id IS NULL AND runner_id IS NOT NULL)
    )
);
CREATE INDEX worldstream_authority_capabilities_by_principal
    ON worldstream_authority_capabilities(principal_id);
CREATE INDEX worldstream_authority_capabilities_by_runner
    ON worldstream_authority_capabilities(runner_id) WHERE runner_id IS NOT NULL;

CREATE TABLE worldstream_authority_capability_scopes (
    capability_id text NOT NULL REFERENCES worldstream_authority_capabilities(capability_id) ON DELETE CASCADE,
    scope text NOT NULL CHECK (scope IN (
        'room:attach', 'room:act', 'room:observe_public', 'room:observe_member',
        'room:replay', 'activation:offer_receive', 'activation:claim',
        'activation:complete', 'operator:room_admin', 'operator:backup'
    )),
    PRIMARY KEY (capability_id, scope)
);

CREATE TABLE worldstream_authority_runner_capability_memberships (
    capability_id text NOT NULL REFERENCES worldstream_authority_capabilities(capability_id) ON DELETE CASCADE,
    room_id text NOT NULL,
    member_id text NOT NULL,
    PRIMARY KEY (capability_id, room_id, member_id)
);

CREATE TABLE worldstream_authority_change_receipts (
    change_id text PRIMARY KEY,
    authenticated_principal text REFERENCES worldstream_authority_principals(principal_id),
    request_hash bytea NOT NULL CHECK (octet_length(request_hash) = 32),
    result_kind text NOT NULL CHECK (result_kind IN (
        'authority_bootstrapped', 'principal_created', 'capability_registered',
        'capability_narrowed', 'capability_revoked', 'principal_status_changed',
        'runner_registered', 'runner_revoked'
    )),
    target_kind text NOT NULL CHECK (target_kind IN ('bootstrap', 'principal', 'capability', 'runner')),
    target_id text NOT NULL,
    secondary_target_id text,
    resulting_generation bigint NOT NULL CHECK (resulting_generation BETWEEN 1 AND 9007199254740991),
    checked_at text NOT NULL,
    CHECK (
        (result_kind = 'authority_bootstrapped' AND authenticated_principal IS NULL
            AND target_kind = 'bootstrap' AND secondary_target_id IS NOT NULL AND resulting_generation = 1)
        OR (result_kind <> 'authority_bootstrapped' AND authenticated_principal IS NOT NULL
            AND target_kind <> 'bootstrap' AND secondary_target_id IS NULL)
    )
);

CREATE TABLE worldstream_authority_audit (
    audit_seq bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    change_id text NOT NULL UNIQUE REFERENCES worldstream_authority_change_receipts(change_id),
    actor_principal_id text REFERENCES worldstream_authority_principals(principal_id),
    target_kind text NOT NULL CHECK (target_kind IN ('bootstrap', 'principal', 'capability', 'runner')),
    target_id text NOT NULL,
    secondary_target_id text,
    change_kind text NOT NULL CHECK (change_kind IN (
        'bootstrap_authority', 'create_principal', 'register_capability',
        'register_runner', 'narrow_capability', 'revoke_capability',
        'revoke_runner', 'set_principal_status'
    )),
    prior_generation bigint CHECK (prior_generation BETWEEN 1 AND 9007199254740991),
    resulting_generation bigint NOT NULL CHECK (resulting_generation BETWEEN 1 AND 9007199254740991),
    checked_at text NOT NULL,
    reason_code text,
    request_hash bytea NOT NULL CHECK (octet_length(request_hash) = 32),
    CHECK ((prior_generation IS NULL AND resulting_generation = 1)
        OR resulting_generation = prior_generation + 1),
    CHECK (
        (change_kind = 'bootstrap_authority' AND actor_principal_id IS NULL
            AND target_kind = 'bootstrap' AND secondary_target_id IS NOT NULL)
        OR (change_kind <> 'bootstrap_authority' AND actor_principal_id IS NOT NULL
            AND target_kind <> 'bootstrap' AND secondary_target_id IS NULL)
    )
);

CREATE OR REPLACE FUNCTION worldstream_reject_authority_fact_mutation()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'WorldStream authority facts are immutable';
END;
$$;
CREATE TRIGGER worldstream_authority_receipts_immutable_update
    BEFORE UPDATE OR DELETE ON worldstream_authority_change_receipts
    FOR EACH ROW EXECUTE FUNCTION worldstream_reject_authority_fact_mutation();
CREATE TRIGGER worldstream_authority_audit_immutable_update
    BEFORE UPDATE OR DELETE ON worldstream_authority_audit
    FOR EACH ROW EXECUTE FUNCTION worldstream_reject_authority_fact_mutation();
";

/// Retains exact content-addressed deployment bytes and the legacy authority
/// fence rows that remain durable in a migrated SQLite source. These are
/// immutable transfer/restore facts, not mutable runtime authority state.
pub const MIGRATION_0010_SQL: &str = r"
CREATE TABLE worldstream_deployment_resource_blobs (
    resource_kind text NOT NULL CHECK (resource_kind IN ('artifact', 'codec', 'schema')),
    resource_identity text NOT NULL,
    resource_bytes bytea NOT NULL CHECK (octet_length(resource_bytes) <= 16777216),
    resource_digest bytea NOT NULL CHECK (octet_length(resource_digest) = 32),
    PRIMARY KEY (resource_kind, resource_identity),
    FOREIGN KEY (resource_kind, resource_identity)
        REFERENCES worldstream_deployment_resource_identities(resource_kind, resource_identity)
        ON DELETE RESTRICT
);
CREATE TABLE worldstream_retired_authority_fences_v1 (
    witness_id text PRIMARY KEY,
    authenticated_principal text NOT NULL,
    generation bigint NOT NULL CHECK (generation BETWEEN 1 AND 9007199254740991),
    scope_revocation_bytes bytea NOT NULL,
    scope_revocation_hash bytea NOT NULL CHECK (octet_length(scope_revocation_hash) = 32),
    active boolean NOT NULL
);
CREATE TRIGGER worldstream_deployment_resource_blobs_immutable
    BEFORE UPDATE OR DELETE ON worldstream_deployment_resource_blobs
    FOR EACH ROW EXECUTE FUNCTION worldstream_reject_authority_fact_mutation();
CREATE TRIGGER worldstream_retired_authority_fences_immutable
    BEFORE UPDATE OR DELETE ON worldstream_retired_authority_fences_v1
    FOR EACH ROW EXECUTE FUNCTION worldstream_reject_authority_fact_mutation();
";

/// Closes the deployment resource namespace globally. Backup and transfer
/// references carry the identity alone, so the same identity may not name
/// different artifact/schema/codec rows.
pub const MIGRATION_0011_SQL: &str = r"
CREATE UNIQUE INDEX worldstream_deployment_resource_identity_global_v1
    ON worldstream_deployment_resource_identities(resource_identity);
CREATE UNIQUE INDEX worldstream_deployment_resource_blob_identity_global_v1
    ON worldstream_deployment_resource_blobs(resource_identity);
ALTER TABLE worldstream_transfer_target_fence
    ADD COLUMN state text NOT NULL DEFAULT 'importing'
    CHECK (state IN ('importing', 'aborted'));
CREATE OR REPLACE FUNCTION worldstream_reject_write_while_transfer_fenced()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS (SELECT 1 FROM public.worldstream_transfer_target_fence WHERE fence_id = true) THEN
        RAISE EXCEPTION 'WorldStream target is non-serving during transfer';
    END IF;
    RETURN NULL;
END;
$$;
CREATE TRIGGER worldstream_transfer_fence_operation_guards
    BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON worldstream_operation_guards
    FOR EACH STATEMENT EXECUTE FUNCTION worldstream_reject_write_while_transfer_fenced();
CREATE TRIGGER worldstream_transfer_fence_room_roots
    BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON worldstream_room_roots
    FOR EACH STATEMENT EXECUTE FUNCTION worldstream_reject_write_while_transfer_fenced();
CREATE TRIGGER worldstream_transfer_fence_genesis
    BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON worldstream_genesis
    FOR EACH STATEMENT EXECUTE FUNCTION worldstream_reject_write_while_transfer_fenced();
CREATE TRIGGER worldstream_transfer_fence_materializations
    BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON worldstream_materializations
    FOR EACH STATEMENT EXECUTE FUNCTION worldstream_reject_write_while_transfer_fenced();
CREATE TRIGGER worldstream_transfer_fence_members
    BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON worldstream_members
    FOR EACH STATEMENT EXECUTE FUNCTION worldstream_reject_write_while_transfer_fenced();
CREATE TRIGGER worldstream_transfer_fence_timers
    BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON worldstream_timers
    FOR EACH STATEMENT EXECUTE FUNCTION worldstream_reject_write_while_transfer_fenced();
CREATE TRIGGER worldstream_transfer_fence_transitions
    BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON worldstream_transitions
    FOR EACH STATEMENT EXECUTE FUNCTION worldstream_reject_write_while_transfer_fenced();
CREATE TRIGGER worldstream_transfer_fence_frames
    BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON worldstream_frames
    FOR EACH STATEMENT EXECUTE FUNCTION worldstream_reject_write_while_transfer_fenced();
CREATE TRIGGER worldstream_transfer_fence_observation_consequences
    BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON worldstream_observation_consequences
    FOR EACH STATEMENT EXECUTE FUNCTION worldstream_reject_write_while_transfer_fenced();
CREATE TRIGGER worldstream_transfer_fence_activation_decisions
    BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON worldstream_activation_decisions
    FOR EACH STATEMENT EXECUTE FUNCTION worldstream_reject_write_while_transfer_fenced();
CREATE TRIGGER worldstream_transfer_fence_activation_intents
    BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON worldstream_activation_intents
    FOR EACH STATEMENT EXECUTE FUNCTION worldstream_reject_write_while_transfer_fenced();
CREATE TRIGGER worldstream_transfer_fence_activation_receipts
    BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON worldstream_activation_operation_receipts
    FOR EACH STATEMENT EXECUTE FUNCTION worldstream_reject_write_while_transfer_fenced();
CREATE TRIGGER worldstream_transfer_fence_room_snapshots
    BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON worldstream_room_snapshots
    FOR EACH STATEMENT EXECUTE FUNCTION worldstream_reject_write_while_transfer_fenced();
CREATE TRIGGER worldstream_transfer_fence_semantic_receipts
    BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON worldstream_semantic_receipts
    FOR EACH STATEMENT EXECUTE FUNCTION worldstream_reject_write_while_transfer_fenced();
CREATE TRIGGER worldstream_transfer_fence_integrity_incidents
    BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON worldstream_integrity_incidents
    FOR EACH STATEMENT EXECUTE FUNCTION worldstream_reject_write_while_transfer_fenced();
CREATE TRIGGER worldstream_transfer_fence_authority_fences
    BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON worldstream_authority_fences
    FOR EACH STATEMENT EXECUTE FUNCTION worldstream_reject_write_while_transfer_fenced();
CREATE TRIGGER worldstream_transfer_fence_authority_state
    BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON worldstream_authority_state
    FOR EACH STATEMENT EXECUTE FUNCTION worldstream_reject_write_while_transfer_fenced();
CREATE TRIGGER worldstream_transfer_fence_authority_principals
    BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON worldstream_authority_principals
    FOR EACH STATEMENT EXECUTE FUNCTION worldstream_reject_write_while_transfer_fenced();
CREATE TRIGGER worldstream_transfer_fence_authority_runners
    BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON worldstream_authority_runners
    FOR EACH STATEMENT EXECUTE FUNCTION worldstream_reject_write_while_transfer_fenced();
CREATE TRIGGER worldstream_transfer_fence_authority_capabilities
    BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON worldstream_authority_capabilities
    FOR EACH STATEMENT EXECUTE FUNCTION worldstream_reject_write_while_transfer_fenced();
CREATE TRIGGER worldstream_transfer_fence_authority_scopes
    BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON worldstream_authority_capability_scopes
    FOR EACH STATEMENT EXECUTE FUNCTION worldstream_reject_write_while_transfer_fenced();
CREATE TRIGGER worldstream_transfer_fence_authority_runner_memberships
    BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON worldstream_authority_runner_capability_memberships
    FOR EACH STATEMENT EXECUTE FUNCTION worldstream_reject_write_while_transfer_fenced();
CREATE TRIGGER worldstream_transfer_fence_authority_change_receipts
    BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON worldstream_authority_change_receipts
    FOR EACH STATEMENT EXECUTE FUNCTION worldstream_reject_write_while_transfer_fenced();
CREATE TRIGGER worldstream_transfer_fence_authority_audit
    BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON worldstream_authority_audit
    FOR EACH STATEMENT EXECUTE FUNCTION worldstream_reject_write_while_transfer_fenced();
CREATE TRIGGER worldstream_transfer_fence_deployment_metadata
    BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON worldstream_deployment_metadata
    FOR EACH STATEMENT EXECUTE FUNCTION worldstream_reject_write_while_transfer_fenced();
CREATE TRIGGER worldstream_transfer_fence_deployment_identity_metadata
    BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON worldstream_deployment_identity_metadata
    FOR EACH STATEMENT EXECUTE FUNCTION worldstream_reject_write_while_transfer_fenced();
CREATE TRIGGER worldstream_transfer_fence_deployment_packs
    BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON worldstream_deployment_pack_identities
    FOR EACH STATEMENT EXECUTE FUNCTION worldstream_reject_write_while_transfer_fenced();
CREATE TRIGGER worldstream_transfer_fence_deployment_resources
    BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON worldstream_deployment_resource_identities
    FOR EACH STATEMENT EXECUTE FUNCTION worldstream_reject_write_while_transfer_fenced();
CREATE TRIGGER worldstream_transfer_fence_deployment_resource_blobs
    BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON worldstream_deployment_resource_blobs
    FOR EACH STATEMENT EXECUTE FUNCTION worldstream_reject_write_while_transfer_fenced();
CREATE TRIGGER worldstream_transfer_fence_retired_authority_fences
    BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON worldstream_retired_authority_fences_v1
    FOR EACH STATEMENT EXECUTE FUNCTION worldstream_reject_write_while_transfer_fenced();
";

/// Persists one immutable ExternalInput preparation before pack reduction.
/// The table is transfer-fenced because it is durable operation truth whose
/// first sampled Semantic Time must survive process restarts and lost replies.
pub const MIGRATION_0012_SQL: &str = r"
CREATE TABLE worldstream_external_input_preparations (
    identity_bytes bytea PRIMARY KEY,
    canonical_request_hash bytea NOT NULL CHECK (octet_length(canonical_request_hash) = 32),
    recorded_at text NOT NULL
);
CREATE TRIGGER worldstream_external_input_preparations_immutable_update
    BEFORE UPDATE OR DELETE ON worldstream_external_input_preparations
    FOR EACH ROW EXECUTE FUNCTION worldstream_reject_authority_fact_mutation();
CREATE TRIGGER worldstream_transfer_fence_external_input_preparations
    BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON worldstream_external_input_preparations
    FOR EACH STATEMENT EXECUTE FUNCTION worldstream_reject_write_while_transfer_fenced();
";

/// Adds an operational monotonic epoch for reset markers. It does not alter
/// canonical history, the durable Cursor, or frame sequence semantics.
pub const MIGRATION_0013_SQL: &str = r"
ALTER TABLE worldstream_members
    ADD COLUMN reset_generation bigint NOT NULL DEFAULT 0
    CHECK (reset_generation >= 0);
";

/// Frozen operational retention migration.
pub const MIGRATION_0014_SQL: &str = include_str!("migrations/0015-observation-retention.sql");

/// Durable PostgreSQL cadence metadata. It is operational scheduling state,
/// not canonical Room history, and is fenced with the other writable
/// operational tables during transfer.
pub const MIGRATION_0015_SQL: &str = r"
CREATE TABLE worldstream_room_snapshot_schedules (
    room_id text PRIMARY KEY,
    last_snapshot_room_seq bigint NOT NULL CHECK (last_snapshot_room_seq >= 0),
    transitions_since_snapshot bigint NOT NULL CHECK (transitions_since_snapshot >= 0),
    active_started_at text
);
CREATE TRIGGER worldstream_transfer_fence_snapshot_schedules
    BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON worldstream_room_snapshot_schedules
    FOR EACH STATEMENT EXECUTE FUNCTION worldstream_reject_write_while_transfer_fenced();
";

/// Adds operational metadata for the bounded refresh attention policy. The
/// canonical Activation identity and Room history remain unchanged.
pub const MIGRATION_0016_SQL: &str = r"
ALTER TABLE worldstream_activation_intents
    ADD COLUMN created_at text NOT NULL DEFAULT '';
ALTER TABLE worldstream_activation_intents
    ADD COLUMN attention_bytes bigint NOT NULL DEFAULT 0
        CHECK (attention_bytes >= 0);
ALTER TABLE worldstream_activation_intents
    ADD COLUMN terminal_disposition text
        CHECK (terminal_disposition IS NULL OR terminal_disposition IN (
            'superseded_refresh', 'refresh_capacity_exceeded', 'refresh_age_exceeded',
            'completed', 'expired', 'cancelled'
        ));
ALTER TABLE worldstream_activation_intents
    ADD COLUMN superseded_by_activation_id text;
ALTER TABLE worldstream_activation_intents
    ADD COLUMN terminal_at text;
CREATE INDEX worldstream_activation_pending_refresh_policy
    ON worldstream_activation_intents(room_id, target_member_id, state, semantic_deadline, created_at, cause_room_seq);
";

/// Persists a bounded, manifest-bound stream journal separately from the v1
/// materialized-bundle journal. Each accepted chunk and its decoded per-record
/// staging rows commit with one cursor advance, so an interrupted importer can
/// resume without rebuilding a bundle or retaining the full record set.
pub const MIGRATION_0017_SQL: &str = r"
CREATE TABLE worldstream_transfer_stream_imports_v2 (
    stream_header_digest bytea PRIMARY KEY CHECK (octet_length(stream_header_digest) = 32),
    target_fingerprint bytea NOT NULL CHECK (octet_length(target_fingerprint) = 32),
    manifest_bytes bytea NOT NULL CHECK (octet_length(manifest_bytes) > 0),
    manifest_digest bytea NOT NULL CHECK (octet_length(manifest_digest) = 32),
    state text NOT NULL CHECK (state IN ('pending', 'verified', 'finalized', 'authoritative', 'aborted')),
    next_chunk bigint NOT NULL CHECK (next_chunk >= 0),
    next_ordinal bigint NOT NULL CHECK (next_ordinal >= 0),
    footer_chunk_count bigint CHECK (footer_chunk_count >= 0),
    footer_record_count bigint CHECK (footer_record_count >= 0),
    footer_record_bytes bigint CHECK (footer_record_bytes >= 0),
    footer_digest bytea CHECK (footer_digest IS NULL OR octet_length(footer_digest) = 32),
    CHECK (
        (footer_chunk_count IS NULL AND footer_record_count IS NULL
            AND footer_record_bytes IS NULL AND footer_digest IS NULL)
        OR
        (footer_chunk_count IS NOT NULL AND footer_record_count IS NOT NULL
            AND footer_record_bytes IS NOT NULL AND footer_digest IS NOT NULL)
    )
);
CREATE TABLE worldstream_transfer_stream_chunks_v2 (
    stream_header_digest bytea NOT NULL
        REFERENCES worldstream_transfer_stream_imports_v2(stream_header_digest) ON DELETE CASCADE,
    chunk_index bigint NOT NULL CHECK (chunk_index >= 0),
    chunk_start bigint NOT NULL CHECK (chunk_start >= 0),
    chunk_end bigint NOT NULL CHECK (chunk_end > chunk_start),
    chunk_digest bytea NOT NULL CHECK (octet_length(chunk_digest) = 32),
    records_bytes bytea NOT NULL,
    PRIMARY KEY (stream_header_digest, chunk_index),
    UNIQUE (stream_header_digest, chunk_start)
);
CREATE TABLE worldstream_transfer_stream_records_v2 (
    stream_header_digest bytea NOT NULL
        REFERENCES worldstream_transfer_stream_imports_v2(stream_header_digest) ON DELETE CASCADE,
    ordinal bigint NOT NULL CHECK (ordinal >= 0),
    class_tag smallint NOT NULL CHECK (class_tag IN (1, 2)),
    kind_tag smallint NOT NULL CHECK (kind_tag > 0),
    identity text NOT NULL CHECK (length(identity) > 0),
    record_bytes bytea NOT NULL,
    record_digest bytea NOT NULL CHECK (octet_length(record_digest) = 32),
    PRIMARY KEY (stream_header_digest, ordinal),
    UNIQUE (stream_header_digest, class_tag, kind_tag, identity)
);
CREATE INDEX worldstream_transfer_stream_records_v2_identity
    ON worldstream_transfer_stream_records_v2(stream_header_digest, identity, ordinal);
";

/// Persists one canonical operational witness at the same exact Room sequence
/// as a disposable paired snapshot. Deleting the snapshot removes its witness.
pub const MIGRATION_0018_SQL: &str = r"
CREATE TABLE worldstream_room_snapshot_operational_witnesses (
    room_id text NOT NULL,
    room_seq bigint NOT NULL CHECK (room_seq >= 0),
    witness_schema_version text NOT NULL
        CHECK (witness_schema_version = 'worldstream/checkpoint-operational-witness/v1'),
    witness_hash bytea NOT NULL CHECK (octet_length(witness_hash) = 32),
    witness_bytes bytea NOT NULL CHECK (octet_length(witness_bytes) > 0),
    PRIMARY KEY (room_id, room_seq),
    FOREIGN KEY (room_id, room_seq)
        REFERENCES worldstream_room_snapshots(room_id, room_seq) ON DELETE CASCADE
);
CREATE TRIGGER worldstream_transfer_fence_snapshot_operational_witnesses
    BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON worldstream_room_snapshot_operational_witnesses
    FOR EACH STATEMENT EXECUTE FUNCTION worldstream_reject_write_while_transfer_fenced();
";

/// Persists cache-only, append-only receipts for guard-only operational
/// histories. The source rows remain the forensic authority.
pub const MIGRATION_0019_SQL: &str = r"
CREATE TABLE worldstream_room_operational_history_roots_v2 (
    room_id text NOT NULL REFERENCES worldstream_room_roots(room_id) ON DELETE CASCADE,
    domain text NOT NULL CHECK (domain IN ('frames', 'consequences', 'activation_decisions')),
    entry_count bigint NOT NULL CHECK (entry_count >= 0),
    root_hash bytea NOT NULL CHECK (octet_length(root_hash) = 32),
    PRIMARY KEY (room_id, domain)
);
CREATE TRIGGER worldstream_transfer_fence_operational_history_roots_v2
    BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON worldstream_room_operational_history_roots_v2
    FOR EACH STATEMENT EXECUTE FUNCTION worldstream_reject_write_while_transfer_fenced();
";

/// Retains the latest Timer generation per identity for bounded V2 checkpoint
/// recovery. Historical generations remain in `worldstream_timers`.
pub const MIGRATION_0020_SQL: &str = r"
CREATE TABLE worldstream_room_current_timers_v2 (
    room_id text NOT NULL REFERENCES worldstream_room_roots(room_id) ON DELETE CASCADE,
    timer_id text NOT NULL,
    generation bigint NOT NULL CHECK (generation > 0),
    scheduled_for text NOT NULL,
    payload_bytes bytea NOT NULL,
    state text NOT NULL CHECK (state IN ('scheduled', 'cancelled', 'fired')),
    PRIMARY KEY (room_id, timer_id)
);
CREATE TRIGGER worldstream_transfer_fence_room_current_timers_v2
    BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON worldstream_room_current_timers_v2
    FOR EACH STATEMENT EXECUTE FUNCTION worldstream_reject_write_while_transfer_fenced();
";

pub const MIGRATION_0021_SQL: &str = r"
CREATE TABLE worldstream_room_snapshot_operational_witnesses_v2 (
    room_id text NOT NULL,
    room_seq bigint NOT NULL CHECK (room_seq >= 0),
    witness_schema_version text NOT NULL
        CHECK (witness_schema_version = 'worldstream/checkpoint-operational-witness/v2'),
    witness_hash bytea NOT NULL CHECK (octet_length(witness_hash) = 32),
    witness_bytes bytea NOT NULL CHECK (octet_length(witness_bytes) > 0),
    PRIMARY KEY (room_id, room_seq),
    FOREIGN KEY (room_id, room_seq)
        REFERENCES worldstream_room_snapshots(room_id, room_seq) ON DELETE CASCADE
);
CREATE TRIGGER worldstream_transfer_fence_snapshot_operational_witnesses_v2
    BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON worldstream_room_snapshot_operational_witnesses_v2
    FOR EACH STATEMENT EXECUTE FUNCTION worldstream_reject_write_while_transfer_fenced();
";

/// The result of checking an ordered migration prefix.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MigrationVerification {
    /// Highest verified migration version.
    pub current_version: i32,
    /// Whether the complete current history is installed.
    pub complete: bool,
    /// Whether the final schema fingerprint was present and correct.
    pub fingerprint_verified: bool,
}

/// Migration history failures are deliberately explicit and fail closed.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum MigrationVerificationError {
    #[error("migration history is not contiguous at version {version}")]
    NonContiguous { version: i32 },
    #[error("migration version {version} is unsupported")]
    Unsupported { version: i32 },
    #[error("migration version {version} has identity {found:?}, expected {expected:?}")]
    IdentityDrift {
        version: i32,
        expected: &'static str,
        found: String,
    },
    #[error("migration version {version} checksum differs from the reviewed SQL")]
    ChecksumDrift { version: i32 },
    #[error("migration metadata belongs to {found:?}, expected {expected:?}")]
    LogicalHistoryDrift {
        expected: &'static str,
        found: String,
    },
    #[error("schema contract fingerprint is missing")]
    FingerprintMissing,
    #[error("schema contract fingerprint differs from the reviewed contract")]
    FingerprintDrift,
    #[error("runtime schema is incomplete at migration version {version}")]
    IncompleteRuntimeSchema { version: i32 },
}

/// Verifies a prefix during administration. An empty prefix is valid before
/// bootstrap; an incomplete prefix is valid only while the admin transaction
/// is about to apply its next forward migration.
pub fn verify_migration_prefix(
    records: &[MigrationRecord],
) -> Result<MigrationVerification, MigrationVerificationError> {
    let history = migration_history();
    // A database at the previous release's final migration can carry the
    // previous schema fingerprint while the next forward migration is still
    // unapplied. Identity and checksum checks remain strict; migration 0005
    // rewrites the metadata to the current fingerprint in the same transaction
    // as the new staging tables.
    let allow_stale_fingerprint = records.len() < history.len();
    for (index, record) in records.iter().enumerate() {
        let expected_version = i32::try_from(index + 1).unwrap_or(i32::MAX);
        if record.version != expected_version {
            return Err(MigrationVerificationError::NonContiguous {
                version: record.version,
            });
        }
        let Some(expected) = history.get(index) else {
            return Err(MigrationVerificationError::Unsupported {
                version: record.version,
            });
        };
        verify_record(record, *expected, allow_stale_fingerprint)?;
    }
    Ok(MigrationVerification {
        current_version: records.last().map_or(0, |record| record.version),
        complete: records.len() == history.len(),
        fingerprint_verified: records.last().is_some_and(|record| {
            record.schema_contract_fingerprint.as_deref()
                == Some(schema_contract_fingerprint().as_bytes().as_slice())
        }),
    })
}

/// Verifies the state a runtime process is allowed to serve.
pub fn verify_runtime_migration_history(
    records: &[MigrationRecord],
) -> Result<MigrationVerification, MigrationVerificationError> {
    let state = verify_migration_prefix(records)?;
    if !state.complete {
        return Err(MigrationVerificationError::IncompleteRuntimeSchema {
            version: state.current_version,
        });
    }
    for record in records {
        if record.logical_history_id.as_deref() != Some(LOGICAL_HISTORY_ID) {
            return Err(if record.logical_history_id.is_none() {
                MigrationVerificationError::FingerprintMissing
            } else {
                MigrationVerificationError::LogicalHistoryDrift {
                    expected: LOGICAL_HISTORY_ID,
                    found: record.logical_history_id.clone().unwrap_or_default(),
                }
            });
        }
        if record.schema_contract_fingerprint.as_deref()
            != Some(schema_contract_fingerprint().as_bytes().as_slice())
        {
            return Err(if record.schema_contract_fingerprint.is_none() {
                MigrationVerificationError::FingerprintMissing
            } else {
                MigrationVerificationError::FingerprintDrift
            });
        }
    }
    if records.last().is_none() {
        return Err(MigrationVerificationError::IncompleteRuntimeSchema { version: 0 });
    }
    Ok(MigrationVerification {
        fingerprint_verified: true,
        ..state
    })
}

fn verify_record(
    record: &MigrationRecord,
    expected: MigrationDescriptor,
    allow_stale_fingerprint: bool,
) -> Result<(), MigrationVerificationError> {
    if record.migration_id != expected.id {
        return Err(MigrationVerificationError::IdentityDrift {
            version: record.version,
            expected: expected.id,
            found: record.migration_id.clone(),
        });
    }
    if record.checksum.as_slice() != expected.checksum().as_bytes().as_slice() {
        return Err(MigrationVerificationError::ChecksumDrift {
            version: record.version,
        });
    }
    if let Some(history_id) = &record.logical_history_id
        && history_id != LOGICAL_HISTORY_ID
    {
        return Err(MigrationVerificationError::LogicalHistoryDrift {
            expected: LOGICAL_HISTORY_ID,
            found: history_id.clone(),
        });
    }
    if let Some(fingerprint) = &record.schema_contract_fingerprint
        && fingerprint.as_slice() != schema_contract_fingerprint().as_bytes().as_slice()
        && !allow_stale_fingerprint
    {
        return Err(MigrationVerificationError::FingerprintDrift);
    }
    Ok(())
}

/// A deterministic interruption point for the migration fixture.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MigrationFailpoint {
    /// The migration body ran but its ledger insert was interrupted.
    AfterMigrationBodyBeforeRecord,
}

/// In-process migration provider seam. It models transactional DDL outcomes
/// without claiming that a live PostgreSQL service was exercised.
#[derive(Clone, Default)]
pub struct FixtureMigrationProvider {
    state: std::sync::Arc<Mutex<FixtureMigrationState>>,
}

#[derive(Default)]
struct FixtureMigrationState {
    records: Vec<MigrationRecord>,
    failpoint: Option<MigrationFailpoint>,
    body_attempts: usize,
}

impl fmt::Debug for FixtureMigrationProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FixtureMigrationProvider")
            .finish_non_exhaustive()
    }
}

impl FixtureMigrationProvider {
    /// Arms a deterministic interruption.
    pub fn set_failpoint(&self, failpoint: Option<MigrationFailpoint>) {
        if let Ok(mut state) = self.state.lock() {
            state.failpoint = failpoint;
        }
    }

    /// Returns the number of migration bodies attempted by the fixture.
    #[must_use]
    pub fn body_attempts(&self) -> usize {
        self.state.lock().map_or(0, |state| state.body_attempts)
    }

    /// Returns a snapshot of the durable migration ledger.
    #[must_use]
    pub fn records(&self) -> Vec<MigrationRecord> {
        self.state
            .lock()
            .map_or_else(|_| Vec::new(), |state| state.records.clone())
    }

    /// Applies the next forward migration, restarting safely after an
    /// interrupted body because the fixture models each body as transactional.
    pub fn migrate(&self) -> Result<MigrationVerification, MigrationVerificationError> {
        let history = migration_history();
        let mut state = self
            .state
            .lock()
            .map_err(|_| MigrationVerificationError::IncompleteRuntimeSchema { version: 0 })?;
        verify_migration_prefix(&state.records)?;
        while state.records.len() < history.len() {
            let next = history[state.records.len()];
            state.body_attempts += 1;
            if state.failpoint.take().is_some_and(|failpoint| {
                matches!(
                    failpoint,
                    MigrationFailpoint::AfterMigrationBodyBeforeRecord
                )
            }) {
                return Err(MigrationVerificationError::IncompleteRuntimeSchema {
                    version: next.version - 1,
                });
            }
            let fingerprint = schema_contract_fingerprint();
            if next.version > 1 {
                for record in &mut state.records {
                    record.logical_history_id = Some(LOGICAL_HISTORY_ID.to_owned());
                    record.schema_contract_fingerprint = Some(fingerprint.as_bytes().to_vec());
                }
            }
            state.records.push(MigrationRecord {
                version: next.version,
                migration_id: next.id.to_owned(),
                checksum: next.checksum().as_bytes().to_vec(),
                logical_history_id: (next.version > 1).then(|| LOGICAL_HISTORY_ID.to_owned()),
                schema_contract_fingerprint: (next.version > 1)
                    .then(|| fingerprint.as_bytes().to_vec()),
            });
        }
        verify_runtime_migration_history(&state.records)
    }
}

#[cfg(test)]
mod identity_tests {
    use super::*;

    #[test]
    fn external_input_preparation_migration_is_forward_only_and_immutable() {
        let migration = migration_history()[11];
        assert_eq!(migration.version, 12);
        assert_eq!(migration.id, EXTERNAL_INPUT_PREPARATION_MIGRATION_ID);
        assert!(
            migration
                .sql
                .contains("CREATE TABLE worldstream_external_input_preparations")
        );
        assert!(migration.sql.contains("identity_bytes bytea PRIMARY KEY"));
        assert!(migration.sql.contains(
            "canonical_request_hash bytea NOT NULL CHECK (octet_length(canonical_request_hash) = 32)"
        ));
        assert!(migration.sql.contains("recorded_at text NOT NULL"));
        assert!(
            migration
                .sql
                .contains("worldstream_external_input_preparations_immutable_update")
        );
    }

    #[test]
    fn observation_reset_generation_is_a_forward_operational_migration() {
        let migration = migration_history()[12];
        assert_eq!(migration.version, 13);
        assert_eq!(migration.id, OBSERVATION_RESET_GENERATION_MIGRATION_ID);
        assert!(
            migration
                .sql
                .contains("ADD COLUMN reset_generation bigint NOT NULL DEFAULT 0")
        );
    }

    #[test]
    fn transfer_lifecycle_migration_and_schema_fingerprint_are_stable() {
        let migration = migration_history()[10];
        assert_eq!(migration.id, TRANSFER_RESOURCE_IDENTITY_MIGRATION_ID);
        assert_eq!(
            migration.checksum().to_string(),
            "blake3:f93d4eed88e503bf1a807ea676e9110dbfab3987adff7be9bdd618394345ad97"
        );
        assert_eq!(
            schema_contract_fingerprint().to_string(),
            "blake3:28e9765429afc17fa678ffafcd4be898520a4e716bbeb25995993bb8136cab04"
        );
    }

    #[test]
    fn stream_transfer_journal_migration_is_forward_only_and_complete() {
        let migration = migration_history()[16];
        assert_eq!(migration.version, 17);
        assert_eq!(migration.id, STREAM_TRANSFER_V2_MIGRATION_ID);
        for table in [
            "worldstream_transfer_stream_imports_v2",
            "worldstream_transfer_stream_chunks_v2",
            "worldstream_transfer_stream_records_v2",
        ] {
            assert!(migration.sql.contains(&format!("CREATE TABLE {table}")));
        }
        assert!(migration.sql.contains("next_chunk bigint NOT NULL"));
        assert!(migration.sql.contains("next_ordinal bigint NOT NULL"));
    }

    #[test]
    fn checkpoint_operational_witness_migration_is_forward_only_and_transfer_fenced() {
        let migration = migration_history()[17];
        assert_eq!(migration.version, 18);
        assert_eq!(migration.id, CHECKPOINT_OPERATIONAL_WITNESS_MIGRATION_ID);
        assert_eq!(
            migration.checksum().to_string(),
            "blake3:d4cb185480765df6970248624e81b37c361cfd7dd2a38b4b3d73a598bc286c08"
        );
        assert!(
            migration
                .sql
                .contains("CREATE TABLE worldstream_room_snapshot_operational_witnesses")
        );
        assert!(
            migration
                .sql
                .contains("worldstream_transfer_fence_snapshot_operational_witnesses")
        );
        assert!(migration.sql.contains("ON DELETE CASCADE"));
    }

    #[test]
    fn postgres_snapshot_cadence_migration_is_durable_and_transfer_fenced() {
        let migration = migration_history()[14];
        assert_eq!(migration.version, 15);
        assert_eq!(migration.id, SNAPSHOT_CADENCE_MIGRATION_ID);
        assert!(
            migration
                .sql
                .contains("CREATE TABLE worldstream_room_snapshot_schedules")
        );
        assert!(
            migration
                .sql
                .contains("worldstream_transfer_fence_snapshot_schedules")
        );
    }

    #[test]
    fn logical_tail_keeps_snapshot_cadence_before_activation_and_stream_journal() {
        let tail = &migration_history()[14..];
        assert_eq!(
            tail.iter()
                .map(|migration| migration.id)
                .collect::<Vec<_>>(),
            [
                SNAPSHOT_CADENCE_MIGRATION_ID,
                ACTIVATION_BACKLOG_POLICY_MIGRATION_ID,
                STREAM_TRANSFER_V2_MIGRATION_ID,
                CHECKPOINT_OPERATIONAL_WITNESS_MIGRATION_ID,
                OPERATIONAL_HISTORY_ROOTS_MIGRATION_ID,
                CURRENT_TIMERS_MIGRATION_ID,
                CHECKPOINT_OPERATIONAL_WITNESS_V2_MIGRATION_ID,
            ]
        );
        assert!(
            tail[0]
                .sql
                .contains("CREATE TABLE worldstream_room_snapshot_schedules")
        );
    }

    #[test]
    fn current_timers_migration_is_forward_only_and_transfer_fenced() {
        let migration = migration_history()[19];
        assert_eq!(migration.version, 20);
        assert_eq!(migration.id, CURRENT_TIMERS_MIGRATION_ID);
        assert!(migration
            .sql
            .contains("CREATE TABLE worldstream_room_current_timers_v2"));
        assert!(migration
            .sql
            .contains("worldstream_transfer_fence_room_current_timers_v2"));
        assert!(migration.sql.contains("PRIMARY KEY (room_id, timer_id)"));
    }

    #[test]
    fn checkpoint_operational_witness_v2_migration_is_forward_only_and_transfer_fenced() {
        let migration = migration_history()[20];
        assert_eq!(migration.version, 21);
        assert_eq!(migration.id, CHECKPOINT_OPERATIONAL_WITNESS_V2_MIGRATION_ID);
        assert!(migration
            .sql
            .contains("CREATE TABLE worldstream_room_snapshot_operational_witnesses_v2"));
        assert!(migration
            .sql
            .contains("worldstream_transfer_fence_snapshot_operational_witnesses_v2"));
        assert!(migration.sql.contains("ON DELETE CASCADE"));
    }
}
