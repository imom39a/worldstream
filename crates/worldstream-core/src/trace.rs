use std::{
    cmp::Ordering,
    collections::BTreeMap,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering as AtomicOrdering},
    },
};

use serde::Serialize;
use thiserror::Error;

use crate::{
    AccessModeV1, ActivityApplyV1, ActivityDispositionV1, ActivityReduceInputV1,
    ActivityRejectionV1, AdministrationOperationIdentityV1, Blake3DigestV1, CANONICAL_CODEC_ID,
    CORE_SCHEMA_VERSION, CanonicalJsonError, CanonicalJsonV1, CompleteHeadV1, CoreProposedV1,
    CoreRoomStateV1, CoreValidationErrorV1, GENESIS_VERSION, GenesisInputV1, GenesisV1,
    HASH_SUITE_ID, MembershipStandingV1, RecordedStimulusV1, RoomSequenceV1, RoomStatusV1,
    ScheduledTimerV1, TRANSITION_VERSION, TimerChangeV1, TimerGenerationV1, TimerId,
    TimerRequestV1, TransitionV1,
    canonical::encode,
    lineage::{hash_activity_state, hash_authoritative_state, hash_core_state},
    model::CoreProposalClassV1,
    primitives::compare_timestamp_text,
    reducer::{CheckedCoreErrorV1, RoleValidationFailureV1, propose_core, validate_core_state},
};

/// A pure in-memory canonical conformance tracer. It performs no external
/// effect and is not the runtime prepared-write/COMMIT seam.
pub struct CoreTraceV1 {
    genesis: GenesisV1,
    transitions: Vec<TransitionV1>,
    core_state: CoreRoomStateV1,
    activity_state: CanonicalJsonV1,
    head: CompleteHeadV1,
    timers: TimerBookV1,
    administration_results: BTreeMap<AdministrationOperationIdentityV1, StoredAdministrationV1>,
    activity_callback_count: AtomicUsize,
    administrative_receipt_count: usize,
    preparer: RoomTransitionPreparerV1,
}

type RoleValidatorFn = dyn Fn(&CoreRoomStateV1) -> Result<(), String> + Send + Sync;
type ActivityReducerFn = dyn for<'a> Fn(&ActivityReduceInputV1<'a>) -> Result<ActivityDispositionV1, PackFaultV1>
    + Send
    + Sync;

/// Versioned, stateless reducer that exclusively constructs checked Core state.
#[derive(Clone)]
pub struct CoreReducerV1 {
    validate_roles: Arc<RoleValidatorFn>,
    identity: Arc<()>,
}

impl CoreReducerV1 {
    /// Binds the exact pack Role/cardinality validator. Role vocabulary stays
    /// outside host-owned Core while all host shape rules remain fixed here.
    pub fn new<F>(validate_roles: F) -> Self
    where
        F: Fn(&CoreRoomStateV1) -> Result<(), String> + Send + Sync + 'static,
    {
        Self {
            validate_roles: Arc::new(validate_roles),
            identity: Arc::new(()),
        }
    }

    /// Validates one complete Core value, including host shape and the bound
    /// final-state Role/cardinality rule.
    ///
    /// # Errors
    ///
    /// Returns an error for a host invariant, Role policy, callback panic, or
    /// canonical hashing failure.
    pub fn validate_state(
        &self,
        state: CoreRoomStateV1,
    ) -> Result<VerifiedCoreStateV1, TraceErrorV1> {
        validate_core_state(&state, &safe_role_validator(&self.validate_roles))
            .map_err(map_checked_core_error)?;
        let core_state_hash = hash_core_state(&state)?;
        Ok(VerifiedCoreStateV1 {
            state,
            core_state_hash,
            reducer_identity: Arc::clone(&self.identity),
        })
    }

    /// Purely reduces one typed Core proposal, validates its atomic final
    /// state, and freezes the resulting Core hash without mutating `before`.
    ///
    /// # Errors
    ///
    /// Returns an error for mismatched reducer provenance or any rejected Core
    /// invariant, Role rule, callback panic, or canonical hashing failure.
    pub fn reduce(
        &self,
        before: &VerifiedCoreStateV1,
        proposal: &CoreProposedV1,
    ) -> Result<PreparedCoreStateV1, TraceErrorV1> {
        if !Arc::ptr_eq(&self.identity, &before.reducer_identity) {
            return Err(TraceErrorV1::CoreReducerProvenanceMismatch);
        }
        let proposed = propose_core(
            &before.state,
            proposal,
            &safe_role_validator(&self.validate_roles),
        )
        .map_err(map_checked_core_error)?;
        let core_state_hash = hash_core_state(&proposed.state)?;
        Ok(PreparedCoreStateV1 {
            verified_state: VerifiedCoreStateV1 {
                state: proposed.state,
                core_state_hash,
                reducer_identity: Arc::clone(&self.identity),
            },
            class: proposed.class,
            no_change: proposed.no_change,
        })
    }
}

/// Opaque Core value that passed all host and bound Role/cardinality checks,
/// paired with its exact canonical domain-separated hash.
#[derive(Clone)]
pub struct VerifiedCoreStateV1 {
    state: CoreRoomStateV1,
    core_state_hash: Blake3DigestV1,
    reducer_identity: Arc<()>,
}

impl VerifiedCoreStateV1 {
    /// Returns the checked Core value.
    #[must_use]
    pub fn state(&self) -> &CoreRoomStateV1 {
        &self.state
    }

    /// Returns its exact canonical Core hash.
    #[must_use]
    pub fn core_state_hash(&self) -> &Blake3DigestV1 {
        &self.core_state_hash
    }
}

/// Immutable checked result produced only by [`CoreReducerV1`].
pub struct PreparedCoreStateV1 {
    verified_state: VerifiedCoreStateV1,
    class: CoreProposalClassV1,
    no_change: bool,
}

impl PreparedCoreStateV1 {
    /// Returns the complete checked final Core state.
    #[must_use]
    pub fn state(&self) -> &CoreRoomStateV1 {
        &self.verified_state.state
    }

    /// Returns the domain-separated canonical Core hash of that exact state.
    #[must_use]
    pub fn core_state_hash(&self) -> &Blake3DigestV1 {
        &self.verified_state.core_state_hash
    }

    /// Reports whether the requested desired state was already current.
    #[must_use]
    pub const fn is_no_change(&self) -> bool {
        self.no_change
    }
}

/// Pure higher-level transition preparation that composes the host-owned Core
/// reducer with one pinned Activity executor.
#[derive(Clone)]
pub struct RoomTransitionPreparerV1 {
    core_reducer: CoreReducerV1,
    reduce_activity: Arc<ActivityReducerFn>,
    identity: Arc<()>,
}

