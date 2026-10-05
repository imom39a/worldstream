//! Genesis-selected structural and exact executable history. No serving effects.
use super::*;
use crate::{GenesisRecord, LineageCodecError, TransitionRecord};

impl GenesisRecord {
    /// Checks only the fixed Genesis-selected V2 accounting. Codec identity and
    /// semantic Pack verification remain separate operations.
    pub(crate) fn check_payload_budget(&self) -> Result<(), crate::PayloadBudgetErrorV1> {
        if self.format() == crate::CanonicalHistoryFormat::V2 {
            let budget = crate::PAYLOAD_BUDGET_V1;
            budget.check_bytes(crate::PayloadKindV1::Genesis, &self.canonical_bytes()?)?;
            budget.check_genesis_request(&PackGenesisRequestV1 {
                room_id: self.room_id().clone(),
                pack_digest: self.pack_digest().clone(),
                configuration: self.configuration().clone(),
                room_seed: self.room_seed().clone(),
                created_at: self.created_at().clone(),
                initial_core_state: self.initial_core_state().clone(),
            })?;
            budget.check_authoritative_state(
                self.initial_core_state(),
                self.initial_activity_state(),
            )?;
        }
        Ok(())
    }
    pub(crate) fn check_transition_payload_budget(
        &self,
        record: &TransitionRecord,
        bytes: &[u8],
    ) -> Result<(), crate::PayloadBudgetErrorV1> {
        if self.format() == crate::CanonicalHistoryFormat::V2 {
            let budget = crate::PAYLOAD_BUDGET_V1;
            budget.check_bytes(crate::PayloadKindV1::Transition, bytes)?;
            budget.check_stimulus(record.recorded_stimulus())?;
            budget.check_effects(
                record.ordered_domain_events(),
                record.ordered_timer_changes(),
                record.ordered_attention_signals(),
            )?;
        }
        Ok(())
    }
}

