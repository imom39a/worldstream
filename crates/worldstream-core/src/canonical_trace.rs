//! Additive format-neutral execution. Legacy callers keep their V1 contracts.
use super::*;
use crate::{
    CanonicalHistoryFormat, GenesisRecord, GenesisV2, LineageCodecError, TransitionRecord,
};

#[cfg(feature = "conformance-tracer")]
static RETAINED_PACK_REDUCE_INVOCATIONS: AtomicUsize = AtomicUsize::new(0);

/// Returns process-wide calls to retained Pack reducers through the Core host.
/// This includes both serving formats, Replay and failed reducer outputs.
/// Direct executor calls used by golden authoring are outside this counter.
#[cfg(feature = "conformance-tracer")]
#[must_use]
pub fn retained_pack_reduce_invocation_count_for_conformance() -> usize {
    RETAINED_PACK_REDUCE_INVOCATIONS.load(AtomicOrdering::Relaxed)
}

/// Resets the process counter before an isolated measurement starts.
/// Call only when no measured reduction is in flight. Concurrent work after
/// reset contributes to the count. This does not reset per-executor counters.
#[cfg(feature = "conformance-tracer")]
pub fn reset_retained_pack_reduce_invocations_for_conformance() {
    RETAINED_PACK_REDUCE_INVOCATIONS.store(0, AtomicOrdering::Relaxed);
}

#[cfg(feature = "conformance-tracer")]
pub(super) fn record_retained_pack_reduce_invocation_for_conformance() {
    RETAINED_PACK_REDUCE_INVOCATIONS.fetch_add(1, AtomicOrdering::Relaxed);
}

/// Pure outcome with canonical records separated from prepared state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CanonicalAdvanceDisposition {
    /// Exactly one accepted immutable record.
    TransitionAccepted {
        existing: bool,
        transition: Box<TransitionRecord>,
    },
    /// Stable declared rejection without a Transition.
    RejectionRecorded {
        existing: bool,
        rejection: ActivityRejectionV1,
        canonical_receipt_bytes: Vec<u8>,
    },
    /// Stable administrative NoChange without a Transition.
    NoChangeRecorded {
        existing: bool,
        canonical_receipt_bytes: Vec<u8>,
    },
}

/// Opaque checked reduction. Only Core can construct its complete resulting
/// state and provenance. No storage result installs speculative state.
pub struct PreparedCanonicalRoomTransition {
    pub(super) preparer_identity: Arc<()>,
    pub(super) basis_complete_head: CompleteHeadV1,
    pub(super) recorded_stimulus: RecordedStimulusV1,
    pub(super) action_offer_witness: Option<Vec<u8>>,
    pub(super) outcome: CanonicalAdvanceDisposition,
    pub(super) resulting_state: Option<RoomTransitionStateV1>,
    pub(super) administration: Option<(
        AdministrationOperationIdentityV1,
        Blake3DigestV1,
        CompleteHeadV1,
    )>,
    pub(super) is_new: bool,
}

impl PreparedCanonicalRoomTransition {
    pub(super) fn into_legacy(self) -> Result<PreparedRoomTransitionV1, TraceErrorV1> {
        let outcome = match self.outcome {
            CanonicalAdvanceDisposition::TransitionAccepted {
                existing,
                transition,
            } => {
                let TransitionRecord::V1(transition) = *transition else {
                    return Err(TraceErrorV1::VersionIdentityMismatch);
                };
                AdvanceDispositionV1::TransitionAccepted {
                    existing,
                    transition: Box::new(transition),
                }
            }
            CanonicalAdvanceDisposition::RejectionRecorded {
                existing,
                rejection,
                canonical_receipt_bytes,
            } => AdvanceDispositionV1::RejectionRecorded {
                existing,
                rejection,
                canonical_receipt_bytes,
            },
            CanonicalAdvanceDisposition::NoChangeRecorded {
                existing,
                canonical_receipt_bytes,
            } => AdvanceDispositionV1::NoChangeRecorded {
                existing,
                canonical_receipt_bytes,
            },
        };
        Ok(PreparedRoomTransitionV1 {
            preparer_identity: self.preparer_identity,
            basis_complete_head: self.basis_complete_head,
            recorded_stimulus: self.recorded_stimulus,
            action_offer_witness: self.action_offer_witness,
            outcome,
            resulting_state: self.resulting_state,
            administration: self.administration,
            is_new: self.is_new,
        })
    }