impl RoomTransitionPreparerV1 {
    /// Binds one Core reducer and one exact retained Activity executor.
    pub fn new<R>(core_reducer: CoreReducerV1, reduce_activity: R) -> Self
    where
        R: for<'a> Fn(&ActivityReduceInputV1<'a>) -> Result<ActivityDispositionV1, PackFaultV1>
            + Send
            + Sync
            + 'static,
    {
        Self {
            core_reducer,
            reduce_activity: Arc::new(reduce_activity),
            identity: Arc::new(()),
        }
    }

    /// Purely reduces and hashes one Stimulus from a previously verified
    /// opaque state. It mutates neither that state nor any trace.
    ///
    /// # Errors
    ///
    /// Returns an error for mismatched preparer provenance or any invalid,
    /// rejected, faulty, or noncanonical reduction result.
    pub fn prepare(
        &self,
        state: &RoomTransitionStateV1,
        stimulus: RecordedStimulusV1,
    ) -> Result<PreparedRoomTransitionV1, TraceErrorV1> {
        self.prepare_inner(state, stimulus, None)
    }

    /// Returns the bound host Core reducer.
    #[must_use]
    pub const fn core_reducer(&self) -> &CoreReducerV1 {
        &self.core_reducer
    }

    #[allow(clippy::too_many_lines)]
    fn prepare_inner(
        &self,
        state: &RoomTransitionStateV1,
        stimulus: RecordedStimulusV1,
        callback_counter: Option<&AtomicUsize>,
    ) -> Result<PreparedRoomTransitionV1, TraceErrorV1> {
        if !Arc::ptr_eq(&self.identity, &state.preparer_identity) {
            return Err(TraceErrorV1::TransitionPreparerProvenanceMismatch);
        }
        let administration = match &stimulus {
            RecordedStimulusV1::CoreProposed(proposal) => Some((
                proposal.operation_identity.clone(),
                hash_administration_request(&state.head, proposal)?,
                state.head.clone(),
            )),
            _ => None,
        };
        let proposed = self.validate_stimulus(state, &stimulus)?;
        if proposed.no_change {
            let receipt = administration_receipt(&stimulus, &state.head, "no_change", None)?;
            return Ok(PreparedRoomTransitionV1 {
                preparer_identity: Arc::clone(&self.identity),
                basis_complete_head: state.head.clone(),
                outcome: AdvanceDispositionV1::NoChangeRecorded {
                    existing: false,
                    canonical_receipt_bytes: receipt,
                },
                resulting_state: Some(state.clone()),
                administration,
                is_new: true,
            });
        }

        let next_room_seq = state.head.room_seq.checked_successor()?;
        let input = ActivityReduceInputV1 {
            prior_activity_state: &state.activity_state,
            core_before: &state.core_state.state,
            proposed_core_after: &proposed.verified_state.state,
            scheduled_timers: &state.timers.scheduled,
            next_room_seq,
            recorded_stimulus: &stimulus,
        };
        if let Some(counter) = callback_counter {
            counter.fetch_add(1, AtomicOrdering::Relaxed);
        }
        let reducer = Arc::clone(&self.reduce_activity);
        let disposition = catch_unwind(AssertUnwindSafe(|| reducer(&input)))
            .map_err(|_| PackFaultV1::CallbackPanicked)??;
        let apply = match disposition {
            ActivityDispositionV1::Apply(apply) => apply,
            ActivityDispositionV1::Reject(rejection) => {
                if !clean_rejection_allowed(&stimulus, proposed.class) {
                    return Err(PackFaultV1::MandatoryStimulusRejected.into());
                }
                let receipt =
                    administration_receipt(&stimulus, &state.head, "rejected", Some(&rejection))?;
                return Ok(PreparedRoomTransitionV1 {
                    preparer_identity: Arc::clone(&self.identity),
                    basis_complete_head: state.head.clone(),
                    outcome: AdvanceDispositionV1::RejectionRecorded {
                        existing: false,
                        rejection,
                        canonical_receipt_bytes: receipt,
                    },
                    resulting_state: Some(state.clone()),
                    administration,
                    is_new: true,
                });
            }
        };

        if proposed.verified_state.state.room_status == RoomStatusV1::Archived
            && !apply.timer_requests.is_empty()
        {
            return Err(PackFaultV1::ArchiveTimerMutation.into());
        }
        let (normalized_timer_changes, next_timers) = state
            .timers
            .normalize_requests(
                &stimulus,
                &apply.timer_requests,
                proposed.verified_state.state.room_status,
            )
            .map_err(|error| PackFaultV1::InvalidTimerOutput(error.to_string()))?;
        let transition = Self::build_transition(
            state,
            next_room_seq,
            stimulus,
            proposed,
            apply,
            normalized_timer_changes,
        )?;
        let resulting_state = RoomTransitionStateV1 {
            head: transition.head(),
            core_state: VerifiedCoreStateV1 {
                state: transition.resulting_core_state.clone(),
                core_state_hash: transition.resulting_core_state_hash.clone(),
                reducer_identity: Arc::clone(&self.core_reducer.identity),
            },
            activity_state: transition.resulting_activity_state.clone(),
            timers: next_timers,
            preparer_identity: Arc::clone(&self.identity),
        };
        Ok(PreparedRoomTransitionV1 {
            preparer_identity: Arc::clone(&self.identity),
            basis_complete_head: state.head.clone(),
            outcome: AdvanceDispositionV1::TransitionAccepted {
                existing: false,
                transition: Box::new(transition),
            },
            resulting_state: Some(resulting_state),
            administration,
            is_new: true,
        })
    }

    fn validate_stimulus(
        &self,
        state: &RoomTransitionStateV1,
        stimulus: &RecordedStimulusV1,
    ) -> Result<PreparedCoreStateV1, TraceErrorV1> {
        let class = match stimulus {
            RecordedStimulusV1::ParticipantAction(action) => {
                if action.exact_basis_head != state.head {
                    return Err(TraceErrorV1::CompleteHeadMismatch);
                }
                if state.core_state.state.room_status == RoomStatusV1::Archived {
                    return Err(TraceErrorV1::ArchivedStimulusForbidden);
                }
                let membership = state
                    .core_state
                    .state
                    .memberships
                    .get(&action.member_id)
                    .ok_or(TraceErrorV1::ParticipantNotEligible)?;
                if membership.standing != MembershipStandingV1::Enabled
                    || membership.access_mode != AccessModeV1::Participant
                {
                    return Err(TraceErrorV1::ParticipantNotEligible);
                }
                CoreProposalClassV1::Vetoable
            }
            RecordedStimulusV1::TimerFired(timer) => {
                if state.core_state.state.room_status == RoomStatusV1::Archived {
                    return Err(TraceErrorV1::ArchivedStimulusForbidden);
                }
                if state.timers.scheduled.get(&timer.timer_id)
                    != Some(&ScheduledTimerV1 {
                        timer_id: timer.timer_id.clone(),
                        generation: timer.generation,
                        scheduled_for: timer.scheduled_for.clone(),
                        canonical_payload: timer.canonical_payload.clone(),
                    })
                {
                    return Err(TraceErrorV1::TimerWitnessMismatch);
                }
                CoreProposalClassV1::Mandatory
            }
            RecordedStimulusV1::CoreProposed(proposal) => {
                if proposal.expected_room_seq != state.head.room_seq {
                    return Err(TraceErrorV1::ExpectedSequenceMismatch);
                }
                return self.core_reducer.reduce(&state.core_state, proposal);
            }
            RecordedStimulusV1::ExternalInput(_) => {
                if state.core_state.state.room_status == RoomStatusV1::Archived {
                    return Err(TraceErrorV1::ArchivedStimulusForbidden);
                }
                CoreProposalClassV1::Mandatory
            }
        };
        Ok(PreparedCoreStateV1 {
            verified_state: state.core_state.clone(),
            class,
            no_change: false,
        })
    }

    fn build_transition(
        state: &RoomTransitionStateV1,
        room_seq: RoomSequenceV1,
        stimulus: RecordedStimulusV1,
        proposed: PreparedCoreStateV1,
        apply: ActivityApplyV1,
        normalized_timer_changes: Vec<TimerChangeV1>,
    ) -> Result<TransitionV1, TraceErrorV1> {
        let activity_state_hash =
            hash_activity_state(&state.head.pack_digest, &apply.next_activity_state)?;
        let authoritative_state_hash = hash_authoritative_state(
            &state.head.pack_digest,
            &proposed.verified_state.core_state_hash,
            &activity_state_hash,
        )?;
        let mut transition = TransitionV1 {
            transition_version: TRANSITION_VERSION.to_owned(),
            codec_id: CANONICAL_CODEC_ID.to_owned(),
            hash_suite: HASH_SUITE_ID.to_owned(),
            room_id: state.head.room_id.clone(),
            room_seq,
            core_schema_version: CORE_SCHEMA_VERSION.to_owned(),
            pack_digest: state.head.pack_digest.clone(),
            previous_transition_or_genesis_hash: state.head.genesis_or_transition_hash.clone(),
            recorded_stimulus: stimulus,
            ordered_domain_events: apply.ordered_domain_events,
            ordered_timer_changes: normalized_timer_changes,
            ordered_attention_signals: apply.ordered_attention_signals,
            resulting_core_state: proposed.verified_state.state,
            resulting_activity_state: apply.next_activity_state,
            resulting_core_state_hash: proposed.verified_state.core_state_hash,
            resulting_activity_state_hash: activity_state_hash,
            resulting_authoritative_state_hash: authoritative_state_hash,
            transition_hash: Blake3DigestV1::hash(&[]),
        };
        transition.transition_hash = transition.calculate_hash()?;
        Ok(transition)
    }
}