/// Host-derived lineage facts. Opaque Activity is absent unless the caller
/// supplies canonical bytes that match the final commitment. Such a supplied
/// value is commitment-checked; this type never claims semantic execution.
pub struct CanonicalStructuralHistory {
    genesis: GenesisRecord,
    head: CompleteHeadV1,
    core_state: CoreRoomStateV1,
    timers: TimerBookV1,
    membership_generations: BTreeMap<String, i64>,
    commitment_checked_activity: Option<CanonicalJsonV1>,
    timer_ledger: crate::room_commit::CanonicalTimerLedger,
    activation_decisions: Vec<crate::RecoveredActivationDecisionV1>,
}
impl CanonicalStructuralHistory {
    #[must_use]
    pub const fn genesis(&self) -> &GenesisRecord {
        &self.genesis
    }
    #[must_use]
    pub const fn head(&self) -> &CompleteHeadV1 {
        &self.head
    }
    #[must_use]
    pub const fn core_state(&self) -> &CoreRoomStateV1 {
        &self.core_state
    }
    #[must_use]
    pub const fn scheduled_timers(&self) -> &BTreeMap<TimerId, ScheduledTimerV1> {
        &self.timers.scheduled
    }
    #[must_use]
    pub const fn timer_generations(&self) -> &BTreeMap<TimerId, TimerGenerationV1> {
        &self.timers.last_generation
    }
    #[must_use]
    pub const fn membership_generations(&self) -> &BTreeMap<String, i64> {
        &self.membership_generations
    }
    /// Complete host-derived Timer generation facts, including fired and cancelled rows.
    #[must_use]
    pub fn timer_materializations(&self) -> Vec<crate::RecoveredTimerMaterializationV1> {
        self.timer_ledger.materializations()
    }
    /// Fixed host policy bytes derived from recorded ordered Attention facts.
    /// This does not establish Activity semantics.
    #[must_use]
    pub fn activation_decisions(&self) -> &[crate::RecoveredActivationDecisionV1] {
        &self.activation_decisions
    }
    /// Returns only caller-supplied, commitment-checked Activity. Structural
    /// preflight does not reconstruct opaque Activity or establish semantics.
    #[must_use]
    pub const fn commitment_checked_activity(&self) -> Option<&CanonicalJsonV1> {
        self.commitment_checked_activity.as_ref()
    }
    /// Checks a separately supplied current materialization. This performs no
    /// Pack callback and does not upgrade structural evidence to semantic proof.
    pub fn with_activity_materialization(mut self, bytes: &[u8]) -> Result<Self, ReplayFailureV1> {
        let activity = CanonicalJsonV1::from_canonical_bytes(bytes).map_err(|error| {
            self.failure(ReplayFailureClassV1::NonCanonicalRecord, error.to_string())
        })?;
        if hash_activity_state(self.head.pack_digest(), &activity).map_err(|error| {
            self.failure(ReplayFailureClassV1::CanonicalEncoding, error.to_string())
        })? != *self.head.activity_state_hash()
        {
            return Err(self.failure(
                ReplayFailureClassV1::ActivityState,
                "supplied Activity differs from final commitment".into(),
            ));
        }
        if self.genesis.format() == crate::CanonicalHistoryFormat::V2 {
            crate::PAYLOAD_BUDGET_V1
                .check_authoritative_state(&self.core_state, &activity)
                .map_err(|error| {
                    self.failure(ReplayFailureClassV1::ActivityState, error.to_string())
                })?;
        }
        self.commitment_checked_activity = Some(activity);
        Ok(self)
    }
    fn failure(&self, class: ReplayFailureClassV1, detail: String) -> ReplayFailureV1 {
        ReplayFailureV1::with_head(class, detail, self.head.clone())
    }
    fn from_genesis(genesis: GenesisRecord) -> Result<Self, ReplayFailureV1> {
        genesis.check_payload_budget().map_err(|error| {
            replay_error(
                &genesis.complete_head(),
                ReplayFailureClassV1::CoreInvariant,
                error.to_string(),
            )
        })?;
        genesis
            .verify()
            .map_err(|error| codec_failure(error, None))?;
        validate_core_state(genesis.initial_core_state(), &|_| Ok(())).map_err(|error| {
            ReplayFailureV1::without_head(
                ReplayFailureClassV1::CoreInvariant,
                map_checked_core_error(error).to_string(),
            )
        })?;
        let timers = TimerBookV1::from_genesis(
            genesis.initial_timers(),
            genesis.created_at().as_str(),
            false,
        )
        .map_err(|error| {
            ReplayFailureV1::without_head(classify_genesis_error(&error), error.to_string())
        })?;
        let timer_ledger = crate::room_commit::CanonicalTimerLedger::begin(&genesis, None)
            .map_err(|error| {
                replay_error(
                    &genesis.complete_head(),
                    ReplayFailureClassV1::TimerChanges,
                    error.to_string(),
                )
            })?;
        Ok(Self {
            head: genesis.complete_head(),
            core_state: genesis.initial_core_state().clone(),
            timers,
            membership_generations: genesis
                .initial_core_state()
                .memberships()
                .keys()
                .map(|id| (id.to_string(), 1))
                .collect(),
            genesis,
            commitment_checked_activity: None,
            timer_ledger,
            activation_decisions: Vec::new(),
        })
    }
    fn from_checkpoint(
        genesis: GenesisRecord,
        checkpoint: &crate::RoomRecoveryCheckpointV1,
    ) -> Result<Self, ReplayFailureV1> {
        let evidence = crate::VerifiedCanonicalGenesis::from_canonical_bytes(
            &genesis
                .canonical_bytes()
                .map_err(|error| codec_failure(error.into(), None))?,
        )
        .map_err(|error| {
            replay_error(
                checkpoint.head(),
                ReplayFailureClassV1::CoreInvariant,
                error.to_string(),
            )
        })?;
        let current = crate::VerifiedCanonicalCurrentRoomMaterialization::verify_for_storage(
            &evidence,
            checkpoint.head(),
            checkpoint.record_bytes(),
            checkpoint.core_state_bytes(),
            checkpoint.activity_state_bytes(),
        )
        .map_err(|error| {
            replay_error(
                checkpoint.head(),
                ReplayFailureClassV1::CoreState,
                error.to_string(),
            )
        })?;
        let timers = TimerBookV1::from_checkpoint(checkpoint.timers()).map_err(|error| {
            replay_error(
                checkpoint.head(),
                ReplayFailureClassV1::TimerChanges,
                error.to_string(),
            )
        })?;
        if current.room_status() == RoomStatusV1::Archived && !timers.scheduled.is_empty() {
            return Err(replay_error(
                checkpoint.head(),
                ReplayFailureClassV1::TimerChanges,
                "archived checkpoint retains scheduled Timers".into(),
            ));
        }
        let generations = checkpoint.membership_generations().clone();
        if generations.len() != current.core_state().memberships().len()
            || current.core_state().memberships().keys().any(|id| {
                generations.get(id.as_str()).is_none_or(|generation| {
                    *generation < 1 || *generation > crate::MAX_SAFE_INTEGER
                })
            })
            || checkpoint.observation_frame_heads().len() != generations.len()
            || current
                .core_state()
                .memberships()
                .keys()
                .any(|id| !checkpoint.observation_frame_heads().contains_key(id))
        {
            return Err(replay_error(
                checkpoint.head(),
                ReplayFailureClassV1::CoreInvariant,
                "checkpoint operational Membership witnesses disagree".into(),
            ));
        }
        let timer_ledger =
            crate::room_commit::CanonicalTimerLedger::begin(&genesis, Some(checkpoint)).map_err(
                |error| {
                    replay_error(
                        checkpoint.head(),
                        ReplayFailureClassV1::TimerChanges,
                        error.to_string(),
                    )
                },
            )?;
        Ok(Self {
            genesis,
            head: checkpoint.head().clone(),
            core_state: current.core_state().clone(),
            timers,
            membership_generations: generations,
            commitment_checked_activity: Some(current.activity_state().clone()),
            timer_ledger,
            activation_decisions: checkpoint.activation_decisions().to_vec(),
        })
    }
    fn consume_transition(&mut self, bytes: &[u8]) -> Result<(), ReplayFailureV1> {
        let record = decode_canonical_successor(&self.genesis, &self.head, bytes)?;
        let derived = derive_preflight_core_transition(
            &self.head,
            &self.core_state,
            &self.timers,
            record.recorded_stimulus(),
        )
        .map_err(|(class, detail)| self.failure(class, detail))?;
        if self.genesis.format() == crate::CanonicalHistoryFormat::V2 {
            crate::PAYLOAD_BUDGET_V1
                .check_value(crate::PayloadKindV1::CoreState, &derived)
                .map_err(|error| {
                    self.failure(ReplayFailureClassV1::CoreState, error.to_string())
                })?;
        }
        if hash_core_state(&derived).map_err(|error| {
            self.failure(ReplayFailureClassV1::CanonicalEncoding, error.to_string())
        })? != *record.resulting_core_state_hash()
        {
            return Err(self.failure(
                ReplayFailureClassV1::CoreState,
                "derived Core differs from stored commitment".into(),
            ));
        }
        if let TransitionRecord::V1(legacy) = &record {
            if legacy.resulting_core_state() != &derived {
                return Err(self.failure(
                    ReplayFailureClassV1::CoreState,
                    "embedded Core differs from host derivation".into(),
                ));
            }
        }
        let mut timers = self.timers.clone();
        timers
            .apply_stored_transition(
                record.recorded_stimulus(),
                record.ordered_timer_changes(),
                derived.room_status(),
            )
            .map_err(|error| self.failure(ReplayFailureClassV1::TimerChanges, error.to_string()))?;
        let mut generations = self.membership_generations.clone();
        advance_membership_generations(&self.core_state, &derived, &mut generations)
            .map_err(|detail| self.failure(ReplayFailureClassV1::CoreInvariant, detail))?;
        let mut decisions = Vec::new();
        for signal in record.ordered_attention_signals() {
            let decision =
                crate::PreparedActivationDecisionV1::from_attention(signal, record.room_seq())
                    .map_err(|error| {
                        self.failure(ReplayFailureClassV1::AttentionSignals, error.to_string())
                    })?;
            decisions.push(crate::RecoveredActivationDecisionV1::new(
                record.room_seq(),
                decision.decision_id().to_owned(),
                decision.target_member_id().cloned(),
                decision.canonical_decision_bytes().to_vec(),
            ));
        }
        self.timer_ledger
            .consume(&record)
            .map_err(|error| self.failure(ReplayFailureClassV1::TimerChanges, error.to_string()))?;
        self.activation_decisions.extend(decisions);
        self.timers = timers;
        self.membership_generations = generations;
        self.core_state = derived;
        self.head = record.complete_head();
        self.commitment_checked_activity = None;
        Ok(())
    }
}