    /// Returns the exact immutable reduction basis.
    #[must_use]
    pub const fn basis_complete_head(&self) -> &CompleteHeadV1 {
        &self.basis_complete_head
    }
    /// Returns the complete normalized input.
    #[must_use]
    pub const fn recorded_stimulus(&self) -> &RecordedStimulusV1 {
        &self.recorded_stimulus
    }
    /// Returns the immutable canonical outcome without complete V2 state.
    #[must_use]
    pub const fn disposition(&self) -> &CanonicalAdvanceDisposition {
        &self.outcome
    }
    /// Returns complete verified materializations independently of the record.
    #[must_use]
    pub const fn resulting_state(&self) -> Option<&RoomTransitionStateV1> {
        self.resulting_state.as_ref()
    }
    /// Returns whether this reduction is new work.
    #[must_use]
    pub const fn is_new(&self) -> bool {
        self.is_new
    }
    /// Returns the exact authorized Action Offer witness.
    #[must_use]
    pub fn action_offer_witness(&self) -> Option<&[u8]> {
        self.action_offer_witness.as_deref()
    }
}

struct CanonicalAdministrationResult {
    request_hash: Blake3DigestV1,
    basis_head: CompleteHeadV1,
    outcome: CanonicalAdvanceDisposition,
}

/// Unique current executor for either immutable lineage format. It uses the
/// same retained Pack and Core preparer as the legacy trace. It is not Clone.
pub struct CanonicalRoomTrace {
    pub(super) genesis: GenesisRecord,
    pub(super) transitions: Vec<TransitionRecord>,
    pub(super) state: RoomTransitionStateV1,
    pub(super) preparer: RoomTransitionPreparerV1,
    pub(super) retained_pack: RetainedActivityPackV1,
    administration_results:
        BTreeMap<AdministrationOperationIdentityV1, CanonicalAdministrationResult>,
    activity_callback_count: AtomicUsize,
}

impl CanonicalRoomTrace {
    pub(super) fn from_verified_genesis(
        genesis: GenesisRecord,
        retained_pack: RetainedActivityPackV1,
    ) -> Result<Self, TraceErrorV1> {
        genesis.verify().map_err(map_codec_error)?;
        if genesis.pack_digest() != &retained_pack.descriptor().revision_digest {
            return Err(TraceErrorV1::VersionIdentityMismatch);
        }
        let preparer = RoomTransitionPreparerV1::from_retained_pack(
            retained_pack.clone(),
            genesis.room_seed().clone(),
        );
        let core_state = preparer
            .core_reducer
            .validate_state(genesis.initial_core_state().clone())?;
        let timers = TimerBookV1::from_genesis(
            genesis.initial_timers(),
            genesis.created_at().as_str(),
            false,
        )?;
        let state = RoomTransitionStateV1 {
            head: genesis.complete_head(),
            core_state,
            activity_state: genesis.initial_activity_state().clone(),
            timers,
            preparer_identity: Arc::clone(&preparer.identity),
        };
        Ok(Self {
            genesis,
            transitions: Vec::new(),
            state,
            preparer,
            retained_pack,
            administration_results: BTreeMap::new(),
            activity_callback_count: AtomicUsize::new(0),
        })
    }

