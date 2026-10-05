//! Private owned persistence facts shared by both sealed Room Commit ports.
//! Records and complete materializations remain distinct across the writer channel.

use worldstream_core::{
    CompleteHeadV1, CoreRoomStateV1, IntegrityGenerationV1, PreparedActivationDecisionV1,
    PreparedAdvancePersistenceV1, PreparedCanonicalAdvancePersistence,
    PreparedCanonicalCreationPersistence, PreparedCanonicalExistingIntent,
    PreparedCreationPersistenceV1, PreparedExistingIntentV1, PreparedMembershipMaterializationV1,
    PreparedObservationConsequenceV1, PreparedTimerMaterializationV1, PreparedTimerMutationV1,
    TransitionId, TransitionRecord,
};

#[derive(Clone)]
pub(super) struct SqliteCreationPersistence {
    pub(super) canonical_pack_revision_lock_bytes: Vec<u8>,
    pub(super) canonical_genesis_bytes: Vec<u8>,
    pub(super) complete_head: CompleteHeadV1,
    pub(super) canonical_head_bytes: Vec<u8>,
    pub(super) core_state: CoreRoomStateV1,
    pub(super) canonical_core_state_bytes: Vec<u8>,
    pub(super) canonical_activity_state_bytes: Vec<u8>,
    pub(super) memberships: Vec<PreparedMembershipMaterializationV1>,
    pub(super) initial_timers: Vec<PreparedTimerMaterializationV1>,
    pub(super) integrity_generation: IntegrityGenerationV1,
}

impl SqliteCreationPersistence {
    pub(super) fn from_legacy(value: &PreparedCreationPersistenceV1) -> Self {
        Self {
            canonical_pack_revision_lock_bytes: value.canonical_pack_revision_lock_bytes.clone(),
            canonical_genesis_bytes: value.canonical_genesis_bytes.clone(),
            complete_head: value.complete_head.clone(),
            canonical_head_bytes: value.canonical_head_bytes.clone(),
            core_state: value.core_state.clone(),
            canonical_core_state_bytes: value.canonical_core_state_bytes.clone(),
            canonical_activity_state_bytes: value.canonical_activity_state_bytes.clone(),
            memberships: value.memberships.clone(),
            initial_timers: value.initial_timers.clone(),
            integrity_generation: value.integrity_generation,
        }
    }

    pub(super) fn from_canonical(value: &PreparedCanonicalCreationPersistence) -> Self {
        Self {
            canonical_pack_revision_lock_bytes: value.canonical_pack_revision_lock_bytes().to_vec(),
            canonical_genesis_bytes: value.canonical_genesis_bytes().to_vec(),
            complete_head: value.complete_head().clone(),
            canonical_head_bytes: value.canonical_head_bytes().to_vec(),
            core_state: value.core_state().clone(),
            canonical_core_state_bytes: value.canonical_core_state_bytes().to_vec(),
            canonical_activity_state_bytes: value.canonical_activity_state_bytes().to_vec(),
            memberships: value.memberships().to_vec(),
            initial_timers: value.initial_timers().to_vec(),
            integrity_generation: value.integrity_generation(),
        }
    }
}

#[derive(Clone)]
pub(super) struct SqliteAdvancePersistence {
    pub(super) transition_id: TransitionId,
    pub(super) transition: TransitionRecord,
    pub(super) canonical_transition_bytes: Vec<u8>,
    pub(super) resulting_complete_head: CompleteHeadV1,
    pub(super) canonical_resulting_head_bytes: Vec<u8>,
    pub(super) resulting_core_state: CoreRoomStateV1,
    pub(super) canonical_resulting_core_state_bytes: Vec<u8>,
    pub(super) canonical_resulting_activity_state_bytes: Vec<u8>,
    pub(super) resulting_memberships: Vec<PreparedMembershipMaterializationV1>,
    pub(super) timer_changes: Vec<PreparedTimerMutationV1>,
    pub(super) delivery_consequences: Vec<PreparedObservationConsequenceV1>,
    pub(super) activation_decisions: Vec<PreparedActivationDecisionV1>,
}

impl SqliteAdvancePersistence {
    pub(super) fn from_legacy(value: &PreparedAdvancePersistenceV1) -> Self {
        Self {
            transition_id: value.transition_id.clone(),
            transition: TransitionRecord::V1(value.transition.clone()),
            canonical_transition_bytes: value.canonical_transition_bytes.clone(),
            resulting_complete_head: value.resulting_complete_head.clone(),
            canonical_resulting_head_bytes: value.canonical_resulting_head_bytes.clone(),
            resulting_core_state: value.resulting_core_state.clone(),
            canonical_resulting_core_state_bytes: value
                .canonical_resulting_core_state_bytes
                .clone(),
            canonical_resulting_activity_state_bytes: value
                .canonical_resulting_activity_state_bytes
                .clone(),
            resulting_memberships: value.resulting_memberships.clone(),
            timer_changes: value.timer_changes.clone(),
            delivery_consequences: value.delivery_consequences.clone(),
            activation_decisions: value.activation_decisions.clone(),
        }
    }

    pub(super) fn from_canonical(value: &PreparedCanonicalAdvancePersistence) -> Self {
        Self {
            transition_id: value.transition_id().clone(),
            transition: value.transition().clone(),
            canonical_transition_bytes: value.canonical_transition_bytes().to_vec(),
            resulting_complete_head: value.resulting_complete_head().clone(),
            canonical_resulting_head_bytes: value.canonical_resulting_head_bytes().to_vec(),
            resulting_core_state: value.resulting_core_state().clone(),
            canonical_resulting_core_state_bytes: value
                .canonical_resulting_core_state_bytes()
                .to_vec(),
            canonical_resulting_activity_state_bytes: value
                .canonical_resulting_activity_state_bytes()
                .to_vec(),
            resulting_memberships: value.resulting_memberships().to_vec(),
            timer_changes: value.timer_changes().to_vec(),
            delivery_consequences: value.delivery_consequences().to_vec(),
            activation_decisions: value.activation_decisions().to_vec(),
        }
    }
}

#[derive(Clone)]
pub(super) enum SqliteExistingIntent {
    Advance(Box<SqliteAdvancePersistence>),
    DurableDisposition,
}

impl SqliteExistingIntent {
    pub(super) fn from_legacy(value: &PreparedExistingIntentV1) -> Self {
        match value {
            PreparedExistingIntentV1::Advance(value) => {
                Self::Advance(Box::new(SqliteAdvancePersistence::from_legacy(value)))
            }
            PreparedExistingIntentV1::DurableDisposition => Self::DurableDisposition,
        }
    }

    pub(super) fn from_canonical(value: &PreparedCanonicalExistingIntent) -> Self {
        match value {
            PreparedCanonicalExistingIntent::Advance(value) => {
                Self::Advance(Box::new(SqliteAdvancePersistence::from_canonical(value)))
            }
            PreparedCanonicalExistingIntent::DurableDisposition => Self::DurableDisposition,
        }
    }
}