/// Bounded structural pass. Pages are borrowed and discarded after each call.
pub struct CanonicalStorageHistoryPreflight {
    structure: CanonicalStructuralHistory,
}
impl CanonicalStorageHistoryPreflight {
    pub fn begin(genesis_bytes: &[u8]) -> Result<Self, ReplayFailureV1> {
        let genesis = GenesisRecord::from_canonical_bytes(genesis_bytes)
            .map_err(|error| codec_failure(error, None))?;
        Ok(Self {
            structure: CanonicalStructuralHistory::from_genesis(genesis)?,
        })
    }
    pub fn consume_transition_page(&mut self, page: &[Vec<u8>]) -> Result<(), ReplayFailureV1> {
        for bytes in page {
            self.structure.consume_transition(bytes)?;
        }
        Ok(())
    }
    #[must_use]
    pub const fn final_head(&self) -> &CompleteHeadV1 {
        &self.structure.head
    }
    #[must_use]
    pub const fn structural_state(&self) -> &CanonicalStructuralHistory {
        &self.structure
    }
    #[must_use]
    pub fn finish(self) -> CanonicalStructuralHistory {
        self.structure
    }
    /// Starts exact execution only after the complete captured Head is proven.
    pub fn begin_executable(
        self,
        expected_head: &CompleteHeadV1,
        registry: &PackRegistryV1,
    ) -> Result<CanonicalStorageExecutableReplay, ReplayFailureV1> {
        self.begin_executable_inner(expected_head, registry, false)
    }
    /// Reproduces exact operational witnesses in addition to authoritative state.
    /// Successful finish is required before storage may use those witnesses.
    pub fn begin_executable_with_observations(
        self,
        expected_head: &CompleteHeadV1,
        registry: &PackRegistryV1,
    ) -> Result<CanonicalStorageExecutableReplay, ReplayFailureV1> {
        self.begin_executable_inner(expected_head, registry, true)
    }
    fn begin_executable_inner(
        self,
        expected_head: &CompleteHeadV1,
        registry: &PackRegistryV1,
        observations: bool,
    ) -> Result<CanonicalStorageExecutableReplay, ReplayFailureV1> {
        if self.final_head() != expected_head {
            return Err(self.structure.failure(
                ReplayFailureClassV1::LineageHash,
                "structural preflight does not reach captured Head".into(),
            ));
        }
        let collector = observations
            .then(|| CanonicalOperationalCollector::begin(&self.structure.genesis, None))
            .transpose()?;
        let trace = retained_trace(registry, self.structure.genesis, expected_head, true)?;
        let frame_heads = trace
            .core_state()
            .memberships()
            .keys()
            .cloned()
            .map(|id| (id, 0))
            .collect();
        Ok(CanonicalStorageExecutableReplay {
            trace,
            expected_head: expected_head.clone(),
            membership_generations: self.structure.membership_generations,
            collector,
            frame_heads,
            consequences: Vec::new(),
        })
    }
}

/// Bounded executable pass. Each emitted cut is semantically verified through
/// that cut. Callers must wait for successful finish and captured storage fences
/// before installing any generated cache. This type has no external effects.
pub struct CanonicalStorageExecutableReplay {
    trace: CanonicalRoomTrace,
    expected_head: CompleteHeadV1,
    membership_generations: BTreeMap<String, i64>,
    collector: Option<CanonicalOperationalCollector>,
    frame_heads: BTreeMap<MemberId, u64>,
    consequences: Vec<ReplayObservationConsequenceV1>,
}
impl CanonicalStorageExecutableReplay {
    #[must_use]
    pub fn current_head(&self) -> &CompleteHeadV1 {
        self.trace.head()
    }
    pub fn genesis_materialization(&self) -> Result<ReplayStepV1, ReplayFailureV1> {
        if self.trace.head().room_seq().get() != 0 {
            return Err(replay_error(
                self.trace.head(),
                ReplayFailureClassV1::Sequence,
                "Genesis materialization is available only before Transition execution".into(),
            ));
        }
        canonical_replay_step(
            &self.trace,
            &self.trace.genesis_bytes().map_err(|error| {
                replay_error(
                    self.trace.head(),
                    ReplayFailureClassV1::CanonicalEncoding,
                    error.to_string(),
                )
            })?,
        )
    }
    pub fn consume_transition_page(&mut self, page: &[Vec<u8>]) -> Result<(), ReplayFailureV1> {
        self.consume_transition_page_inner(page, None)
    }
    pub fn consume_transition_page_with_materializations(
        &mut self,
        page: &[Vec<u8>],
        visitor: &mut dyn FnMut(&ReplayStepV1) -> Result<(), ReplayFailureV1>,
    ) -> Result<(), ReplayFailureV1> {
        self.consume_transition_page_inner(page, Some(visitor))
    }
    fn consume_transition_page_inner(
        &mut self,
        page: &[Vec<u8>],
        mut visitor: Option<&mut dyn FnMut(&ReplayStepV1) -> Result<(), ReplayFailureV1>>,
    ) -> Result<(), ReplayFailureV1> {
        for bytes in page {
            if self.trace.head().room_seq() >= self.expected_head.room_seq() {
                return Err(replay_error(
                    self.trace.head(),
                    ReplayFailureClassV1::Sequence,
                    "executable page extends beyond captured Head".into(),
                ));
            }
            self.trace.replay_canonical_transition(
                bytes,
                self.collector
                    .as_ref()
                    .map(|_| (&mut self.frame_heads, &mut self.consequences)),
            )?;
            if let Some(collector) = &mut self.collector {
                collector.consume(self.trace.transitions().last().ok_or_else(|| {
                    replay_error(
                        self.trace.head(),
                        ReplayFailureClassV1::CoreInvariant,
                        "verified Transition absent".into(),
                    )
                })?)?;
            }
            if let Some(visitor) = &mut visitor {
                visitor(&canonical_replay_step(&self.trace, bytes)?)?;
            }
            self.trace.discard_persisted_history();
        }
        Ok(())
    }
    /// Optional serving bytes are compared when supplied. Missing disposable
    /// materializations remain absent and exact execution provides full state.
    pub fn finish(
        self,
        expected_head: &CompleteHeadV1,
        core_bytes: Option<&[u8]>,
        activity_bytes: Option<&[u8]>,
    ) -> Result<CanonicalReplayReport, ReplayFailureV1> {
        if expected_head != &self.expected_head {
            return Err(replay_error(
                self.trace.head(),
                ReplayFailureClassV1::LineageHash,
                "finish Head differs from captured Head".into(),
            ));
        }
        compare_canonical_materializations(&self.trace, expected_head, core_bytes, activity_bytes)?;
        Ok(CanonicalReplayReport::from_trace(
            self.trace,
            Vec::new(),
            self.consequences,
            self.membership_generations,
            self.collector.map(CanonicalOperationalCollector::finish),
        ))
    }
}

