//! Registry-aware cache rebuild. Semantically verified prefixes publish only after final fences.
use super::*;
use worldstream_core::{ReplayFailureV1, ReplayStepV1};

#[allow(
    clippy::needless_pass_by_value,
    reason = "Result::map_err transfers the closed error by value."
)]
pub(super) fn verification_replay_error(error: ReplayFailureV1) -> PostgresRoomVerificationError {
    match error.class {
        ReplayFailureClassV1::RuntimeUnavailable => {
            PostgresRoomVerificationError::RuntimeUnavailable
        }
        ReplayFailureClassV1::RuntimeFault => PostgresRoomVerificationError::RuntimeFault,
        _ => PostgresRoomVerificationError::Corrupt {
            what: "retained Pack replay",
        },
    }
}
#[allow(
    clippy::needless_pass_by_value,
    reason = "Result::map_err transfers the closed error by value."
)]
fn maintenance_replay_error(error: ReplayFailureV1) -> PostgresMaintenanceError {
    match error.class {
        ReplayFailureClassV1::RuntimeUnavailable => PostgresMaintenanceError::RuntimeUnavailable,
        ReplayFailureClassV1::RuntimeFault => PostgresMaintenanceError::RuntimeFault,
        _ => PostgresMaintenanceError::Corrupt,
    }
}
impl PostgresAdmin {
    /// Rebuilds both-format snapshots from the exact retained executable and verified per-cut state.
    pub fn rebuild_canonical_snapshot_cache(
        &self,
        registry: &PackRegistryV1,
        room_id: &str,
    ) -> Result<usize, PostgresMaintenanceError> {
        self.rebuild_canonical_snapshot_cache_inner(registry, room_id, None, None)
    }
    pub(crate) fn rebuild_canonical_snapshot_cache_for_native_restore(
        &self,
        registry: &PackRegistryV1,
        room_id: &str,
        budget: &mut ProviderReadBudgetV1,
        identity: &native_restore::PostgresProviderIdentityV1,
    ) -> Result<usize, PostgresMaintenanceError> {
        self.rebuild_canonical_snapshot_cache_inner(registry, room_id, Some(budget), Some(identity))
    }
    #[allow(clippy::too_many_lines)]
    fn rebuild_canonical_snapshot_cache_inner(
        &self,
        registry: &PackRegistryV1,
        room_id: &str,
        budget: Option<&mut ProviderReadBudgetV1>,
        identity: Option<&native_restore::PostgresProviderIdentityV1>,
    ) -> Result<usize, PostgresMaintenanceError> {
        let mut client = self
            .connect()
            .map_err(PostgresMaintenanceError::Connection)?;
        if let Some(identity) = identity {
            require_native_restore_provider_identity(&mut client, identity)?;
        }
        let mut tx = client
            .build_transaction()
            .isolation_level(IsolationLevel::RepeatableRead)
            .start()
            .map_err(PostgresMaintenanceError::Sql)?;
        let root=tx.query_opt("SELECT head_bytes, integrity_generation, integrity_status FROM worldstream_room_roots WHERE room_id=$1 FOR UPDATE",&[&room_id]).map_err(PostgresMaintenanceError::Sql)?.ok_or(PostgresMaintenanceError::Corrupt)?;
        let captured_head: Vec<u8> = root.try_get(0).map_err(PostgresMaintenanceError::Sql)?;
        let captured_generation: i64 = root.try_get(1).map_err(PostgresMaintenanceError::Sql)?;
        let status: String = root.try_get(2).map_err(PostgresMaintenanceError::Sql)?;
        if status != "healthy" {
            return Err(PostgresMaintenanceError::Corrupt);
        }
        // Cache deletion and all replacement rows remain uncommitted until exact Replay succeeds.
        tx.execute(
            "DELETE FROM worldstream_room_snapshots WHERE room_id=$1",
            &[&room_id],
        )
        .map_err(PostgresMaintenanceError::Sql)?;
        let verification = if let Some(budget) = budget {
            verify_room_for_native_restore(&mut tx, room_id, budget)
                .map_err(PostgresMaintenanceError::Verification)?
        } else {
            Self::verify_room_with_client(&mut tx, room_id, false, false)
                .map_err(PostgresMaintenanceError::Verification)?
        };
        if verification.head_bytes != captured_head
            || i64::try_from(verification.integrity_generation).ok() != Some(captured_generation)
        {
            return Err(PostgresMaintenanceError::ConcurrentChange);
        }
        let lock = PackRevisionLockV1::from_canonical_bytes(
            &verification.pack_revision_lock_bytes,
            verification.head.pack_digest(),
        )
        .map_err(|_| PostgresMaintenanceError::Corrupt)?;
        let mut preflight = CanonicalStorageHistoryPreflight::begin(&verification.genesis_bytes)
            .map_err(maintenance_replay_error)?;
        for page in verification.transition_bytes.chunks(256) {
            preflight
                .consume_transition_page(page)
                .map_err(maintenance_replay_error)?;
        }
        // Runtime lookup follows the complete structural pass.
        let mut replay = preflight
            .begin_executable_with_observations(&verification.head, registry)
            .map_err(maintenance_replay_error)?;
        let genesis = replay
            .genesis_materialization()
            .map_err(maintenance_replay_error)?;
        persist_snapshot(
            &mut tx,
            room_id,
            &genesis.head,
            &genesis
                .head
                .canonical_bytes()
                .map_err(|_| PostgresMaintenanceError::Corrupt)?,
            &genesis.canonical_core_bytes,
            &genesis.canonical_activity_bytes,
        )
        .map_err(PostgresMaintenanceError::Sql)?;
        let mut sql_error = None;
        let mut visitor = |step: &ReplayStepV1| {
            let head_bytes = step.head.canonical_bytes().map_err(|_| ReplayFailureV1 {
                class: ReplayFailureClassV1::CanonicalEncoding,
                detail: "snapshot Head encoding".into(),
                last_verified_head: Some(Box::new(step.head.clone())),
            })?;
            if let Err(error) = persist_snapshot(
                &mut tx,
                room_id,
                &step.head,
                &head_bytes,
                &step.canonical_core_bytes,
                &step.canonical_activity_bytes,
            ) {
                sql_error = Some(error);
                return Err(ReplayFailureV1 {
                    class: ReplayFailureClassV1::CoreInvariant,
                    detail: "snapshot provider write".into(),
                    last_verified_head: Some(Box::new(step.head.clone())),
                });
            }
            Ok(())
        };
        for page in verification.transition_bytes.chunks(256) {
            if let Err(error) =
                replay.consume_transition_page_with_materializations(page, &mut visitor)
            {
                return Err(sql_error.map_or_else(
                    || maintenance_replay_error(error),
                    PostgresMaintenanceError::Sql,
                ));
            }
        }
        let report = replay
            .finish(
                &verification.head,
                Some(&verification.core_state_bytes),
                Some(&verification.activity_state_bytes),
            )
            .map_err(maintenance_replay_error)?;
        if report.retained_pack_revision_lock() != &lock {
            return Err(PostgresMaintenanceError::Corrupt);
        }
        let witness = report
            .storage_verification()
            .map_err(maintenance_replay_error)?;
        verify_replayed_observation_evidence(&witness, &verification)
            .map_err(|_| PostgresMaintenanceError::Corrupt)?;
        canonical_operational::verify_membership_generations(
            &mut tx,
            room_id,
            report.membership_generations(),
        )
        .map_err(PostgresMaintenanceError::Verification)?;
        if let Some(receipts) = capture_operational_mmr_receipts(&mut tx, room_id)
            .map_err(PostgresMaintenanceError::Sql)?
        {
            verify_postgres_operational_mmr_full(&mut tx, room_id, &receipts)
                .map_err(|_| PostgresMaintenanceError::Corrupt)?;
        }
        let final_root=tx.query_one("SELECT head_bytes, integrity_generation, integrity_status FROM worldstream_room_roots WHERE room_id=$1",&[&room_id]).map_err(PostgresMaintenanceError::Sql)?;
        if final_root.get::<_, Vec<u8>>(0) != captured_head
            || final_root.get::<_, i64>(1) != captured_generation
            || final_root.get::<_, String>(2) != "healthy"
        {
            return Err(PostgresMaintenanceError::ConcurrentChange);
        }
        if let Some(identity) = identity {
            require_native_restore_provider_identity(&mut tx, identity)?;
        }
        let sequence = i64::try_from(verification.head.room_seq().get())
            .map_err(|_| PostgresMaintenanceError::Corrupt)?;
        persist_checkpoint_operational_witness(
            &mut tx,
            room_id,
            &verification.head,
            &captured_head,
        )
        .map_err(PostgresMaintenanceError::Sql)?;
        let witness_present: bool = tx.query_one(
            "SELECT EXISTS(SELECT 1 FROM worldstream_room_snapshot_operational_witnesses WHERE room_id=$1 AND room_seq=$2) OR EXISTS(SELECT 1 FROM worldstream_room_snapshot_operational_witnesses_v2 WHERE room_id=$1 AND room_seq=$2) OR EXISTS(SELECT 1 FROM worldstream_room_snapshot_operational_witnesses_v3 WHERE room_id=$1 AND room_seq=$2)", &[&room_id,&sequence]
        ).map_err(PostgresMaintenanceError::Sql)?.try_get(0).map_err(PostgresMaintenanceError::Sql)?;
        if !witness_present {
            return Err(PostgresMaintenanceError::Corrupt);
        }
        tx.execute("INSERT INTO worldstream_room_snapshot_schedules(room_id,last_snapshot_room_seq,transitions_since_snapshot,active_started_at) VALUES($1,$2,0,NULL) ON CONFLICT(room_id) DO UPDATE SET last_snapshot_room_seq=EXCLUDED.last_snapshot_room_seq,transitions_since_snapshot=0,active_started_at=NULL",&[&room_id,&sequence]).map_err(PostgresMaintenanceError::Sql)?;
        tx.commit().map_err(PostgresMaintenanceError::Sql)?;
        Ok(verification.transition_count + 1)
    }
}