/// Opaque verified transition state, including hidden Timer generation history.
/// Only successful Genesis verification, Replay, or prepared installation can
/// construct this value.
#[derive(Clone)]
pub struct RoomTransitionStateV1 {
    head: CompleteHeadV1,
    core_state: VerifiedCoreStateV1,
    activity_state: CanonicalJsonV1,
    timers: TimerBookV1,
    preparer_identity: Arc<()>,
}

impl RoomTransitionStateV1 {
    /// Returns the exact indivisible current Head.
    #[must_use]
    pub fn head(&self) -> &CompleteHeadV1 {
        &self.head
    }

    /// Returns the exact current Core state.
    #[must_use]
    pub fn core_state(&self) -> &CoreRoomStateV1 {
        &self.core_state.state
    }

    /// Returns the exact current opaque Activity state.
    #[must_use]
    pub fn activity_state(&self) -> &CanonicalJsonV1 {
        &self.activity_state
    }

    /// Returns only currently scheduled Timer facts. Hidden generation history
    /// remains Core-owned so a pack cannot author a successor generation.
    #[must_use]
    pub fn scheduled_timers(&self) -> &BTreeMap<TimerId, ScheduledTimerV1> {
        &self.timers.scheduled
    }
}

impl CoreTraceV1 {
    /// Creates and verifies Genesis, including initial Role/cardinality and
    /// Timer normalization. The Role closure is an internal pack-fixture seam,
    /// not a published hypothetical policy trait.
    ///
    /// # Errors
    ///
    /// Returns an error if Genesis, initial Core/Activity state, timers, the
    /// bound Role validator, or canonical hashes fail validation.
    pub fn create<F, R>(
        input: GenesisInputV1,
        validate_roles: F,
        reduce_activity: R,
    ) -> Result<Self, TraceErrorV1>
    where
        F: Fn(&CoreRoomStateV1) -> Result<(), String> + Send + Sync + 'static,
        R: for<'a> Fn(&ActivityReduceInputV1<'a>) -> Result<ActivityDispositionV1, PackFaultV1>
            + Send
            + Sync
            + 'static,
    {
        if input.initial_core_state.room_status != RoomStatusV1::Active {
            return Err(TraceErrorV1::GenesisMustBeActive);
        }
        let core_reducer = CoreReducerV1::new(validate_roles);
        let verified_core = core_reducer.validate_state(input.initial_core_state.clone())?;
        let preparer = RoomTransitionPreparerV1::new(core_reducer, reduce_activity);
        let timers = TimerBookV1::from_genesis(&input.initial_timers, input.created_at.as_str())?;
        let core_state_hash = verified_core.core_state_hash.clone();
        let activity_state_hash =
            hash_activity_state(&input.pack_digest, &input.initial_activity_state)?;
        let authoritative_state_hash =
            hash_authoritative_state(&input.pack_digest, &core_state_hash, &activity_state_hash)?;

        let mut genesis = GenesisV1 {
            genesis_version: GENESIS_VERSION.to_owned(),
            codec_id: CANONICAL_CODEC_ID.to_owned(),
            hash_suite: HASH_SUITE_ID.to_owned(),
            room_id: input.room_id,
            core_schema_version: CORE_SCHEMA_VERSION.to_owned(),
            pack_digest: input.pack_digest,
            configuration: input.configuration,
            room_seed: input.room_seed,
            created_at: input.created_at,
            initial_timers: input.initial_timers,
            initial_core_state: input.initial_core_state,
            initial_activity_state: input.initial_activity_state,
            initial_core_state_hash: core_state_hash,
            initial_activity_state_hash: activity_state_hash,
            initial_authoritative_state_hash: authoritative_state_hash,
            // Replaced immediately by the checked typed hash below.
            genesis_hash: Blake3DigestV1::hash(&[]),
        };
        genesis.genesis_hash = genesis.calculate_hash()?;
        let head = genesis.head();
        Ok(Self {
            core_state: genesis.initial_core_state.clone(),
            activity_state: genesis.initial_activity_state.clone(),
            genesis,
            transitions: Vec::new(),
            head,
            timers,
            administration_results: BTreeMap::new(),
            activity_callback_count: AtomicUsize::new(0),
            administrative_receipt_count: 0,
            preparer,
        })
    }

    /// Convenience conformance operation that prepares and immediately
    /// installs one normalized Stimulus in memory.
    ///
    /// # Errors
    ///
    /// Returns an error if preparation or checked installation fails.
    pub fn advance(
        &mut self,
        stimulus: RecordedStimulusV1,
    ) -> Result<AdvanceDispositionV1, TraceErrorV1> {
        let prepared = self.prepare(stimulus)?;
        self.install_prepared_inner(prepared, true)
    }

    /// Purely evaluates one normalized Stimulus against the exact current
    /// Head. This performs Core reduction, Activity reduction, normalization,
    /// canonical encoding, and hashing without installing state or consuming
    /// a sequence. A runtime may persist the sealed result before calling
    /// [`Self::install_prepared`].
    ///
    /// # Errors
    ///
    /// Returns an error if the Stimulus is invalid, the bound executor faults,
    /// or normalization/canonical hashing fails.
    pub fn prepare(
        &self,
        stimulus: RecordedStimulusV1,
    ) -> Result<PreparedRoomTransitionV1, TraceErrorV1> {
        if let Some(existing) = self.resolve_existing_administration(&stimulus)? {
            return Ok(existing);
        }
        self.preparer.prepare_inner(
            &self.transition_state(),
            stimulus,
            Some(&self.activity_callback_count),
        )
    }

    /// Installs a privately constructed prepared result after the caller's
    /// durable commit point. Installation fails closed if the indivisible
    /// eight-field basis Head has changed since preparation.
    ///
    /// # Errors
    ///
    /// Returns an error for mismatched provenance, Room, basis Head, or
    /// administrative identity/request semantics.
    pub fn install_prepared(
        &mut self,
        prepared: PreparedRoomTransitionV1,
    ) -> Result<AdvanceDispositionV1, TraceErrorV1> {
        self.install_prepared_inner(prepared, true)
    }

    fn install_prepared_inner(
        &mut self,
        prepared: PreparedRoomTransitionV1,
        record_administration_receipt: bool,
    ) -> Result<AdvanceDispositionV1, TraceErrorV1> {
        if !Arc::ptr_eq(&prepared.preparer_identity, &self.preparer.identity) {
            return Err(TraceErrorV1::TransitionPreparerProvenanceMismatch);
        }
        if let Some(resulting_state) = &prepared.resulting_state {
            if !Arc::ptr_eq(&resulting_state.preparer_identity, &self.preparer.identity) {
                return Err(TraceErrorV1::TransitionPreparerProvenanceMismatch);
            }
            if !Arc::ptr_eq(
                &resulting_state.core_state.reducer_identity,
                &self.preparer.core_reducer.identity,
            ) {
                return Err(TraceErrorV1::CoreReducerProvenanceMismatch);
            }
        }
        if prepared.basis_complete_head.room_id != self.head.room_id {
            return Err(TraceErrorV1::PreparedBasisMismatch);
        }
        if let Some((identity, request_hash, basis_head)) = &prepared.administration {
            if basis_head != &prepared.basis_complete_head {
                return Err(TraceErrorV1::InvalidPreparedAdvance);
            }
            if let Some(stored) = self.administration_results.get(identity) {
                if &stored.request_hash != request_hash || &stored.basis_head != basis_head {
                    return Err(TraceErrorV1::IdempotencyConflict);
                }
                return Ok(stored.outcome.to_existing());
            }
        }
        if !prepared.is_new {
            return Err(TraceErrorV1::InvalidPreparedAdvance);
        }
        if self.head != prepared.basis_complete_head {
            return Err(TraceErrorV1::PreparedBasisMismatch);
        }

        if let AdvanceDispositionV1::TransitionAccepted { transition, .. } = &prepared.outcome {
            let resulting_state = prepared
                .resulting_state
                .as_ref()
                .ok_or(TraceErrorV1::InvalidPreparedAdvance)?;
            if resulting_state.head != transition.head()
                || resulting_state.core_state.state != transition.resulting_core_state
                || resulting_state.activity_state != transition.resulting_activity_state
            {
                return Err(TraceErrorV1::InvalidPreparedAdvance);
            }
            self.core_state = resulting_state.core_state.state.clone();
            self.activity_state = resulting_state.activity_state.clone();
            self.head = resulting_state.head.clone();
            self.timers = resulting_state.timers.clone();
            self.transitions.push((**transition).clone());
        } else if prepared.resulting_state.as_ref().map(|state| &state.head)
            != Some(&prepared.basis_complete_head)
        {
            return Err(TraceErrorV1::InvalidPreparedAdvance);
        }

        self.maybe_store_administration(
            prepared.administration,
            &prepared.outcome,
            record_administration_receipt,
        );
        Ok(prepared.outcome)
    }

    fn resolve_existing_administration(
        &self,
        stimulus: &RecordedStimulusV1,
    ) -> Result<Option<PreparedRoomTransitionV1>, TraceErrorV1> {
        let RecordedStimulusV1::CoreProposed(proposal) = stimulus else {
            return Ok(None);
        };
        let identity = &proposal.operation_identity;
        if let Some(stored) = self.administration_results.get(identity) {
            let request_hash = hash_administration_request(&stored.basis_head, proposal)?;
            if request_hash != stored.request_hash {
                return Err(TraceErrorV1::IdempotencyConflict);
            }
            return Ok(Some(PreparedRoomTransitionV1 {
                preparer_identity: Arc::clone(&self.preparer.identity),
                basis_complete_head: stored.basis_head.clone(),
                outcome: stored.outcome.to_existing(),
                resulting_state: None,
                administration: Some((
                    identity.clone(),
                    stored.request_hash.clone(),
                    stored.basis_head.clone(),
                )),
                is_new: false,
            }));
        }
        Ok(None)
    }

    fn transition_state(&self) -> RoomTransitionStateV1 {
        RoomTransitionStateV1 {
            head: self.head.clone(),
            core_state: VerifiedCoreStateV1 {
                state: self.core_state.clone(),
                core_state_hash: self.head.core_state_hash.clone(),
                reducer_identity: Arc::clone(&self.preparer.core_reducer.identity),
            },
            activity_state: self.activity_state.clone(),
            timers: self.timers.clone(),
            preparer_identity: Arc::clone(&self.preparer.identity),
        }
    }

    fn maybe_store_administration(
        &mut self,
        administration: Option<(
            AdministrationOperationIdentityV1,
            Blake3DigestV1,
            CompleteHeadV1,
        )>,
        outcome: &AdvanceDispositionV1,
        enabled: bool,
    ) {
        if let Some((identity, request_hash, basis_head)) = administration {
            let stored_outcome = StoredOutcomeV1::from(outcome);
            self.administration_results.insert(
                identity,
                StoredAdministrationV1 {
                    request_hash,
                    basis_head,
                    outcome: stored_outcome,
                },
            );
            if enabled {
                self.administrative_receipt_count += 1;
            }
        }
    }

    /// Replays exact persisted canonical bytes. There is intentionally no
    /// overload accepting already-deserialized records: the original bytes
    /// must survive parse-and-reencode equality.
    ///
    /// # Errors
    ///
    /// Returns a classified failure with the last verified Head when strict
    /// decoding, lineage, deterministic reduction, or invariants disagree.
    #[allow(clippy::too_many_lines)]
    pub fn replay<F, R>(
        genesis_bytes: &[u8],
        transition_bytes: &[Vec<u8>],
        validate_roles: F,
        reduce_activity: R,
    ) -> Result<ReplayReportV1, ReplayFailureV1>
    where
        F: Fn(&CoreRoomStateV1) -> Result<(), String> + Send + Sync + 'static,
        R: for<'a> Fn(&ActivityReduceInputV1<'a>) -> Result<ActivityDispositionV1, PackFaultV1>
            + Send
            + Sync
            + 'static,
    {
        let genesis = GenesisV1::from_canonical_bytes(genesis_bytes).map_err(|error| {
            ReplayFailureV1::without_head(
                ReplayFailureClassV1::NonCanonicalRecord,
                error.to_string(),
            )
        })?;
        let transition_preparer =
            RoomTransitionPreparerV1::new(CoreReducerV1::new(validate_roles), reduce_activity);
        let mut trace =
            Self::from_verified_genesis(genesis, transition_preparer).map_err(|error| {
                ReplayFailureV1::without_head(classify_genesis_error(&error), error.to_string())
            })?;
        let mut steps = vec![
            ReplayStepV1::genesis(&trace, genesis_bytes).map_err(|error| {
                ReplayFailureV1::without_head(
                    ReplayFailureClassV1::CanonicalEncoding,
                    error.to_string(),
                )
            })?,
        ];
        for bytes in transition_bytes {
            let last_verified_head = trace.head.clone();
            let stored = TransitionV1::from_canonical_bytes(bytes).map_err(|error| {
                ReplayFailureV1::with_head(
                    ReplayFailureClassV1::NonCanonicalRecord,
                    error.to_string(),
                    last_verified_head.clone(),
                )
            })?;
            validate_transition_metadata(&trace.head, &stored).map_err(|(class, detail)| {
                ReplayFailureV1::with_head(class, detail, last_verified_head.clone())
            })?;
            validate_stored_transition_hashes(&stored).map_err(|(class, detail)| {
                ReplayFailureV1::with_head(class, detail, last_verified_head.clone())
            })?;
            trace
                .preparer
                .core_reducer
                .validate_state(stored.resulting_core_state.clone())
                .map_err(|error| {
                    let class = match error {
                        TraceErrorV1::Pack(PackFaultV1::RoleValidatorPanicked) => {
                            ReplayFailureClassV1::ActivityReduction
                        }
                        _ => ReplayFailureClassV1::CoreInvariant,
                    };
                    ReplayFailureV1::with_head(class, error.to_string(), last_verified_head.clone())
                })?;

            let stimulus = stored.recorded_stimulus.clone();
            let prepared_transition = trace.prepare(stimulus).map_err(|error| {
                ReplayFailureV1::with_head(
                    classify_replay_advance_error(&error),
                    error.to_string(),
                    last_verified_head.clone(),
                )
            })?;
            let generated = trace
                .install_prepared_inner(prepared_transition, false)
                .map_err(|error| {
                    ReplayFailureV1::with_head(
                        classify_replay_advance_error(&error),
                        error.to_string(),
                        last_verified_head.clone(),
                    )
                })?;
            let generated_transition = match generated {
                AdvanceDispositionV1::TransitionAccepted { transition, .. } => *transition,
                AdvanceDispositionV1::RejectionRecorded { .. }
                | AdvanceDispositionV1::NoChangeRecorded { .. } => {
                    return Err(ReplayFailureV1::with_head(
                        ReplayFailureClassV1::Stimulus,
                        "stored Transition replayed to a no-Transition disposition".to_owned(),
                        last_verified_head,
                    ));
                }
            };
            compare_transition(&generated_transition, &stored).map_err(|(class, detail)| {
                ReplayFailureV1::with_head(class, detail, last_verified_head.clone())
            })?;
            if generated_transition.canonical_bytes().map_err(|error| {
                ReplayFailureV1::with_head(
                    ReplayFailureClassV1::CanonicalEncoding,
                    error.to_string(),
                    last_verified_head.clone(),
                )
            })? != *bytes
            {
                return Err(ReplayFailureV1::with_head(
                    ReplayFailureClassV1::RecordBytes,
                    "replayed Transition bytes differ".to_owned(),
                    last_verified_head,
                ));
            }
            steps.push(ReplayStepV1::transition(&trace, bytes).map_err(|error| {
                ReplayFailureV1::with_head(
                    ReplayFailureClassV1::CanonicalEncoding,
                    error.to_string(),
                    last_verified_head,
                )
            })?);
        }

        let final_state = trace.transition_state();
        Ok(ReplayReportV1 {
            final_head: trace.head.clone(),
            final_state,
            continuation_preparer: trace.preparer.clone(),
            steps,
            activity_callback_count: trace.activity_callback_count.load(AtomicOrdering::Relaxed),
            external_effect_count: 0,
            receipt_count: 0,
        })
    }

    fn from_verified_genesis(
        genesis: GenesisV1,
        preparer: RoomTransitionPreparerV1,
    ) -> Result<Self, TraceErrorV1> {
        if genesis.genesis_version != GENESIS_VERSION
            || genesis.codec_id != CANONICAL_CODEC_ID
            || genesis.hash_suite != HASH_SUITE_ID
            || genesis.core_schema_version != CORE_SCHEMA_VERSION
        {
            return Err(TraceErrorV1::VersionIdentityMismatch);
        }
        if genesis.initial_core_state.room_status != RoomStatusV1::Active {
            return Err(TraceErrorV1::GenesisMustBeActive);
        }
        let timers =
            TimerBookV1::from_genesis(&genesis.initial_timers, genesis.created_at.as_str())?;
        let core_hash = hash_core_state(&genesis.initial_core_state)?;
        let activity_hash =
            hash_activity_state(&genesis.pack_digest, &genesis.initial_activity_state)?;
        let authoritative_hash =
            hash_authoritative_state(&genesis.pack_digest, &core_hash, &activity_hash)?;
        if genesis.initial_core_state_hash != core_hash
            || genesis.initial_activity_state_hash != activity_hash
            || genesis.initial_authoritative_state_hash != authoritative_hash
        {
            return Err(TraceErrorV1::StateHashMismatch);
        }
        if genesis.genesis_hash != genesis.calculate_hash()? {
            return Err(TraceErrorV1::LineageHashMismatch);
        }
        preparer
            .core_reducer
            .validate_state(genesis.initial_core_state.clone())?;
        let head = genesis.head();
        Ok(Self {
            core_state: genesis.initial_core_state.clone(),
            activity_state: genesis.initial_activity_state.clone(),
            genesis,
            transitions: Vec::new(),
            head,
            timers,
            administration_results: BTreeMap::new(),
            activity_callback_count: AtomicUsize::new(0),
            administrative_receipt_count: 0,
            preparer,
        })
    }

    /// Returns the exact current eight-field Head.
    #[must_use]
    pub fn head(&self) -> &CompleteHeadV1 {
        &self.head
    }

    /// Returns an opaque verified snapshot usable only with this trace's exact
    /// cloned transition preparer.
    #[must_use]
    pub fn verified_transition_state(&self) -> RoomTransitionStateV1 {
        self.transition_state()
    }

    /// Returns the exact Core/Activity preparer bound for this lineage.
    #[must_use]
    pub const fn transition_preparer(&self) -> &RoomTransitionPreparerV1 {
        &self.preparer
    }

    /// Returns the exact current Core state.
    #[must_use]
    pub fn core_state(&self) -> &CoreRoomStateV1 {
        &self.core_state
    }

    /// Returns the exact current opaque Activity state.
    #[must_use]
    pub fn activity_state(&self) -> &CanonicalJsonV1 {
        &self.activity_state
    }

    /// Returns immutable Genesis.
    #[must_use]
    pub fn genesis(&self) -> &GenesisV1 {
        &self.genesis
    }

    /// Returns accepted immutable Transitions only.
    #[must_use]
    pub fn transitions(&self) -> &[TransitionV1] {
        &self.transitions
    }

    /// Returns exact persisted Genesis bytes.
    ///
    /// # Errors
    ///
    /// Returns an error if Genesis cannot be canonically encoded.
    pub fn genesis_bytes(&self) -> Result<Vec<u8>, CanonicalJsonError> {
        self.genesis.canonical_bytes()
    }

    /// Returns exact persisted Transition bytes in Room order.
    ///
    /// # Errors
    ///
    /// Returns an error if any Transition cannot be canonically encoded.
    pub fn transition_bytes(&self) -> Result<Vec<Vec<u8>>, CanonicalJsonError> {
        self.transitions
            .iter()
            .map(TransitionV1::canonical_bytes)
            .collect()
    }

    /// Number of accepted Transitions.
    #[must_use]
    pub fn transition_count(&self) -> usize {
        self.transitions.len()
    }

    /// Number of Activity reduction closure invocations during live tracing.
    #[must_use]
    pub fn activity_callback_count(&self) -> usize {
        self.activity_callback_count.load(AtomicOrdering::Relaxed)
    }

    /// Number of in-memory administrative semantic receipts recorded.
    #[must_use]
    pub const fn administrative_receipt_count(&self) -> usize {
        self.administrative_receipt_count
    }

    /// Core tracing has no effect capability.
    #[must_use]
    pub const fn external_effect_count(&self) -> usize {
        0
    }
}

