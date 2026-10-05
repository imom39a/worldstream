use std::{
    cmp::Ordering,
    collections::BTreeMap,
    fmt,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering as AtomicOrdering},
    },
};

use serde::Serialize;
use thiserror::Error;

use crate::{
    AccessModeV1, ActionAdmissionErrorV1, ActivityApplyV1, ActivityDispositionV1,
    ActivityObservationOutcomeV1, ActivityPackOperationV1, ActivityPackReduceErrorV1,
    ActivityReduceInputV1, ActivityRejectionV1, AdministrationOperationIdentityV1, Blake3DigestV1,
    CANONICAL_CODEC_ID, CORE_SCHEMA_VERSION, CanonicalJsonError, CanonicalJsonV1, CompleteHeadV1,
    CoreProposedV1, CoreRoomStateV1, CoreValidationErrorV1, GENESIS_VERSION, GenesisInputV1,
    GenesisV1, HASH_SUITE_ID, MemberId, MembershipStandingV1, PackGenesisErrorV1,
    PackGenesisRequestV1, PackRegistryErrorV1, PackRegistryV1, PackViewerV1,
    PreparedNewRoomGenesisV1, RecordedStimulusV1, ReplayProjectionKindV1, RetainedActivityPackV1,
    RoomIntegrityStateV1, RoomIntegrityStatusV1, RoomSeedV1, RoomSequenceV1, RoomStatusV1,
    ScheduledTimerV1, TRANSITION_VERSION, TimerChangeV1, TimerGenerationV1, TimerId,
    TimerRequestV1, TransitionV1, ViewInputV1,
    activity_pack::{ObserveTransitionInputV1, VerifiedRetainedGenesisV1},
    canonical::encode,
    lineage::{hash_activity_state, hash_authoritative_state, hash_core_state},
    model::CoreProposalClassV1,
    primitives::compare_timestamp_text,
    reducer::{CheckedCoreErrorV1, RoleValidationFailureV1, propose_core, validate_core_state},
};

#[path = "canonical_trace.rs"]
mod canonical_trace;
pub use canonical_trace::{
    CanonicalAdvanceDisposition, CanonicalRoomTrace, PreparedCanonicalRoomTransition,
};
#[cfg(feature = "conformance-tracer")]
pub use canonical_trace::{
    reset_retained_pack_reduce_invocations_for_conformance,
    retained_pack_reduce_invocation_count_for_conformance,
};
#[path = "canonical_history.rs"]
mod canonical_history;
pub(crate) use canonical_history::advance_membership_generations;
pub use canonical_history::{
    CanonicalHistoricalReplayAccumulator, CanonicalReplayReport, CanonicalStorageExecutableReplay,
    CanonicalStorageHistoryPreflight, CanonicalStructuralHistory,
};

/// Unique in-memory current-Room executor. Production instances own the exact
/// retained pack, immutable seed, and current reduction basis, and the type is
/// deliberately not cloneable so stale snapshots cannot become independent
/// execution lanes. It performs no external effect.
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
    room_seed: RoomSeedV1,
    retained_pack: Option<RetainedActivityPackV1>,
    preparer: RoomTransitionPreparerV1,
}

/// Crate-private pre-reducer classification used to seal a stable Action
/// disposition without exposing pack host internals to storage.
pub(crate) struct StableActionAdmissionV1 {
    pub(crate) code: &'static str,
    pub(crate) normalized_action: Option<crate::ParticipantActionV1>,
    pub(crate) current_action_offers: Option<Vec<u8>>,
    pub(crate) unavailable_reason: Option<&'static str>,
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
    pub(crate) fn new<F>(validate_roles: F) -> Self
    where
        F: Fn(&CoreRoomStateV1) -> Result<(), String> + Send + Sync + 'static,
    {
        Self {
            validate_roles: Arc::new(validate_roles),
            identity: Arc::new(()),
        }
    }

    fn from_retained_pack(retained_pack: &RetainedActivityPackV1) -> Self {
        let host = retained_pack.host();
        Self::new(move |state| {
            host.validate_runtime_roles(state)
                .map_err(|error| error.to_string())
        })
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
    activity_reducer: BoundActivityReducerV1,
    identity: Arc<()>,
}

#[derive(Clone)]
enum BoundActivityReducerV1 {
    Retained {
        retained_pack: RetainedActivityPackV1,
        room_seed: RoomSeedV1,
    },
    Conformance(Arc<ActivityReducerFn>),
}

impl RoomTransitionPreparerV1 {
    /// Binds one raw reducer only for crate-owned conformance fixtures. No
    /// production caller can construct or invoke this seam.
    pub(crate) fn new<R>(core_reducer: CoreReducerV1, reduce_activity: R) -> Self
    where
        R: for<'a> Fn(&ActivityReduceInputV1<'a>) -> Result<ActivityDispositionV1, PackFaultV1>
            + Send
            + Sync
            + 'static,
    {
        Self {
            core_reducer,
            activity_reducer: BoundActivityReducerV1::Conformance(Arc::new(reduce_activity)),
            identity: Arc::new(()),
        }
    }