/// Exact semantic Replay result. Complete state lives in its opaque owning
/// executor, separate from compact canonical records. Replay emits no effects.
pub struct CanonicalReplayReport {
    pub final_head: CompleteHeadV1,
    pub steps: Vec<ReplayStepV1>,
    pub activity_callback_count: usize,
    pub external_effect_count: usize,
    pub receipt_count: usize,
    trace: CanonicalRoomTrace,
    observation_consequences: Vec<ReplayObservationConsequenceV1>,
    membership_generations: BTreeMap<String, i64>,
    operational: Option<CanonicalOperationalFacts>,
}
impl CanonicalReplayReport {
    fn from_trace(
        trace: CanonicalRoomTrace,
        steps: Vec<ReplayStepV1>,
        observation_consequences: Vec<ReplayObservationConsequenceV1>,
        membership_generations: BTreeMap<String, i64>,
        operational: Option<CanonicalOperationalFacts>,
    ) -> Self {
        Self {
            final_head: trace.head().clone(),
            activity_callback_count: trace.activity_callback_count(),
            external_effect_count: 0,
            receipt_count: 0,
            trace,
            steps,
            observation_consequences,
            membership_generations,
            operational,
        }
    }
    /// Requires the explicit observation-enabled pass. State-only Replay cannot
    /// establish operational delivery witnesses.
    pub fn storage_verification(&self) -> Result<ReplayStorageVerificationV1, ReplayFailureV1> {
        let facts = self.operational.as_ref().ok_or_else(|| {
            replay_error(
                &self.final_head,
                ReplayFailureClassV1::CoreInvariant,
                "operational witness reproduction was not requested".into(),
            )
        })?;
        let memberships = self
            .final_state()
            .core_state()
            .memberships()
            .values()
            .map(|membership| {
                Ok(ReplayMembershipWitnessV1 {
                    member_id: membership.member_id().clone(),
                    canonical_membership_bytes: encode(membership).map_err(|error| {
                        replay_error(
                            &self.final_head,
                            ReplayFailureClassV1::CanonicalEncoding,
                            error.to_string(),
                        )
                    })?,
                })
            })
            .collect::<Result<Vec<_>, ReplayFailureV1>>()?;
        let (observation_positions, observation_frames, observation_consequences) =
            replay_observation_storage_witnesses(&self.observation_consequences, &memberships);
        Ok(ReplayStorageVerificationV1 {
            memberships,
            timers: facts.timers.clone(),
            activation_decisions: facts.activation_decisions.clone(),
            observation_positions,
            observation_frames,
            observation_consequences,
        })
    }
    #[must_use]
    pub fn final_state(&self) -> &RoomTransitionStateV1 {
        &self.trace.state
    }
    #[must_use]
    pub fn into_trace(self) -> CanonicalRoomTrace {
        self.trace
    }
    /// Returns the number of verified replay delivery witnesses. These are
    /// diagnostic facts; Replay does not publish or persist observations.
    #[must_use]
    pub fn observation_consequence_count(&self) -> usize {
        self.observation_consequences.len()
    }
    #[must_use]
    pub const fn membership_generations(&self) -> &BTreeMap<String, i64> {
        &self.membership_generations
    }
    pub(crate) fn observation_consequences(&self) -> &[ReplayObservationConsequenceV1] {
        &self.observation_consequences
    }
    #[must_use]
    pub fn retained_pack_revision_lock(&self) -> &crate::PackRevisionLockV1 {
        self.trace.retained_pack().revision_lock()
    }
}