    pub(crate) fn create_uncommitted(
        prepared_genesis: PreparedNewRoomGenesisV1,
        format: CanonicalHistoryFormat,
    ) -> Result<Self, TraceErrorV1> {
        let input = prepared_genesis.genesis_input().clone();
        let retained_pack = prepared_genesis.retained_pack().clone();
        if input.pack_digest != retained_pack.descriptor().revision_digest {
            return Err(TraceErrorV1::VersionIdentityMismatch);
        }
        let preparer = RoomTransitionPreparerV1::from_retained_pack(
            retained_pack.clone(),
            input.room_seed.clone(),
        );
        if format == CanonicalHistoryFormat::V1 {
            let legacy =
                CoreTraceV1::create_with_preparer(input, preparer, Some(retained_pack.clone()))?;
            return Ok(Self {
                genesis: GenesisRecord::V1(legacy.genesis.clone()),
                transitions: Vec::new(),
                state: legacy.transition_state(),
                preparer: legacy.preparer,
                retained_pack,
                administration_results: BTreeMap::new(),
                activity_callback_count: AtomicUsize::new(0),
            });
        }
        if input.initial_core_state.memberships().len() > MAX_ROOM_MEMBERSHIPS_V1 {
            return Err(TraceErrorV1::RoomMembershipLimitExceeded);
        }
        crate::PAYLOAD_BUDGET_V1.check_value(
            crate::PayloadKindV1::CreationConfiguration,
            &input.configuration,
        )?;
        crate::PAYLOAD_BUDGET_V1
            .check_value(crate::PayloadKindV1::CoreState, &input.initial_core_state)?;
        let core = preparer
            .core_reducer
            .validate_state(input.initial_core_state.clone())?;
        let timers =
            TimerBookV1::from_genesis(&input.initial_timers, input.created_at.as_str(), true)?;
        let activity_state = input.initial_activity_state.clone();
        crate::PAYLOAD_BUDGET_V1
            .check_authoritative_state(&core.state, &activity_state)
            .and_then(|()| crate::PAYLOAD_BUDGET_V1.check_initial_timers(&input.initial_timers))
            .map_err(|error| PackFaultV1::OperationFault {
                operation: ActivityPackOperationV1::Initialize,
                fault: Box::new(PackFaultV1::OutputBoundExceeded(error.to_string())),
            })?;
        let genesis = GenesisRecord::V2(GenesisV2::new(input).map_err(map_codec_error)?);
        crate::PAYLOAD_BUDGET_V1
            .check_bytes(crate::PayloadKindV1::Genesis, &genesis.canonical_bytes()?)
            .map_err(|error| PackFaultV1::OperationFault {
                operation: ActivityPackOperationV1::Initialize,
                fault: Box::new(PackFaultV1::OutputBoundExceeded(error.to_string())),
            })?;
        let state = RoomTransitionStateV1 {
            head: genesis.complete_head(),
            core_state: core,
            activity_state,
            timers,
            preparer_identity: Arc::clone(&preparer.identity),
        };
        Ok(Self {
            genesis,
            transitions: Vec::new(),
            state,
            preparer,
            retained_pack,
            administration_results: BTreeMap::new(),
            activity_callback_count: AtomicUsize::new(0),
        })
    }

    /// Creates an explicitly selected in-memory executor for conformance.
    /// Production creation stays behind the sealed storage coordinator.
    ///
    /// # Errors
    /// Returns a trace error for invalid initial facts or exact Pack identity.
    #[cfg(any(test, feature = "conformance-tracer"))]
    #[doc(hidden)]
    pub fn create_from_retained_for_conformance(
        prepared: PreparedNewRoomGenesisV1,
        format: CanonicalHistoryFormat,
    ) -> Result<Self, TraceErrorV1> {
        Self::create_uncommitted(prepared, format)
    }

