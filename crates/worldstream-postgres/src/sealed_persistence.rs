//! Borrowed sealed facts shared by both production Room Commit ports.
//! Complete materializations stay separate from canonical record commitments.
use worldstream_core::{
    Blake3DigestV1, CanonicalRequestHashV1, CompleteHeadV1, CoreRoomStateV1, IntegrityGenerationV1,
    OperationIdentityV1, PreparedActivationDecisionV1, PreparedAdvancePersistenceV1,
    PreparedAuthorityWitnessV1, PreparedCanonicalAdvancePersistence,
    PreparedCanonicalCreationPersistence, PreparedCanonicalExistingIntent,
    PreparedCanonicalRoomWrite, PreparedCreationPersistenceV1, PreparedExistingIntentV1,
    PreparedMembershipMaterializationV1, PreparedObservationConsequenceV1,
    PreparedOperationInputWitnessV1, PreparedRoomWriteV1, PreparedTimerMaterializationV1,
    PreparedTimerMutationV1, RoomSequenceV1, StoredSemanticResultV1, TransitionRecord,
    TransitionV1,
};

pub(super) enum TransitionCommitments<'a> {
    Legacy(&'a TransitionV1),
    Canonical(&'a TransitionRecord),
}
macro_rules! commitment_getter {
    ($name:ident, $return:ty) => {
        pub(super) fn $name(&self) -> $return {
            match self {
                Self::Legacy(value) => value.$name(),
                Self::Canonical(value) => value.$name(),
            }
        }
    };
}
impl TransitionCommitments<'_> {
    commitment_getter!(room_seq, RoomSequenceV1);
    commitment_getter!(transition_hash, &Blake3DigestV1);
    commitment_getter!(resulting_core_state_hash, &Blake3DigestV1);
    commitment_getter!(resulting_activity_state_hash, &Blake3DigestV1);
    commitment_getter!(resulting_authoritative_state_hash, &Blake3DigestV1);
}
pub(super) struct PostgresCreationPersistence<'a> {
    pub(super) canonical_pack_revision_lock_bytes: &'a [u8],
    pub(super) canonical_genesis_bytes: &'a [u8],
    pub(super) complete_head: &'a CompleteHeadV1,
    pub(super) canonical_head_bytes: &'a [u8],
    pub(super) canonical_core_state_bytes: &'a [u8],
    pub(super) canonical_activity_state_bytes: &'a [u8],
    pub(super) memberships: &'a [PreparedMembershipMaterializationV1],
    pub(super) initial_timers: &'a [PreparedTimerMaterializationV1],
    pub(super) integrity_generation: IntegrityGenerationV1,
}
impl<'a> PostgresCreationPersistence<'a> {
    fn legacy(value: &'a PreparedCreationPersistenceV1) -> Self {
        Self {
            canonical_pack_revision_lock_bytes: &value.canonical_pack_revision_lock_bytes,
            canonical_genesis_bytes: &value.canonical_genesis_bytes,
            complete_head: &value.complete_head,
            canonical_head_bytes: &value.canonical_head_bytes,
            canonical_core_state_bytes: &value.canonical_core_state_bytes,
            canonical_activity_state_bytes: &value.canonical_activity_state_bytes,
            memberships: &value.memberships,
            initial_timers: &value.initial_timers,
            integrity_generation: value.integrity_generation,
        }
    }
    fn canonical(value: &'a PreparedCanonicalCreationPersistence) -> Self {
        Self {
            canonical_pack_revision_lock_bytes: value.canonical_pack_revision_lock_bytes(),
            canonical_genesis_bytes: value.canonical_genesis_bytes(),
            complete_head: value.complete_head(),
            canonical_head_bytes: value.canonical_head_bytes(),
            canonical_core_state_bytes: value.canonical_core_state_bytes(),
            canonical_activity_state_bytes: value.canonical_activity_state_bytes(),
            memberships: value.memberships(),
            initial_timers: value.initial_timers(),
            integrity_generation: value.integrity_generation(),
        }
    }
}
pub(super) struct PostgresAdvancePersistence<'a> {
    pub(super) transition: TransitionCommitments<'a>,
    pub(super) canonical_transition_bytes: &'a [u8],
    pub(super) resulting_complete_head: &'a CompleteHeadV1,
    pub(super) canonical_resulting_head_bytes: &'a [u8],
    pub(super) resulting_core_state: &'a CoreRoomStateV1,
    pub(super) canonical_resulting_core_state_bytes: &'a [u8],
    pub(super) canonical_resulting_activity_state_bytes: &'a [u8],
    pub(super) resulting_memberships: &'a [PreparedMembershipMaterializationV1],
    pub(super) timer_changes: &'a [PreparedTimerMutationV1],
    pub(super) delivery_consequences: &'a [PreparedObservationConsequenceV1],
    pub(super) activation_decisions: &'a [PreparedActivationDecisionV1],
}
impl<'a> PostgresAdvancePersistence<'a> {
    fn legacy(value: &'a PreparedAdvancePersistenceV1) -> Self {
        Self {
            transition: TransitionCommitments::Legacy(&value.transition),
            canonical_transition_bytes: &value.canonical_transition_bytes,
            resulting_complete_head: &value.resulting_complete_head,
            canonical_resulting_head_bytes: &value.canonical_resulting_head_bytes,
            resulting_core_state: &value.resulting_core_state,
            canonical_resulting_core_state_bytes: &value.canonical_resulting_core_state_bytes,
            canonical_resulting_activity_state_bytes: &value
                .canonical_resulting_activity_state_bytes,
            resulting_memberships: &value.resulting_memberships,
            timer_changes: &value.timer_changes,
            delivery_consequences: &value.delivery_consequences,
            activation_decisions: &value.activation_decisions,
        }
    }
    fn canonical(value: &'a PreparedCanonicalAdvancePersistence) -> Self {
        Self {
            transition: TransitionCommitments::Canonical(value.transition()),
            canonical_transition_bytes: value.canonical_transition_bytes(),
            resulting_complete_head: value.resulting_complete_head(),
            canonical_resulting_head_bytes: value.canonical_resulting_head_bytes(),
            resulting_core_state: value.resulting_core_state(),
            canonical_resulting_core_state_bytes: value.canonical_resulting_core_state_bytes(),
            canonical_resulting_activity_state_bytes: value
                .canonical_resulting_activity_state_bytes(),
            resulting_memberships: value.resulting_memberships(),
            timer_changes: value.timer_changes(),
            delivery_consequences: value.delivery_consequences(),
            activation_decisions: value.activation_decisions(),
        }
    }
}
pub(super) enum PostgresExistingIntent<'a> {
    Advance(Box<PostgresAdvancePersistence<'a>>),
    DurableDisposition,
}
pub(super) struct PostgresPreparedCreation<'a> {
    identity: &'a OperationIdentityV1,
    hash: &'a CanonicalRequestHashV1,
    authority: &'a PreparedAuthorityWitnessV1,
    semantic: &'a StoredSemanticResultV1,
    persistence: PostgresCreationPersistence<'a>,
}
pub(super) struct PostgresPreparedCommit<'a> {
    identity: &'a OperationIdentityV1,
    hash: &'a CanonicalRequestHashV1,
    authority: &'a PreparedAuthorityWitnessV1,
    semantic: &'a StoredSemanticResultV1,
    basis: &'a CompleteHeadV1,
    generation: IntegrityGenerationV1,
    input: &'a PreparedOperationInputWitnessV1,
    intent: PostgresExistingIntent<'a>,
}
impl PostgresPreparedCreation<'_> {
    pub(super) fn authority_witness(&self) -> &PreparedAuthorityWitnessV1 {
        self.authority
    }
    pub(super) fn semantic_result(&self) -> &StoredSemanticResultV1 {
        self.semantic
    }
    pub(super) fn persistence(&self) -> &PostgresCreationPersistence<'_> {
        &self.persistence
    }
}
impl PostgresPreparedCommit<'_> {
    pub(super) fn authority_witness(&self) -> &PreparedAuthorityWitnessV1 {
        self.authority
    }
    pub(super) fn semantic_result(&self) -> &StoredSemanticResultV1 {
        self.semantic
    }
    pub(super) fn basis_complete_head(&self) -> &CompleteHeadV1 {
        self.basis
    }
    pub(super) fn integrity_generation(&self) -> IntegrityGenerationV1 {
        self.generation
    }
    pub(super) fn input_witness(&self) -> &PreparedOperationInputWitnessV1 {
        self.input
    }
    pub(super) fn intent(&self) -> &PostgresExistingIntent<'_> {
        &self.intent
    }
}
pub(super) enum PostgresPreparedWrite<'a> {
    Create(PostgresPreparedCreation<'a>),
    Existing(PostgresPreparedCommit<'a>),
}
impl<'a> PostgresPreparedWrite<'a> {
    pub(super) fn legacy(write: &'a PreparedRoomWriteV1) -> Self {
        match write {
            PreparedRoomWriteV1::Create(value) => Self::Create(PostgresPreparedCreation {
                identity: write.identity(),
                hash: write.request_hash(),
                authority: value.authority_witness(),
                semantic: value.semantic_result(),
                persistence: PostgresCreationPersistence::legacy(value.persistence()),
            }),
            PreparedRoomWriteV1::Existing(value) => Self::Existing(PostgresPreparedCommit {
                identity: write.identity(),
                hash: write.request_hash(),
                authority: value.authority_witness(),
                semantic: value.semantic_result(),
                basis: value.basis_complete_head(),
                generation: value.integrity_generation(),
                input: value.input_witness(),
                intent: match value.intent() {
                    PreparedExistingIntentV1::Advance(advance) => PostgresExistingIntent::Advance(
                        Box::new(PostgresAdvancePersistence::legacy(advance)),
                    ),
                    PreparedExistingIntentV1::DurableDisposition => {
                        PostgresExistingIntent::DurableDisposition
                    }
                },
            }),
        }
    }
    pub(super) fn canonical(write: &'a PreparedCanonicalRoomWrite) -> Self {
        match write {
            PreparedCanonicalRoomWrite::Create(value) => Self::Create(PostgresPreparedCreation {
                identity: value.identity(),
                hash: value.request_hash(),
                authority: value.authority_witness(),
                semantic: value.semantic_result(),
                persistence: PostgresCreationPersistence::canonical(value.persistence()),
            }),
            PreparedCanonicalRoomWrite::Existing(value) => Self::Existing(PostgresPreparedCommit {
                identity: value.identity(),
                hash: value.request_hash(),
                authority: value.authority_witness(),
                semantic: value.semantic_result(),
                basis: value.basis_complete_head(),
                generation: value.integrity_generation(),
                input: value.input_witness(),
                intent: match value.intent() {
                    PreparedCanonicalExistingIntent::Advance(advance) => {
                        PostgresExistingIntent::Advance(Box::new(
                            PostgresAdvancePersistence::canonical(advance),
                        ))
                    }
                    PreparedCanonicalExistingIntent::DurableDisposition => {
                        PostgresExistingIntent::DurableDisposition
                    }
                },
            }),
        }
    }
    pub(super) fn identity(&self) -> &OperationIdentityV1 {
        match self {
            Self::Create(value) => value.identity,
            Self::Existing(value) => value.identity,
        }
    }
    pub(super) fn request_hash(&self) -> &CanonicalRequestHashV1 {
        match self {
            Self::Create(value) => value.hash,
            Self::Existing(value) => value.hash,
        }
    }
}