fn safe_role_validator(
    validator: &Arc<RoleValidatorFn>,
) -> impl Fn(&CoreRoomStateV1) -> Result<(), RoleValidationFailureV1> + '_ {
    move |state| {
        catch_unwind(AssertUnwindSafe(|| validator(state)))
            .map_err(|_| RoleValidationFailureV1::Panicked)?
            .map_err(RoleValidationFailureV1::Rejected)
    }
}

fn map_checked_core_error(error: CheckedCoreErrorV1) -> TraceErrorV1 {
    match error {
        CheckedCoreErrorV1::Validation(error) => error.into(),
        CheckedCoreErrorV1::RoleValidatorPanicked => PackFaultV1::RoleValidatorPanicked.into(),
    }
}

fn clean_rejection_allowed(stimulus: &RecordedStimulusV1, class: CoreProposalClassV1) -> bool {
    matches!(stimulus, RecordedStimulusV1::ParticipantAction(_))
        || matches!(
            (stimulus, class),
            (
                RecordedStimulusV1::CoreProposed(_),
                CoreProposalClassV1::Vetoable
            )
        )
}

#[derive(Clone)]
struct TimerBookV1 {
    scheduled: BTreeMap<TimerId, ScheduledTimerV1>,
    last_generation: BTreeMap<TimerId, TimerGenerationV1>,
}