struct CanonicalOperationalFacts {
    timers: Vec<crate::RecoveredTimerMaterializationV1>,
    activation_decisions: Vec<ReplayActivationDecisionWitnessV1>,
}
struct CanonicalOperationalCollector {
    timers: crate::room_commit::CanonicalTimerLedger,
    activation_decisions: Vec<ReplayActivationDecisionWitnessV1>,
}
impl CanonicalOperationalCollector {
    fn begin(
        genesis: &GenesisRecord,
        checkpoint: Option<&crate::RoomRecoveryCheckpointV1>,
    ) -> Result<Self, ReplayFailureV1> {
        Ok(Self {
            timers: crate::room_commit::CanonicalTimerLedger::begin(genesis, checkpoint).map_err(
                |error| {
                    replay_error(
                        &genesis.complete_head(),
                        ReplayFailureClassV1::TimerChanges,
                        error.to_string(),
                    )
                },
            )?,
            activation_decisions: Vec::new(),
        })
    }
    fn consume(&mut self, record: &TransitionRecord) -> Result<(), ReplayFailureV1> {
        self.timers.consume(record).map_err(|error| {
            replay_error(
                &record.complete_head(),
                ReplayFailureClassV1::TimerChanges,
                error.to_string(),
            )
        })?;
        for signal in record.ordered_attention_signals() {
            let decision =
                crate::PreparedActivationDecisionV1::from_attention(signal, record.room_seq())
                    .map_err(|error| {
                        replay_error(
                            &record.complete_head(),
                            ReplayFailureClassV1::AttentionSignals,
                            error.to_string(),
                        )
                    })?;
            self.activation_decisions
                .push(ReplayActivationDecisionWitnessV1 {
                    cause_room_seq: record.room_seq(),
                    decision_id: decision.decision_id().to_owned(),
                    target_member_id: decision.target_member_id().cloned(),
                    canonical_decision_bytes: decision.canonical_decision_bytes().to_vec(),
                });
        }
        Ok(())
    }
    fn finish(self) -> CanonicalOperationalFacts {
        CanonicalOperationalFacts {
            timers: self.timers.finish(),
            activation_decisions: self.activation_decisions,
        }
    }
}

