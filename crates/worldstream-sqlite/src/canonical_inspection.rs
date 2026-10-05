//! Shared operational inspection facts with no fabricated opaque Activity.

use std::collections::BTreeMap;
use worldstream_core::{
    CanonicalJsonV1, CanonicalStorageHistoryPreflight, CompleteHeadV1, MemberId,
    PreparedMembershipMaterializationV1, RecoveredActivationDecisionV1,
    RecoveredRoomMaterializationsV1, RecoveredTimerMaterializationV1, RoomRecoveryErrorV1,
    RoomStatusV1,
};

/// The SQL inspection helpers need host-derived facts. Complete semantic
/// recovery adds state and delivery facts through its separate sealed type.
pub(super) trait SqliteInspectionFacts {
    fn room_status(&self) -> RoomStatusV1;
    fn memberships(&self) -> &[PreparedMembershipMaterializationV1];
    fn timers(&self) -> &[RecoveredTimerMaterializationV1];
    fn activation_decisions(&self) -> &[RecoveredActivationDecisionV1];
    fn membership_generations(&self) -> Option<&BTreeMap<String, i64>>;
    fn observation_frame_heads(&self) -> &BTreeMap<MemberId, u64>;
}

impl SqliteInspectionFacts for RecoveredRoomMaterializationsV1 {
    fn room_status(&self) -> RoomStatusV1 {
        self.room_status()
    }
    fn memberships(&self) -> &[PreparedMembershipMaterializationV1] {
        self.memberships()
    }
    fn timers(&self) -> &[RecoveredTimerMaterializationV1] {
        self.timers()
    }
    fn activation_decisions(&self) -> &[RecoveredActivationDecisionV1] {
        self.activation_decisions()
    }
    fn membership_generations(&self) -> Option<&BTreeMap<String, i64>> {
        self.membership_generations()
    }
    fn observation_frame_heads(&self) -> &BTreeMap<MemberId, u64> {
        self.observation_frame_heads()
    }
}

pub(super) struct SqliteStructuralInspectionFacts {
    room_status: RoomStatusV1,
    memberships: Vec<PreparedMembershipMaterializationV1>,
    timers: Vec<RecoveredTimerMaterializationV1>,
    activation_decisions: Vec<RecoveredActivationDecisionV1>,
    membership_generations: BTreeMap<String, i64>,
    observation_frame_heads: BTreeMap<MemberId, u64>,
}

impl SqliteStructuralInspectionFacts {
    pub(super) fn verify(
        expected_head: &CompleteHeadV1,
        genesis_bytes: &[u8],
        transitions: &[Vec<u8>],
        core_bytes: Option<&[u8]>,
        activity_bytes: Option<&[u8]>,
    ) -> Result<Self, RoomRecoveryErrorV1> {
        let mut preflight = CanonicalStorageHistoryPreflight::begin(genesis_bytes)
            .map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
        preflight
            .consume_transition_page(transitions)
            .map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
        if preflight.final_head() != expected_head {
            return Err(RoomRecoveryErrorV1::Corrupt);
        }
        let mut structure = preflight.finish();
        if let Some(core_bytes) = core_bytes
            && structure
                .core_state()
                .canonical_bytes()
                .map_err(|_| RoomRecoveryErrorV1::Corrupt)?
                != core_bytes
        {
            return Err(RoomRecoveryErrorV1::Corrupt);
        }
        if let Some(activity_bytes) = activity_bytes {
            structure = structure
                .with_activity_materialization(activity_bytes)
                .map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
        }
        let memberships = structure
            .core_state()
            .memberships()
            .values()
            .map(|membership| {
                Ok(PreparedMembershipMaterializationV1 {
                    membership: membership.clone(),
                    canonical_membership_bytes: CanonicalJsonV1::parse(
                        &serde_json::to_vec(membership)
                            .map_err(|_| RoomRecoveryErrorV1::Corrupt)?,
                    )
                    .map_err(|_| RoomRecoveryErrorV1::Corrupt)?
                    .to_bytes()
                    .map_err(|_| RoomRecoveryErrorV1::Corrupt)?,
                })
            })
            .collect::<Result<Vec<_>, RoomRecoveryErrorV1>>()?;
        Ok(Self {
            room_status: structure.core_state().room_status(),
            memberships,
            timers: structure.timer_materializations(),
            activation_decisions: structure.activation_decisions().to_vec(),
            membership_generations: structure.membership_generations().clone(),
            // Structural history cannot reconstruct addressed observations.
            // Empty means no semantic frame-head comparison is claimed here.
            observation_frame_heads: BTreeMap::new(),
        })
    }
}

impl SqliteInspectionFacts for SqliteStructuralInspectionFacts {
    fn room_status(&self) -> RoomStatusV1 {
        self.room_status
    }
    fn memberships(&self) -> &[PreparedMembershipMaterializationV1] {
        &self.memberships
    }
    fn timers(&self) -> &[RecoveredTimerMaterializationV1] {
        &self.timers
    }
    fn activation_decisions(&self) -> &[RecoveredActivationDecisionV1] {
        &self.activation_decisions
    }
    fn membership_generations(&self) -> Option<&BTreeMap<String, i64>> {
        Some(&self.membership_generations)
    }
    fn observation_frame_heads(&self) -> &BTreeMap<MemberId, u64> {
        &self.observation_frame_heads
    }
}