impl TimerBookV1 {
    fn from_genesis(initial: &[ScheduledTimerV1], created_at: &str) -> Result<Self, TraceErrorV1> {
        if initial
            .windows(2)
            .any(|pair| pair[0].timer_id >= pair[1].timer_id)
        {
            return Err(TraceErrorV1::TimerChangesNotStrictlySorted);
        }
        let generation_one = TimerGenerationV1::new(1)?;
        let mut scheduled = BTreeMap::new();
        let mut last_generation = BTreeMap::new();
        for timer in initial {
            if timer.generation != generation_one
                || compare_timestamp_text(timer.scheduled_for.as_str(), created_at)
                    != Ordering::Greater
            {
                return Err(TraceErrorV1::InvalidTimerNormalization);
            }
            scheduled.insert(timer.timer_id.clone(), timer.clone());
            last_generation.insert(timer.timer_id.clone(), timer.generation);
        }
        Ok(Self {
            scheduled,
            last_generation,
        })
    }

    #[allow(clippy::too_many_lines)]
    fn normalize_requests(
        &self,
        stimulus: &RecordedStimulusV1,
        requests: &[TimerRequestV1],
        resulting_room_status: RoomStatusV1,
    ) -> Result<(Vec<TimerChangeV1>, Self), TraceErrorV1> {
        let mut requests = requests.to_vec();
        requests.sort_by(|left, right| left.timer_id().cmp(right.timer_id()));
        if requests
            .windows(2)
            .any(|pair| pair[0].timer_id() == pair[1].timer_id())
        {
            return Err(TraceErrorV1::TimerChangesNotStrictlySorted);
        }

        if resulting_room_status == RoomStatusV1::Archived {
            if !requests.is_empty() {
                return Err(TraceErrorV1::InvalidTimerNormalization);
            }
            let changes = self.archive_cancellations();
            let mut next = self.clone();
            next.scheduled.clear();
            return Ok((changes, next));
        }

        let mut next = self.clone();
        if let RecordedStimulusV1::TimerFired(fired) = stimulus {
            next.scheduled.remove(&fired.timer_id);
        }

        let mut changes = Vec::with_capacity(requests.len());
        for request in requests {
            match request {
                TimerRequestV1::ScheduleNext {
                    timer_id,
                    due,
                    canonical_payload,
                } => {
                    let generation = next.successor(&timer_id)?;
                    if next.scheduled.contains_key(&timer_id)
                        || compare_timestamp_text(due.as_str(), stimulus.semantic_time())
                            != Ordering::Greater
                    {
                        return Err(TraceErrorV1::InvalidTimerNormalization);
                    }
                    next.last_generation.insert(timer_id.clone(), generation);
                    next.scheduled.insert(
                        timer_id.clone(),
                        ScheduledTimerV1 {
                            timer_id: timer_id.clone(),
                            generation,
                            scheduled_for: due.clone(),
                            canonical_payload: canonical_payload.clone(),
                        },
                    );
                    changes.push(TimerChangeV1::Schedule {
                        timer_id,
                        generation,
                        scheduled_for: due,
                        canonical_payload,
                    });
                }
                TimerRequestV1::CancelCurrent {
                    timer_id,
                    expected_generation,
                } => {
                    if next.scheduled.get(&timer_id).map(|timer| timer.generation)
                        != Some(expected_generation)
                    {
                        return Err(TraceErrorV1::InvalidTimerNormalization);
                    }
                    next.scheduled.remove(&timer_id);
                    changes.push(TimerChangeV1::Cancel {
                        timer_id,
                        generation: expected_generation,
                    });
                }
                TimerRequestV1::RescheduleCurrent {
                    timer_id,
                    expected_generation,
                    new_due,
                    new_canonical_payload,
                } => {
                    if next.scheduled.get(&timer_id).map(|timer| timer.generation)
                        != Some(expected_generation)
                        || compare_timestamp_text(new_due.as_str(), stimulus.semantic_time())
                            != Ordering::Greater
                    {
                        return Err(TraceErrorV1::InvalidTimerNormalization);
                    }
                    let generation = next.successor(&timer_id)?;
                    next.last_generation.insert(timer_id.clone(), generation);
                    next.scheduled.insert(
                        timer_id.clone(),
                        ScheduledTimerV1 {
                            timer_id: timer_id.clone(),
                            generation,
                            scheduled_for: new_due.clone(),
                            canonical_payload: new_canonical_payload.clone(),
                        },
                    );
                    changes.push(TimerChangeV1::Reschedule {
                        timer_id,
                        previous_generation: expected_generation,
                        generation,
                        scheduled_for: new_due,
                        canonical_payload: new_canonical_payload,
                    });
                }
            }
        }
        Ok((changes, next))
    }