impl CanonicalRoomTrace {
    pub(crate) fn replay_checkpoint_for_recovery(
        registry: &PackRegistryV1,
        genesis_bytes: &[u8],
        checkpoint: &crate::RoomRecoveryCheckpointV1,
        transition_bytes: &[Vec<u8>],
    ) -> Result<CanonicalReplayReport, ReplayFailureV1> {
        if transition_bytes.len() > 250 {
            return Err(replay_error(
                checkpoint.head(),
                ReplayFailureClassV1::Sequence,
                "checkpoint tail exceeds 250 Transitions".into(),
            ));
        }
        let genesis = GenesisRecord::from_canonical_bytes(genesis_bytes)
            .map_err(|error| codec_failure(error, None))?;
        let mut preflight =
            CanonicalStructuralHistory::from_checkpoint(genesis.clone(), checkpoint)?;
        for bytes in transition_bytes {
            preflight.consume_transition(bytes)?;
        }
        let mut collector = CanonicalOperationalCollector::begin(&genesis, Some(checkpoint))?;
        // This trusted checkpoint follows a complete verified history pass.
        // Keep checkpoint reconstruction free of initial view callbacks.
        let mut trace = retained_trace(registry, genesis, &preflight.head, false)?;
        let checked_core = trace
            .preparer
            .core_reducer
            .validate_state(
                CanonicalJsonV1::decode_canonical(checkpoint.core_state_bytes()).map_err(
                    |error| {
                        replay_error(
                            checkpoint.head(),
                            ReplayFailureClassV1::NonCanonicalRecord,
                            error.to_string(),
                        )
                    },
                )?,
            )
            .map_err(|error| {
                replay_error(
                    checkpoint.head(),
                    classify_replay_advance_error(&error),
                    error.to_string(),
                )
            })?;
        trace.state.head = checkpoint.head().clone();
        trace.state.core_state = checked_core;
        trace.state.activity_state = CanonicalJsonV1::from_canonical_bytes(
            checkpoint.activity_state_bytes(),
        )
        .map_err(|error| {
            replay_error(
                checkpoint.head(),
                ReplayFailureClassV1::NonCanonicalRecord,
                error.to_string(),
            )
        })?;
        trace.state.timers =
            TimerBookV1::from_checkpoint(checkpoint.timers()).map_err(|error| {
                replay_error(
                    checkpoint.head(),
                    ReplayFailureClassV1::TimerChanges,
                    error.to_string(),
                )
            })?;
        let steps = Vec::new();
        let mut frames = checkpoint.observation_frame_heads().clone();
        let mut consequences = Vec::new();
        for bytes in transition_bytes {
            trace.replay_canonical_transition(bytes, Some((&mut frames, &mut consequences)))?;
            collector.consume(trace.transitions().last().ok_or_else(|| {
                replay_error(
                    trace.head(),
                    ReplayFailureClassV1::CoreInvariant,
                    "verified Transition absent".into(),
                )
            })?)?;
        }
        compare_canonical_materializations(&trace, &preflight.head, None, None)?;
        Ok(CanonicalReplayReport::from_trace(
            trace,
            steps,
            consequences,
            preflight.membership_generations,
            Some(collector.finish()),
        ))
    }
    /// Verifies the whole structural prefix before resolving the exact retained
    /// executor. Exact execution then compares every effect and original byte.
    pub fn replay(
        registry: &PackRegistryV1,
        genesis_bytes: &[u8],
        transition_bytes: &[Vec<u8>],
    ) -> Result<CanonicalReplayReport, ReplayFailureV1> {
        Self::replay_registry_canonical(registry, genesis_bytes, transition_bytes, false)
    }
    pub(crate) fn replay_registry_canonical(
        registry: &PackRegistryV1,
        genesis_bytes: &[u8],
        transition_bytes: &[Vec<u8>],
        observations: bool,
    ) -> Result<CanonicalReplayReport, ReplayFailureV1> {
        let mut preflight = CanonicalStorageHistoryPreflight::begin(genesis_bytes)?;
        preflight.consume_transition_page(transition_bytes)?;
        let expected_head = preflight.final_head().clone();
        let executable = preflight.begin_executable(&expected_head, registry)?;
        let generations = executable.membership_generations;
        let mut trace = executable.trace;
        let mut collector = observations
            .then(|| CanonicalOperationalCollector::begin(trace.genesis(), None))
            .transpose()?;
        let mut steps = if observations {
            Vec::new()
        } else {
            vec![canonical_replay_step(&trace, genesis_bytes)?]
        };
        let mut frame_heads = trace
            .core_state()
            .memberships()
            .keys()
            .cloned()
            .map(|id| (id, 0))
            .collect();
        let mut consequences = Vec::new();
        for bytes in transition_bytes {
            trace.replay_canonical_transition(
                bytes,
                observations.then_some((&mut frame_heads, &mut consequences)),
            )?;
            if let Some(collector) = &mut collector {
                collector.consume(trace.transitions().last().ok_or_else(|| {
                    replay_error(
                        trace.head(),
                        ReplayFailureClassV1::CoreInvariant,
                        "verified Transition absent".into(),
                    )
                })?)?;
            }
            if !observations {
                steps.push(canonical_replay_step(&trace, bytes)?);
            }
        }
        compare_canonical_materializations(&trace, &expected_head, None, None)?;
        Ok(CanonicalReplayReport::from_trace(
            trace,
            steps,
            consequences,
            generations,
            collector.map(CanonicalOperationalCollector::finish),
        ))
    }
    /// Exact full storage Replay with optional disposable serving materials.
    pub fn replay_for_storage(
        registry: &PackRegistryV1,
        expected_head: &CompleteHeadV1,
        genesis_bytes: &[u8],
        transition_bytes: &[Vec<u8>],
        core_bytes: Option<&[u8]>,
        activity_bytes: Option<&[u8]>,
    ) -> Result<CanonicalReplayReport, ReplayFailureV1> {
        let report =
            Self::replay_registry_canonical(registry, genesis_bytes, transition_bytes, true)?;
        compare_canonical_materializations(
            &report.trace,
            expected_head,
            core_bytes,
            activity_bytes,
        )?;
        Ok(report)
    }
    pub fn verify_executable_history_for_storage(
        registry: &PackRegistryV1,
        expected_head: &CompleteHeadV1,
        genesis_bytes: &[u8],
        transition_bytes: &[Vec<u8>],
        core_bytes: Option<&[u8]>,
        activity_bytes: Option<&[u8]>,
    ) -> Result<ReplayStorageVerificationV1, ReplayFailureV1> {
        Self::replay_for_storage(
            registry,
            expected_head,
            genesis_bytes,
            transition_bytes,
            core_bytes,
            activity_bytes,
        )?
        .storage_verification()
    }
    /// Pure historical projection. Durable adapters must consume and revalidate
    /// present AuthorizedReplayV1 authority and captured integrity fences.
    pub fn project_replayed_history(
        registry: &PackRegistryV1,
        genesis_bytes: &[u8],
        transition_bytes: &[Vec<u8>],
        request: HistoricalReplayProjectionRequestV1,
    ) -> Result<HistoricalReplayProjectionV1, HistoricalReplayErrorV1> {
        let count = usize::try_from(request.at_room_seq.get())
            .map_err(|_| HistoricalReplayErrorV1::SequenceUnavailable)?;
        let prefix = transition_bytes
            .get(..count)
            .ok_or(HistoricalReplayErrorV1::SequenceUnavailable)?;
        let mut accumulator =
            CanonicalHistoricalReplayAccumulator::begin(registry, genesis_bytes, request)?;
        accumulator.consume_page(prefix)?;
        accumulator.finish()
    }
    pub(super) fn replay_canonical_transition(
        &mut self,
        bytes: &[u8],
        frame_output: Option<(
            &mut BTreeMap<MemberId, u64>,
            &mut Vec<ReplayObservationConsequenceV1>,
        )>,
    ) -> Result<(), ReplayFailureV1> {
        let stored = decode_canonical_successor(self.genesis(), self.head(), bytes)?;
        let prepared = self
            .prepare(stored.recorded_stimulus().clone())
            .map_err(|error| {
                replay_error(
                    self.head(),
                    classify_replay_advance_error(&error),
                    error.to_string(),
                )
            })?;
        let CanonicalAdvanceDisposition::TransitionAccepted {
            transition: generated,
            existing: false,
        } = prepared.disposition()
        else {
            return Err(replay_error(
                self.head(),
                ReplayFailureClassV1::Stimulus,
                "stored Transition replayed to a no-Transition disposition".into(),
            ));
        };
        compare_canonical_transition(generated, &stored)
            .map_err(|(class, detail)| replay_error(self.head(), class, detail))?;
        if generated.canonical_bytes().map_err(|error| {
            replay_error(
                self.head(),
                ReplayFailureClassV1::CanonicalEncoding,
                error.to_string(),
            )
        })? != bytes
        {
            return Err(replay_error(
                self.head(),
                ReplayFailureClassV1::RecordBytes,
                "replayed Transition differs from original canonical bytes".into(),
            ));
        }
        if let Some((frame_heads, consequences)) = frame_output {
            let state = prepared.resulting_state().ok_or_else(|| {
                replay_error(
                    self.head(),
                    ReplayFailureClassV1::CoreInvariant,
                    "prepared state absent".into(),
                )
            })?;
            let classified = crate::room_commit::prepare_canonical_transition_consequences(
                self,
                &prepared,
                state.core_state(),
                state.head().room_seq(),
                frame_heads,
            )
            .map_err(|error| {
                replay_error(
                    self.head(),
                    ReplayFailureClassV1::ActivityReduction,
                    error.to_string(),
                )
            })?;
            fold_replay_observation_consequences(classified, state, frame_heads, consequences)
                .map_err(|error| {
                    replay_error(
                        self.head(),
                        classify_replay_advance_error(&error),
                        error.to_string(),
                    )
                })?;
        }
        self.install_prepared(prepared).map_err(|error| {
            replay_error(
                self.head(),
                classify_replay_advance_error(&error),
                error.to_string(),
            )
        })?;
        Ok(())
    }
}