    fn from_retained_pack(retained_pack: RetainedActivityPackV1, room_seed: RoomSeedV1) -> Self {
        Self {
            core_reducer: CoreReducerV1::from_retained_pack(&retained_pack),
            activity_reducer: BoundActivityReducerV1::Retained {
                retained_pack,
                room_seed,
            },
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
    #[cfg(test)]
    pub(crate) fn prepare(
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

    /// Returns the exact retained revision for a production preparer. `None`
    /// is reserved for crate-private conformance fixtures.
    #[must_use]
    pub fn retained_pack(&self) -> Option<&RetainedActivityPackV1> {
        match &self.activity_reducer {
            BoundActivityReducerV1::Retained { retained_pack, .. } => Some(retained_pack),
            BoundActivityReducerV1::Conformance(_) => None,
        }
    }

    /// Returns the immutable Room seed bound to production reduction. `None`
    /// is reserved for crate-private conformance fixtures.
    #[must_use]
    pub fn room_seed(&self) -> Option<&RoomSeedV1> {
        match &self.activity_reducer {
            BoundActivityReducerV1::Retained { room_seed, .. } => Some(room_seed),
            BoundActivityReducerV1::Conformance(_) => None,
        }
    }

    fn prepare_inner(
        &self,
        state: &RoomTransitionStateV1,
        stimulus: RecordedStimulusV1,
        callback_counter: Option<&AtomicUsize>,
    ) -> Result<PreparedRoomTransitionV1, TraceErrorV1> {
        self.prepare_canonical_inner(
            state,
            crate::CanonicalHistoryFormat::V1,
            stimulus,
            callback_counter,
        )?
        .into_legacy()
    }

    #[allow(clippy::too_many_lines)]
    fn prepare_canonical_inner(
        &self,
        state: &RoomTransitionStateV1,
        format: crate::CanonicalHistoryFormat,
        stimulus: RecordedStimulusV1,
        callback_counter: Option<&AtomicUsize>,
    ) -> Result<PreparedCanonicalRoomTransition, TraceErrorV1> {
        if !Arc::ptr_eq(&self.identity, &state.preparer_identity) {
            return Err(TraceErrorV1::TransitionPreparerProvenanceMismatch);
        }
        let payload_budget =
            (format == crate::CanonicalHistoryFormat::V2).then_some(crate::PAYLOAD_BUDGET_V1);
        if let Some(budget) = payload_budget {
            budget.check_stimulus(&stimulus)?;
        }
        let recorded_stimulus = stimulus.clone();
        let proposed = self.validate_stimulus(state, &stimulus)?;
        if let Some(budget) = payload_budget {
            budget.check_value(
                crate::PayloadKindV1::CoreState,
                &proposed.verified_state.state,
            )?;
        }
        let administration = match &stimulus {
            RecordedStimulusV1::CoreProposed(proposal) => Some((
                proposal.operation_identity.clone(),
                hash_administration_request(&state.head, proposal)?,
                state.head.clone(),
            )),
            _ => None,
        };
        if proposed.no_change {
            let receipt = administration_receipt(&stimulus, &state.head, "no_change", None)?;
            return Ok(PreparedCanonicalRoomTransition {
                preparer_identity: Arc::clone(&self.identity),
                basis_complete_head: state.head.clone(),
                recorded_stimulus,
                action_offer_witness: None,
                outcome: CanonicalAdvanceDisposition::NoChangeRecorded {
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
        let (disposition, action_offer_witness) =
            self.reduce_activity(state, &input, callback_counter, payload_budget)?;
        let apply = match disposition {
            ActivityDispositionV1::Apply(apply) => apply,
            ActivityDispositionV1::Reject(rejection) => {
                if !clean_rejection_allowed(&stimulus, proposed.class) {
                    return Err(self
                        .retained_reduce_output_fault(PackFaultV1::MandatoryStimulusRejected)
                        .into());
                }
                let receipt =
                    administration_receipt(&stimulus, &state.head, "rejected", Some(&rejection))?;
                return Ok(PreparedCanonicalRoomTransition {
                    preparer_identity: Arc::clone(&self.identity),
                    basis_complete_head: state.head.clone(),
                    recorded_stimulus,
                    action_offer_witness,
                    outcome: CanonicalAdvanceDisposition::RejectionRecorded {
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
            return Err(self
                .retained_reduce_output_fault(PackFaultV1::ArchiveTimerMutation)
                .into());
        }
        let (normalized_timer_changes, next_timers) = state
            .timers
            .normalize_requests(
                &stimulus,
                &apply.timer_requests,
                proposed.verified_state.state.room_status,
            )
            .map_err(|error| {
                self.retained_reduce_output_fault(PackFaultV1::InvalidTimerOutput(
                    error.to_string(),
                ))
            })?;
        let core_after = proposed.verified_state.clone();
        let activity_after = apply.next_activity_state.clone();
        if let Some(budget) = payload_budget {
            budget
                .check_authoritative_state(&core_after.state, &activity_after)
                .and_then(|()| {
                    budget.check_effects(
                        &apply.ordered_domain_events,
                        &normalized_timer_changes,
                        &apply.ordered_attention_signals,
                    )
                })
                .map_err(|error| {
                    self.retained_reduce_output_fault(PackFaultV1::OutputBoundExceeded(
                        error.to_string(),
                    ))
                })?;
        }
        let transition = match format {
            crate::CanonicalHistoryFormat::V1 => {
                crate::TransitionRecord::V1(Self::build_transition(
                    state,
                    next_room_seq,
                    stimulus,
                    proposed,
                    apply,
                    normalized_timer_changes,
                )?)
            }
            crate::CanonicalHistoryFormat::V2 => crate::TransitionRecord::V2(
                crate::TransitionV2::new(crate::TransitionV2Input {
                    room_id: state.head.room_id().clone(),
                    room_seq: next_room_seq,
                    pack_digest: state.head.pack_digest().clone(),
                    previous_lineage_hash: state.head.genesis_or_transition_hash().clone(),
                    recorded_stimulus: stimulus,
                    ordered_domain_events: apply.ordered_domain_events,
                    ordered_timer_changes: normalized_timer_changes,
                    ordered_attention_signals: apply.ordered_attention_signals,
                    resulting_core_state: &core_after.state,
                    resulting_activity_state: &activity_after,
                })
                .map_err(canonical_trace::map_codec_error)?,
            ),
        };
        if let Some(budget) = payload_budget {
            budget
                .check_bytes(
                    crate::PayloadKindV1::Transition,
                    &transition.canonical_bytes()?,
                )
                .map_err(|error| {
                    self.retained_reduce_output_fault(PackFaultV1::OutputBoundExceeded(
                        error.to_string(),
                    ))
                })?;
        }
        let resulting_state = RoomTransitionStateV1 {
            head: transition.complete_head(),
            core_state: core_after,
            activity_state: activity_after,
            timers: next_timers,
            preparer_identity: Arc::clone(&self.identity),
        };
        Ok(PreparedCanonicalRoomTransition {
            preparer_identity: Arc::clone(&self.identity),
            basis_complete_head: state.head.clone(),
            recorded_stimulus,
            action_offer_witness,
            outcome: CanonicalAdvanceDisposition::TransitionAccepted {
                existing: false,
                transition: Box::new(transition),
            },
            resulting_state: Some(resulting_state),
            administration,
            is_new: true,
        })
    }

    fn reduce_activity(
        &self,
        state: &RoomTransitionStateV1,
        input: &ActivityReduceInputV1<'_>,
        callback_counter: Option<&AtomicUsize>,
        payload_budget: Option<crate::PayloadBudgetV1>,
    ) -> Result<(ActivityDispositionV1, Option<Vec<u8>>), TraceErrorV1> {
        match &self.activity_reducer {
            BoundActivityReducerV1::Retained {
                retained_pack,
                room_seed,
            } => {
                let host = retained_pack.host();
                let participant_view = if let RecordedStimulusV1::ParticipantAction(action) =
                    input.recorded_stimulus
                {
                    let viewer = PackViewerV1::Participant(action.member_id.clone());
                    Some(host.view_with_budget(
                        &ViewInputV1 {
                            core: &state.core_state.state,
                            activity_state: &state.activity_state,
                            complete_head: &state.head,
                            viewer: &viewer,
                        },
                        payload_budget,
                    )?)
                } else {
                    None
                };
                let disposition = host
                    .reduce_with_invocation_hook(
                        input,
                        &state.head,
                        room_seed,
                        participant_view.as_ref(),
                        || {
                            #[cfg(feature = "conformance-tracer")]
                            canonical_trace::record_retained_pack_reduce_invocation_for_conformance(
                            );
                            if let Some(counter) = callback_counter {
                                counter.fetch_add(1, AtomicOrdering::Relaxed);
                            }
                        },
                    )
                    .map_err(map_activity_pack_reduce_error)?;
                let action_offer_witness = participant_view
                    .as_ref()
                    .map(|view| view.action_offers().canonical_bytes().to_vec());
                Ok((disposition, action_offer_witness))
            }
            BoundActivityReducerV1::Conformance(reducer) => {
                if let Some(counter) = callback_counter {
                    counter.fetch_add(1, AtomicOrdering::Relaxed);
                }
                let disposition = catch_unwind(AssertUnwindSafe(|| reducer(input)))
                    .map_err(|_| PackFaultV1::CallbackPanicked)?
                    .map_err(TraceErrorV1::from)?;
                Ok((disposition, None))
            }
        }
    }

    fn retained_reduce_output_fault(&self, fault: PackFaultV1) -> PackFaultV1 {
        match self.activity_reducer {
            BoundActivityReducerV1::Retained { .. } => PackFaultV1::OperationFault {
                operation: ActivityPackOperationV1::Reduce,
                fault: Box::new(fault),
            },
            BoundActivityReducerV1::Conformance(_) => fault,
        }
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
                let prepared = self.core_reducer.reduce(&state.core_state, proposal)?;
                let proposed_memberships = prepared.verified_state.state.memberships();
                let introduces_member = proposed_memberships
                    .keys()
                    .any(|member_id| !state.core_state.state.memberships().contains_key(member_id));
                if introduces_member && proposed_memberships.len() > MAX_ROOM_MEMBERSHIPS_V1 {
                    return Err(TraceErrorV1::RoomMembershipLimitExceeded);
                }
                return Ok(prepared);
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
}

fn record_replay_observation_consequences(
    trace: &CoreTraceV1,
    prepared: &PreparedRoomTransitionV1,
    frame_heads: &mut BTreeMap<crate::MemberId, u64>,
    consequences: &mut Vec<ReplayObservationConsequenceV1>,
) -> Result<(), TraceErrorV1> {
    if !prepared.is_new()
        || prepared.basis_complete_head() != trace.head()
        || !matches!(
            prepared.disposition(),
            AdvanceDispositionV1::TransitionAccepted {
                existing: false,
                ..
            }
        )
    {
        return Err(TraceErrorV1::InvalidPreparedAdvance);
    }
    let resulting_state = prepared
        .resulting_state
        .as_ref()
        .ok_or(TraceErrorV1::InvalidPreparedAdvance)?;
    let classified = crate::room_commit::prepare_transition_consequences(
        trace,
        prepared,
        resulting_state.core_state(),
        resulting_state.head.room_seq(),
        frame_heads,
    )
    .map_err(|error| match error {
        crate::PrepareRoomWriteErrorV1::Canonical(error) => TraceErrorV1::Canonical(error),
        crate::PrepareRoomWriteErrorV1::Trace(error) => error,
        _ => TraceErrorV1::InvalidPreparedAdvance,
    })?;
    fold_replay_observation_consequences(classified, resulting_state, frame_heads, consequences)
}

fn fold_replay_observation_consequences(
    classified: Vec<crate::PreparedObservationConsequenceV1>,
    resulting_state: &RoomTransitionStateV1,
    frame_heads: &mut BTreeMap<MemberId, u64>,
    consequences: &mut Vec<ReplayObservationConsequenceV1>,
) -> Result<(), TraceErrorV1> {
    for consequence in classified {
        match consequence {
            crate::PreparedObservationConsequenceV1::ObservationFrame(frame) => {
                let frame_seq = frame.frame_seq();
                frame_heads.insert(frame.member_id().clone(), frame_seq);
                consequences.push(ReplayObservationConsequenceV1::ObservationFrame(
                    ReplayObservationFrameV1 {
                        member_id: frame.member_id().clone(),
                        frame_seq,
                        cause_room_seq: frame.cause_room_seq(),
                        payload_hash: frame.payload_hash().clone(),
                    },
                ));
            }
            crate::PreparedObservationConsequenceV1::ResetRequired(view) => {
                consequences.push(ReplayObservationConsequenceV1::ResetRequired {
                    member_id: view.viewer().member_id().clone(),
                    cause_room_seq: resulting_state.head().room_seq(),
                    projection_hash: view.projection_hash().map_err(TraceErrorV1::Canonical)?,
                });
            }
            crate::PreparedObservationConsequenceV1::VisibilityLost(member_id) => {
                consequences.push(ReplayObservationConsequenceV1::VisibilityLost {
                    member_id,
                    cause_room_seq: resulting_state.head().room_seq(),
                });
            }
        }
    }
    frame_heads.retain(|member_id, _| {
        resulting_state
            .core_state()
            .memberships()
            .contains_key(member_id)
    });
    for member_id in resulting_state.core_state().memberships().keys() {
        frame_heads.entry(member_id.clone()).or_insert(0);
    }
    Ok(())
}

impl CoreTraceV1 {
    /// Creates a Room only from the opaque registry-selected and checked
    /// Genesis token. The exact retained executor and immutable Room seed are
    /// captured for every subsequent preparation.
    ///
    /// # Errors
    ///
    /// Returns an error if Genesis, initial Core/Activity state, timers, the
    /// retained revision identity, or canonical hashes fail validation.
    #[allow(clippy::needless_pass_by_value)]
    pub(crate) fn create_uncommitted(
        prepared_genesis: PreparedNewRoomGenesisV1,
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
        Self::create_with_preparer(input, preparer, Some(retained_pack))
    }

    #[cfg(test)]
    pub(crate) fn create_uncommitted_retained_for_test(
        prepared_genesis: &VerifiedRetainedGenesisV1,
    ) -> Result<Self, TraceErrorV1> {
        let input = prepared_genesis.genesis_input().clone();
        let retained_pack = prepared_genesis.retained_pack().clone();
        let preparer = RoomTransitionPreparerV1::from_retained_pack(
            retained_pack.clone(),
            input.room_seed.clone(),
        );
        Self::create_with_preparer(input, preparer, Some(retained_pack))
    }

    /// Explicit in-memory retained-pack constructor for conformance only.
    #[cfg(feature = "conformance-tracer")]
    #[doc(hidden)]
    pub fn create_from_retained_for_conformance(
        prepared_genesis: PreparedNewRoomGenesisV1,
    ) -> Result<Self, TraceErrorV1> {
        Self::create_uncommitted(prepared_genesis)
    }

    /// Explicit opt-in raw closure seam for generating frozen conformance
    /// artifacts. Ordinary production builds keep this method crate-private;
    /// Room creation must use [`Self::create`].
    #[cfg(feature = "conformance-tracer")]
    #[doc(hidden)]
    pub fn create_for_conformance<F, R>(
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
        Self::create_for_conformance_inner(input, validate_roles, reduce_activity)
    }

    /// Crate-private raw closure seam for embedded golden-corpus validation.
    #[cfg(not(feature = "conformance-tracer"))]
    pub(crate) fn create_for_conformance<F, R>(
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
        Self::create_for_conformance_inner(input, validate_roles, reduce_activity)
    }

    fn create_for_conformance_inner<F, R>(
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
        let core_reducer = CoreReducerV1::new(validate_roles);
        let preparer = RoomTransitionPreparerV1::new(core_reducer, reduce_activity);
        Self::create_with_preparer(input, preparer, None)
    }

    fn create_with_preparer(
        input: GenesisInputV1,
        preparer: RoomTransitionPreparerV1,
        retained_pack: Option<RetainedActivityPackV1>,
    ) -> Result<Self, TraceErrorV1> {
        if input.initial_core_state.room_status != RoomStatusV1::Active {
            return Err(TraceErrorV1::GenesisMustBeActive);
        }
        if input.initial_core_state.memberships().len() > MAX_ROOM_MEMBERSHIPS_V1 {
            return Err(TraceErrorV1::RoomMembershipLimitExceeded);
        }
        let verified_core = preparer
            .core_reducer
            .validate_state(input.initial_core_state.clone())?;
        let timers =
            TimerBookV1::from_genesis(&input.initial_timers, input.created_at.as_str(), true)?;
        let core_state_hash = verified_core.core_state_hash.clone();
        let activity_state_hash =
            hash_activity_state(&input.pack_digest, &input.initial_activity_state)?;
        let authoritative_state_hash =
            hash_authoritative_state(&input.pack_digest, &core_state_hash, &activity_state_hash)?;

        let room_seed = input.room_seed.clone();
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
            room_seed,
            retained_pack,
            preparer,
        })
    }

    /// Convenience conformance operation that prepares and immediately
    /// installs one normalized Stimulus in memory.
    ///
    /// # Errors
    ///
    /// Returns an error if preparation or checked installation fails.
    pub(crate) fn advance(
        &mut self,
        stimulus: RecordedStimulusV1,
    ) -> Result<AdvanceDispositionV1, TraceErrorV1> {
        let prepared = self.prepare(stimulus)?;
        self.install_prepared_inner(prepared, true)
    }

    /// Bypasses durable commit only for explicit conformance tracing.
    #[cfg(feature = "conformance-tracer")]
    #[doc(hidden)]
    pub fn advance_for_conformance(
        &mut self,
        stimulus: RecordedStimulusV1,
    ) -> Result<AdvanceDispositionV1, TraceErrorV1> {
        self.advance(stimulus)
    }

    /// Purely evaluates one normalized Stimulus against the exact current
    /// Head. This performs Core reduction, Activity reduction, normalization,
    /// canonical encoding, and hashing without installing state or consuming
    /// a sequence. A runtime must pass the sealed result through the
    /// storage-backed commit coordinator before installation.
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

    /// Performs only the stable host-controlled Action admission checks. It
    /// never invokes Activity reduction and returns `None` when reduction is
    /// legal rather than fabricating a durable rejection.
    pub(crate) fn assess_stable_action_disposition(
        &self,
        request: &crate::ParticipantActionRequestV1,
        admitted_at: &crate::ActionAdmittedAt,
    ) -> Result<Option<StableActionAdmissionV1>, TraceErrorV1> {
        if request.room_id() != self.head.room_id() {
            return Err(TraceErrorV1::CompleteHeadMismatch);
        }
        let retained = self
            .retained_pack
            .as_ref()
            .ok_or(TraceErrorV1::RetainedPackUnavailable)?;
        Self::assess_stable_action_from_values(
            &self.head,
            &self.core_state,
            &self.activity_state,
            retained,
            request,
            admitted_at,
            None,
        )
    }

    #[allow(clippy::too_many_lines)]
    fn assess_stable_action_from_values(
        head: &CompleteHeadV1,
        core_state: &CoreRoomStateV1,
        activity_state: &CanonicalJsonV1,
        retained_pack: &RetainedActivityPackV1,
        request: &crate::ParticipantActionRequestV1,
        admitted_at: &crate::ActionAdmittedAt,
        payload_budget: Option<crate::PayloadBudgetV1>,
    ) -> Result<Option<StableActionAdmissionV1>, TraceErrorV1> {
        if request.room_id() != head.room_id() {
            return Err(TraceErrorV1::CompleteHeadMismatch);
        }
        if let Some(budget) = payload_budget {
            budget.check_value(crate::PayloadKindV1::ActionPayload, request.payload())?;
        }
        let declared_payload_schema = retained_pack
            .host()
            .preflight_action_payload(request.action_type(), request.payload())
            .map_err(map_activity_pack_reduce_error)?;
        let membership = core_state
            .membership(request.member_id())
            .ok_or(TraceErrorV1::ParticipantNotEligible)?;
        if membership.access_mode() != AccessModeV1::Participant {
            return Err(TraceErrorV1::ParticipantNotEligible);
        }
        if core_state.room_status() == RoomStatusV1::Archived {
            return Ok(Some(StableActionAdmissionV1 {
                code: "room_archived",
                normalized_action: None,
                current_action_offers: None,
                unavailable_reason: Some("room_archived"),
            }));
        }
        if membership.standing() != MembershipStandingV1::Enabled {
            return Ok(Some(StableActionAdmissionV1 {
                code: "membership_not_enabled",
                normalized_action: None,
                current_action_offers: None,
                unavailable_reason: Some("membership_not_enabled"),
            }));
        }
        let current_action_offers = || -> Result<Option<Vec<u8>>, TraceErrorV1> {
            let viewer = PackViewerV1::Participant(request.member_id().clone());
            let view = retained_pack.host().view_with_budget(
                &ViewInputV1 {
                    core: &core_state,
                    activity_state: &activity_state,
                    complete_head: &head,
                    viewer: &viewer,
                },
                payload_budget,
            )?;
            Ok(Some(view.action_offers().canonical_bytes().to_vec()))
        };

        if request.based_on_room_seq() != head.room_seq() {
            let offers = current_action_offers()?;
            return Ok(Some(StableActionAdmissionV1 {
                code: "stale_room_state",
                normalized_action: None,
                unavailable_reason: offers
                    .is_none()
                    .then_some("current_view_unavailable_for_stale_basis"),
                current_action_offers: offers,
            }));
        }
        let viewer = PackViewerV1::Participant(request.member_id().clone());
        let view = retained_pack.host().view_with_budget(
            &ViewInputV1 {
                core: &core_state,
                activity_state: &activity_state,
                complete_head: &head,
                viewer: &viewer,
            },
            payload_budget,
        )?;
        let offers = view.action_offers().canonical_bytes().to_vec();
        let Some(offer) = view
            .action_offers()
            .offers()
            .iter()
            .find(|offer| offer.action_type == request.action_type())
        else {
            return Ok(Some(StableActionAdmissionV1 {
                code: "action_not_allowed",
                normalized_action: None,
                current_action_offers: Some(offers),
                unavailable_reason: None,
            }));
        };
        if declared_payload_schema.as_ref() != Some(&offer.payload_schema_digest) {
            return Err(ActionAdmissionErrorV1::InvalidPayload(
                "payload schema digest disagrees with the exact offer".to_owned(),
            )
            .into());
        }
        let action = crate::ParticipantActionV1 {
            member_id: request.member_id().clone(),
            action_id: request.action_id().clone(),
            action_type: request.action_type().to_owned(),
            payload_schema_digest: offer.payload_schema_digest.clone(),
            canonical_payload: request.payload().clone(),
            exact_basis_head: head.clone(),
            admitted_at: admitted_at.clone(),
        };
        match retained_pack.host().pre_admit_action(&action, &view, &head) {
            Ok(()) => Ok(None),
            Err(ActivityPackReduceErrorV1::Admission(error)) => {
                let code = match error {
                    ActionAdmissionErrorV1::ActionNotAllowed(_)
                    | ActionAdmissionErrorV1::NotOpen => "action_not_allowed",
                    ActionAdmissionErrorV1::DeadlinePassed => "deadline_passed",
                    ActionAdmissionErrorV1::StaleBasis => "stale_room_state",
                    ActionAdmissionErrorV1::InvalidPayload(_) => {
                        return Err(error.into());
                    }
                };
                Ok(Some(StableActionAdmissionV1 {
                    code,
                    normalized_action: Some(action),
                    current_action_offers: Some(offers),
                    unavailable_reason: None,
                }))
            }
            Err(ActivityPackReduceErrorV1::Pack(error)) => Err(error.into()),
        }
    }

    /// Constructs the checked observation for one newly prepared Transition.
    /// The caller supplies only a typed viewer; all before/after state, Heads,
    /// causing Stimulus, and ordered Domain Events come from this current trace
    /// and the opaque sealed preparation.
    ///
    /// # Errors
    ///
    /// Returns an error when the preparation is stale, foreign, does not
    /// contain a new Transition, or the retained observation contract faults.
    pub(crate) fn observe_prepared(
        &self,
        prepared: &PreparedRoomTransitionV1,
        viewer: &PackViewerV1,
    ) -> Result<ActivityObservationOutcomeV1, TraceErrorV1> {
        if !Arc::ptr_eq(&prepared.preparer_identity, &self.preparer.identity) {
            return Err(TraceErrorV1::TransitionPreparerProvenanceMismatch);
        }
        if !prepared.is_new || prepared.basis_complete_head != self.head {
            return Err(TraceErrorV1::PreparedBasisMismatch);
        }
        let AdvanceDispositionV1::TransitionAccepted { transition, .. } = &prepared.outcome else {
            return Err(TraceErrorV1::PreparedAdvanceHasNoTransition);
        };
        let resulting_state = prepared
            .resulting_state
            .as_ref()
            .ok_or(TraceErrorV1::InvalidPreparedAdvance)?;
        if !Arc::ptr_eq(&resulting_state.preparer_identity, &self.preparer.identity)
            || resulting_state.head != transition.head()
            || resulting_state.core_state.state != transition.resulting_core_state
            || resulting_state.activity_state != transition.resulting_activity_state
        {
            return Err(TraceErrorV1::InvalidPreparedAdvance);
        }
        let retained_pack = self
            .retained_pack
            .as_ref()
            .ok_or(TraceErrorV1::RetainedPackUnavailable)?;
        retained_pack
            .host()
            .observe(&ObserveTransitionInputV1 {
                core_before: &self.core_state,
                activity_before: &self.activity_state,
                head_before: &self.head,
                core_after: &transition.resulting_core_state,
                activity_after: &transition.resulting_activity_state,
                head_after: &resulting_state.head,
                recorded_stimulus: &transition.recorded_stimulus,
                ordered_domain_events: &transition.ordered_domain_events,
                viewer,
            })
            .map_err(Into::into)
    }

    /// Exposes a sealed precommit observation only to explicit conformance
    /// tests; production publication must wait for the commit coordinator.
    #[cfg(feature = "conformance-tracer")]
    #[doc(hidden)]
    pub fn observe_prepared_for_conformance(
        &self,
        prepared: &PreparedRoomTransitionV1,
        viewer: &PackViewerV1,
    ) -> Result<ActivityObservationOutcomeV1, TraceErrorV1> {
        self.observe_prepared(prepared, viewer)
    }

    /// Installs a privately constructed prepared result after the caller's
    /// durable commit point. Installation fails closed if the indivisible
    /// eight-field basis Head has changed since preparation.
    ///
    /// # Errors
    ///
    /// Returns an error for mismatched provenance, Room, basis Head, or
    /// administrative identity/request semantics.
    pub(crate) fn install_prepared(
        &mut self,
        prepared: PreparedRoomTransitionV1,
    ) -> Result<AdvanceDispositionV1, TraceErrorV1> {
        self.install_prepared_inner(prepared, true)
    }

    /// Bypasses durable commit only for explicit conformance tracing.
    #[cfg(feature = "conformance-tracer")]
    #[doc(hidden)]
    pub fn install_prepared_for_conformance(
        &mut self,
        prepared: PreparedRoomTransitionV1,
    ) -> Result<AdvanceDispositionV1, TraceErrorV1> {
        self.install_prepared(prepared)
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
                recorded_stimulus: stimulus.clone(),
                action_offer_witness: None,
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

    /// Executes the exact retained Pack revision across persisted
    /// Genesis/Transition bytes and checks the resulting Head and serving
    /// Core/Activity materializations without returning an executable trace.
    ///
    /// This is the production storage-verifier seam used by offline transfer
    /// and native restore. It reproduces observation consequences during
    /// Replay, owns no external-effect capability, and cannot be used to
    /// continue the Room.
    ///
    /// # Errors
    ///
    /// Returns a classified Replay failure when retained code is absent or
    /// faults, canonical lineage disagrees, or the replayed result differs
    /// from the exact captured serving state.
    pub fn verify_executable_history_for_storage(
        registry: &PackRegistryV1,
        expected_head: &CompleteHeadV1,
        genesis_bytes: &[u8],
        transition_bytes: &[Vec<u8>],
        expected_core_state_bytes: &[u8],
        expected_activity_state_bytes: &[u8],
    ) -> Result<ReplayStorageVerificationV1, ReplayFailureV1> {
        let report = Self::replay_registry(registry, genesis_bytes, transition_bytes, true)?;
        if &report.final_head != expected_head {
            return Err(ReplayFailureV1::with_head(
                ReplayFailureClassV1::LineageHash,
                "replayed final Head differs from captured Head".to_owned(),
                report.final_head.clone(),
            ));
        }
        let replayed_core_state_bytes = report
            .final_state()
            .core_state()
            .canonical_bytes()
            .map_err(|error| {
                ReplayFailureV1::with_head(
                    ReplayFailureClassV1::CanonicalEncoding,
                    error.to_string(),
                    expected_head.clone(),
                )
            })?;
        if replayed_core_state_bytes != expected_core_state_bytes {
            return Err(ReplayFailureV1::with_head(
                ReplayFailureClassV1::CoreState,
                "replayed Core materialization differs from captured bytes".to_owned(),
                expected_head.clone(),
            ));
        }
        let replayed_activity_state_bytes = report
            .final_state()
            .activity_state()
            .to_bytes()
            .map_err(|error| {
                ReplayFailureV1::with_head(
                    ReplayFailureClassV1::CanonicalEncoding,
                    error.to_string(),
                    expected_head.clone(),
                )
            })?;
        if replayed_activity_state_bytes != expected_activity_state_bytes {
            return Err(ReplayFailureV1::with_head(
                ReplayFailureClassV1::ActivityState,
                "replayed Activity materialization differs from captured bytes".to_owned(),
                expected_head.clone(),
            ));
        }
        ReplayStorageVerificationV1::from_report(&report, genesis_bytes, transition_bytes).map_err(
            |detail| {
                ReplayFailureV1::with_head(
                    ReplayFailureClassV1::CoreInvariant,
                    detail,
                    expected_head.clone(),
                )
            },
        )
    }

    /// Replays exact persisted canonical bytes with the exact runnable
    /// revision named by stored Genesis. The registry lookup never substitutes
    /// a selectable or newer revision.
    ///
    /// # Errors
    ///
    /// Returns a classified failure with the last verified Head when strict
    /// decoding, retained-revision lookup/initialization, lineage,
    /// deterministic reduction, or invariants disagree.
    #[cfg(any(test, feature = "conformance-tracer"))]
    pub fn replay(
        registry: &PackRegistryV1,
        genesis_bytes: &[u8],
        transition_bytes: &[Vec<u8>],
    ) -> Result<ReplayReportV1, ReplayFailureV1> {
        Self::replay_registry(registry, genesis_bytes, transition_bytes, false)
    }

    /// Replays one exact historical sequence and constructs a host-validated,
    /// structurally complete projection envelope.
    ///
    /// This is a pure history verifier, not a present-authorization boundary.
    /// A durable Adapter must first consume an [`crate::AuthorizedReplayV1`],
    /// revalidate its current authority with a trusted clock, and capture the
    /// exact lineage prefix plus operational integrity under one serialized
    /// boundary. Historical Standing, Access Mode, and Role are then
    /// reconstructed from lineage rather than copied from present state.
    ///
    /// # Errors
    ///
    /// Returns a closed failure if the address/sequence does not match,
    /// historical Membership is absent or disabled, exact replay fails, or
    /// the retained pack cannot construct the authorized projection.
    pub fn project_replayed_history(
        registry: &PackRegistryV1,
        genesis_bytes: &[u8],
        transition_bytes: &[Vec<u8>],
        request: HistoricalReplayProjectionRequestV1,
    ) -> Result<HistoricalReplayProjectionV1, HistoricalReplayErrorV1> {
        if request.integrity.status() == RoomIntegrityStatusV1::Quarantined {
            return Err(HistoricalReplayErrorV1::IntegrityUnavailable);
        }
        let transition_count = usize::try_from(request.at_room_seq.get())
            .map_err(|_| HistoricalReplayErrorV1::SequenceUnavailable)?;
        let requested_transitions = transition_bytes
            .get(..transition_count)
            .ok_or(HistoricalReplayErrorV1::SequenceUnavailable)?;
        let report = Self::replay_registry(registry, genesis_bytes, requested_transitions, false)
            .map_err(|failure| HistoricalReplayErrorV1::ReplayFailed(failure.class))?;
        Self::historical_projection_from_trace(&report.continuation_trace, request)
    }

    fn historical_projection_from_trace(
        trace: &Self,
        request: HistoricalReplayProjectionRequestV1,
    ) -> Result<HistoricalReplayProjectionV1, HistoricalReplayErrorV1> {
        let retained_pack = trace
            .retained_pack
            .as_ref()
            .ok_or(HistoricalReplayErrorV1::ProjectionUnavailable)?;
        historical_projection_from_values(
            &trace.head,
            &trace.core_state,
            &trace.activity_state,
            retained_pack,
            request,
            None,
        )
    }

    pub(crate) fn replay_for_recovery(
        registry: &PackRegistryV1,
        genesis_bytes: &[u8],
        transition_bytes: &[Vec<u8>],
    ) -> Result<ReplayReportV1, ReplayFailureV1> {
        Self::replay_registry(registry, genesis_bytes, transition_bytes, true)
    }

    /// Reconstructs an executor from a storage-verified checkpoint and folds
    /// only the immutable Transition tail after that checkpoint.  The
    /// checkpoint is never trusted for lineage: its record, Head, hashes,
    /// materializations, timer ledger, and retained pack are checked before
    /// the first tail reduction.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn replay_checkpoint_for_recovery(
        registry: &PackRegistryV1,
        genesis_bytes: &[u8],
        checkpoint: &crate::RoomRecoveryCheckpointV1,
        transition_bytes: &[Vec<u8>],
    ) -> Result<ReplayReportV1, ReplayFailureV1> {
        let genesis = decode_replay_genesis(genesis_bytes)?;
        let room_seed = genesis.room_seed.clone();
        let checkpoint_head = checkpoint.head().clone();
        if checkpoint_head.room_id() != genesis.room_id() {
            return Err(ReplayFailureV1::without_head(
                ReplayFailureClassV1::Sequence,
                "checkpoint is outside the captured recovery range".to_owned(),
            ));
        }
        let request = pack_genesis_request_from_record(&genesis);
        let verified = registry
            .prepare_genesis_for_retained_room(&request)
            .map_err(|error| map_retained_genesis_error(&error, checkpoint_head.clone()))?;
        verify_retained_genesis_matches_record(&genesis, &verified)?;
        let retained_pack = verified.retained_pack().clone();
        let transition_preparer = RoomTransitionPreparerV1::from_retained_pack(
            retained_pack.clone(),
            genesis.room_seed.clone(),
        );
        let core_state =
            CanonicalJsonV1::decode_canonical::<CoreRoomStateV1>(checkpoint.core_state_bytes())
                .map_err(|_| {
                    ReplayFailureV1::with_head(
                        ReplayFailureClassV1::NonCanonicalRecord,
                        "checkpoint Core state is not canonical".to_owned(),
                        checkpoint_head.clone(),
                    )
                })?;
        let activity_state = CanonicalJsonV1::from_canonical_bytes(
            checkpoint.activity_state_bytes(),
        )
        .map_err(|_| {
            ReplayFailureV1::with_head(
                ReplayFailureClassV1::NonCanonicalRecord,
                "checkpoint Activity state is not canonical".to_owned(),
                checkpoint_head.clone(),
            )
        })?;
        transition_preparer
            .core_reducer
            .validate_state(core_state.clone())
            .map_err(|error| {
                ReplayFailureV1::with_head(
                    ReplayFailureClassV1::CoreInvariant,
                    error.to_string(),
                    checkpoint_head.clone(),
                )
            })?;
        if hash_core_state(&core_state).map_err(|error| {
            ReplayFailureV1::with_head(
                ReplayFailureClassV1::CanonicalEncoding,
                error.to_string(),
                checkpoint_head.clone(),
            )
        })? != *checkpoint_head.core_state_hash()
            || hash_activity_state(checkpoint_head.pack_digest(), &activity_state).map_err(
                |error| {
                    ReplayFailureV1::with_head(
                        ReplayFailureClassV1::CanonicalEncoding,
                        error.to_string(),
                        checkpoint_head.clone(),
                    )
                },
            )? != *checkpoint_head.activity_state_hash()
            || hash_authoritative_state(
                checkpoint_head.pack_digest(),
                checkpoint_head.core_state_hash(),
                checkpoint_head.activity_state_hash(),
            )
            .map_err(|error| {
                ReplayFailureV1::with_head(
                    ReplayFailureClassV1::CanonicalEncoding,
                    error.to_string(),
                    checkpoint_head.clone(),
                )
            })? != *checkpoint_head.authoritative_state_hash()
        {
            return Err(ReplayFailureV1::with_head(
                ReplayFailureClassV1::CoreState,
                "checkpoint materialization hash mismatch".to_owned(),
                checkpoint_head.clone(),
            ));
        }
        let record_head = if checkpoint_head.room_seq().get() == 0 {
            let record =
                GenesisV1::from_canonical_bytes(checkpoint.record_bytes()).map_err(|_| {
                    ReplayFailureV1::with_head(
                        ReplayFailureClassV1::NonCanonicalRecord,
                        "checkpoint Genesis is not canonical".to_owned(),
                        checkpoint_head.clone(),
                    )
                })?;
            validate_stored_genesis_integrity(&record).map_err(|error| {
                ReplayFailureV1::with_head(
                    ReplayFailureClassV1::CoreInvariant,
                    error.to_string(),
                    checkpoint_head.clone(),
                )
            })?;
            record.complete_head()
        } else {
            let record =
                TransitionV1::from_canonical_bytes(checkpoint.record_bytes()).map_err(|_| {
                    ReplayFailureV1::with_head(
                        ReplayFailureClassV1::NonCanonicalRecord,
                        "checkpoint Transition is not canonical".to_owned(),
                        checkpoint_head.clone(),
                    )
                })?;
            validate_stored_transition_hashes(&record).map_err(|(class, detail)| {
                ReplayFailureV1::with_head(class, detail, checkpoint_head.clone())
            })?;
            record.complete_head()
        };
        if record_head != checkpoint_head
            || core_state.canonical_bytes().map_err(|_| {
                ReplayFailureV1::without_head(
                    ReplayFailureClassV1::CanonicalEncoding,
                    "checkpoint Core encoding failed".to_owned(),
                )
            })? != checkpoint.core_state_bytes()
            || activity_state.to_bytes().map_err(|_| {
                ReplayFailureV1::without_head(
                    ReplayFailureClassV1::CanonicalEncoding,
                    "checkpoint Activity encoding failed".to_owned(),
                )
            })? != checkpoint.activity_state_bytes()
        {
            return Err(ReplayFailureV1::with_head(
                ReplayFailureClassV1::LineageHash,
                "checkpoint record and materializations disagree".to_owned(),
                checkpoint_head.clone(),
            ));
        }
        let timers = TimerBookV1::from_checkpoint(checkpoint.timers()).map_err(|error| {
            ReplayFailureV1::with_head(
                ReplayFailureClassV1::CoreInvariant,
                error.to_string(),
                checkpoint_head.clone(),
            )
        })?;
        let mut trace = Self {
            genesis,
            transitions: Vec::new(),
            core_state,
            activity_state,
            head: checkpoint_head.clone(),
            timers,
            administration_results: BTreeMap::new(),
            activity_callback_count: AtomicUsize::new(0),
            administrative_receipt_count: 0,
            room_seed,
            retained_pack: Some(retained_pack),
            preparer: transition_preparer,
        };
        let mut steps = Vec::new();
        let mut frame_heads = checkpoint.observation_frame_heads().clone();
        if frame_heads.len() != trace.core_state.memberships().len()
            || trace
                .core_state
                .memberships()
                .keys()
                .any(|member_id| !frame_heads.contains_key(member_id))
        {
            return Err(ReplayFailureV1::with_head(
                ReplayFailureClassV1::CoreInvariant,
                "checkpoint observation positions disagree with Memberships".to_owned(),
                checkpoint_head,
            ));
        }
        let mut observation_consequences = Vec::new();
        for bytes in transition_bytes {
            let last_verified_head = trace.head.clone();
            trace.replay_stored_transition(
                bytes,
                Some((&mut frame_heads, &mut observation_consequences)),
            )?;
            steps.push(ReplayStepV1::transition(&trace, bytes).map_err(|error| {
                ReplayFailureV1::with_head(
                    ReplayFailureClassV1::CanonicalEncoding,
                    error.to_string(),
                    last_verified_head,
                )
            })?);
        }
        let final_state = trace.transition_state();
        let final_head = trace.head.clone();
        let continuation_preparer = trace.preparer.clone();
        let activity_callback_count = trace.activity_callback_count.load(AtomicOrdering::Relaxed);
        Ok(ReplayReportV1 {
            final_head,
            final_state,
            continuation_preparer,
            continuation_trace: trace,
            observation_consequences,
            steps,
            activity_callback_count,
            external_effect_count: 0,
            receipt_count: 0,
        })
    }

    pub(crate) fn preflight_recovery_materializations(
        genesis_bytes: &[u8],
        transition_bytes: &[Vec<u8>],
    ) -> Result<(CompleteHeadV1, Vec<u8>, Vec<u8>), ReplayFailureV1> {
        let genesis = decode_replay_genesis(genesis_bytes)?;
        let head = preflight_stored_history(&genesis, transition_bytes)?;
        let (core_state, activity_state) = if let Some(bytes) = transition_bytes.last() {
            let transition = TransitionV1::from_canonical_bytes(bytes).map_err(|error| {
                ReplayFailureV1::with_head(
                    ReplayFailureClassV1::NonCanonicalRecord,
                    error.to_string(),
                    head.clone(),
                )
            })?;
            (
                transition.resulting_core_state().clone(),
                transition.resulting_activity_state().clone(),
            )
        } else {
            (
                genesis.initial_core_state().clone(),
                genesis.initial_activity_state().clone(),
            )
        };
        let core_state_bytes = core_state.canonical_bytes().map_err(|error| {
            ReplayFailureV1::with_head(
                ReplayFailureClassV1::CanonicalEncoding,
                error.to_string(),
                head.clone(),
            )
        })?;
        let activity_state_bytes = activity_state.to_bytes().map_err(|error| {
            ReplayFailureV1::with_head(
                ReplayFailureClassV1::CanonicalEncoding,
                error.to_string(),
                head.clone(),
            )
        })?;
        Ok((head, core_state_bytes, activity_state_bytes))
    }

    /// Verifies the one persisted record named by the current Room Head.
    ///
    /// This deliberately does not replay the Room. It is the bounded storage
    /// primitive used to prove that a current serving materialization is the
    /// exact state embedded in the current canonical Genesis/Transition row.
    pub(crate) fn preflight_current_storage_materialization(
        expected_head: &CompleteHeadV1,
        canonical_current_record_bytes: &[u8],
        canonical_core_state_bytes: &[u8],
        canonical_activity_state_bytes: &[u8],
    ) -> Result<(CoreRoomStateV1, Option<Blake3DigestV1>), TraceErrorV1> {
        let (core_state, activity_state, previous_lineage_hash) = if expected_head.room_seq().get()
            == 0
        {
            let genesis = GenesisV1::from_canonical_bytes(canonical_current_record_bytes)?;
            validate_stored_genesis_integrity(&genesis)?;
            validate_core_state(genesis.initial_core_state(), &|_| Ok(()))
                .map_err(map_checked_core_error)?;
            if genesis.complete_head() != *expected_head {
                return Err(TraceErrorV1::CompleteHeadMismatch);
            }
            (
                genesis.initial_core_state().clone(),
                genesis.initial_activity_state().clone(),
                None,
            )
        } else {
            let transition = TransitionV1::from_canonical_bytes(canonical_current_record_bytes)?;
            if transition.transition_version != TRANSITION_VERSION
                || transition.codec_id != CANONICAL_CODEC_ID
                || transition.hash_suite != HASH_SUITE_ID
                || transition.core_schema_version != CORE_SCHEMA_VERSION
                || transition.room_id != *expected_head.room_id()
                || transition.room_seq != expected_head.room_seq()
                || transition.pack_digest != *expected_head.pack_digest()
            {
                return Err(TraceErrorV1::VersionIdentityMismatch);
            }
            validate_stored_transition_hashes(&transition).map_err(|(class, _)| match class {
                ReplayFailureClassV1::LineageHash => TraceErrorV1::LineageHashMismatch,
                ReplayFailureClassV1::CanonicalEncoding | ReplayFailureClassV1::StateHashes => {
                    TraceErrorV1::StateHashMismatch
                }
                _ => TraceErrorV1::StateHashMismatch,
            })?;
            validate_core_state(transition.resulting_core_state(), &|_| Ok(()))
                .map_err(map_checked_core_error)?;
            if transition.complete_head() != *expected_head {
                return Err(TraceErrorV1::CompleteHeadMismatch);
            }
            (
                transition.resulting_core_state().clone(),
                transition.resulting_activity_state().clone(),
                Some(transition.previous_lineage_hash().clone()),
            )
        };
        if core_state.canonical_bytes()? != canonical_core_state_bytes
            || activity_state.to_bytes()? != canonical_activity_state_bytes
        {
            return Err(TraceErrorV1::StateHashMismatch);
        }
        Ok((core_state, previous_lineage_hash))
    }

    fn replay_registry(
        registry: &PackRegistryV1,
        genesis_bytes: &[u8],
        transition_bytes: &[Vec<u8>],
        reproduce_observation_frames: bool,
    ) -> Result<ReplayReportV1, ReplayFailureV1> {
        let genesis = decode_replay_genesis(genesis_bytes)?;
        let preflight_head = preflight_stored_history(&genesis, transition_bytes)?;
        let request = pack_genesis_request_from_record(&genesis);
        let verified = registry
            .prepare_genesis_for_retained_room(&request)
            .map_err(|error| map_retained_genesis_error(&error, preflight_head))?;
        verify_retained_genesis_matches_record(&genesis, &verified)?;
        let retained_pack = verified.retained_pack().clone();
        let transition_preparer = RoomTransitionPreparerV1::from_retained_pack(
            retained_pack.clone(),
            genesis.room_seed.clone(),
        );
        Self::replay_with_preparer(
            genesis,
            genesis_bytes,
            transition_bytes,
            transition_preparer,
            Some(retained_pack),
            reproduce_observation_frames,
        )
    }

    /// Crate-private raw closure seam used only by frozen replay conformance
    /// fixtures. Production replay must use [`Self::replay`].
    #[cfg(test)]
    pub(crate) fn replay_for_conformance<F, R>(
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
        let genesis = decode_replay_genesis(genesis_bytes)?;
        let transition_preparer =
            RoomTransitionPreparerV1::new(CoreReducerV1::new(validate_roles), reduce_activity);
        Self::replay_with_preparer(
            genesis,
            genesis_bytes,
            transition_bytes,
            transition_preparer,
            None,
            false,
        )
    }

    #[allow(clippy::too_many_lines)]
    fn replay_with_preparer(
        genesis: GenesisV1,
        genesis_bytes: &[u8],
        transition_bytes: &[Vec<u8>],
        transition_preparer: RoomTransitionPreparerV1,
        retained_pack: Option<RetainedActivityPackV1>,
        reproduce_observation_frames: bool,
    ) -> Result<ReplayReportV1, ReplayFailureV1> {
        let mut trace = Self::from_verified_genesis(genesis, transition_preparer, retained_pack)
            .map_err(|error| {
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
        let mut frame_heads = trace
            .core_state
            .memberships()
            .keys()
            .cloned()
            .map(|member_id| (member_id, 0_u64))
            .collect::<BTreeMap<_, _>>();
        let mut observation_consequences = Vec::new();
        for bytes in transition_bytes {
            let last_verified_head = trace.head.clone();
            let frame_output = reproduce_observation_frames
                .then_some((&mut frame_heads, &mut observation_consequences));
            trace.replay_stored_transition(bytes, frame_output)?;
            steps.push(ReplayStepV1::transition(&trace, bytes).map_err(|error| {
                ReplayFailureV1::with_head(
                    ReplayFailureClassV1::CanonicalEncoding,
                    error.to_string(),
                    last_verified_head,
                )
            })?);
        }

        let final_state = trace.transition_state();
        let final_head = trace.head.clone();
        let continuation_preparer = trace.preparer.clone();
        let activity_callback_count = trace.activity_callback_count.load(AtomicOrdering::Relaxed);
        Ok(ReplayReportV1 {
            final_head,
            final_state,
            continuation_preparer,
            continuation_trace: trace,
            observation_consequences,
            steps,
            activity_callback_count,
            external_effect_count: 0,
            receipt_count: 0,
        })
    }

    #[allow(clippy::too_many_lines)]
    fn replay_stored_transition(
        &mut self,
        bytes: &[u8],
        frame_output: Option<(
            &mut BTreeMap<MemberId, u64>,
            &mut Vec<ReplayObservationConsequenceV1>,
        )>,
    ) -> Result<(), ReplayFailureV1> {
        let last_verified_head = self.head.clone();
        let stored = TransitionV1::from_canonical_bytes(bytes).map_err(|error| {
            ReplayFailureV1::with_head(
                ReplayFailureClassV1::NonCanonicalRecord,
                error.to_string(),
                last_verified_head.clone(),
            )
        })?;
        validate_transition_metadata(&self.head, &stored).map_err(|(class, detail)| {
            ReplayFailureV1::with_head(class, detail, last_verified_head.clone())
        })?;
        validate_stored_transition_hashes(&stored).map_err(|(class, detail)| {
            ReplayFailureV1::with_head(class, detail, last_verified_head.clone())
        })?;
        self.preparer
            .core_reducer
            .validate_state(stored.resulting_core_state.clone())
            .map_err(|error| {
                let class = match error {
                    TraceErrorV1::Pack(PackFaultV1::RoleValidatorPanicked) => {
                        ReplayFailureClassV1::RuntimeFault
                    }
                    _ => ReplayFailureClassV1::CoreInvariant,
                };
                ReplayFailureV1::with_head(class, error.to_string(), last_verified_head.clone())
            })?;

        let prepared_transition =
            self.prepare(stored.recorded_stimulus.clone())
                .map_err(|error| {
                    ReplayFailureV1::with_head(
                        classify_replay_advance_error(&error),
                        error.to_string(),
                        last_verified_head.clone(),
                    )
                })?;
        if let Some((frame_heads, observation_consequences)) = frame_output {
            record_replay_observation_consequences(
                self,
                &prepared_transition,
                frame_heads,
                observation_consequences,
            )
            .map_err(|error| {
                ReplayFailureV1::with_head(
                    classify_replay_advance_error(&error),
                    error.to_string(),
                    last_verified_head.clone(),
                )
            })?;
        }
        let generated = self
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
        })? != bytes
        {
            return Err(ReplayFailureV1::with_head(
                ReplayFailureClassV1::RecordBytes,
                "replayed Transition bytes differ".to_owned(),
                last_verified_head,
            ));
        }
        Ok(())
    }

    fn from_verified_genesis(
        genesis: GenesisV1,
        preparer: RoomTransitionPreparerV1,
        retained_pack: Option<RetainedActivityPackV1>,
    ) -> Result<Self, TraceErrorV1> {
        let timers = validate_stored_genesis_integrity(&genesis)?;
        preparer
            .core_reducer
            .validate_state(genesis.initial_core_state.clone())?;
        let head = genesis.head();
        let room_seed = genesis.room_seed.clone();
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
            room_seed,
            retained_pack,
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
    #[cfg(test)]
    pub(crate) fn verified_transition_state(&self) -> RoomTransitionStateV1 {
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

    /// Returns the immutable Room seed captured from checked Genesis.
    #[must_use]
    pub const fn room_seed(&self) -> &RoomSeedV1 {
        &self.room_seed
    }

    /// Returns the exact retained revision bound to this production trace.
    /// `None` is reserved for crate-private conformance fixtures.
    #[must_use]
    pub const fn retained_pack(&self) -> Option<&RetainedActivityPackV1> {
        self.retained_pack.as_ref()
    }

    /// Returns the exact current Timer view to the retained pack host.
    pub(crate) fn scheduled_timers(&self) -> &BTreeMap<TimerId, ScheduledTimerV1> {
        &self.timers.scheduled
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

    /// A live adapter owns durable history and receipt resolution in storage.
    /// Keep only the current execution basis between cache borrows, as for
    /// the bounded historical replay accumulator. This never changes Head,
    /// Pack provenance, Timer state, or canonical storage.
    pub(crate) fn discard_persisted_history(&mut self) {
        self.transitions = Vec::new();
        self.administration_results.clear();
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

    /// Number of counted Activity reductions. The registry path excludes work
    /// rejected by checked-host admission before a disposition is returned;
    /// the explicit conformance seam counts raw callback entry.
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

fn historical_projection_from_values(
    head: &CompleteHeadV1,
    core_state: &CoreRoomStateV1,
    activity_state: &CanonicalJsonV1,
    retained_pack: &RetainedActivityPackV1,
    request: HistoricalReplayProjectionRequestV1,
    payload_budget: Option<crate::PayloadBudgetV1>,
) -> Result<HistoricalReplayProjectionV1, HistoricalReplayErrorV1> {
    if head.room_id() != &request.room_id || head.room_seq() != request.at_room_seq {
        return Err(HistoricalReplayErrorV1::AddressMismatch);
    }
    let historical_membership = core_state
        .membership(&request.member_id)
        .ok_or(HistoricalReplayErrorV1::HistoricalMembershipUnavailable)?;
    if historical_membership.standing() != MembershipStandingV1::Enabled {
        return Err(HistoricalReplayErrorV1::HistoricalMembershipUnavailable);
    }
    let viewer = match request.projection_kind {
        ReplayProjectionKindV1::HistoricalMembership => {
            PackViewerV1::Historical(request.member_id.clone())
        }
        ReplayProjectionKindV1::FinalReveal => PackViewerV1::FinalReveal(request.member_id.clone()),
    };
    let view = retained_pack
        .host()
        .view_with_budget(
            &ViewInputV1 {
                core: core_state,
                activity_state,
                complete_head: head,
                viewer: &viewer,
            },
            payload_budget,
        )
        .map_err(|_| HistoricalReplayErrorV1::ProjectionUnavailable)?;
    if !view.action_offers().offers().is_empty() {
        return Err(HistoricalReplayErrorV1::ProjectionUnavailable);
    }
    let canonical_view = CanonicalJsonV1::from_canonical_bytes(view.canonical_bytes())
        .map_err(|_| HistoricalReplayErrorV1::ProjectionUnavailable)?;
    let historical_membership = historical_membership.clone();
    let canonical_envelope = CanonicalJsonV1::from_serialize(&HistoricalReplayEnvelopeV1 {
        envelope: "worldstream/historical-replay-projection/v1",
        verified_head: head,
        integrity: &request.integrity,
        room_status: core_state.room_status(),
        membership: &historical_membership,
        projection_kind: request.projection_kind,
        activity_projection: &canonical_view,
    })
    .map_err(|_| HistoricalReplayErrorV1::ProjectionUnavailable)?;
    if let Some(budget) = payload_budget {
        budget
            .check_value(crate::PayloadKindV1::Projection, &canonical_envelope)
            .map_err(|_| HistoricalReplayErrorV1::ProjectionUnavailable)?;
    }
    Ok(HistoricalReplayProjectionV1 {
        verified_head: head.clone(),
        integrity: request.integrity,
        historical_room_status: core_state.room_status(),
        historical_membership,
        canonical_envelope,
    })
}

fn decode_replay_genesis(genesis_bytes: &[u8]) -> Result<GenesisV1, ReplayFailureV1> {
    GenesisV1::from_canonical_bytes(genesis_bytes).map_err(|error| {
        ReplayFailureV1::without_head(ReplayFailureClassV1::NonCanonicalRecord, error.to_string())
    })
}

fn validate_stored_genesis_integrity(genesis: &GenesisV1) -> Result<TimerBookV1, TraceErrorV1> {
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
        TimerBookV1::from_genesis(&genesis.initial_timers, genesis.created_at.as_str(), false)?;
    let core_hash = hash_core_state(&genesis.initial_core_state)?;
    let activity_hash = hash_activity_state(&genesis.pack_digest, &genesis.initial_activity_state)?;
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
    Ok(timers)
}

fn pack_genesis_request_from_record(genesis: &GenesisV1) -> PackGenesisRequestV1 {
    PackGenesisRequestV1 {
        room_id: genesis.room_id.clone(),
        pack_digest: genesis.pack_digest.clone(),
        configuration: genesis.configuration.clone(),
        room_seed: genesis.room_seed.clone(),
        created_at: genesis.created_at.clone(),
        initial_core_state: genesis.initial_core_state.clone(),
    }
}

fn genesis_input_from_record(genesis: &GenesisV1) -> GenesisInputV1 {
    GenesisInputV1::new(
        genesis.room_id.clone(),
        genesis.pack_digest.clone(),
        genesis.configuration.clone(),
        genesis.room_seed.clone(),
        genesis.created_at.clone(),
        genesis.initial_core_state.clone(),
        genesis.initial_activity_state.clone(),
    )
    .with_initial_timers(genesis.initial_timers.clone())
}

fn verify_retained_genesis_matches_record(
    genesis: &GenesisV1,
    verified: &VerifiedRetainedGenesisV1,
) -> Result<(), ReplayFailureV1> {
    if verified.genesis_input() != &genesis_input_from_record(genesis) {
        return Err(ReplayFailureV1::without_head(
            ReplayFailureClassV1::ActivityReduction,
            "retained pack initialization does not exactly reproduce stored Genesis input"
                .to_owned(),
        ));
    }
    Ok(())
}

fn map_retained_genesis_error(
    error: &PackGenesisErrorV1,
    preflight_head: CompleteHeadV1,
) -> ReplayFailureV1 {
    let class = match error {
        PackGenesisErrorV1::Registry(
            PackRegistryErrorV1::MissingRevision(_) | PackRegistryErrorV1::NotRunnable(_),
        ) => ReplayFailureClassV1::RuntimeUnavailable,
        PackGenesisErrorV1::Registry(_) => ReplayFailureClassV1::VersionOrDigest,
        PackGenesisErrorV1::Pack(error) if error.is_runtime_fault() => {
            ReplayFailureClassV1::RuntimeFault
        }
        PackGenesisErrorV1::Pack(_) => ReplayFailureClassV1::ActivityReduction,
    };
    ReplayFailureV1::with_head(class, error.to_string(), preflight_head)
}

fn preflight_stored_history(
    genesis: &GenesisV1,
    transition_bytes: &[Vec<u8>],
) -> Result<CompleteHeadV1, ReplayFailureV1> {
    let mut preflight = StoredHistoryPreflightV1::from_genesis(genesis)?;
    for bytes in transition_bytes {
        preflight.consume_transition(bytes)?;
    }
    Ok(preflight.head)
}

struct StoredHistoryPreflightV1 {
    head: CompleteHeadV1,
    core_state: CoreRoomStateV1,
    timers: TimerBookV1,
}

/// Bounded structural preflight for an exact persisted Room lineage.
///
/// Callers feed ordered Transition pages and may discard every page after
/// [`Self::consume_transition_page`] returns. It deliberately finishes this
/// complete canonical pass before a retained Pack runtime can be started.
pub struct StorageHistoryPreflightV1 {
    genesis: GenesisV1,
    preflight: StoredHistoryPreflightV1,
}

impl StorageHistoryPreflightV1 {
    /// Starts a structural preflight from exact canonical Genesis bytes.
    pub fn begin(genesis_bytes: &[u8]) -> Result<Self, ReplayFailureV1> {
        let genesis = decode_replay_genesis(genesis_bytes)?;
        let preflight = StoredHistoryPreflightV1::from_genesis(&genesis)?;
        Ok(Self { genesis, preflight })
    }

    /// Checks one ordered page without retaining its Transition bytes.
    pub fn consume_transition_page(
        &mut self,
        transition_bytes: &[Vec<u8>],
    ) -> Result<(), ReplayFailureV1> {
        for bytes in transition_bytes {
            self.preflight.consume_transition(bytes)?;
        }
        Ok(())
    }

    /// Returns the final structurally verified Head.
    #[must_use]
    pub const fn final_head(&self) -> &CompleteHeadV1 {
        &self.preflight.head
    }

    /// Starts the retained-Pack executable pass after all structural pages
    /// have been consumed. The resulting accumulator retains only current
    /// executor state and folds subsequent pages independently.
    pub fn begin_executable(
        self,
        expected_head: &CompleteHeadV1,
        registry: &PackRegistryV1,
    ) -> Result<StorageExecutableReplayV1, ReplayFailureV1> {
        if &self.preflight.head != expected_head {
            return Err(ReplayFailureV1::with_head(
                ReplayFailureClassV1::LineageHash,
                "structural preflight does not reach the captured Head".to_owned(),
                self.preflight.head,
            ));
        }
        let request = pack_genesis_request_from_record(&self.genesis);
        let verified = registry
            .prepare_genesis_for_retained_room(&request)
            .map_err(|error| map_retained_genesis_error(&error, self.preflight.head.clone()))?;
        verify_retained_genesis_matches_record(&self.genesis, &verified)?;
        let retained_pack = verified.retained_pack().clone();
        let transition_preparer = RoomTransitionPreparerV1::from_retained_pack(
            retained_pack.clone(),
            self.genesis.room_seed.clone(),
        );
        let trace = CoreTraceV1::from_verified_genesis(
            self.genesis,
            transition_preparer,
            Some(retained_pack),
        )
        .map_err(|error| {
            ReplayFailureV1::without_head(classify_genesis_error(&error), error.to_string())
        })?;
        Ok(StorageExecutableReplayV1 { trace })
    }
}

/// Bounded retained-Pack executable replay for an already preflighted Room.
pub struct StorageExecutableReplayV1 {
    trace: CoreTraceV1,
}

impl StorageExecutableReplayV1 {
    /// Folds one exact preflighted Transition page without retaining history.
    pub fn consume_transition_page(
        &mut self,
        transition_bytes: &[Vec<u8>],
    ) -> Result<(), ReplayFailureV1> {
        for bytes in transition_bytes {
            self.trace.replay_stored_transition(bytes, None)?;
            // `install_prepared_inner` retains a Transition for ordinary
            // executor history. Storage verification only needs the current
            // state, so release it before accepting the next record.
            self.trace.transitions.clear();
        }
        Ok(())
    }

    /// Compares the bounded replay result with exact captured serving bytes.
    pub fn finish(
        self,
        expected_head: &CompleteHeadV1,
        expected_core_state_bytes: &[u8],
        expected_activity_state_bytes: &[u8],
    ) -> Result<(), ReplayFailureV1> {
        if &self.trace.head != expected_head {
            return Err(ReplayFailureV1::with_head(
                ReplayFailureClassV1::LineageHash,
                "replayed final Head differs from captured Head".to_owned(),
                self.trace.head,
            ));
        }
        let replayed_core_state_bytes =
            self.trace.core_state.canonical_bytes().map_err(|error| {
                ReplayFailureV1::with_head(
                    ReplayFailureClassV1::CanonicalEncoding,
                    error.to_string(),
                    expected_head.clone(),
                )
            })?;
        if replayed_core_state_bytes != expected_core_state_bytes {
            return Err(ReplayFailureV1::with_head(
                ReplayFailureClassV1::CoreState,
                "replayed Core materialization differs from captured bytes".to_owned(),
                expected_head.clone(),
            ));
        }
        let replayed_activity_state_bytes =
            self.trace.activity_state.to_bytes().map_err(|error| {
                ReplayFailureV1::with_head(
                    ReplayFailureClassV1::CanonicalEncoding,
                    error.to_string(),
                    expected_head.clone(),
                )
            })?;
        if replayed_activity_state_bytes != expected_activity_state_bytes {
            return Err(ReplayFailureV1::with_head(
                ReplayFailureClassV1::ActivityState,
                "replayed Activity materialization differs from captured bytes".to_owned(),
                expected_head.clone(),
            ));
        }
        Ok(())
    }
}

impl StoredHistoryPreflightV1 {
    fn from_genesis(genesis: &GenesisV1) -> Result<Self, ReplayFailureV1> {
        let timers = validate_stored_genesis_integrity(genesis).map_err(|error| {
            ReplayFailureV1::without_head(classify_genesis_error(&error), error.to_string())
        })?;
        validate_core_state(&genesis.initial_core_state, &|_| Ok(())).map_err(|error| {
            let error = map_checked_core_error(error);
            ReplayFailureV1::without_head(classify_genesis_error(&error), error.to_string())
        })?;
        Ok(Self {
            head: genesis.head(),
            core_state: genesis.initial_core_state.clone(),
            timers,
        })
    }

    fn consume_transition(&mut self, bytes: &[u8]) -> Result<(), ReplayFailureV1> {
        let last_verified_head = self.head.clone();
        let stored = TransitionV1::from_canonical_bytes(bytes).map_err(|error| {
            ReplayFailureV1::with_head(
                ReplayFailureClassV1::NonCanonicalRecord,
                error.to_string(),
                last_verified_head.clone(),
            )
        })?;
        validate_transition_metadata(&self.head, &stored).map_err(|(class, detail)| {
            ReplayFailureV1::with_head(class, detail, last_verified_head.clone())
        })?;
        validate_stored_transition_hashes(&stored).map_err(|(class, detail)| {
            ReplayFailureV1::with_head(class, detail, last_verified_head.clone())
        })?;
        validate_preflight_core_transition(&self.head, &self.core_state, &self.timers, &stored)
            .map_err(|(class, detail)| {
                ReplayFailureV1::with_head(class, detail, last_verified_head.clone())
            })?;
        self.timers
            .apply_stored_transition(
                stored.recorded_stimulus(),
                stored.ordered_timer_changes(),
                stored.resulting_core_state().room_status(),
            )
            .map_err(|error| {
                ReplayFailureV1::with_head(
                    ReplayFailureClassV1::TimerChanges,
                    error.to_string(),
                    last_verified_head,
                )
            })?;
        self.core_state = stored.resulting_core_state.clone();
        self.head = stored.complete_head();
        Ok(())
    }
}

fn validate_preflight_core_transition(
    head: &CompleteHeadV1,
    core_before: &CoreRoomStateV1,
    timers: &TimerBookV1,
    transition: &TransitionV1,
) -> Result<(), (ReplayFailureClassV1, String)> {
    validate_core_state(transition.resulting_core_state(), &|_| Ok(())).map_err(|error| {
        (
            ReplayFailureClassV1::CoreInvariant,
            map_checked_core_error(error).to_string(),
        )
    })?;
    let derived = derive_preflight_core_transition(
        head,
        core_before,
        timers,
        transition.recorded_stimulus(),
    )?;
    if &derived != transition.resulting_core_state() {
        return Err((
            ReplayFailureClassV1::CoreState,
            "stored Core result differs from the host reducer".to_owned(),
        ));
    }
    Ok(())
}

fn derive_preflight_core_transition(
    head: &CompleteHeadV1,
    core_before: &CoreRoomStateV1,
    timers: &TimerBookV1,
    stimulus: &RecordedStimulusV1,
) -> Result<CoreRoomStateV1, (ReplayFailureClassV1, String)> {
    match stimulus {
        RecordedStimulusV1::ParticipantAction(action) => {
            let eligible =
                core_before
                    .memberships
                    .get(&action.member_id)
                    .is_some_and(|membership| {
                        membership.standing == MembershipStandingV1::Enabled
                            && membership.access_mode == AccessModeV1::Participant
                    });
            if action.exact_basis_head != *head
                || core_before.room_status != RoomStatusV1::Active
                || !eligible
            {
                return Err((
                    ReplayFailureClassV1::Stimulus,
                    "stored participant Action is invalid at its exact prefix".to_owned(),
                ));
            }
        }
        RecordedStimulusV1::TimerFired(fired) => {
            if core_before.room_status != RoomStatusV1::Active
                || timers.scheduled.get(&fired.timer_id)
                    != Some(&ScheduledTimerV1 {
                        timer_id: fired.timer_id.clone(),
                        generation: fired.generation,
                        scheduled_for: fired.scheduled_for.clone(),
                        canonical_payload: fired.canonical_payload.clone(),
                    })
            {
                return Err((
                    ReplayFailureClassV1::Stimulus,
                    "stored Timer firing is invalid at its exact prefix".to_owned(),
                ));
            }
        }
        RecordedStimulusV1::CoreProposed(proposal) => {
            if proposal.expected_room_seq != head.room_seq {
                return Err((
                    ReplayFailureClassV1::Stimulus,
                    "stored Core proposal expected a different prefix".to_owned(),
                ));
            }
            let proposed = propose_core(core_before, proposal, &|_| Ok(())).map_err(|error| {
                (
                    ReplayFailureClassV1::CoreInvariant,
                    map_checked_core_error(error).to_string(),
                )
            })?;
            if proposed.no_change {
                return Err((
                    ReplayFailureClassV1::CoreState,
                    "stored Transition contains a no-change Core proposal".to_owned(),
                ));
            }
            return Ok(proposed.state);
        }
        RecordedStimulusV1::ExternalInput(_) => {
            if core_before.room_status != RoomStatusV1::Active {
                return Err((
                    ReplayFailureClassV1::Stimulus,
                    "stored external input occurred after archive".to_owned(),
                ));
            }
        }
    }
    Ok(core_before.clone())
}

fn map_activity_pack_reduce_error(error: ActivityPackReduceErrorV1) -> TraceErrorV1 {
    match error {
        ActivityPackReduceErrorV1::Admission(error) => error.into(),
        ActivityPackReduceErrorV1::Pack(error) => error.into(),
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

/// Maximum distinct Timer identifiers admitted for a newly prepared Room.
///
/// The last-generation map needs one entry for every identifier ever used,
/// including fired and cancelled Timers. This cap therefore bounds that
/// serving-state component; it does not reinterpret historical V1 traces.
pub const MAX_DISTINCT_TIMER_IDS_V1: usize = 1_024;
/// Maximum distinct membership identities admitted to a newly prepared Room.
///
/// Stored V1 history is replayed independently of this current-admission
/// limit, allowing older Rooms to retain their exact forensic history.
pub const MAX_ROOM_MEMBERSHIPS_V1: usize = 1_024;

impl TimerBookV1 {
    fn from_checkpoint(
        rows: &[crate::RecoveredTimerMaterializationV1],
    ) -> Result<Self, TraceErrorV1> {
        let mut scheduled = BTreeMap::new();
        let mut last_generation = BTreeMap::new();
        for row in rows {
            if last_generation
                .insert(row.timer_id().clone(), row.generation())
                .is_some_and(|previous| previous >= row.generation())
            {
                return Err(TraceErrorV1::InvalidTimerNormalization);
            }
            if row.state() == crate::RecoveredTimerStateV1::Scheduled {
                let payload = CanonicalJsonV1::from_canonical_bytes(row.canonical_payload_bytes())
                    .map_err(|_| TraceErrorV1::InvalidTimerNormalization)?;
                if scheduled
                    .insert(
                        row.timer_id().clone(),
                        ScheduledTimerV1 {
                            timer_id: row.timer_id().clone(),
                            generation: row.generation(),
                            scheduled_for: row.scheduled_for().clone(),
                            canonical_payload: payload,
                        },
                    )
                    .is_some()
                {
                    return Err(TraceErrorV1::InvalidTimerNormalization);
                }
            }
        }
        Ok(Self {
            scheduled,
            last_generation,
        })
    }

    fn from_genesis(
        initial: &[ScheduledTimerV1],
        created_at: &str,
        enforce_distinct_timer_id_cap: bool,
    ) -> Result<Self, TraceErrorV1> {
        if initial
            .windows(2)
            .any(|pair| pair[0].timer_id >= pair[1].timer_id)
        {
            return Err(TraceErrorV1::TimerChangesNotStrictlySorted);
        }
        if enforce_distinct_timer_id_cap && initial.len() > MAX_DISTINCT_TIMER_IDS_V1 {
            return Err(TraceErrorV1::DistinctTimerIdLimitExceeded);
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
                    if !next.admits_timer_id(&timer_id) {
                        return Err(TraceErrorV1::DistinctTimerIdLimitExceeded);
                    }
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

    fn apply_stored_transition(
        &mut self,
        stimulus: &RecordedStimulusV1,
        changes: &[TimerChangeV1],
        resulting_room_status: RoomStatusV1,
    ) -> Result<(), TraceErrorV1> {
        if changes
            .windows(2)
            .any(|pair| timer_change_id(&pair[0]) >= timer_change_id(&pair[1]))
        {
            return Err(TraceErrorV1::TimerChangesNotStrictlySorted);
        }

        if resulting_room_status == RoomStatusV1::Archived {
            if changes != self.archive_cancellations() {
                return Err(TraceErrorV1::InvalidTimerNormalization);
            }
            self.scheduled.clear();
            return Ok(());
        }

        if let RecordedStimulusV1::TimerFired(fired) = stimulus {
            let expected = ScheduledTimerV1 {
                timer_id: fired.timer_id.clone(),
                generation: fired.generation,
                scheduled_for: fired.scheduled_for.clone(),
                canonical_payload: fired.canonical_payload.clone(),
            };
            if self.scheduled.get(&fired.timer_id) != Some(&expected) {
                return Err(TraceErrorV1::InvalidTimerNormalization);
            }
            self.scheduled.remove(&fired.timer_id);
        }

        for change in changes {
            match change {
                TimerChangeV1::Schedule {
                    timer_id,
                    generation,
                    scheduled_for,
                    canonical_payload,
                } => {
                    if *generation != self.successor(timer_id)?
                        || self.scheduled.contains_key(timer_id)
                        || compare_timestamp_text(scheduled_for.as_str(), stimulus.semantic_time())
                            != Ordering::Greater
                    {
                        return Err(TraceErrorV1::InvalidTimerNormalization);
                    }
                    self.last_generation.insert(timer_id.clone(), *generation);
                    self.scheduled.insert(
                        timer_id.clone(),
                        ScheduledTimerV1 {
                            timer_id: timer_id.clone(),
                            generation: *generation,
                            scheduled_for: scheduled_for.clone(),
                            canonical_payload: canonical_payload.clone(),
                        },
                    );
                }
                TimerChangeV1::Cancel {
                    timer_id,
                    generation,
                } => {
                    if self.scheduled.get(timer_id).map(|timer| timer.generation)
                        != Some(*generation)
                    {
                        return Err(TraceErrorV1::InvalidTimerNormalization);
                    }
                    self.scheduled.remove(timer_id);
                }
                TimerChangeV1::Reschedule {
                    timer_id,
                    previous_generation,
                    generation,
                    scheduled_for,
                    canonical_payload,
                } => {
                    if self.scheduled.get(timer_id).map(|timer| timer.generation)
                        != Some(*previous_generation)
                        || *generation != self.successor(timer_id)?
                        || compare_timestamp_text(scheduled_for.as_str(), stimulus.semantic_time())
                            != Ordering::Greater
                    {
                        return Err(TraceErrorV1::InvalidTimerNormalization);
                    }
                    self.last_generation.insert(timer_id.clone(), *generation);
                    self.scheduled.insert(
                        timer_id.clone(),
                        ScheduledTimerV1 {
                            timer_id: timer_id.clone(),
                            generation: *generation,
                            scheduled_for: scheduled_for.clone(),
                            canonical_payload: canonical_payload.clone(),
                        },
                    );
                }
            }
        }
        Ok(())
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

    fn admits_timer_id(&self, timer_id: &TimerId) -> bool {
        self.last_generation.contains_key(timer_id)
            || self.last_generation.len() < MAX_DISTINCT_TIMER_IDS_V1
    }
}

#[cfg(test)]
mod timer_book_tests {
    use super::*;

    #[test]
    fn distinct_timer_id_cap_allows_reuse_and_rejects_one_more() {
        let generation =
            TimerGenerationV1::new(1).unwrap_or_else(|error| panic!("timer generation: {error}"));
        let last_generation = (1..=MAX_DISTINCT_TIMER_IDS_V1)
            .map(|value| {
                let timer_id = format!("{value:026}")
                    .parse::<TimerId>()
                    .unwrap_or_else(|error| panic!("timer id {value}: {error}"));
                (timer_id, generation)
            })
            .collect::<BTreeMap<_, _>>();
        let book = TimerBookV1 {
            scheduled: BTreeMap::new(),
            last_generation,
        };
        let existing = "00000000000000000000000001"
            .parse::<TimerId>()
            .unwrap_or_else(|error| panic!("existing timer id: {error}"));
        let one_more = format!("{:026}", MAX_DISTINCT_TIMER_IDS_V1 + 1)
            .parse::<TimerId>()
            .unwrap_or_else(|error| panic!("new timer id: {error}"));
        assert!(book.admits_timer_id(&existing));
        assert!(!book.admits_timer_id(&one_more));
    }
}

fn timer_change_id(change: &TimerChangeV1) -> &TimerId {
    match change {
        TimerChangeV1::Schedule { timer_id, .. }
        | TimerChangeV1::Cancel { timer_id, .. }
        | TimerChangeV1::Reschedule { timer_id, .. } => timer_id,
    }
}

pub(crate) fn hash_administration_request(
    basis_complete_head: &CompleteHeadV1,
    proposal: &CoreProposedV1,
) -> Result<Blake3DigestV1, CanonicalJsonError> {
    let request = crate::CoreAdministrationRequestV1::from_proposal(
        basis_complete_head.room_id().clone(),
        proposal,
    );
    Ok(request.canonical_request_hash()?.digest().clone())
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
/// before consuming it through the storage-backed commit coordinator.
pub struct PreparedRoomTransitionV1 {
    preparer_identity: Arc<()>,
    basis_complete_head: CompleteHeadV1,
    recorded_stimulus: RecordedStimulusV1,
    #[cfg_attr(not(any(test, feature = "conformance-tracer")), allow(dead_code))]
    action_offer_witness: Option<Vec<u8>>,
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
    pub(crate) fn basis_complete_head(&self) -> &CompleteHeadV1 {
        &self.basis_complete_head
    }

    /// Returns the exact normalized Stimulus used during pure preparation.
    #[must_use]
    pub(crate) fn recorded_stimulus(&self) -> &RecordedStimulusV1 {
        &self.recorded_stimulus
    }

    /// Returns the exact current canonical Action Offer list used for
    /// participant pre-admission and reduction.
    #[must_use]
    pub(crate) fn action_offer_witness(&self) -> Option<&[u8]> {
        self.action_offer_witness.as_deref()
    }

    /// Returns the exact Transition or durable no-Transition disposition to
    /// persist. The value cannot be mutated or constructed outside Core.
    #[must_use]
    pub(crate) fn disposition(&self) -> &AdvanceDispositionV1 {
        &self.outcome
    }

    /// Reports whether this preparation produced a new value to commit rather
    /// than resolving an existing administration result.
    #[must_use]
    pub(crate) const fn is_new(&self) -> bool {
        self.is_new
    }

    /// Returns the verified post-disposition state for a newly prepared value.
    /// It equals the basis for Rejection/NoChange and advances for Apply.
    #[must_use]
    pub(crate) fn resulting_state(&self) -> Option<&RoomTransitionStateV1> {
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
    /// Any frozen Activity Pack operation panicked under the checked host.
    #[error("Activity Pack {0:?} operation panicked")]
    OperationPanicked(crate::ActivityPackOperationV1),
    /// A frozen Activity Pack operation returned a fail-closed fault.
    #[error("Activity Pack {operation:?} operation faulted: {fault}")]
    OperationFault {
        operation: crate::ActivityPackOperationV1,
        fault: Box<Self>,
    },
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
    /// A canonical value did not satisfy its exact declared schema.
    #[error("Activity Pack schema violation: {0}")]
    SchemaViolation(String),
    /// A canonical output exceeded a descriptor hard bound.
    #[error("Activity Pack output bound exceeded: {0}")]
    OutputBoundExceeded(String),
    /// A clean rejection code was not declared by the exact descriptor.
    #[error("Activity Pack emitted undeclared rejection code: {0}")]
    UndeclaredRejectionCode(String),
    /// Action payload digest did not equal the descriptor/offer schema digest.
    #[error("Activity Action payload schema digest mismatch")]
    ActionPayloadSchemaMismatch,
    /// A view/observation contradicted viewer authorization or exact offer
    /// reuse.
    #[error("Activity Pack privacy contract failure: {0}")]
    PrivacyContract(String),
    /// Descriptor-declared ordering or output shape was malformed.
    #[error("invalid Activity Pack output: {0}")]
    InvalidOutput(String),
}

impl PackFaultV1 {
    const fn is_runtime_fault(&self) -> bool {
        matches!(
            self,
            Self::OperationPanicked(_) | Self::OperationFault { .. } | Self::RoleValidatorPanicked
        )
    }
}

/// Live tracing failure. No partial state or Transition is installed.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum TraceErrorV1 {
    /// Canonical codec failure.
    #[error(transparent)]
    Canonical(#[from] CanonicalJsonError),
    /// Unresolved fresh caller input exceeds its immutable Room policy.
    #[error(transparent)]
    PayloadBudget(#[from] crate::PayloadBudgetErrorV1),
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
    /// Participant Action was not admitted by the exact current checked view.
    #[error(transparent)]
    ActionAdmission(#[from] ActionAdmissionErrorV1),
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
    /// A new Timer identifier would exceed the fixed serving-state budget.
    #[error("distinct Timer identifier limit exceeded")]
    DistinctTimerIdLimitExceeded,
    /// A new membership identity would exceed the fixed serving-state budget.
    #[error("Room membership identity limit exceeded")]
    RoomMembershipLimitExceeded,
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
    /// A no-Transition disposition cannot produce a transition observation.
    #[error("prepared disposition has no Transition to observe")]
    PreparedAdvanceHasNoTransition,
    /// Raw conformance traces have no retained pack observation surface.
    #[error("trace has no retained Activity Pack binding")]
    RetainedPackUnavailable,
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
#[derive(Clone, Eq, PartialEq)]
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

impl fmt::Debug for ReplayStepV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ReplayStepV1([REDACTED])")
    }
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

/// Exact addressed observation consequence reproduced during retained-Pack replay.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReplayObservationFrameV1 {
    member_id: crate::MemberId,
    frame_seq: u64,
    cause_room_seq: RoomSequenceV1,
    payload_hash: Blake3DigestV1,
}

/// Exact complete delivery consequence reproduced during retained-Pack
/// replay. Reset and visibility consequences intentionally retain only the
/// non-secret witnesses required by storage recovery.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ReplayObservationConsequenceV1 {
    ObservationFrame(ReplayObservationFrameV1),
    ResetRequired {
        member_id: crate::MemberId,
        cause_room_seq: RoomSequenceV1,
        projection_hash: Blake3DigestV1,
    },
    VisibilityLost {
        member_id: crate::MemberId,
        cause_room_seq: RoomSequenceV1,
    },
}

impl ReplayObservationFrameV1 {
    #[must_use]
    pub(crate) const fn member_id(&self) -> &crate::MemberId {
        &self.member_id
    }

    #[must_use]
    pub(crate) const fn frame_seq(&self) -> u64 {
        self.frame_seq
    }

    #[must_use]
    pub(crate) const fn cause_room_seq(&self) -> RoomSequenceV1 {
        self.cause_room_seq
    }

    #[must_use]
    pub(crate) const fn payload_hash(&self) -> &Blake3DigestV1 {
        &self.payload_hash
    }
}

/// Maximum references returned by one external historical-evidence page.
pub const MAX_HISTORICAL_EVIDENCE_ROWS_PER_PAGE_V1: usize = 128;
/// Maximum encoded reference bytes returned by one external evidence page.
pub const MAX_HISTORICAL_EVIDENCE_BYTES_PER_PAGE_V1: usize = 256 * 1024;
/// Maximum wall-clock capture time for one external evidence page.
pub const MAX_HISTORICAL_EVIDENCE_TIME_MS_V1: u64 = 250;

/// Storage result for one bounded evidence page.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HistoricalEvidencePageOutcomeV1 {
    Complete,
    Exhausted,
    Missing,
    Pruned,
    Retired,
}

/// One durable, privacy-safe pointer into retained historical evidence.
///
/// The pointer contains lineage metadata and an addressable reference only;
/// canonical Transition/Genesis bytes and model summaries deliberately never
/// cross this boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoricalEvidenceReferenceV1 {
    room_seq: u64,
    transition_id: String,
    transition_hash: String,
    previous_lineage_hash: String,
    evidence_reference: String,
}

impl HistoricalEvidenceReferenceV1 {
    /// Constructs one bounded reference from verified retained metadata.
    #[must_use]
    pub fn new(
        room_seq: u64,
        transition_id: String,
        transition_hash: String,
        previous_lineage_hash: String,
        evidence_reference: String,
    ) -> Option<Self> {
        if room_seq == 0
            || transition_id.is_empty()
            || transition_hash.is_empty()
            || previous_lineage_hash.is_empty()
            || evidence_reference.is_empty()
            || transition_id.len() > 128
            || transition_hash.len() > 128
            || previous_lineage_hash.len() > 128
            || evidence_reference.len() > 512
            || !transition_id.bytes().all(|byte| byte.is_ascii_graphic())
            || !transition_hash.bytes().all(|byte| byte.is_ascii_graphic())
            || !previous_lineage_hash
                .bytes()
                .all(|byte| byte.is_ascii_graphic())
            || !evidence_reference
                .bytes()
                .all(|byte| byte.is_ascii_graphic())
        {
            return None;
        }
        Some(Self {
            room_seq,
            transition_id,
            transition_hash,
            previous_lineage_hash,
            evidence_reference,
        })
    }

    #[must_use]
    pub const fn room_seq(&self) -> u64 {
        self.room_seq
    }

    #[must_use]
    pub fn transition_id(&self) -> &str {
        &self.transition_id
    }

    #[must_use]
    pub fn transition_hash(&self) -> &str {
        &self.transition_hash
    }

    #[must_use]
    pub fn previous_lineage_hash(&self) -> &str {
        &self.previous_lineage_hash
    }

    #[must_use]
    pub fn evidence_reference(&self) -> &str {
        &self.evidence_reference
    }

    /// Encoded size used by storage adapters for the page byte fence.
    #[must_use]
    pub fn encoded_bytes(&self) -> usize {
        self.transition_id.len()
            + self.transition_hash.len()
            + self.previous_lineage_hash.len()
            + self.evidence_reference.len()
            + std::mem::size_of::<u64>()
    }
}

/// Pure historical-projection address supplied by a trusted storage Adapter.
///
/// Constructing this value does not establish present authority. The public
/// durable facade consumes [`crate::AuthorizedReplayV1`] before constructing
/// it, while this type keeps the deterministic history verifier independently
/// testable.
pub struct HistoricalReplayProjectionRequestV1 {
    room_id: crate::RoomId,
    member_id: MemberId,
    at_room_seq: RoomSequenceV1,
    projection_kind: ReplayProjectionKindV1,
    integrity: RoomIntegrityStateV1,
}

/// Opaque incremental verifier for one exact historical projection.
///
/// The accumulator folds canonical Transition pages into only the current
/// verified Core/Activity/Timer state. It never retains input pages, Replay
/// steps, observation Frames, receipts, or effect capabilities. Pack/runtime
/// failures are held until the complete prefix has independently passed the
/// host-owned lineage/Core/Timer preflight, so runtime unavailability cannot
/// mask later canonical corruption.
#[must_use]
pub struct HistoricalReplayAccumulatorV1 {
    request: HistoricalReplayProjectionRequestV1,
    preflight: StoredHistoryPreflightV1,
    semantic: HistoricalReplaySemanticStateV1,
}

enum HistoricalReplaySemanticStateV1 {
    Active(Box<CoreTraceV1>),
    Failed(ReplayFailureClassV1),
}

impl fmt::Debug for HistoricalReplayAccumulatorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HistoricalReplayAccumulatorV1")
            .field("room_id", &self.request.room_id)
            .field("at_room_seq", &self.request.at_room_seq)
            .field("verified_through", &self.preflight.head.room_seq())
            .field("semantic", &"[REDACTED]")
            .finish()
    }
}

impl HistoricalReplayAccumulatorV1 {
    /// Initializes bounded historical Replay from exact canonical Genesis.
    ///
    /// Registry/runtime failures are retained rather than returned until all
    /// requested Transition pages have passed pack-independent preflight.
    ///
    /// # Errors
    ///
    /// Returns a closed historical Replay failure when Genesis is malformed,
    /// does not address the request, or the Room is quarantined.
    pub fn begin(
        registry: &PackRegistryV1,
        canonical_genesis_bytes: &[u8],
        request: HistoricalReplayProjectionRequestV1,
    ) -> Result<Self, HistoricalReplayErrorV1> {
        if request.integrity.status() == RoomIntegrityStatusV1::Quarantined {
            return Err(HistoricalReplayErrorV1::IntegrityUnavailable);
        }
        let genesis = decode_replay_genesis(canonical_genesis_bytes)
            .map_err(|failure| HistoricalReplayErrorV1::ReplayFailed(failure.class))?;
        let preflight = StoredHistoryPreflightV1::from_genesis(&genesis)
            .map_err(|failure| HistoricalReplayErrorV1::ReplayFailed(failure.class))?;
        if preflight.head.room_id() != &request.room_id {
            return Err(HistoricalReplayErrorV1::AddressMismatch);
        }

        let semantic = match registry
            .prepare_genesis_for_retained_room(&pack_genesis_request_from_record(&genesis))
        {
            Ok(verified) => match verify_retained_genesis_matches_record(&genesis, &verified) {
                Ok(()) => {
                    let retained_pack = verified.retained_pack().clone();
                    let preparer = RoomTransitionPreparerV1::from_retained_pack(
                        retained_pack.clone(),
                        genesis.room_seed.clone(),
                    );
                    match CoreTraceV1::from_verified_genesis(genesis, preparer, Some(retained_pack))
                    {
                        Ok(trace) => HistoricalReplaySemanticStateV1::Active(Box::new(trace)),
                        Err(error) => {
                            HistoricalReplaySemanticStateV1::Failed(classify_genesis_error(&error))
                        }
                    }
                }
                Err(failure) => HistoricalReplaySemanticStateV1::Failed(failure.class),
            },
            Err(error) => HistoricalReplaySemanticStateV1::Failed(
                map_retained_genesis_error(&error, preflight.head.clone()).class,
            ),
        };
        Ok(Self {
            request,
            preflight,
            semantic,
        })
    }

    /// Folds one bounded, strictly ordered Transition page.
    ///
    /// The page is borrowed only for this call and is not retained.
    ///
    /// # Errors
    ///
    /// Returns the first canonical/lineage/Core/Timer disagreement in this
    /// page, or a sequence error if the page extends beyond the requested
    /// historical prefix.
    pub fn consume_page(
        &mut self,
        canonical_transition_bytes: &[Vec<u8>],
    ) -> Result<(), HistoricalReplayErrorV1> {
        for bytes in canonical_transition_bytes {
            if self.preflight.head.room_seq().get() >= self.request.at_room_seq.get() {
                return Err(HistoricalReplayErrorV1::SequenceUnavailable);
            }
            self.preflight
                .consume_transition(bytes)
                .map_err(|failure| HistoricalReplayErrorV1::ReplayFailed(failure.class))?;
            if let HistoricalReplaySemanticStateV1::Active(trace) = &mut self.semantic {
                if let Err(failure) = trace.replay_stored_transition(bytes, None) {
                    self.semantic = HistoricalReplaySemanticStateV1::Failed(failure.class);
                } else {
                    // Historical projection needs only the current verified
                    // state. Durable operation receipts own idempotency, and
                    // the Adapter validates their one-to-one Transition rows.
                    trace.transitions.clear();
                    trace.administration_results.clear();
                }
            }
        }
        Ok(())
    }

    /// Returns whether the exact requested sequence has been preflighted.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.preflight.head.room_seq() == self.request.at_room_seq
    }

    /// Finishes the projection after the exact requested prefix was folded.
    ///
    /// # Errors
    ///
    /// Returns a closed sequence, retained-runtime, deterministic Replay, or
    /// historical-view failure. No projection is returned from partial state.
    pub fn finish(self) -> Result<HistoricalReplayProjectionV1, HistoricalReplayErrorV1> {
        if !self.is_complete() {
            return Err(HistoricalReplayErrorV1::SequenceUnavailable);
        }
        match self.semantic {
            HistoricalReplaySemanticStateV1::Active(trace) => {
                CoreTraceV1::historical_projection_from_trace(&trace, self.request)
            }
            HistoricalReplaySemanticStateV1::Failed(class) => {
                Err(HistoricalReplayErrorV1::ReplayFailed(class))
            }
        }
    }
}

impl fmt::Debug for HistoricalReplayProjectionRequestV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HistoricalReplayProjectionRequestV1")
            .field("room_id", &self.room_id)
            .field("member_id", &self.member_id)
            .field("at_room_seq", &self.at_room_seq)
            .field("projection_kind", &self.projection_kind)
            .field("integrity", &self.integrity)
            .finish()
    }
}

impl HistoricalReplayProjectionRequestV1 {
    /// Constructs a pure historical projection address. Present authority and
    /// the integrity generation must be fenced by the calling Adapter.
    #[must_use]
    pub const fn new(
        room_id: crate::RoomId,
        member_id: MemberId,
        at_room_seq: RoomSequenceV1,
        projection_kind: ReplayProjectionKindV1,
        integrity: RoomIntegrityStateV1,
    ) -> Self {
        Self {
            room_id,
            member_id,
            at_room_seq,
            projection_kind,
            integrity,
        }
    }
}

#[derive(Serialize)]
struct HistoricalReplayEnvelopeV1<'a> {
    envelope: &'static str,
    verified_head: &'a CompleteHeadV1,
    integrity: &'a RoomIntegrityStateV1,
    room_status: RoomStatusV1,
    membership: &'a crate::MembershipV1,
    projection_kind: ReplayProjectionKindV1,
    activity_projection: &'a CanonicalJsonV1,
}

/// Verified projection of one caller-supplied immutable historical prefix.
///
/// The canonical envelope collision-proofly binds the exact verified Head,
/// operational integrity status/generation, historical Membership metadata,
/// and host-validated Activity projection. It does not claim present
/// authorization; the durable Adapter wraps it only after a current authority
/// fence. It never contains canonical Activity
/// State, lineage bytes, receipts, bearer facts, or an executable continuation.
pub struct HistoricalReplayProjectionV1 {
    verified_head: CompleteHeadV1,
    integrity: RoomIntegrityStateV1,
    historical_room_status: RoomStatusV1,
    historical_membership: crate::MembershipV1,
    canonical_envelope: CanonicalJsonV1,
}

impl fmt::Debug for HistoricalReplayProjectionV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HistoricalReplayProjectionV1")
            .field("verified_head", &self.verified_head)
            .field("integrity", &self.integrity)
            .field("historical_room_status", &self.historical_room_status)
            .field(
                "historical_member_id",
                self.historical_membership.member_id(),
            )
            .field("canonical_envelope", &"[REDACTED]")
            .finish()
    }
}

impl HistoricalReplayProjectionV1 {
    /// Returns the exact historical Head verified by Replay.
    #[must_use]
    pub const fn verified_head(&self) -> &CompleteHeadV1 {
        &self.verified_head
    }

    /// Returns the exact operational integrity state fenced during capture.
    #[must_use]
    pub const fn integrity(&self) -> &RoomIntegrityStateV1 {
        &self.integrity
    }

    /// Returns the historical Core Room lifecycle status.
    #[must_use]
    pub const fn historical_room_status(&self) -> RoomStatusV1 {
        self.historical_room_status
    }

    /// Returns the exact Membership metadata reconstructed at the requested
    /// historical sequence.
    #[must_use]
    pub const fn historical_membership(&self) -> &crate::MembershipV1 {
        &self.historical_membership
    }

    /// Returns the historically verified, collision-proof Core/Membership/Activity
    /// projection envelope.
    #[must_use]
    pub const fn canonical_envelope(&self) -> &CanonicalJsonV1 {
        &self.canonical_envelope
    }
}

/// Closed public failure for pure historical projection verification.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum HistoricalReplayErrorV1 {
    #[error("historical projection address does not match reconstructed lineage")]
    AddressMismatch,
    #[error("historical projection sequence is unavailable")]
    SequenceUnavailable,
    #[error("historical Membership is absent or not enabled at that sequence")]
    HistoricalMembershipUnavailable,
    #[error("Room integrity does not permit historical projection")]
    IntegrityUnavailable,
    #[error("exact historical Replay failed: {0:?}")]
    ReplayFailed(ReplayFailureClassV1),
    #[error("historical projection is unavailable")]
    ProjectionUnavailable,
}

/// Successful no-effect replay report with every prefix Head.
pub struct ReplayReportV1 {
    /// Final exact Head.
    pub final_head: CompleteHeadV1,
    final_state: RoomTransitionStateV1,
    continuation_preparer: RoomTransitionPreparerV1,
    continuation_trace: CoreTraceV1,
    observation_consequences: Vec<ReplayObservationConsequenceV1>,
    /// Sequence-zero plus every accepted Transition prefix.
    pub steps: Vec<ReplayStepV1>,
    /// Activity reduction callback count; exactly one per replayed Transition.
    pub activity_callback_count: usize,
    /// Always zero: replay owns no effect capability.
    pub external_effect_count: usize,
    /// Always zero: replay creates no receipt.
    pub receipt_count: usize,
}

/// Exact Membership materialization regenerated by retained-Pack replay.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplayMembershipWitnessV1 {
    member_id: MemberId,
    canonical_membership_bytes: Vec<u8>,
}

impl ReplayMembershipWitnessV1 {
    #[must_use]
    pub const fn member_id(&self) -> &MemberId {
        &self.member_id
    }

    #[must_use]
    pub fn canonical_membership_bytes(&self) -> &[u8] {
        &self.canonical_membership_bytes
    }
}

/// Exact Activation decision regenerated from one retained Transition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplayActivationDecisionWitnessV1 {
    cause_room_seq: RoomSequenceV1,
    decision_id: String,
    target_member_id: Option<MemberId>,
    canonical_decision_bytes: Vec<u8>,
}

impl ReplayActivationDecisionWitnessV1 {
    #[must_use]
    pub const fn cause_room_seq(&self) -> RoomSequenceV1 {
        self.cause_room_seq
    }

    #[must_use]
    pub fn decision_id(&self) -> &str {
        &self.decision_id
    }

    #[must_use]
    pub const fn target_member_id(&self) -> Option<&MemberId> {
        self.target_member_id.as_ref()
    }

    #[must_use]
    pub fn canonical_decision_bytes(&self) -> &[u8] {
        &self.canonical_decision_bytes
    }
}

/// Replay-derived delivery head and Core-required reset marker for one Member.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplayObservationPositionWitnessV1 {
    member_id: MemberId,
    frame_head: u64,
    reset_required_through: Option<u64>,
}

impl ReplayObservationPositionWitnessV1 {
    #[must_use]
    pub const fn member_id(&self) -> &MemberId {
        &self.member_id
    }

    #[must_use]
    pub const fn frame_head(&self) -> u64 {
        self.frame_head
    }

    #[must_use]
    pub const fn reset_required_through(&self) -> Option<u64> {
        self.reset_required_through
    }
}

/// Semantic storage witnesses regenerated by exact retained-Pack replay.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplayStorageVerificationV1 {
    memberships: Vec<ReplayMembershipWitnessV1>,
    timers: Vec<crate::RecoveredTimerMaterializationV1>,
    activation_decisions: Vec<ReplayActivationDecisionWitnessV1>,
    observation_positions: Vec<ReplayObservationPositionWitnessV1>,
    observation_frames: Vec<crate::RecoveredObservationFrameV1>,
    observation_consequences: Vec<crate::RecoveredObservationConsequenceV1>,
}

fn replay_activation_decision_witnesses(
    transition_bytes: &[Vec<u8>],
) -> Result<Vec<ReplayActivationDecisionWitnessV1>, String> {
    let mut activation_decisions = Vec::new();
    for bytes in transition_bytes {
        let transition =
            TransitionV1::from_canonical_bytes(bytes).map_err(|error| error.to_string())?;
        for signal in transition.ordered_attention_signals() {
            let decision =
                crate::PreparedActivationDecisionV1::from_attention(signal, transition.room_seq())
                    .map_err(|error| error.to_string())?;
            activation_decisions.push(ReplayActivationDecisionWitnessV1 {
                cause_room_seq: transition.room_seq(),
                decision_id: decision.decision_id().to_owned(),
                target_member_id: decision.target_member_id().cloned(),
                canonical_decision_bytes: decision.canonical_decision_bytes().to_vec(),
            });
        }
    }
    Ok(activation_decisions)
}

fn replay_observation_storage_witnesses(
    consequences: &[ReplayObservationConsequenceV1],
    memberships: &[ReplayMembershipWitnessV1],
) -> (
    Vec<ReplayObservationPositionWitnessV1>,
    Vec<crate::RecoveredObservationFrameV1>,
    Vec<crate::RecoveredObservationConsequenceV1>,
) {
    let final_members = memberships
        .iter()
        .map(|membership| membership.member_id.clone())
        .collect::<std::collections::BTreeSet<_>>();
    let mut replay_positions = BTreeMap::<MemberId, (u64, Option<u64>)>::new();
    let mut observation_frames = Vec::new();
    let mut observation_consequences = Vec::new();
    for consequence in consequences {
        match consequence {
            ReplayObservationConsequenceV1::ObservationFrame(frame) => {
                replay_positions
                    .entry(frame.member_id().clone())
                    .or_insert((0, None))
                    .0 = frame.frame_seq();
                observation_frames.push(crate::RecoveredObservationFrameV1::from_replay(
                    frame.member_id().clone(),
                    frame.frame_seq(),
                    frame.cause_room_seq(),
                    frame.payload_hash().clone(),
                ));
            }
            ReplayObservationConsequenceV1::ResetRequired {
                member_id,
                cause_room_seq,
                projection_hash,
            } => {
                let position = replay_positions
                    .entry(member_id.clone())
                    .or_insert((0, None));
                position.1 = Some(position.1.map_or(position.0, |prior| prior.max(position.0)));
                observation_consequences.push(
                    crate::RecoveredObservationConsequenceV1::from_replay_reset(
                        member_id.clone(),
                        *cause_room_seq,
                        projection_hash.clone(),
                    ),
                );
            }
            ReplayObservationConsequenceV1::VisibilityLost {
                member_id,
                cause_room_seq,
            } => {
                let position = replay_positions
                    .entry(member_id.clone())
                    .or_insert((0, None));
                position.1 = Some(position.1.map_or(position.0, |prior| prior.max(position.0)));
                observation_consequences.push(
                    crate::RecoveredObservationConsequenceV1::from_replay_visibility_lost(
                        member_id.clone(),
                        *cause_room_seq,
                    ),
                );
            }
        }
    }
    let observation_positions = final_members
        .into_iter()
        .map(|member_id| {
            let (frame_head, reset_required_through) =
                replay_positions.remove(&member_id).unwrap_or((0, None));
            ReplayObservationPositionWitnessV1 {
                member_id,
                frame_head,
                reset_required_through,
            }
        })
        .collect();
    (
        observation_positions,
        observation_frames,
        observation_consequences,
    )
}

impl ReplayStorageVerificationV1 {
    fn from_report(
        report: &ReplayReportV1,
        genesis_bytes: &[u8],
        transition_bytes: &[Vec<u8>],
    ) -> Result<Self, String> {
        let memberships = report
            .final_state()
            .core_state()
            .memberships()
            .values()
            .map(|membership| {
                Ok(ReplayMembershipWitnessV1 {
                    member_id: membership.member_id().clone(),
                    canonical_membership_bytes: encode(membership)
                        .map_err(|error| error.to_string())?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let timers = crate::room_commit::recover_timer_ledger(genesis_bytes, transition_bytes)
            .map_err(|error| error.to_string())?;
        let activation_decisions = replay_activation_decision_witnesses(transition_bytes)?;
        let (observation_positions, observation_frames, observation_consequences) =
            replay_observation_storage_witnesses(report.observation_consequences(), &memberships);
        Ok(Self {
            memberships,
            timers,
            activation_decisions,
            observation_positions,
            observation_frames,
            observation_consequences,
        })
    }

    #[must_use]
    pub fn memberships(&self) -> &[ReplayMembershipWitnessV1] {
        &self.memberships
    }

    #[must_use]
    pub fn timers(&self) -> &[crate::RecoveredTimerMaterializationV1] {
        &self.timers
    }

    #[must_use]
    pub fn activation_decisions(&self) -> &[ReplayActivationDecisionWitnessV1] {
        &self.activation_decisions
    }

    #[must_use]
    pub fn observation_positions(&self) -> &[ReplayObservationPositionWitnessV1] {
        &self.observation_positions
    }

    /// Returns exact addressed frame integrity witnesses in replay order.
    #[must_use]
    pub fn observation_frames(&self) -> &[crate::RecoveredObservationFrameV1] {
        &self.observation_frames
    }

    /// Returns exact non-frame delivery witnesses in replay order.
    #[must_use]
    pub fn observation_consequences(&self) -> &[crate::RecoveredObservationConsequenceV1] {
        &self.observation_consequences
    }

    /// Constructs a witness set for adapter conformance tests only.
    #[doc(hidden)]
    #[cfg(feature = "conformance-tracer")]
    #[must_use]
    pub fn from_observation_witnesses_for_conformance(
        observation_frames: Vec<crate::RecoveredObservationFrameV1>,
        observation_consequences: Vec<crate::RecoveredObservationConsequenceV1>,
    ) -> Self {
        Self {
            memberships: Vec::new(),
            timers: Vec::new(),
            activation_decisions: Vec::new(),
            observation_positions: Vec::new(),
            observation_frames,
            observation_consequences,
        }
    }
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

    /// Returns the exact retained Pack revision lock selected from replayed
    /// Genesis, when replay used the production registry-bound path.
    #[must_use]
    pub fn retained_pack_revision_lock(&self) -> Option<&crate::PackRevisionLockV1> {
        self.continuation_trace
            .retained_pack()
            .map(crate::RetainedActivityPackV1::revision_lock)
    }

    /// Returns exact addressed observation consequences reproduced from the
    /// retained Pack without publishing them.
    #[must_use]
    pub(crate) fn observation_consequences(&self) -> &[ReplayObservationConsequenceV1] {
        &self.observation_consequences
    }

    /// Consumes the report and returns the recovered current Room executor.
    /// Subsequent preparation therefore still routes through the owning trace
    /// and cannot be invoked with a retained stale snapshot.
    #[must_use]
    #[cfg(any(test, feature = "conformance-tracer"))]
    pub fn into_trace(self) -> CoreTraceV1 {
        self.continuation_trace
    }

    pub(crate) fn into_trace_after_recovery_fence(self) -> CoreTraceV1 {
        self.continuation_trace
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
    /// The exact retained Pack revision is currently absent or not runnable.
    RuntimeUnavailable,
    /// The exact retained Pack runtime faulted while executing pure code.
    RuntimeFault,
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

impl fmt::Display for ReplayFailureV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Replay failed: {:?}", self.class)
    }
}

impl std::error::Error for ReplayFailureV1 {}

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
        TraceErrorV1::Pack(error) if error.is_runtime_fault() => ReplayFailureClassV1::RuntimeFault,
        TraceErrorV1::Pack(_) => ReplayFailureClassV1::ActivityReduction,
        TraceErrorV1::Canonical(_) => ReplayFailureClassV1::CanonicalEncoding,
        _ => ReplayFailureClassV1::Stimulus,
    }
}

fn classify_replay_advance_error(error: &TraceErrorV1) -> ReplayFailureClassV1 {
    match error {
        TraceErrorV1::Core(_) => ReplayFailureClassV1::CoreInvariant,
        TraceErrorV1::Pack(error) if error.is_runtime_fault() => ReplayFailureClassV1::RuntimeFault,
        TraceErrorV1::Pack(_) => ReplayFailureClassV1::ActivityReduction,
        TraceErrorV1::Canonical(_) => ReplayFailureClassV1::CanonicalEncoding,
        TraceErrorV1::StateHashMismatch => ReplayFailureClassV1::StateHashes,
        TraceErrorV1::LineageHashMismatch => ReplayFailureClassV1::LineageHash,
        TraceErrorV1::VersionIdentityMismatch => ReplayFailureClassV1::VersionOrDigest,
        _ => ReplayFailureClassV1::Stimulus,
    }
}

#[cfg(test)]
mod registry_replay_tests {
    use std::{fmt::Display, str::FromStr};

    use super::*;
    use crate::{
        AccessModeV1, ActionId, MemberId, MembershipV1, PrincipalKindV1, TimerScheduledFor,
        builtin_counter_registry, counter_v1_digest, counter_v1_only_registry_for_conformance,
        counter_v2_digest, counter_v2_runtime_fault_registry_for_conformance,
    };

    const ROOM: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const PARTICIPANT: &str = "01ARZ3NDEKTSV4RRFFQ69G5FC0";
    const PRINCIPAL: &str = "01ARZ3NDEKTSV4RRFFQ69G5FD0";
    const SEED: &str = "hex:000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

    fn parsed<T>(value: &str) -> T
    where
        T: FromStr,
        T::Err: Display,
    {
        value
            .parse()
            .unwrap_or_else(|error| unreachable!("fixture value {value}: {error}"))
    }

    fn canonical(bytes: &[u8]) -> CanonicalJsonV1 {
        CanonicalJsonV1::parse(bytes)
            .unwrap_or_else(|error| unreachable!("fixture canonical JSON: {error}"))
    }

    fn request(pack_digest: crate::PackDigestV1) -> PackGenesisRequestV1 {
        let participant = MembershipV1::new(
            parsed(PARTICIPANT),
            parsed(PRINCIPAL),
            PrincipalKindV1::Human,
            MembershipStandingV1::Enabled,
            AccessModeV1::Participant,
            Some("counter".to_owned()),
        )
        .unwrap_or_else(|error| unreachable!("participant fixture: {error}"));
        PackGenesisRequestV1 {
            room_id: parsed(ROOM),
            pack_digest,
            configuration: canonical(br#"{"initial_value":0,"maximum_value":2}"#),
            room_seed: parsed(SEED),
            created_at: parsed("2026-08-15T12:00:00Z"),
            initial_core_state: CoreRoomStateV1::active([participant])
                .unwrap_or_else(|error| unreachable!("Core fixture: {error}")),
        }
    }

    fn retained_trace(registry: &PackRegistryV1, pack_digest: crate::PackDigestV1) -> CoreTraceV1 {
        let verified = registry
            .prepare_genesis_for_retained_room(&request(pack_digest))
            .unwrap_or_else(|error| unreachable!("retained Genesis: {error}"));
        let input = verified.genesis_input().clone();
        let retained_pack = verified.retained_pack().clone();
        let preparer = RoomTransitionPreparerV1::from_retained_pack(
            retained_pack.clone(),
            input.room_seed.clone(),
        );
        CoreTraceV1::create_with_preparer(input, preparer, Some(retained_pack))
            .unwrap_or_else(|error| unreachable!("retained trace: {error}"))
    }

    fn increment(trace: &CoreTraceV1, action_id: &str, admitted_at: &str) -> RecordedStimulusV1 {
        let retained = trace
            .retained_pack()
            .unwrap_or_else(|| unreachable!("retained test trace"));
        let definition = retained
            .descriptor()
            .actions
            .iter()
            .find(|definition| definition.action_type == "increment")
            .unwrap_or_else(|| unreachable!("Counter increment"));
        RecordedStimulusV1::ParticipantAction(crate::ParticipantActionV1 {
            member_id: parsed::<MemberId>(PARTICIPANT),
            action_id: parsed::<ActionId>(action_id),
            action_type: "increment".to_owned(),
            payload_schema_digest: definition.payload_schema.schema_digest.clone(),
            canonical_payload: canonical(br"{}"),
            exact_basis_head: trace.head().clone(),
            admitted_at: parsed(admitted_at),
        })
    }

    fn replay_genesis_error(registry: &PackRegistryV1, genesis: &GenesisV1) -> ReplayFailureV1 {
        let bytes = genesis
            .canonical_bytes()
            .unwrap_or_else(|error| unreachable!("Genesis bytes: {error}"));
        CoreTraceV1::replay(registry, &bytes, &[])
            .err()
            .unwrap_or_else(|| unreachable!("altered Genesis unexpectedly replayed"))
    }

    #[test]
    fn retained_v1_replay_uses_v1_and_can_resume_through_the_owning_trace() {
        let registry = builtin_counter_registry()
            .unwrap_or_else(|error| unreachable!("Counter registry: {error}"));
        let mut trace = retained_trace(&registry, counter_v1_digest());
        trace
            .advance(increment(
                &trace,
                "01ARZ3NDEKTSV4RRFFQ69G5FC3",
                "2026-08-15T12:00:01Z",
            ))
            .unwrap_or_else(|error| unreachable!("v1 increment: {error}"));
        assert_eq!(
            trace
                .activity_state()
                .to_bytes()
                .unwrap_or_else(|error| unreachable!("Activity bytes: {error}")),
            br#"{"maximum_value":2,"private_ack_count":0,"value":1}"#
        );
        let genesis = trace
            .genesis_bytes()
            .unwrap_or_else(|error| unreachable!("Genesis bytes: {error}"));
        let transitions = trace
            .transition_bytes()
            .unwrap_or_else(|error| unreachable!("Transition bytes: {error}"));
        let report = CoreTraceV1::replay(&registry, &genesis, &transitions)
            .unwrap_or_else(|failure| unreachable!("v1 replay: {}", failure.detail));
        assert_eq!(report.final_head.pack_digest(), &counter_v1_digest());
        let mut restored = report.into_trace();
        let continuation = increment(
            &restored,
            "01ARZ3NDEKTSV4RRFFQ69G5FC4",
            "2026-08-15T12:00:02Z",
        );
        restored
            .advance(continuation)
            .unwrap_or_else(|error| unreachable!("v1 continuation: {error}"));
        assert_eq!(restored.head().room_seq().get(), 2);
    }

    #[test]
    fn cached_executor_discards_history_without_losing_its_execution_basis() {
        let registry = builtin_counter_registry()
            .unwrap_or_else(|error| unreachable!("Counter registry: {error}"));
        let mut trace = retained_trace(&registry, counter_v1_digest());
        trace
            .advance(increment(
                &trace,
                "01ARZ3NDEKTSV4RRFFQ69G5FC3",
                "2026-08-15T12:00:01Z",
            ))
            .unwrap_or_else(|error| unreachable!("increment: {error}"));
        let head = trace.head().clone();
        let cache = crate::RoomTraceCacheV1::default();
        let integrity = RoomIntegrityStateV1::new(
            RoomIntegrityStatusV1::Healthy,
            crate::IntegrityGenerationV1::new(1)
                .unwrap_or_else(|error| unreachable!("integrity generation: {error}")),
        );
        cache
            .with_room(&parsed(ROOM), |slot| {
                *slot = Some(crate::CachedRoomTraceV1::new(trace, integrity));
                let cached = slot
                    .as_mut()
                    .unwrap_or_else(|| unreachable!("installed trace"));
                assert!(cached.trace().transitions().is_empty());
                assert_eq!(cached.trace().head(), &head);
                let next = increment(
                    cached.trace(),
                    "01ARZ3NDEKTSV4RRFFQ69G5FC4",
                    "2026-08-15T12:00:02Z",
                );
                cached
                    .trace_mut()
                    .advance(next)
                    .unwrap_or_else(|error| unreachable!("continuation: {error}"));
                assert_eq!(cached.trace().head().room_seq().get(), 2);
            })
            .unwrap_or_else(|error| unreachable!("cache borrow: {error}"));
        cache
            .with_room(&parsed(ROOM), |slot| {
                let cached = slot
                    .as_ref()
                    .unwrap_or_else(|| unreachable!("retained trace"));
                assert!(cached.trace().transitions().is_empty());
                assert_eq!(cached.trace().head().room_seq().get(), 2);
            })
            .unwrap_or_else(|error| unreachable!("cache borrow: {error}"));
    }

    #[test]
    fn replay_rejects_recomputed_initial_activity_and_timer_substitution() {
        let registry = builtin_counter_registry()
            .unwrap_or_else(|error| unreachable!("Counter registry: {error}"));
        let trace = retained_trace(&registry, counter_v2_digest());

        let mut changed_activity = trace.genesis.clone();
        changed_activity.initial_activity_state =
            canonical(br#"{"maximum_value":2,"private_ack_count":0,"value":1}"#);
        changed_activity.initial_activity_state_hash = hash_activity_state(
            &changed_activity.pack_digest,
            &changed_activity.initial_activity_state,
        )
        .unwrap_or_else(|error| unreachable!("Activity hash: {error}"));
        changed_activity.initial_authoritative_state_hash = hash_authoritative_state(
            &changed_activity.pack_digest,
            &changed_activity.initial_core_state_hash,
            &changed_activity.initial_activity_state_hash,
        )
        .unwrap_or_else(|error| unreachable!("authoritative hash: {error}"));
        changed_activity.genesis_hash = changed_activity
            .calculate_hash()
            .unwrap_or_else(|error| unreachable!("Genesis hash: {error}"));
        let failure = replay_genesis_error(&registry, &changed_activity);
        assert_eq!(failure.class, ReplayFailureClassV1::ActivityReduction);
        assert!(failure.detail.contains("does not exactly reproduce"));

        let mut changed_timers = trace.genesis.clone();
        changed_timers.initial_timers.push(ScheduledTimerV1 {
            timer_id: parsed("01ARZ3NDEKTSV4RRFFQ69G5FE0"),
            generation: TimerGenerationV1::new(1)
                .unwrap_or_else(|error| unreachable!("Timer generation: {error}")),
            scheduled_for: parsed::<TimerScheduledFor>("2026-08-15T12:30:00Z"),
            canonical_payload: canonical(br#"{"kind":"substituted"}"#),
        });
        changed_timers.genesis_hash = changed_timers
            .calculate_hash()
            .unwrap_or_else(|error| unreachable!("Genesis hash: {error}"));
        let failure = replay_genesis_error(&registry, &changed_timers);
        assert_eq!(failure.class, ReplayFailureClassV1::ActivityReduction);
        assert!(failure.detail.contains("does not exactly reproduce"));
    }

    #[test]
    fn stored_genesis_input_disagreement_is_corrupt_but_initialize_panic_is_runtime_fault() {
        let registry = builtin_counter_registry()
            .unwrap_or_else(|error| unreachable!("Counter registry: {error}"));
        let trace = retained_trace(&registry, counter_v2_digest());
        let mut invalid_configuration = trace.genesis.clone();
        invalid_configuration.configuration = canonical(br"{}");
        invalid_configuration.genesis_hash = invalid_configuration
            .calculate_hash()
            .unwrap_or_else(|error| unreachable!("Genesis hash: {error}"));
        let failure = replay_genesis_error(&registry, &invalid_configuration);
        assert_eq!(failure.class, ReplayFailureClassV1::ActivityReduction);

        let panic_registry =
            counter_v2_runtime_fault_registry_for_conformance(ActivityPackOperationV1::Initialize)
                .unwrap_or_else(|error| unreachable!("initialize-panic registry: {error}"));
        let failure = replay_genesis_error(&panic_registry, &trace.genesis);
        assert_eq!(failure.class, ReplayFailureClassV1::RuntimeFault);
    }

    #[test]
    fn replay_missing_exact_digest_never_falls_forward_and_corruption_precedes_initialize() {
        let registry = builtin_counter_registry()
            .unwrap_or_else(|error| unreachable!("Counter registry: {error}"));
        let trace = retained_trace(&registry, counter_v2_digest());

        let mut missing = trace.genesis.clone();
        missing.pack_digest =
            parsed("blake3:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff");
        missing.initial_activity_state_hash =
            hash_activity_state(&missing.pack_digest, &missing.initial_activity_state)
                .unwrap_or_else(|error| unreachable!("Activity hash: {error}"));
        missing.initial_authoritative_state_hash = hash_authoritative_state(
            &missing.pack_digest,
            &missing.initial_core_state_hash,
            &missing.initial_activity_state_hash,
        )
        .unwrap_or_else(|error| unreachable!("authoritative hash: {error}"));
        missing.genesis_hash = missing
            .calculate_hash()
            .unwrap_or_else(|error| unreachable!("Genesis hash: {error}"));
        let failure = replay_genesis_error(&registry, &missing);
        assert_eq!(failure.class, ReplayFailureClassV1::RuntimeUnavailable);
        assert!(failure.detail.contains("pack semantic revision is absent"));

        let mut advanced = retained_trace(&registry, counter_v2_digest());
        let stimulus = increment(
            &advanced,
            "01ARZ3NDEKTSV4RRFFQ69G5FC3",
            "2026-08-15T12:00:01Z",
        );
        advanced
            .advance(stimulus)
            .unwrap_or_else(|error| unreachable!("v2 increment: {error}"));
        let mut corrupt_transition = advanced.transitions[0].clone();
        corrupt_transition.transition_hash =
            parsed("blake3:0000000000000000000000000000000000000000000000000000000000000000");
        let missing_registry = counter_v1_only_registry_for_conformance()
            .unwrap_or_else(|error| unreachable!("v1-only registry: {error}"));
        let failure = CoreTraceV1::replay(
            &missing_registry,
            &advanced
                .genesis_bytes()
                .unwrap_or_else(|error| unreachable!("Genesis bytes: {error}")),
            &[corrupt_transition
                .canonical_bytes()
                .unwrap_or_else(|error| unreachable!("Transition bytes: {error}"))],
        )
        .err()
        .unwrap_or_else(|| unreachable!("corrupt Transition unexpectedly replayed"));
        assert_eq!(failure.class, ReplayFailureClassV1::LineageHash);

        let mut corrupted = trace.genesis.clone();
        corrupted.configuration = canonical(br#"{"initial_value":1,"maximum_value":2}"#);
        let failure = replay_genesis_error(&registry, &corrupted);
        assert_eq!(failure.class, ReplayFailureClassV1::LineageHash);
    }
}