    fn archive_cancellations(&self) -> Vec<TimerChangeV1> {
        self.scheduled
            .values()
            .map(|timer| TimerChangeV1::Cancel {
                timer_id: timer.timer_id.clone(),
                generation: timer.generation,
            })
            .collect()
    }

    fn successor(&self, timer_id: &TimerId) -> Result<TimerGenerationV1, TraceErrorV1> {
        match self.last_generation.get(timer_id) {
            Some(generation) => generation.checked_successor().map_err(Into::into),
            None => TimerGenerationV1::new(1).map_err(Into::into),
        }
    }
}

#[derive(Serialize)]
struct AdministrationRequestHashObject<'a> {
    domain: &'static str,
    codec_id: &'static str,
    hash_suite: &'static str,
    room_id: &'a crate::RoomId,
    basis_complete_head: &'a CompleteHeadV1,
    operation_kind: &'static str,
    proposal_kind: crate::CoreProposedKindV1,
    expected_room_seq: RoomSequenceV1,
    reason_code: &'a str,
    canonical_changeset: &'a crate::CoreChangeSetV1,
}

pub(crate) fn hash_administration_request(
    basis_complete_head: &CompleteHeadV1,
    proposal: &CoreProposedV1,
) -> Result<Blake3DigestV1, CanonicalJsonError> {
    let bytes = encode(&AdministrationRequestHashObject {
        domain: "worldstream/core-administration-request/v1",
        codec_id: CANONICAL_CODEC_ID,
        hash_suite: HASH_SUITE_ID,
        room_id: &basis_complete_head.room_id,
        basis_complete_head,
        operation_kind: crate::CORE_OPERATION_KIND,
        proposal_kind: proposal.kind,
        expected_room_seq: proposal.expected_room_seq,
        reason_code: &proposal.reason_code,
        canonical_changeset: &proposal.canonical_changeset,
    })?;
    Ok(Blake3DigestV1::hash(&bytes))
}

#[derive(Serialize)]
struct AdministrationReceiptObject<'a> {
    domain: &'static str,
    basis_complete_head: &'a CompleteHeadV1,
    recorded_stimulus: &'a RecordedStimulusV1,
    disposition: &'a str,
    rejection: Option<&'a ActivityRejectionV1>,
}

fn administration_receipt(
    stimulus: &RecordedStimulusV1,
    head: &CompleteHeadV1,
    disposition: &str,
    rejection: Option<&ActivityRejectionV1>,
) -> Result<Vec<u8>, CanonicalJsonError> {
    encode(&AdministrationReceiptObject {
        domain: "worldstream/administrative-disposition/v1",
        basis_complete_head: head,
        recorded_stimulus: stimulus,
        disposition,
        rejection,
    })
}

struct StoredAdministrationV1 {
    request_hash: Blake3DigestV1,
    basis_head: CompleteHeadV1,
    outcome: StoredOutcomeV1,
}

enum StoredOutcomeV1 {
    Transition(Box<TransitionV1>),
    Rejection(ActivityRejectionV1, Vec<u8>),
    NoChange(Vec<u8>),
}

/// Sealed result of pure checked Core/Activity reduction and canonical
/// hashing at one exact basis Head. Callers can persist its immutable output
/// before consuming it through [`CoreTraceV1::install_prepared`].
pub struct PreparedRoomTransitionV1 {
    preparer_identity: Arc<()>,
    basis_complete_head: CompleteHeadV1,
    outcome: AdvanceDispositionV1,
    resulting_state: Option<RoomTransitionStateV1>,
    administration: Option<(
        AdministrationOperationIdentityV1,
        Blake3DigestV1,
        CompleteHeadV1,
    )>,
    is_new: bool,
}

impl PreparedRoomTransitionV1 {
    /// Returns the indivisible eight-field basis that must still match at
    /// installation/commit time.
    #[must_use]
    pub fn basis_complete_head(&self) -> &CompleteHeadV1 {
        &self.basis_complete_head
    }

    /// Returns the exact Transition or durable no-Transition disposition to
    /// persist. The value cannot be mutated or constructed outside Core.
    #[must_use]
    pub fn disposition(&self) -> &AdvanceDispositionV1 {
        &self.outcome
    }

    /// Returns the administration identity when this is a new Core proposal.
    #[must_use]
    pub fn administration_identity(&self) -> Option<&AdministrationOperationIdentityV1> {
        self.administration.as_ref().map(|values| &values.0)
    }

    /// Returns the canonical caller-semantic administration request hash when
    /// this is a new Core proposal.
    #[must_use]
    pub fn administration_request_hash(&self) -> Option<&Blake3DigestV1> {
        self.administration.as_ref().map(|values| &values.1)
    }

    /// Reports whether this preparation produced a new value to commit rather
    /// than resolving an existing administration result.
    #[must_use]
    pub const fn is_new(&self) -> bool {
        self.is_new
    }

    /// Returns the verified post-disposition state for a newly prepared value.
    /// It equals the basis for Rejection/NoChange and advances for Apply.
    #[must_use]
    pub fn resulting_state(&self) -> Option<&RoomTransitionStateV1> {
        self.resulting_state.as_ref()
    }

    #[cfg(test)]
    pub(crate) fn corrupt_resulting_provenance_for_test(&mut self, core_only: bool) {
        let state = self
            .resulting_state
            .as_mut()
            .unwrap_or_else(|| unreachable!("new fixture preparation has resulting state"));
        if core_only {
            state.core_state.reducer_identity = Arc::new(());
        } else {
            state.preparer_identity = Arc::new(());
        }
    }
}