    /// Prepares one input through the shared exact reducer without mutating
    /// state, scheduling work, or publishing observations.
    ///
    /// # Errors
    /// Returns the existing trace error for invalid input or Pack failure.
    pub fn prepare(
        &self,
        stimulus: RecordedStimulusV1,
    ) -> Result<PreparedCanonicalRoomTransition, TraceErrorV1> {
        if let RecordedStimulusV1::CoreProposed(proposal) = &stimulus
            && let Some(stored) = self
                .administration_results
                .get(&proposal.operation_identity)
        {
            if hash_administration_request(&stored.basis_head, proposal)? != stored.request_hash {
                return Err(TraceErrorV1::IdempotencyConflict);
            }
            let mut outcome = stored.outcome.clone();
            match &mut outcome {
                CanonicalAdvanceDisposition::TransitionAccepted { existing, .. }
                | CanonicalAdvanceDisposition::RejectionRecorded { existing, .. }
                | CanonicalAdvanceDisposition::NoChangeRecorded { existing, .. } => {
                    *existing = true
                }
            }
            let administration = Some((
                proposal.operation_identity.clone(),
                stored.request_hash.clone(),
                stored.basis_head.clone(),
            ));
            return Ok(PreparedCanonicalRoomTransition {
                preparer_identity: Arc::clone(&self.preparer.identity),
                basis_complete_head: stored.basis_head.clone(),
                recorded_stimulus: stimulus,
                action_offer_witness: None,
                outcome,
                resulting_state: None,
                administration,
                is_new: false,
            });
        }
        self.preparer.prepare_canonical_inner(
            &self.state,
            self.genesis.format(),
            stimulus,
            Some(&self.activity_callback_count),
        )
    }

    pub(crate) fn validate_prepared(
        &self,
        prepared: &PreparedCanonicalRoomTransition,
    ) -> Result<(), TraceErrorV1> {
        if !Arc::ptr_eq(&prepared.preparer_identity, &self.preparer.identity) {
            return Err(TraceErrorV1::TransitionPreparerProvenanceMismatch);
        }
        if !prepared.is_new || prepared.basis_complete_head != self.state.head {
            return Err(TraceErrorV1::PreparedBasisMismatch);
        }
        let state = prepared
            .resulting_state
            .as_ref()
            .ok_or(TraceErrorV1::InvalidPreparedAdvance)?;
        if !Arc::ptr_eq(&state.preparer_identity, &self.preparer.identity) {
            return Err(TraceErrorV1::TransitionPreparerProvenanceMismatch);
        }
        if !Arc::ptr_eq(
            &state.core_state.reducer_identity,
            &self.preparer.core_reducer.identity,
        ) {
            return Err(TraceErrorV1::CoreReducerProvenanceMismatch);
        }
        if let CanonicalAdvanceDisposition::TransitionAccepted { transition, .. } =
            &prepared.outcome
        {
            if transition.format() != self.genesis.format() {
                return Err(TraceErrorV1::VersionIdentityMismatch);
            }
            transition
                .verify_successor(&self.state.head)
                .map_err(map_codec_error)?;
            if state.head != transition.complete_head()
                || hash_core_state(&state.core_state.state)?
                    != *transition.resulting_core_state_hash()
                || hash_activity_state(state.head.pack_digest(), &state.activity_state)?
                    != *transition.resulting_activity_state_hash()
            {
                return Err(TraceErrorV1::InvalidPreparedAdvance);
            }
        } else if state.head != self.state.head {
            return Err(TraceErrorV1::InvalidPreparedAdvance);
        }
        Ok(())
    }

    pub(crate) fn observe_prepared(
        &self,
        prepared: &PreparedCanonicalRoomTransition,
        viewer: &PackViewerV1,
    ) -> Result<ActivityObservationOutcomeV1, TraceErrorV1> {
        self.validate_prepared(prepared)?;
        let CanonicalAdvanceDisposition::TransitionAccepted { transition, .. } = &prepared.outcome
        else {
            return Err(TraceErrorV1::PreparedAdvanceHasNoTransition);
        };
        let after = prepared
            .resulting_state
            .as_ref()
            .ok_or(TraceErrorV1::InvalidPreparedAdvance)?;
        Ok(self.retained_pack.host().observe_with_budget(
            &ObserveTransitionInputV1 {
                core_before: self.core_state(),
                activity_before: self.activity_state(),
                head_before: self.head(),
                core_after: after.core_state(),
                activity_after: after.activity_state(),
                head_after: after.head(),
                recorded_stimulus: transition.recorded_stimulus(),
                ordered_domain_events: transition.ordered_domain_events(),
                viewer,
            },
            self.payload_budget(),
        )?)
    }