/// Bounded historical projection verifier. Structural corruption has priority
/// over absent or faulting exact runtime; opaque state is never substituted.
pub struct CanonicalHistoricalReplayAccumulator {
    request: HistoricalReplayProjectionRequestV1,
    preflight: CanonicalStorageHistoryPreflight,
    semantic: CanonicalHistoricalSemanticState,
}
enum CanonicalHistoricalSemanticState {
    Active(Box<CanonicalRoomTrace>),
    Failed(ReplayFailureClassV1),
}
impl CanonicalHistoricalReplayAccumulator {
    pub fn begin(
        registry: &PackRegistryV1,
        genesis_bytes: &[u8],
        request: HistoricalReplayProjectionRequestV1,
    ) -> Result<Self, HistoricalReplayErrorV1> {
        if request.integrity.status() == RoomIntegrityStatusV1::Quarantined {
            return Err(HistoricalReplayErrorV1::IntegrityUnavailable);
        }
        let preflight = CanonicalStorageHistoryPreflight::begin(genesis_bytes)
            .map_err(|failure| HistoricalReplayErrorV1::ReplayFailed(failure.class))?;
        if preflight.final_head().room_id() != &request.room_id {
            return Err(HistoricalReplayErrorV1::AddressMismatch);
        }
        let semantic = match retained_trace(
            registry,
            preflight.structural_state().genesis().clone(),
            preflight.final_head(),
            true,
        ) {
            Ok(trace) => CanonicalHistoricalSemanticState::Active(Box::new(trace)),
            Err(failure) => CanonicalHistoricalSemanticState::Failed(failure.class),
        };
        Ok(Self {
            request,
            preflight,
            semantic,
        })
    }
    pub fn consume_page(&mut self, page: &[Vec<u8>]) -> Result<(), HistoricalReplayErrorV1> {
        for bytes in page {
            if self.preflight.final_head().room_seq() >= self.request.at_room_seq {
                return Err(HistoricalReplayErrorV1::SequenceUnavailable);
            }
            self.preflight
                .consume_transition_page(std::slice::from_ref(bytes))
                .map_err(|failure| HistoricalReplayErrorV1::ReplayFailed(failure.class))?;
            if let CanonicalHistoricalSemanticState::Active(trace) = &mut self.semantic {
                if let Err(failure) = trace.replay_canonical_transition(bytes, None) {
                    self.semantic = CanonicalHistoricalSemanticState::Failed(failure.class);
                } else {
                    trace.discard_persisted_history();
                }
            }
        }
        Ok(())
    }
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.preflight.final_head().room_seq() == self.request.at_room_seq
    }
    pub fn finish(self) -> Result<HistoricalReplayProjectionV1, HistoricalReplayErrorV1> {
        if !self.is_complete() {
            return Err(HistoricalReplayErrorV1::SequenceUnavailable);
        }
        match self.semantic {
            CanonicalHistoricalSemanticState::Active(trace) => historical_projection_from_values(
                trace.head(),
                trace.core_state(),
                trace.activity_state(),
                trace.retained_pack(),
                self.request,
                trace.payload_budget(),
            ),
            CanonicalHistoricalSemanticState::Failed(class) => {
                Err(HistoricalReplayErrorV1::ReplayFailed(class))
            }
        }
    }
}