impl StoredOutcomeV1 {
    fn from(outcome: &AdvanceDispositionV1) -> Self {
        match outcome {
            AdvanceDispositionV1::TransitionAccepted { transition, .. } => {
                Self::Transition(transition.clone())
            }
            AdvanceDispositionV1::RejectionRecorded {
                rejection,
                canonical_receipt_bytes,
                ..
            } => Self::Rejection(rejection.clone(), canonical_receipt_bytes.clone()),
            AdvanceDispositionV1::NoChangeRecorded {
                canonical_receipt_bytes,
                ..
            } => Self::NoChange(canonical_receipt_bytes.clone()),
        }
    }

    fn to_existing(&self) -> AdvanceDispositionV1 {
        match self {
            Self::Transition(transition) => AdvanceDispositionV1::TransitionAccepted {
                existing: true,
                transition: transition.clone(),
            },
            Self::Rejection(rejection, bytes) => AdvanceDispositionV1::RejectionRecorded {
                existing: true,
                rejection: rejection.clone(),
                canonical_receipt_bytes: bytes.clone(),
            },
            Self::NoChange(bytes) => AdvanceDispositionV1::NoChangeRecorded {
                existing: true,
                canonical_receipt_bytes: bytes.clone(),
            },
        }
    }
}

/// Observable result of one live in-memory trace attempt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AdvanceDispositionV1 {
    /// One accepted Transition, either newly traced or idempotently resolved.
    TransitionAccepted {
        /// `true` when the administration identity already resolved.
        existing: bool,
        /// Exact immutable Transition.
        transition: Box<TransitionV1>,
    },
    /// Stable clean veto with no Transition or consumed sequence.
    RejectionRecorded {
        /// `true` when the administration identity already resolved.
        existing: bool,
        /// Stable declared pack result.
        rejection: ActivityRejectionV1,
        /// Operational receipt bytes, excluded from all lineage hashes.
        canonical_receipt_bytes: Vec<u8>,
    },
    /// Desired final state was already current; no callback or Transition.
    NoChangeRecorded {
        /// `true` when the administration identity already resolved.
        existing: bool,
        /// Operational receipt bytes, excluded from all lineage hashes.
        canonical_receipt_bytes: Vec<u8>,
    },
}

/// Pack contract failure, distinct from a clean declared Reject.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum PackFaultV1 {
    /// Caller-supplied pure Activity reducer failed.
    #[error("Activity reducer fault: {0}")]
    Callback(String),
    /// The pure callback panicked; no state was installed.
    #[error("Activity reducer panicked")]
    CallbackPanicked,
    /// The pinned Role/cardinality validator panicked.
    #[error("Role validator panicked")]
    RoleValidatorPanicked,
    /// A mandatory Core/timer/external stimulus was vetoed.
    #[error("mandatory Stimulus cannot be cleanly rejected")]
    MandatoryStimulusRejected,
    /// Archive Timer cancellation is host-owned.
    #[error("Activity reducer emitted Timer changes for Archive")]
    ArchiveTimerMutation,
    /// Timer output violated sorting, generation, or semantic-time rules.
    #[error("invalid Activity Timer output: {0}")]
    InvalidTimerOutput(String),
}

/// Live tracing failure. No partial state or Transition is installed.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum TraceErrorV1 {
    /// Canonical codec failure.
    #[error(transparent)]
    Canonical(#[from] CanonicalJsonError),
    /// Core invariant failure.
    #[error(transparent)]
    Core(#[from] CoreValidationErrorV1),
    /// Safe counter exhausted or invalid.
    #[error("safe canonical counter exhausted")]
    SafeCounter,
    /// Exact expected Room sequence differs.
    #[error("expected Room sequence differs from current Head")]
    ExpectedSequenceMismatch,
    /// Participant Action basis is not the exact current eight-field Head.
    #[error("complete basis Head mismatch")]
    CompleteHeadMismatch,
    /// Participant Membership is absent, suspended, departed, or nonparticipant.
    #[error("participant Membership is not eligible")]
    ParticipantNotEligible,
    /// Exact Timer generation/time/payload witness differs.
    #[error("Timer firing witness mismatch")]
    TimerWitnessMismatch,
    /// Archived Room forbids this stimulus class.
    #[error("stimulus is forbidden after archive")]
    ArchivedStimulusForbidden,
    /// Timer changes must be canonical by Timer ID.
    #[error("Timer changes are not strictly sorted by Timer ID")]
    TimerChangesNotStrictlySorted,
    /// Timer mutation did not match generation/current/time rules.
    #[error("invalid normalized Timer mutation")]
    InvalidTimerNormalization,
    /// Archive output left a scheduled Timer.
    #[error("archive must cancel every scheduled Timer")]
    ArchiveDidNotCancelAllTimers,
    /// Same administration identity carried different semantic bytes.
    #[error("administration idempotency conflict")]
    IdempotencyConflict,
    /// A prepared value was evaluated at a different indivisible Head.
    #[error("prepared Core reduction basis no longer matches current Head")]
    PreparedBasisMismatch,
    /// A privately sealed preparation was internally inconsistent.
    #[error("invalid sealed Core preparation")]
    InvalidPreparedAdvance,
    /// A verified Core state came from a different bound Role validator.
    #[error("verified Core state belongs to a different Core reducer")]
    CoreReducerProvenanceMismatch,
    /// A verified transition state came from a different Activity executor.
    #[error("verified transition state belongs to a different transition preparer")]
    TransitionPreparerProvenanceMismatch,
    /// Pack contract failure.
    #[error(transparent)]
    Pack(#[from] PackFaultV1),
    /// Stored record identity/version differs.
    #[error("record version identity mismatch")]
    VersionIdentityMismatch,
    /// A stored state hash differs from recomputation.
    #[error("stored state hash mismatch")]
    StateHashMismatch,
    /// Genesis/Transition lineage hash differs from recomputation.
    #[error("stored lineage hash mismatch")]
    LineageHashMismatch,
    /// Genesis always begins with active Core status.
    #[error("Genesis Core status must be active")]
    GenesisMustBeActive,
}

impl From<crate::SafeCounterError> for TraceErrorV1 {
    fn from(_error: crate::SafeCounterError) -> Self {
        Self::SafeCounter
    }
}

/// One verified replay prefix, including sequence zero.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplayStepV1 {
    /// Exact prefix Head.
    pub head: CompleteHeadV1,
    /// Reproduced canonical Core bytes.
    pub canonical_core_bytes: Vec<u8>,
    /// Reproduced canonical Activity bytes.
    pub canonical_activity_bytes: Vec<u8>,
    /// Original byte-equal Genesis or Transition record.
    pub canonical_lineage_record_bytes: Vec<u8>,
}

impl ReplayStepV1 {
    fn genesis(trace: &CoreTraceV1, bytes: &[u8]) -> Result<Self, CanonicalJsonError> {
        Ok(Self {
            head: trace.head.clone(),
            canonical_core_bytes: encode(&trace.core_state)?,
            canonical_activity_bytes: trace.activity_state.to_bytes()?,
            canonical_lineage_record_bytes: bytes.to_vec(),
        })
    }

    fn transition(trace: &CoreTraceV1, bytes: &[u8]) -> Result<Self, CanonicalJsonError> {
        Self::genesis(trace, bytes)
    }
}

/// Successful no-effect replay report with every prefix Head.
#[derive(Clone)]
pub struct ReplayReportV1 {
    /// Final exact Head.
    pub final_head: CompleteHeadV1,
    final_state: RoomTransitionStateV1,
    continuation_preparer: RoomTransitionPreparerV1,
    /// Sequence-zero plus every accepted Transition prefix.
    pub steps: Vec<ReplayStepV1>,
    /// Activity callback count; exactly one per replayed Transition.
    pub activity_callback_count: usize,
    /// Always zero: replay owns no effect capability.
    pub external_effect_count: usize,
    /// Always zero: replay creates no receipt.
    pub receipt_count: usize,
}

impl ReplayReportV1 {
    /// Returns the opaque continuable state reconstructed by verified Replay.
    #[must_use]
    pub fn final_state(&self) -> &RoomTransitionStateV1 {
        &self.final_state
    }

    /// Returns the exact bound preparer that can continue from
    /// [`Self::final_state`].
    #[must_use]
    pub const fn continuation_preparer(&self) -> &RoomTransitionPreparerV1 {
        &self.continuation_preparer
    }
}

/// Stable high-level replay failure class.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReplayFailureClassV1 {
    /// Stored bytes were invalid or not byte-equal canonical JSON.
    NonCanonicalRecord,
    /// Canonical encoding failed for a reconstructed typed value.
    CanonicalEncoding,
    /// Codec/hash/schema/pack/record version identity changed.
    VersionOrDigest,
    /// Room sequence changed or skipped.
    Sequence,
    /// Previous Genesis/Transition hash changed.
    PriorHash,
    /// Recorded Stimulus was invalid at the verified prefix.
    Stimulus,
    /// Core reducer/final-state invariant failed.
    CoreInvariant,
    /// Resulting Core value differs.
    CoreState,
    /// Resulting Activity value differs.
    ActivityState,
    /// Domain Event content or order differs.
    DomainEvents,
    /// Timer changes differ.
    TimerChanges,
    /// Attention content or order differs.
    AttentionSignals,
    /// Component or aggregate state hashes differ.
    StateHashes,
    /// Genesis or Transition hash differs.
    LineageHash,
    /// Full reconstructed canonical record bytes differ.
    RecordBytes,
    /// Pure Activity callback faulted or rejected a stored Transition.
    ActivityReduction,
}