    pub(crate) fn install_prepared(
        &mut self,
        prepared: PreparedCanonicalRoomTransition,
    ) -> Result<CanonicalAdvanceDisposition, TraceErrorV1> {
        self.validate_prepared(&prepared)?;
        let state = prepared
            .resulting_state
            .ok_or(TraceErrorV1::InvalidPreparedAdvance)?;
        if let CanonicalAdvanceDisposition::TransitionAccepted { transition, .. } =
            &prepared.outcome
        {
            self.state = state;
            self.transitions.push((**transition).clone());
        }
        if let Some((identity, request_hash, basis_head)) = prepared.administration {
            self.administration_results.insert(
                identity,
                CanonicalAdministrationResult {
                    request_hash,
                    basis_head,
                    outcome: prepared.outcome.clone(),
                },
            );
        }
        Ok(prepared.outcome)
    }

    /// Returns an exact authorized current view through the retained Pack.
    ///
    /// # Errors
    /// Returns the existing Pack/trace error when the view is unavailable.
    pub fn view(&self, viewer: &PackViewerV1) -> Result<crate::ValidatedPackViewV1, TraceErrorV1> {
        Ok(self.retained_pack.host().view_with_budget(
            &ViewInputV1 {
                core: self.core_state(),
                activity_state: self.activity_state(),
                complete_head: self.head(),
                viewer,
            },
            self.payload_budget(),
        )?)
    }

    /// Checks the complete live views required by V2 Genesis admission.
    /// Suspended Members have no authorized live view. V1 retains its original
    /// callback contract. Trusted checkpoint reconstruction does not call this.
    pub(crate) fn validate_initial_authorized_views(&self) -> Result<(), TraceErrorV1> {
        if self.genesis.format() != CanonicalHistoryFormat::V2 {
            return Ok(());
        }
        for membership in self.core_state().memberships().values() {
            if membership.standing() != MembershipStandingV1::Enabled {
                continue;
            }
            let viewer = match membership.access_mode() {
                AccessModeV1::Participant => {
                    PackViewerV1::Participant(membership.member_id().clone())
                }
                AccessModeV1::Spectator => PackViewerV1::Public(membership.member_id().clone()),
                AccessModeV1::Operator => PackViewerV1::Operator(membership.member_id().clone()),
            };
            self.view(&viewer)?;
        }
        Ok(())
    }

    pub(crate) fn payload_budget(&self) -> Option<crate::PayloadBudgetV1> {
        (self.genesis.format() == CanonicalHistoryFormat::V2).then_some(crate::PAYLOAD_BUDGET_V1)
    }
    /// Returns immutable Genesis and its selected format.
    #[must_use]
    pub const fn genesis(&self) -> &GenesisRecord {
        &self.genesis
    }
    /// Returns the exact current Head.
    #[must_use]
    pub const fn head(&self) -> &CompleteHeadV1 {
        &self.state.head
    }
    /// Returns complete verified current Core state.
    #[must_use]
    pub fn core_state(&self) -> &CoreRoomStateV1 {
        self.state.core_state()
    }
    /// Returns complete verified current Activity state.
    #[must_use]
    pub fn activity_state(&self) -> &CanonicalJsonV1 {
        self.state.activity_state()
    }
    /// Returns the exact retained Pack for this executor.
    #[must_use]
    pub const fn retained_pack(&self) -> &RetainedActivityPackV1 {
        &self.retained_pack
    }
    /// Returns the accepted immutable records retained in this accumulator.
    #[must_use]
    pub fn transitions(&self) -> &[TransitionRecord] {
        &self.transitions
    }
    /// Returns the exact currently scheduled Timer generations.
    #[must_use]
    pub fn scheduled_timers(&self) -> &BTreeMap<TimerId, ScheduledTimerV1> {
        &self.state.timers.scheduled
    }
}