fn retained_trace(
    registry: &PackRegistryV1,
    genesis: GenesisRecord,
    structural_head: &CompleteHeadV1,
    validate_initial_views: bool,
) -> Result<CanonicalRoomTrace, ReplayFailureV1> {
    let request = PackGenesisRequestV1 {
        room_id: genesis.room_id().clone(),
        pack_digest: genesis.pack_digest().clone(),
        configuration: genesis.configuration().clone(),
        room_seed: genesis.room_seed().clone(),
        created_at: genesis.created_at().clone(),
        initial_core_state: genesis.initial_core_state().clone(),
    };
    let verified = registry
        .prepare_genesis_for_retained_room(&request)
        .map_err(|error| map_retained_genesis_error(&error, structural_head.clone()))?;
    let input = verified.genesis_input();
    if input.initial_activity_state != *genesis.initial_activity_state()
        || input.initial_timers != genesis.initial_timers()
        || input.initial_core_state != *genesis.initial_core_state()
        || input.room_id != *genesis.room_id()
        || input.pack_digest != *genesis.pack_digest()
        || input.configuration != *genesis.configuration()
        || input.room_seed != *genesis.room_seed()
        || input.created_at != *genesis.created_at()
    {
        return Err(ReplayFailureV1::without_head(
            ReplayFailureClassV1::ActivityReduction,
            "retained initialization differs from exact stored Genesis".into(),
        ));
    }
    let trace =
        CanonicalRoomTrace::from_verified_genesis(genesis, verified.retained_pack().clone())
            .map_err(|error| {
                ReplayFailureV1::without_head(classify_genesis_error(&error), error.to_string())
            })?;
    if validate_initial_views {
        trace.validate_initial_authorized_views().map_err(|error| {
            ReplayFailureV1::without_head(classify_genesis_error(&error), error.to_string())
        })?;
    }
    Ok(trace)
}
fn codec_failure(error: LineageCodecError, head: Option<&CompleteHeadV1>) -> ReplayFailureV1 {
    let class = match error {
        LineageCodecError::Canonical(_) => ReplayFailureClassV1::NonCanonicalRecord,
        LineageCodecError::UnsupportedIdentity | LineageCodecError::MixedFormat => {
            ReplayFailureClassV1::VersionOrDigest
        }
        LineageCodecError::CommitmentMismatch => ReplayFailureClassV1::StateHashes,
        LineageCodecError::SuccessorMismatch => ReplayFailureClassV1::PriorHash,
        LineageCodecError::InvalidCoreOrGenesis => ReplayFailureClassV1::CoreInvariant,
    };
    if let Some(head) = head {
        replay_error(head, class, error.to_string())
    } else {
        ReplayFailureV1::without_head(class, error.to_string())
    }
}
fn replay_error(
    head: &CompleteHeadV1,
    class: ReplayFailureClassV1,
    detail: String,
) -> ReplayFailureV1 {
    ReplayFailureV1::with_head(class, detail, head.clone())
}
fn decode_canonical_successor(
    genesis: &GenesisRecord,
    head: &CompleteHeadV1,
    bytes: &[u8],
) -> Result<TransitionRecord, ReplayFailureV1> {
    let record = genesis
        .decode_transition(bytes)
        .map_err(|error| codec_failure(error, Some(head)))?;
    genesis
        .check_transition_payload_budget(&record, bytes)
        .map_err(|error| replay_error(head, ReplayFailureClassV1::Stimulus, error.to_string()))?;
    if record.room_seq()
        != head.room_seq().checked_successor().map_err(|error| {
            replay_error(head, ReplayFailureClassV1::Sequence, error.to_string())
        })?
    {
        return Err(replay_error(
            head,
            ReplayFailureClassV1::Sequence,
            "record is not exact successor sequence".into(),
        ));
    }
    if record.previous_lineage_hash() != head.genesis_or_transition_hash() {
        return Err(replay_error(
            head,
            ReplayFailureClassV1::PriorHash,
            "record predecessor differs".into(),
        ));
    }
    if record.room_id() != head.room_id()
        || record.pack_digest() != head.pack_digest()
        || record.core_schema_version() != head.core_schema_version()
    {
        return Err(replay_error(
            head,
            ReplayFailureClassV1::VersionOrDigest,
            "record Room, Pack, or Core schema differs".into(),
        ));
    }
    record
        .verify_successor(head)
        .map_err(|error| codec_failure(error, Some(head)))?;
    Ok(record)
}
fn compare_canonical_transition(
    generated: &TransitionRecord,
    stored: &TransitionRecord,
) -> Result<(), (ReplayFailureClassV1, String)> {
    if let (TransitionRecord::V1(generated), TransitionRecord::V1(stored)) = (generated, stored) {
        return compare_transition(generated, stored);
    }
    if generated.format() != stored.format() {
        return Err((
            ReplayFailureClassV1::VersionOrDigest,
            "record format differs".into(),
        ));
    }
    for (different, class, detail) in [
        (
            generated.ordered_domain_events() != stored.ordered_domain_events(),
            ReplayFailureClassV1::DomainEvents,
            "ordered Domain Events differ",
        ),
        (
            generated.ordered_timer_changes() != stored.ordered_timer_changes(),
            ReplayFailureClassV1::TimerChanges,
            "ordered Timer changes differ",
        ),
        (
            generated.ordered_attention_signals() != stored.ordered_attention_signals(),
            ReplayFailureClassV1::AttentionSignals,
            "ordered Attention differs",
        ),
        (
            generated.resulting_core_state_hash() != stored.resulting_core_state_hash(),
            ReplayFailureClassV1::CoreState,
            "resulting Core commitment differs",
        ),
        (
            generated.resulting_activity_state_hash() != stored.resulting_activity_state_hash(),
            ReplayFailureClassV1::ActivityState,
            "resulting Activity commitment differs",
        ),
        (
            generated.resulting_authoritative_state_hash()
                != stored.resulting_authoritative_state_hash(),
            ReplayFailureClassV1::StateHashes,
            "aggregate state commitment differs",
        ),
        (
            generated.transition_hash() != stored.transition_hash(),
            ReplayFailureClassV1::LineageHash,
            "record hash differs",
        ),
    ] {
        if different {
            return Err((class, detail.into()));
        }
    }
    Ok(())
}
fn compare_canonical_materializations(
    trace: &CanonicalRoomTrace,
    head: &CompleteHeadV1,
    core_bytes: Option<&[u8]>,
    activity_bytes: Option<&[u8]>,
) -> Result<(), ReplayFailureV1> {
    if trace.head() != head {
        return Err(replay_error(
            trace.head(),
            ReplayFailureClassV1::LineageHash,
            "executed history does not reach captured Head".into(),
        ));
    }
    if let Some(bytes) = core_bytes {
        if trace.core_state().canonical_bytes().map_err(|error| {
            replay_error(
                head,
                ReplayFailureClassV1::CanonicalEncoding,
                error.to_string(),
            )
        })? != bytes
        {
            return Err(replay_error(
                head,
                ReplayFailureClassV1::CoreState,
                "executed Core differs from supplied materialization".into(),
            ));
        }
    }
    if let Some(bytes) = activity_bytes {
        if trace.activity_state().to_bytes().map_err(|error| {
            replay_error(
                head,
                ReplayFailureClassV1::CanonicalEncoding,
                error.to_string(),
            )
        })? != bytes
        {
            return Err(replay_error(
                head,
                ReplayFailureClassV1::ActivityState,
                "executed Activity differs from supplied materialization".into(),
            ));
        }
    }
    Ok(())
}
pub(crate) fn advance_membership_generations(
    before: &CoreRoomStateV1,
    after: &CoreRoomStateV1,
    generations: &mut BTreeMap<String, i64>,
) -> Result<(), String> {
    if before
        .memberships()
        .keys()
        .any(|id| !after.memberships().contains_key(id))
    {
        return Err("Core removed retained Membership".into());
    }
    for (id, membership) in after.memberships() {
        match before.memberships().get(id) {
            Some(prior) if prior != membership => {
                let generation = generations
                    .get_mut(id.as_str())
                    .ok_or("Membership generation is absent")?;
                *generation = generation
                    .checked_add(1)
                    .filter(|value| *value <= crate::MAX_SAFE_INTEGER)
                    .ok_or("Membership generation overflow")?;
            }
            Some(_) => {}
            None => {
                if generations.insert(id.to_string(), 1).is_some() {
                    return Err("Membership identity was reused".into());
                }
            }
        }
    }
    Ok(())
}
fn canonical_replay_step(
    trace: &CanonicalRoomTrace,
    bytes: &[u8],
) -> Result<ReplayStepV1, ReplayFailureV1> {
    Ok(ReplayStepV1 {
        head: trace.head().clone(),
        canonical_core_bytes: trace.core_state().canonical_bytes().map_err(|error| {
            replay_error(
                trace.head(),
                ReplayFailureClassV1::CanonicalEncoding,
                error.to_string(),
            )
        })?,
        canonical_activity_bytes: trace.activity_state().to_bytes().map_err(|error| {
            replay_error(
                trace.head(),
                ReplayFailureClassV1::CanonicalEncoding,
                error.to_string(),
            )
        })?,
        canonical_lineage_record_bytes: bytes.to_vec(),
    })
}