/// Replay failure paired with the last fully verified Head, if Genesis passed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplayFailureV1 {
    /// Stable classification.
    pub class: ReplayFailureClassV1,
    /// Bounded diagnostic suitable for tests/operators, not protocol branching.
    pub detail: String,
    /// Head immediately before the corrupt/unreplayable record.
    pub last_verified_head: Option<Box<CompleteHeadV1>>,
}

impl ReplayFailureV1 {
    fn without_head(class: ReplayFailureClassV1, detail: String) -> Self {
        Self {
            class,
            detail,
            last_verified_head: None,
        }
    }

    fn with_head(
        class: ReplayFailureClassV1,
        detail: String,
        last_verified_head: CompleteHeadV1,
    ) -> Self {
        Self {
            class,
            detail,
            last_verified_head: Some(Box::new(last_verified_head)),
        }
    }
}

fn validate_transition_metadata(
    head: &CompleteHeadV1,
    transition: &TransitionV1,
) -> Result<(), (ReplayFailureClassV1, String)> {
    let expected_seq = head
        .room_seq
        .checked_successor()
        .map_err(|error| (ReplayFailureClassV1::Sequence, error.to_string()))?;
    if transition.room_seq != expected_seq {
        return Err((
            ReplayFailureClassV1::Sequence,
            "Transition sequence is not the exact successor".to_owned(),
        ));
    }
    if transition.previous_transition_or_genesis_hash != head.genesis_or_transition_hash {
        return Err((
            ReplayFailureClassV1::PriorHash,
            "previous lineage hash differs".to_owned(),
        ));
    }
    if transition.transition_version != TRANSITION_VERSION
        || transition.codec_id != CANONICAL_CODEC_ID
        || transition.hash_suite != HASH_SUITE_ID
        || transition.core_schema_version != CORE_SCHEMA_VERSION
        || transition.room_id != head.room_id
        || transition.pack_digest != head.pack_digest
    {
        return Err((
            ReplayFailureClassV1::VersionOrDigest,
            "Transition Room/version/digest identity differs".to_owned(),
        ));
    }
    Ok(())
}

fn validate_stored_transition_hashes(
    transition: &TransitionV1,
) -> Result<(), (ReplayFailureClassV1, String)> {
    let core_hash = hash_core_state(&transition.resulting_core_state)
        .map_err(|error| (ReplayFailureClassV1::CanonicalEncoding, error.to_string()))?;
    let activity_hash = hash_activity_state(
        &transition.pack_digest,
        &transition.resulting_activity_state,
    )
    .map_err(|error| (ReplayFailureClassV1::CanonicalEncoding, error.to_string()))?;
    let authoritative_hash =
        hash_authoritative_state(&transition.pack_digest, &core_hash, &activity_hash)
            .map_err(|error| (ReplayFailureClassV1::CanonicalEncoding, error.to_string()))?;
    if transition.resulting_core_state_hash != core_hash
        || transition.resulting_activity_state_hash != activity_hash
        || transition.resulting_authoritative_state_hash != authoritative_hash
    {
        return Err((
            ReplayFailureClassV1::StateHashes,
            "stored Transition state hashes differ from canonical state".to_owned(),
        ));
    }
    let transition_hash = transition
        .calculate_hash()
        .map_err(|error| (ReplayFailureClassV1::CanonicalEncoding, error.to_string()))?;
    if transition.transition_hash != transition_hash {
        return Err((
            ReplayFailureClassV1::LineageHash,
            "stored Transition lineage hash differs".to_owned(),
        ));
    }
    Ok(())
}

fn compare_transition(
    generated: &TransitionV1,
    stored: &TransitionV1,
) -> Result<(), (ReplayFailureClassV1, String)> {
    if generated.resulting_core_state != stored.resulting_core_state {
        return Err((
            ReplayFailureClassV1::CoreState,
            "resulting Core differs".to_owned(),
        ));
    }
    if generated.resulting_activity_state != stored.resulting_activity_state {
        return Err((
            ReplayFailureClassV1::ActivityState,
            "resulting Activity differs".to_owned(),
        ));
    }
    if generated.ordered_domain_events != stored.ordered_domain_events {
        return Err((
            ReplayFailureClassV1::DomainEvents,
            "ordered Domain Events differ".to_owned(),
        ));
    }
    if generated.ordered_timer_changes != stored.ordered_timer_changes {
        return Err((
            ReplayFailureClassV1::TimerChanges,
            "ordered Timer changes differ".to_owned(),
        ));
    }
    if generated.ordered_attention_signals != stored.ordered_attention_signals {
        return Err((
            ReplayFailureClassV1::AttentionSignals,
            "ordered Attention Signals differ".to_owned(),
        ));
    }
    if generated.resulting_core_state_hash != stored.resulting_core_state_hash
        || generated.resulting_activity_state_hash != stored.resulting_activity_state_hash
        || generated.resulting_authoritative_state_hash != stored.resulting_authoritative_state_hash
    {
        return Err((
            ReplayFailureClassV1::StateHashes,
            "resulting state hashes differ".to_owned(),
        ));
    }
    if generated.transition_hash != stored.transition_hash {
        return Err((
            ReplayFailureClassV1::LineageHash,
            "Transition hash differs".to_owned(),
        ));
    }
    Ok(())
}

fn classify_genesis_error(error: &TraceErrorV1) -> ReplayFailureClassV1 {
    match error {
        TraceErrorV1::VersionIdentityMismatch => ReplayFailureClassV1::VersionOrDigest,
        TraceErrorV1::Core(_) => ReplayFailureClassV1::CoreInvariant,
        TraceErrorV1::StateHashMismatch => ReplayFailureClassV1::StateHashes,
        TraceErrorV1::LineageHashMismatch => ReplayFailureClassV1::LineageHash,
        TraceErrorV1::Pack(_) => ReplayFailureClassV1::ActivityReduction,
        TraceErrorV1::Canonical(_) => ReplayFailureClassV1::CanonicalEncoding,
        _ => ReplayFailureClassV1::Stimulus,
    }
}

fn classify_replay_advance_error(error: &TraceErrorV1) -> ReplayFailureClassV1 {
    match error {
        TraceErrorV1::Core(_) => ReplayFailureClassV1::CoreInvariant,
        TraceErrorV1::Pack(_) => ReplayFailureClassV1::ActivityReduction,
        TraceErrorV1::Canonical(_) => ReplayFailureClassV1::CanonicalEncoding,
        TraceErrorV1::StateHashMismatch => ReplayFailureClassV1::StateHashes,
        TraceErrorV1::LineageHashMismatch => ReplayFailureClassV1::LineageHash,
        TraceErrorV1::VersionIdentityMismatch => ReplayFailureClassV1::VersionOrDigest,
        _ => ReplayFailureClassV1::Stimulus,
    }
}