pub(super) fn map_codec_error(error: LineageCodecError) -> TraceErrorV1 {
    match error {
        LineageCodecError::Canonical(error) => TraceErrorV1::Canonical(error),
        LineageCodecError::UnsupportedIdentity | LineageCodecError::MixedFormat => {
            TraceErrorV1::VersionIdentityMismatch
        }
        LineageCodecError::CommitmentMismatch => TraceErrorV1::StateHashMismatch,
        LineageCodecError::SuccessorMismatch => TraceErrorV1::CompleteHeadMismatch,
        LineageCodecError::InvalidCoreOrGenesis => TraceErrorV1::InvalidPreparedAdvance,
    }
}

impl CanonicalRoomTrace {
    pub(crate) fn assess_stable_action_disposition(
        &self,
        request: &crate::ParticipantActionRequestV1,
        admitted_at: &crate::ActionAdmittedAt,
    ) -> Result<Option<StableActionAdmissionV1>, TraceErrorV1> {
        CoreTraceV1::assess_stable_action_from_values(
            self.head(),
            self.core_state(),
            self.activity_state(),
            self.retained_pack(),
            request,
            admitted_at,
            self.payload_budget(),
        )
    }
    /// Returns exact immutable Genesis bytes.
    ///
    /// # Errors
    /// Returns a canonical encoding error.
    pub fn genesis_bytes(&self) -> Result<Vec<u8>, CanonicalJsonError> {
        self.genesis.canonical_bytes()
    }
}

impl CanonicalRoomTrace {
    /// Drops accumulator history after storage has retained it. Current state,
    /// Head, exact executor, seed, and Timer generation fences remain intact.
    pub fn discard_persisted_history(&mut self) {
        self.transitions = Vec::new();
        self.administration_results.clear();
    }
    /// Returns the reducer invocation count for conformance and diagnostics.
    #[must_use]
    pub fn activity_callback_count(&self) -> usize {
        self.activity_callback_count.load(AtomicOrdering::Relaxed)
    }

    /// Converts only a genuine V1 lineage to the legacy executor contract.
    /// Compact state is never placed in a fabricated V1 record.
    ///
    /// # Errors
    /// Returns this unchanged executor when its lineage is compact.
    pub fn try_into_legacy(self) -> Result<CoreTraceV1, Box<Self>> {
        if self.genesis.format() != CanonicalHistoryFormat::V1
            || self
                .transitions
                .iter()
                .any(|record| record.format() != CanonicalHistoryFormat::V1)
        {
            return Err(Box::new(self));
        }
        let room_seed = self.genesis.room_seed().clone();
        let GenesisRecord::V1(genesis) = self.genesis else {
            unreachable!("format checked above")
        };
        let transitions = self
            .transitions
            .into_iter()
            .map(|record| {
                let TransitionRecord::V1(record) = record else {
                    unreachable!("all formats checked above")
                };
                record
            })
            .collect();
        let administration_results = self
            .administration_results
            .into_iter()
            .map(|(identity, stored)| {
                let outcome = match stored.outcome {
                    CanonicalAdvanceDisposition::TransitionAccepted { transition, .. } => {
                        let TransitionRecord::V1(record) = *transition else {
                            unreachable!("V1 trace prepares only V1 records")
                        };
                        StoredOutcomeV1::Transition(Box::new(record))
                    }
                    CanonicalAdvanceDisposition::RejectionRecorded {
                        rejection,
                        canonical_receipt_bytes,
                        ..
                    } => StoredOutcomeV1::Rejection(rejection, canonical_receipt_bytes),
                    CanonicalAdvanceDisposition::NoChangeRecorded {
                        canonical_receipt_bytes,
                        ..
                    } => StoredOutcomeV1::NoChange(canonical_receipt_bytes),
                };
                (
                    identity,
                    StoredAdministrationV1 {
                        request_hash: stored.request_hash,
                        basis_head: stored.basis_head,
                        outcome,
                    },
                )
            })
            .collect();
        Ok(CoreTraceV1 {
            genesis,
            transitions,
            core_state: self.state.core_state.state,
            activity_state: self.state.activity_state,
            head: self.state.head,
            timers: self.state.timers,
            administration_results,
            activity_callback_count: self.activity_callback_count,
            administrative_receipt_count: 0,
            room_seed,
            retained_pack: Some(self.retained_pack),
            preparer: self.preparer,
        })
    }
}
