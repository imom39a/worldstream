use std::{
    collections::{BTreeMap, VecDeque},
    fmt::Display,
    str::FromStr,
    sync::Mutex,
};

use super::*;

const ROOM: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const MEMBER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FC0";
const PRINCIPAL: &str = "01ARZ3NDEKTSV4RRFFQ69G5FD0";
const ACTION: &str = "01ARZ3NDEKTSV4RRFFQ69G5FC3";
const TIMER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FC4";
const SEED: &str = "hex:000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

fn parsed<T>(value: &str) -> T
where
    T: FromStr,
    T::Err: Display,
{
    value
        .parse()
        .unwrap_or_else(|error| unreachable!("fixture {value}: {error}"))
}

fn canonical(value: &[u8]) -> CanonicalJsonV1 {
    CanonicalJsonV1::parse(value)
        .unwrap_or_else(|error| unreachable!("fixture canonical JSON: {error}"))
}

fn participant_with(member_id: &str) -> MembershipV1 {
    MembershipV1::new(
        parsed(member_id),
        parsed(PRINCIPAL),
        PrincipalKindV1::Human,
        MembershipStandingV1::Enabled,
        AccessModeV1::Participant,
        Some("counter".to_owned()),
    )
    .unwrap_or_else(|error| unreachable!("fixture membership: {error}"))
}

fn creation_request(initial_value: u32) -> RoomCreationRequestV1 {
    RoomCreationRequestV1::new(
        counter_v2_digest(),
        canonical(format!(r#"{{"initial_value":{initial_value},"maximum_value":4}}"#).as_bytes()),
        vec![
            InitialMembershipProposalV1::new(
                parsed(PRINCIPAL),
                PrincipalKindV1::Human,
                MembershipStandingV1::Enabled,
                AccessModeV1::Participant,
                Some("counter".to_owned()),
            )
            .unwrap_or_else(|error| unreachable!("fixture proposal: {error}")),
        ],
    )
}

fn authority() -> PreparedAuthorityWitnessV1 {
    authority_with("room-commit-test-authority", parsed(PRINCIPAL), 1)
}

fn authority_with(
    witness_id: &str,
    authenticated_principal: PrincipalId,
    generation: u64,
) -> PreparedAuthorityWitnessV1 {
    PreparedAuthorityWitnessV1::new(
        witness_id,
        authenticated_principal,
        generation,
        &canonical(br#"{"scope":"room-commit-test","revoked":false}"#),
    )
    .unwrap_or_else(|error| unreachable!("fixture authority: {error}"))
}

fn creation_identity() -> AdministrationOperationIdentityV1 {
    AdministrationOperationIdentityV1 {
        authenticated_principal: parsed(PRINCIPAL),
        versioned_operation_kind: CREATE_ROOM_OPERATION_KIND.to_owned(),
        idempotency_key: "room-commit-test-create".to_owned(),
    }
}

fn counter_genesis(initial_value: u32) -> PreparedNewRoomGenesisV1 {
    counter_genesis_with(initial_value, ROOM, MEMBER, SEED, "2026-08-15T12:00:00Z")
}

fn counter_genesis_with(
    initial_value: u32,
    room_id: &str,
    member_id: &str,
    room_seed: &str,
    created_at: &str,
) -> PreparedNewRoomGenesisV1 {
    let registry = builtin_counter_registry()
        .unwrap_or_else(|error| unreachable!("Counter registry: {error}"));
    let request = PackGenesisRequestV1 {
        room_id: parsed(room_id),
        pack_digest: counter_v2_digest(),
        configuration: canonical(
            format!(r#"{{"initial_value":{initial_value},"maximum_value":4}}"#).as_bytes(),
        ),
        room_seed: parsed(room_seed),
        created_at: parsed(created_at),
        initial_core_state: CoreRoomStateV1::active([participant_with(member_id)])
            .unwrap_or_else(|error| unreachable!("fixture Core state: {error}")),
    };
    registry
        .prepare_genesis_for_new_room(&request)
        .unwrap_or_else(|error| unreachable!("fixture Counter Genesis: {error}"))
}

fn creation_plan(initial_value: u32) -> PreparedRoomCreationV1 {
    PreparedRoomCreationV1::from_registry_genesis(
        creation_identity(),
        &creation_request(initial_value),
        authority(),
        counter_genesis(initial_value),
    )
    .unwrap_or_else(|error| unreachable!("fixture Create plan: {error}"))
}

fn counter_trace() -> CoreTraceV1 {
    CoreTraceV1::create_uncommitted(counter_genesis(0))
        .unwrap_or_else(|error| unreachable!("fixture Counter trace: {error}"))
}

fn action_plan() -> (CoreTraceV1, PreparedRoomCommitV1) {
    let trace = counter_trace();
    let action_schema = trace
        .retained_pack()
        .unwrap_or_else(|| unreachable!("retained Counter pack"))
        .descriptor()
        .actions
        .iter()
        .find(|action| action.action_type == "increment")
        .unwrap_or_else(|| unreachable!("Counter increment action"))
        .payload_schema
        .schema_digest
        .clone();
    let request = ParticipantActionRequestV1::new(
        parsed(ROOM),
        parsed(MEMBER),
        parsed(ACTION),
        trace.head().room_seq(),
        "increment",
        canonical(br"{}"),
    );
    let prepared = trace
        .prepare(RecordedStimulusV1::ParticipantAction(ParticipantActionV1 {
            member_id: parsed(MEMBER),
            action_id: parsed(ACTION),
            action_type: "increment".to_owned(),
            payload_schema_digest: action_schema,
            canonical_payload: canonical(br"{}"),
            exact_basis_head: trace.head().clone(),
            admitted_at: parsed("2026-08-15T12:00:01Z"),
        }))
        .unwrap_or_else(|error| unreachable!("fixture Action preparation: {error}"));
    let frame_heads = trace
        .core_state()
        .memberships()
        .keys()
        .cloned()
        .map(|member| (member, 0))
        .collect::<BTreeMap<_, _>>();
    let plan = PreparedRoomCommitV1::for_action(
        &trace,
        &request,
        prepared,
        parsed("01ARZ3NDEKTSV4RRFFQ69G5FC5"),
        IntegrityGenerationV1::new(1)
            .unwrap_or_else(|error| unreachable!("fixture integrity generation: {error}")),
        authority(),
        &frame_heads,
    )
    .unwrap_or_else(|error| unreachable!("fixture Action plan: {error}"));
    (trace, plan)
}

fn counter_action_trace() -> CoreTraceV1 {
    let mut trace = counter_trace();
    let action_schema = trace
        .retained_pack()
        .unwrap_or_else(|| unreachable!("retained Counter pack"))
        .descriptor()
        .actions
        .iter()
        .find(|action| action.action_type == "increment")
        .unwrap_or_else(|| unreachable!("Counter increment action"))
        .payload_schema
        .schema_digest
        .clone();
    trace
        .advance(RecordedStimulusV1::ParticipantAction(ParticipantActionV1 {
            member_id: parsed(MEMBER),
            action_id: parsed(ACTION),
            action_type: "increment".to_owned(),
            payload_schema_digest: action_schema,
            canonical_payload: canonical(br"{}"),
            exact_basis_head: trace.head().clone(),
            admitted_at: parsed("2026-08-15T12:00:01Z"),
        }))
        .unwrap_or_else(|error| unreachable!("fixture Action advance: {error}"));
    trace
}

fn lifecycle_stimulus(
    trace: &CoreTraceV1,
    kind: CoreProposedKindV1,
    changeset: CoreChangeSetV1,
    idempotency_key: &str,
    recorded_at: &str,
) -> RecordedStimulusV1 {
    RecordedStimulusV1::CoreProposed(CoreProposedV1::new(
        kind,
        CoreAuthorityAttributionV1 {
            principal_id: parsed(PRINCIPAL),
            authority_kind: CoreAuthorityKindV1::RoomAdministrator,
        },
        AdministrationOperationIdentityV1 {
            authenticated_principal: parsed(PRINCIPAL),
            versioned_operation_kind: CORE_OPERATION_KIND.to_owned(),
            idempotency_key: idempotency_key.to_owned(),
        },
        trace.head().room_seq(),
        "room-commit-test-lifecycle",
        parsed(recorded_at),
        changeset,
    ))
}

fn install_lifecycle_transition(trace: &mut CoreTraceV1, stimulus: RecordedStimulusV1) {
    assert!(matches!(
        trace
            .advance(stimulus)
            .unwrap_or_else(|error| unreachable!("fixture lifecycle transition: {error}")),
        AdvanceDispositionV1::TransitionAccepted {
            existing: false,
            ..
        }
    ));
}

fn archive_trace(suspended: bool) -> CoreTraceV1 {
    let mut trace = counter_trace();
    if suspended {
        let membership = trace
            .core_state()
            .membership(&parsed(MEMBER))
            .unwrap_or_else(|| unreachable!("fixture membership"))
            .clone();
        let stimulus = lifecycle_stimulus(
            &trace,
            CoreProposedKindV1::Suspend,
            CoreChangeSetV1::one(MembershipChangeV1::suspend(membership)),
            "room-commit-test-suspend",
            "2026-08-15T12:00:01Z",
        );
        install_lifecycle_transition(&mut trace, stimulus);
    }
    let stimulus = lifecycle_stimulus(
        &trace,
        CoreProposedKindV1::Archive,
        CoreChangeSetV1::archive(trace.core_state().room_status()),
        "room-commit-test-archive",
        "2026-08-15T12:00:02Z",
    );
    install_lifecycle_transition(&mut trace, stimulus);
    trace
}

fn action_request(
    trace: &CoreTraceV1,
    based_on_room_seq: RoomSequenceV1,
    payload: &[u8],
) -> ParticipantActionRequestV1 {
    ParticipantActionRequestV1::new(
        trace.head().room_id().clone(),
        parsed(MEMBER),
        parsed(ACTION),
        based_on_room_seq,
        "increment",
        canonical(payload),
    )
}

fn timer_plan() -> (CoreTraceV1, PreparedRoomCommitV1) {
    let trace = CoreTraceV1::create_for_conformance(
        GenesisInputV1::new(
            parsed(ROOM),
            counter_v2_digest(),
            canonical(br#"{"fixture":"timer"}"#),
            parsed(SEED),
            parsed("2026-08-15T12:00:00Z"),
            CoreRoomStateV1::active(std::iter::empty::<MembershipV1>())
                .unwrap_or_else(|error| unreachable!("fixture Core state: {error}")),
            canonical(br#"{"fixture":"timer"}"#),
        )
        .with_initial_timers(vec![ScheduledTimerV1 {
            timer_id: parsed(TIMER),
            generation: TimerGenerationV1::new(1)
                .unwrap_or_else(|error| unreachable!("fixture Timer generation: {error}")),
            scheduled_for: parsed("2026-08-15T12:30:00Z"),
            canonical_payload: canonical(br#"{"kind":"deadline"}"#),
        }]),
        |_| Ok(()),
        |input| {
            Ok(ActivityDispositionV1::Apply(ActivityApplyV1 {
                next_activity_state: input.prior_activity_state.clone(),
                ordered_domain_events: Vec::new(),
                timer_requests: Vec::new(),
                ordered_attention_signals: Vec::new(),
            }))
        },
    )
    .unwrap_or_else(|error| unreachable!("fixture Timer trace: {error}"));
    let request = TimerFiredRequestV1::new(
        parsed(ROOM),
        parsed(TIMER),
        TimerGenerationV1::new(1)
            .unwrap_or_else(|error| unreachable!("fixture Timer generation: {error}")),
        parsed("2026-08-15T12:30:00Z"),
        canonical(br#"{"kind":"deadline"}"#),
    );
    let prepared = trace
        .prepare(RecordedStimulusV1::TimerFired(TimerFiredV1 {
            timer_id: parsed(TIMER),
            generation: TimerGenerationV1::new(1)
                .unwrap_or_else(|error| unreachable!("fixture Timer generation: {error}")),
            scheduled_for: parsed("2026-08-15T12:30:00Z"),
            canonical_payload: canonical(br#"{"kind":"deadline"}"#),
        }))
        .unwrap_or_else(|error| unreachable!("fixture Timer preparation: {error}"));
    let frame_heads = trace
        .core_state()
        .memberships()
        .keys()
        .cloned()
        .map(|member| (member, 0))
        .collect::<BTreeMap<_, _>>();
    let plan = PreparedRoomCommitV1::for_timer_fired(
        &trace,
        &request,
        prepared,
        parsed("01ARZ3NDEKTSV4RRFFQ69G5FC6"),
        IntegrityGenerationV1::new(1)
            .unwrap_or_else(|error| unreachable!("fixture integrity generation: {error}")),
        authority(),
        &frame_heads,
    )
    .unwrap_or_else(|error| unreachable!("fixture Timer plan: {error}"));
    (trace, plan)
}

enum CommitReply {
    Exact,
    Fixed(RoomCommitResolutionV1),
    Indeterminate,
    NotApplicable,
}

struct ScriptedStorage {
    commits: Mutex<VecDeque<CommitReply>>,
    resolutions: Mutex<VecDeque<ResolveOutcomeV1>>,
    observed: Mutex<Vec<(Vec<u8>, CanonicalRequestHashV1)>>,
}

struct RecoveryStorage {
    candidate: RoomRecoveryCandidateV1,
    recorded: Mutex<Vec<RecoveryIntegrityDispositionV1>>,
}

impl RecoveryStorage {
    fn new(candidate: RoomRecoveryCandidateV1) -> Self {
        Self {
            candidate,
            recorded: Mutex::new(Vec::new()),
        }
    }

    fn recorded(&self) -> Vec<RecoveryIntegrityDispositionV1> {
        self.recorded
            .lock()
            .unwrap_or_else(|_| unreachable!("recovery storage mutex"))
            .clone()
    }
}

impl RoomRecoveryStorageV1 for RecoveryStorage {
    fn inspect_recovery_candidate(
        &self,
        _room_id: &RoomId,
    ) -> Result<Option<RoomRecoveryCandidateV1>, RoomRecoveryErrorV1> {
        Ok(Some(self.candidate.clone()))
    }

    fn guard_recovery_install(
        &self,
        _room_id: &RoomId,
        _expected_head: &CompleteHeadV1,
        _expected_integrity_generation: IntegrityGenerationV1,
        _recovered_materializations: &RecoveredRoomMaterializationsV1,
    ) -> Result<(), RoomRecoveryErrorV1> {
        Ok(())
    }

    fn record_recovery_failure(
        &self,
        _room_id: &RoomId,
        _expected_head: &CompleteHeadV1,
        _expected_integrity_generation: IntegrityGenerationV1,
        disposition: RecoveryIntegrityDispositionV1,
    ) -> Result<(), RoomRecoveryErrorV1> {
        self.recorded
            .lock()
            .unwrap_or_else(|_| unreachable!("recovery storage mutex"))
            .push(disposition);
        Ok(())
    }
}

fn recovery_candidate(
    genesis: &GenesisV1,
    transitions: &[TransitionV1],
) -> RoomRecoveryCandidateV1 {
    let head = transitions
        .last()
        .map_or_else(|| genesis.complete_head(), TransitionV1::complete_head);
    let (core, activity) = transitions.last().map_or_else(
        || {
            (
                genesis.initial_core_state().clone(),
                genesis.initial_activity_state().clone(),
            )
        },
        |transition| {
            (
                transition.resulting_core_state().clone(),
                transition.resulting_activity_state().clone(),
            )
        },
    );
    let registry = builtin_counter_registry()
        .unwrap_or_else(|error| unreachable!("Counter registry: {error}"));
    let lock = registry
        .load_retained(head.pack_digest())
        .unwrap_or_else(|error| unreachable!("retained Counter revision: {error}"))
        .revision_lock()
        .canonical_bytes()
        .unwrap_or_else(|error| unreachable!("revision lock bytes: {error}"));
    RoomRecoveryCandidateV1::new(
        head.clone(),
        IntegrityGenerationV1::new(1)
            .unwrap_or_else(|error| unreachable!("integrity generation: {error}")),
        head.canonical_bytes()
            .unwrap_or_else(|error| unreachable!("Head bytes: {error}")),
        lock,
        genesis
            .canonical_bytes()
            .unwrap_or_else(|error| unreachable!("Genesis bytes: {error}")),
        transitions
            .iter()
            .map(|transition| {
                transition
                    .canonical_bytes()
                    .unwrap_or_else(|error| unreachable!("Transition bytes: {error}"))
            })
            .collect(),
        Some(
            core.canonical_bytes()
                .unwrap_or_else(|error| unreachable!("Core bytes: {error}")),
        ),
        Some(
            activity
                .to_bytes()
                .unwrap_or_else(|error| unreachable!("Activity bytes: {error}")),
        ),
    )
}

fn assert_candidate_quarantines(candidate: RoomRecoveryCandidateV1, registry: &PackRegistryV1) {
    let storage = RecoveryStorage::new(candidate);
    assert!(matches!(
        recover_room_from_storage(&storage, registry, &parsed(ROOM)),
        Err(RoomRecoveryErrorV1::Corrupt)
    ));
    assert_eq!(
        storage.recorded(),
        vec![RecoveryIntegrityDispositionV1::Quarantined]
    );
}

fn assert_candidate_quarantines_before_missing_runtime(candidate: RoomRecoveryCandidateV1) {
    let missing_runtime = counter_v1_only_registry_for_conformance()
        .unwrap_or_else(|error| unreachable!("v1-only registry: {error}"));
    assert_candidate_quarantines(candidate, &missing_runtime);
}

fn refresh_genesis_hashes(genesis: &mut GenesisV1) {
    genesis.initial_core_state_hash = crate::lineage::hash_core_state(&genesis.initial_core_state)
        .unwrap_or_else(|error| unreachable!("Core hash: {error}"));
    genesis.initial_activity_state_hash =
        crate::lineage::hash_activity_state(&genesis.pack_digest, &genesis.initial_activity_state)
            .unwrap_or_else(|error| unreachable!("Activity hash: {error}"));
    genesis.initial_authoritative_state_hash = crate::lineage::hash_authoritative_state(
        &genesis.pack_digest,
        &genesis.initial_core_state_hash,
        &genesis.initial_activity_state_hash,
    )
    .unwrap_or_else(|error| unreachable!("authoritative hash: {error}"));
    genesis.genesis_hash = genesis
        .calculate_hash()
        .unwrap_or_else(|error| unreachable!("Genesis hash: {error}"));
}

fn refresh_transition_hashes(transition: &mut TransitionV1) {
    transition.resulting_core_state_hash =
        crate::lineage::hash_core_state(&transition.resulting_core_state)
            .unwrap_or_else(|error| unreachable!("Core hash: {error}"));
    transition.resulting_activity_state_hash = crate::lineage::hash_activity_state(
        &transition.pack_digest,
        &transition.resulting_activity_state,
    )
    .unwrap_or_else(|error| unreachable!("Activity hash: {error}"));
    transition.resulting_authoritative_state_hash = crate::lineage::hash_authoritative_state(
        &transition.pack_digest,
        &transition.resulting_core_state_hash,
        &transition.resulting_activity_state_hash,
    )
    .unwrap_or_else(|error| unreachable!("authoritative hash: {error}"));
    transition.transition_hash = transition
        .calculate_hash()
        .unwrap_or_else(|error| unreachable!("Transition hash: {error}"));
}

impl ScriptedStorage {
    fn new(
        commits: impl IntoIterator<Item = CommitReply>,
        resolutions: impl IntoIterator<Item = ResolveOutcomeV1>,
    ) -> Self {
        Self {
            commits: Mutex::new(commits.into_iter().collect()),
            resolutions: Mutex::new(resolutions.into_iter().collect()),
            observed: Mutex::new(Vec::new()),
        }
    }

    fn observed(&self) -> Vec<(Vec<u8>, CanonicalRequestHashV1)> {
        self.observed
            .lock()
            .unwrap_or_else(|_| unreachable!("test storage mutex"))
            .clone()
    }

    fn record(&self, identity: &OperationIdentityV1, hash: &CanonicalRequestHashV1) {
        self.observed
            .lock()
            .unwrap_or_else(|_| unreachable!("test storage mutex"))
            .push((
                identity
                    .canonical_bytes()
                    .unwrap_or_else(|error| unreachable!("canonical identity: {error}")),
                hash.clone(),
            ));
    }
}

impl RoomCommitStorageV1 for ScriptedStorage {
    fn commit(&self, prepared: &PreparedRoomWriteV1) -> RoomCommitResolutionV1 {
        self.record(prepared.identity(), prepared.request_hash());
        let reply = self
            .commits
            .lock()
            .unwrap_or_else(|_| unreachable!("test storage mutex"))
            .pop_front()
            .unwrap_or_else(|| unreachable!("unexpected commit"));
        match reply {
            CommitReply::Exact => match prepared {
                PreparedRoomWriteV1::Create(create) => RoomCommitResolutionV1::resolved(
                    ResolutionStatusV1::New,
                    create.semantic_result().clone(),
                ),
                PreparedRoomWriteV1::Existing(existing) => RoomCommitResolutionV1::resolved(
                    ResolutionStatusV1::New,
                    existing.semantic_result().clone(),
                ),
            },
            CommitReply::Fixed(resolution) => resolution,
            CommitReply::Indeterminate => RoomCommitResolutionV1::Indeterminate,
            CommitReply::NotApplicable => RoomCommitResolutionV1::NotApplicable,
        }
    }

    fn resolve(
        &self,
        identity: &OperationIdentityV1,
        request_hash: &CanonicalRequestHashV1,
    ) -> ResolveOutcomeV1 {
        self.record(identity, request_hash);
        self.resolutions
            .lock()
            .unwrap_or_else(|_| unreachable!("test storage mutex"))
            .pop_front()
            .unwrap_or_else(|| unreachable!("unexpected resolve"))
    }
}

#[test]
fn create_releases_only_the_exact_new_genesis_receipt() {
    let plan = creation_plan(0);
    let expected_head = plan.persistence().complete_head.clone();
    let storage = ScriptedStorage::new([CommitReply::Exact], []);
    let outcome = commit_room_creation(&storage, plan);
    assert!(matches!(
        outcome.resolution(),
        RoomCommitResolutionV1::GenesisCreated {
            status: ResolutionStatusV1::New,
            ..
        }
    ));
    let trace = outcome
        .into_committed_trace()
        .unwrap_or_else(|| unreachable!("exact new Create releases trace"));
    assert_eq!(trace.head(), &expected_head);

    let mutated = creation_plan(1).semantic_result().clone();
    let storage = ScriptedStorage::new(
        [CommitReply::Fixed(RoomCommitResolutionV1::GenesisCreated {
            status: ResolutionStatusV1::New,
            result: Box::new(mutated),
        })],
        [],
    );
    let outcome = commit_room_creation(&storage, creation_plan(0));
    assert!(matches!(
        outcome.resolution(),
        RoomCommitResolutionV1::Fault
    ));
    assert!(outcome.into_committed_trace().is_none());
}

#[test]
fn create_reprepare_reseals_new_genesis_without_changing_caller_identity() {
    let prepared = creation_plan(0);
    let expected_identity = prepared.semantic_result().operation_identity().clone();
    let expected_hash = prepared.semantic_result().canonical_request_hash().clone();
    let expected_lock = prepared.persistence().pack_revision_lock.clone();
    let old_room_id = prepared.persistence().complete_head.room_id().clone();
    let old_member_id = prepared.persistence().memberships[0]
        .membership
        .member_id()
        .clone();
    let old_seed = prepared.persistence().genesis.room_seed().clone();
    let old_created_at = prepared.persistence().genesis.created_at().clone();
    let storage = ScriptedStorage::new([CommitReply::Fixed(RoomCommitResolutionV1::Reprepare)], []);
    let (_, trace, pending) = commit_room_creation(&storage, prepared).into_parts();
    assert!(trace.is_none());
    let Some(RoomCreationPendingAttemptV1::Reprepare(reprepare)) = pending else {
        unreachable!("Create collision exposes only the opaque reprepare token");
    };
    assert_eq!(reprepare.selected_pack_revision_lock(), &expected_lock);

    let resealed = reprepare
        .reseal(
            authority_with("room-commit-test-authority-refreshed", parsed(PRINCIPAL), 2),
            counter_genesis_with(
                0,
                "01ARZ3NDEKTSV4RRFFQ69G5FAW",
                "01ARZ3NDEKTSV4RRFFQ69G5FC1",
                "hex:101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f",
                "2026-08-15T13:00:00Z",
            ),
        )
        .unwrap_or_else(|error| unreachable!("same-principal reseal: {error}"));
    assert_eq!(
        resealed.authority_witness().authenticated_principal(),
        &parsed(PRINCIPAL)
    );
    assert_eq!(resealed.authority_witness().generation(), 2);
    assert_eq!(resealed.persistence().pack_revision_lock, expected_lock);
    assert_ne!(resealed.persistence().complete_head.room_id(), &old_room_id);
    assert_ne!(
        resealed.persistence().memberships[0].membership.member_id(),
        &old_member_id
    );
    assert_ne!(resealed.persistence().genesis.room_seed(), &old_seed);
    assert_ne!(resealed.persistence().genesis.created_at(), &old_created_at);
    let resealed = PreparedRoomWriteV1::from(resealed);
    assert_eq!(resealed.identity(), &expected_identity);
    assert_eq!(resealed.request_hash(), &expected_hash);

    let storage = ScriptedStorage::new([CommitReply::Fixed(RoomCommitResolutionV1::Reprepare)], []);
    let (_, _, pending) = commit_room_creation(&storage, creation_plan(0)).into_parts();
    let Some(RoomCreationPendingAttemptV1::Reprepare(reprepare)) = pending else {
        unreachable!("second Create collision exposes reprepare token");
    };
    assert!(matches!(
        reprepare.reseal(
            authority_with(
                "room-commit-test-authority-wrong-principal",
                parsed("01ARZ3NDEKTSV4RRFFQ69G5FD1"),
                2,
            ),
            counter_genesis_with(
                0,
                "01ARZ3NDEKTSV4RRFFQ69G5FAX",
                "01ARZ3NDEKTSV4RRFFQ69G5FC2",
                "hex:202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f",
                "2026-08-15T13:01:00Z",
            ),
        ),
        Err(PrepareRoomWriteErrorV1::AuthorityIdentityMismatch)
    ));
}

#[test]
fn impossible_storage_result_variants_are_rejected_by_operation_kind() {
    let create_result = creation_plan(0).semantic_result().clone();
    let storage = ScriptedStorage::new(
        [CommitReply::Fixed(
            RoomCommitResolutionV1::TransitionCommitted {
                status: ResolutionStatusV1::New,
                result: Box::new(create_result),
            },
        )],
        [],
    );
    assert!(matches!(
        commit_room_creation(&storage, creation_plan(0)).resolution(),
        RoomCommitResolutionV1::Fault
    ));

    let (mut trace, action) = action_plan();
    let action_result = action.semantic_result().clone();
    let storage = ScriptedStorage::new(
        [CommitReply::Fixed(RoomCommitResolutionV1::GenesisCreated {
            status: ResolutionStatusV1::New,
            result: Box::new(action_result),
        })],
        [],
    );
    assert!(matches!(
        commit_existing_room(&storage, &mut trace, action).resolution(),
        RoomCommitResolutionV1::Fault
    ));

    let (mut trace, action) = action_plan();
    let storage = ScriptedStorage::new([CommitReply::NotApplicable], []);
    assert!(matches!(
        commit_existing_room(&storage, &mut trace, action).resolution(),
        RoomCommitResolutionV1::Fault
    ));

    let (mut trace, timer) = timer_plan();
    let storage = ScriptedStorage::new([CommitReply::NotApplicable], []);
    assert!(matches!(
        commit_existing_room(&storage, &mut trace, timer).resolution(),
        RoomCommitResolutionV1::NotApplicable
    ));
}

#[test]
#[allow(clippy::too_many_lines)]
fn invalid_action_payload_never_becomes_stable_but_archival_is_lifecycle_first() {
    let mut suspended = counter_trace();
    let membership = suspended
        .core_state()
        .membership(&parsed(MEMBER))
        .unwrap_or_else(|| unreachable!("fixture membership"))
        .clone();
    let stimulus = lifecycle_stimulus(
        &suspended,
        CoreProposedKindV1::Suspend,
        CoreChangeSetV1::one(MembershipChangeV1::suspend(membership)),
        "room-commit-test-invalid-payload-suspend",
        "2026-08-15T12:00:01Z",
    );
    install_lifecycle_transition(&mut suspended, stimulus);

    let active = counter_trace();
    let archived_enabled = archive_trace(false);
    let archived_suspended = archive_trace(true);
    for (label, trace, based_on_room_seq) in [
        (
            "active-stale",
            &active,
            RoomSequenceV1::new(1)
                .unwrap_or_else(|error| unreachable!("fixture sequence: {error}")),
        ),
        ("active-disabled", &suspended, suspended.head().room_seq()),
        (
            "archived-enabled-current",
            &archived_enabled,
            archived_enabled.head().room_seq(),
        ),
        (
            "archived-enabled-stale",
            &archived_enabled,
            RoomSequenceV1::new(0)
                .unwrap_or_else(|error| unreachable!("fixture sequence: {error}")),
        ),
        (
            "archived-suspended-current",
            &archived_suspended,
            archived_suspended.head().room_seq(),
        ),
        (
            "archived-suspended-stale",
            &archived_suspended,
            RoomSequenceV1::new(0)
                .unwrap_or_else(|error| unreachable!("fixture sequence: {error}")),
        ),
    ] {
        let request = action_request(trace, based_on_room_seq, br"[]");
        assert!(
            PreparedRoomCommitV1::for_stable_action_disposition(
                trace,
                &request,
                parsed("2026-08-15T12:01:00Z"),
                IntegrityGenerationV1::new(1)
                    .unwrap_or_else(|error| unreachable!("fixture integrity generation: {error}")),
                authority(),
            )
            .is_err(),
            "strict invalid payload unexpectedly sealed a stable receipt for {label}"
        );
    }

    for (label, trace, based_on_room_seq) in [
        (
            "archived-enabled-current",
            &archived_enabled,
            archived_enabled.head().room_seq(),
        ),
        (
            "archived-enabled-stale",
            &archived_enabled,
            RoomSequenceV1::new(0)
                .unwrap_or_else(|error| unreachable!("fixture sequence: {error}")),
        ),
        (
            "archived-suspended-current",
            &archived_suspended,
            archived_suspended.head().room_seq(),
        ),
        (
            "archived-suspended-stale",
            &archived_suspended,
            RoomSequenceV1::new(0)
                .unwrap_or_else(|error| unreachable!("fixture sequence: {error}")),
        ),
    ] {
        let request = action_request(trace, based_on_room_seq, br"{}");
        let prepared = PreparedRoomCommitV1::for_stable_action_disposition(
            trace,
            &request,
            parsed("2026-08-15T12:01:00Z"),
            IntegrityGenerationV1::new(1)
                .unwrap_or_else(|error| unreachable!("fixture integrity generation: {error}")),
            authority(),
        )
        .unwrap_or_else(|error| unreachable!("{label} archived stable receipt: {error}"));
        assert!(matches!(
            prepared.semantic_result().result(),
            SemanticResultV1::RejectionRecorded { code, .. } if code == "room_archived"
        ));
        let decoded = StoredSemanticResultV1::from_canonical_receipt_bytes(
            prepared.semantic_result().canonical_receipt_bytes(),
        )
        .unwrap_or_else(|error| unreachable!("{label} archived receipt decode: {error}"));
        assert_eq!(decoded, *prepared.semantic_result());
    }
}

#[test]
fn existing_winner_never_installs_speculative_transition() {
    let (mut trace, plan) = action_plan();
    let before = trace.head().clone();
    let winner = plan.semantic_result().clone();
    let storage = ScriptedStorage::new(
        [CommitReply::Fixed(RoomCommitResolutionV1::resolved(
            ResolutionStatusV1::Existing,
            winner,
        ))],
        [],
    );
    let outcome = commit_existing_room(&storage, &mut trace, plan);
    assert!(matches!(
        outcome.resolution(),
        RoomCommitResolutionV1::TransitionCommitted {
            status: ResolutionStatusV1::Existing,
            ..
        }
    ));
    assert_eq!(
        outcome.actor_installation(),
        ActorInstallationV1::ReloadRequired
    );
    assert_eq!(trace.head(), &before);
}

#[test]
fn timer_reprepare_obsolescence_is_typed_not_applicable_without_pack_entry() {
    let (mut trace, timer) = timer_plan();
    let storage = ScriptedStorage::new([CommitReply::Fixed(RoomCommitResolutionV1::Reprepare)], []);
    let outcome = commit_existing_room(&storage, &mut trace, timer);
    let (_, _, _, reprepare) = outcome.into_parts();
    let Some(ExistingRoomReprepareV1::TimerFired(reprepare)) = reprepare else {
        unreachable!("changed Head retains an opaque Timer reprepare capability")
    };
    let consumed = RecordedStimulusV1::TimerFired(TimerFiredV1 {
        timer_id: parsed(TIMER),
        generation: TimerGenerationV1::new(1)
            .unwrap_or_else(|error| unreachable!("fixture Timer generation: {error}")),
        scheduled_for: parsed("2026-08-15T12:30:00Z"),
        canonical_payload: canonical(br#"{"kind":"deadline"}"#),
    });
    trace
        .advance(consumed)
        .unwrap_or_else(|error| unreachable!("consume Timer fixture: {error}"));
    let callbacks_after_consumption = trace.activity_callback_count();
    let resealed = reprepare
        .seal_against(
            &trace,
            parsed("01ARZ3NDEKTSV4RRFFQ69G5FC8"),
            IntegrityGenerationV1::new(1)
                .unwrap_or_else(|error| unreachable!("integrity generation: {error}")),
            authority(),
            &BTreeMap::new(),
        )
        .unwrap_or_else(|error| unreachable!("obsolete Timer classification: {error}"));
    assert!(matches!(resealed, TimerReprepareOutcomeV1::NotApplicable));
    assert_eq!(trace.activity_callback_count(), callbacks_after_consumption);
}

#[test]
fn create_resolution_tokens_retain_the_identical_sealed_plan() {
    let storage = ScriptedStorage::new(
        [CommitReply::Indeterminate, CommitReply::Exact],
        [
            ResolveOutcomeV1::ResolutionUnavailable,
            ResolveOutcomeV1::KnownAbsent,
        ],
    );
    let first = commit_room_creation(&storage, creation_plan(0));
    let (_, trace, pending) = first.into_parts();
    assert!(trace.is_none());
    let Some(RoomCreationPendingAttemptV1::ResolveOnly(resolve)) = pending else {
        unreachable!("indeterminate Create exposes resolve-only token");
    };
    let second = resolve.resolve(&storage);
    let (_, trace, pending) = second.into_parts();
    assert!(trace.is_none());
    let Some(RoomCreationPendingAttemptV1::ResolveOnly(resolve)) = pending else {
        unreachable!("unavailable resolution retains resolve-only token");
    };
    let third = resolve.resolve(&storage);
    let (_, trace, pending) = third.into_parts();
    assert!(trace.is_none());
    let Some(RoomCreationPendingAttemptV1::Retryable(retry)) = pending else {
        unreachable!("known absence exposes retry token");
    };
    let final_outcome = retry.retry(&storage);
    assert!(final_outcome.into_committed_trace().is_some());

    let observed = storage.observed();
    assert_eq!(observed.len(), 4);
    assert!(observed.windows(2).all(|pair| pair[0] == pair[1]));
}

#[test]
#[allow(clippy::too_many_lines)]
fn canonical_request_hash_vectors_bind_semantics_and_not_identity_only_fields() {
    let create = RoomCreationRequestV1::new(
        parsed("blake3:1111111111111111111111111111111111111111111111111111111111111111"),
        canonical(br#"{"initial_value":0,"maximum_value":4}"#),
        vec![
            InitialMembershipProposalV1::new(
                parsed(PRINCIPAL),
                PrincipalKindV1::Human,
                MembershipStandingV1::Enabled,
                AccessModeV1::Participant,
                Some("counter".to_owned()),
            )
            .unwrap_or_else(|error| unreachable!("fixture proposal: {error}")),
        ],
    );
    let action = ParticipantActionRequestV1::new(
        parsed(ROOM),
        parsed(MEMBER),
        parsed(ACTION),
        RoomSequenceV1::new(0).unwrap_or_else(|error| unreachable!("fixture sequence: {error}")),
        "increment",
        canonical(br"{}"),
    );
    let timer = TimerFiredRequestV1::new(
        parsed(ROOM),
        parsed(TIMER),
        TimerGenerationV1::new(1)
            .unwrap_or_else(|error| unreachable!("fixture Timer generation: {error}")),
        parsed("2026-08-15T12:30:00Z"),
        canonical(br#"{"kind":"deadline"}"#),
    );
    assert_eq!(
        (
            create
                .canonical_request_hash()
                .unwrap_or_else(|error| unreachable!("Create hash: {error}"))
                .to_string(),
            action
                .canonical_request_hash()
                .unwrap_or_else(|error| unreachable!("Action hash: {error}"))
                .to_string(),
            timer
                .canonical_request_hash()
                .unwrap_or_else(|error| unreachable!("Timer hash: {error}"))
                .to_string(),
        ),
        (
            "blake3:80546edf7c213bc86e25c05f13ea1344e3ba66b66259aa3764d2981c1523a6a2".to_owned(),
            "blake3:70943dd000ed7c711e4c428bb06a60c481e13706233b7750f97d9947e2ab91bf".to_owned(),
            "blake3:02970bc922b8161be9ec42baef43cf69a757c249056e7c5d755846cfc9927d69".to_owned(),
        )
    );

    let changed_create = RoomCreationRequestV1::new(
        parsed("blake3:1111111111111111111111111111111111111111111111111111111111111111"),
        canonical(br#"{"initial_value":1,"maximum_value":4}"#),
        create.ordered_initial_memberships().to_vec(),
    );
    assert_ne!(
        create
            .canonical_request_hash()
            .unwrap_or_else(|error| unreachable!("Create hash: {error}")),
        changed_create
            .canonical_request_hash()
            .unwrap_or_else(|error| unreachable!("Create mutation hash: {error}")),
    );
    let changed_action_id = ParticipantActionRequestV1::new(
        parsed(ROOM),
        parsed(MEMBER),
        parsed("01ARZ3NDEKTSV4RRFFQ69G5FC7"),
        RoomSequenceV1::new(0).unwrap_or_else(|error| unreachable!("fixture sequence: {error}")),
        "increment",
        canonical(br"{}"),
    );
    assert_eq!(
        action
            .canonical_request_hash()
            .unwrap_or_else(|error| unreachable!("Action hash: {error}")),
        changed_action_id
            .canonical_request_hash()
            .unwrap_or_else(|error| unreachable!("Action identity-only hash: {error}")),
    );
    let changed_action_payload = ParticipantActionRequestV1::new(
        parsed(ROOM),
        parsed(MEMBER),
        parsed(ACTION),
        RoomSequenceV1::new(0).unwrap_or_else(|error| unreachable!("fixture sequence: {error}")),
        "increment",
        canonical(br#"{"delta":1}"#),
    );
    assert_ne!(
        action
            .canonical_request_hash()
            .unwrap_or_else(|error| unreachable!("Action hash: {error}")),
        changed_action_payload
            .canonical_request_hash()
            .unwrap_or_else(|error| unreachable!("Action mutation hash: {error}")),
    );
    let changed_timer = TimerFiredRequestV1::new(
        parsed(ROOM),
        parsed(TIMER),
        TimerGenerationV1::new(1)
            .unwrap_or_else(|error| unreachable!("fixture Timer generation: {error}")),
        parsed("2026-08-15T12:31:00Z"),
        canonical(br#"{"kind":"deadline"}"#),
    );
    assert_ne!(
        timer
            .canonical_request_hash()
            .unwrap_or_else(|error| unreachable!("Timer hash: {error}")),
        changed_timer
            .canonical_request_hash()
            .unwrap_or_else(|error| unreachable!("Timer mutation hash: {error}")),
    );
}

#[test]
fn recovery_quarantines_rehashed_host_core_disagreement_before_missing_runtime() {
    let action_trace = counter_action_trace();
    let mut action_transition = action_trace.transitions()[0].clone();
    action_transition
        .resulting_core_state
        .memberships
        .get_mut(&parsed(MEMBER))
        .unwrap_or_else(|| unreachable!("Action result Membership"))
        .standing = MembershipStandingV1::Suspended;
    refresh_transition_hashes(&mut action_transition);
    assert_candidate_quarantines_before_missing_runtime(recovery_candidate(
        action_trace.genesis(),
        &[action_transition],
    ));

    let proposal_trace = archive_trace(false);
    let mut proposal_transition = proposal_trace.transitions()[0].clone();
    proposal_transition.resulting_core_state.room_status = RoomStatusV1::Active;
    refresh_transition_hashes(&mut proposal_transition);
    assert_candidate_quarantines_before_missing_runtime(recovery_candidate(
        proposal_trace.genesis(),
        &[proposal_transition],
    ));
}

#[test]
fn recovery_quarantines_rehashed_invalid_timer_history_before_missing_runtime() {
    let (mut trace, _plan) = timer_plan();
    trace
        .advance(RecordedStimulusV1::TimerFired(TimerFiredV1 {
            timer_id: parsed(TIMER),
            generation: TimerGenerationV1::new(1)
                .unwrap_or_else(|error| unreachable!("Timer generation: {error}")),
            scheduled_for: parsed("2026-08-15T12:30:00Z"),
            canonical_payload: canonical(br#"{"kind":"deadline"}"#),
        }))
        .unwrap_or_else(|error| unreachable!("fixture Timer advance: {error}"));
    let original = trace.transitions()[0].clone();
    let generation_two =
        TimerGenerationV1::new(2).unwrap_or_else(|error| unreachable!("Timer generation: {error}"));
    let generation_three =
        TimerGenerationV1::new(3).unwrap_or_else(|error| unreachable!("Timer generation: {error}"));
    for changes in [
        vec![TimerChangeV1::Schedule {
            timer_id: parsed(TIMER),
            generation: generation_three,
            scheduled_for: parsed("2026-08-15T13:00:00Z"),
            canonical_payload: canonical(br"{}"),
        }],
        vec![TimerChangeV1::Schedule {
            timer_id: parsed(TIMER),
            generation: generation_two,
            scheduled_for: parsed("2026-08-15T12:30:00Z"),
            canonical_payload: canonical(br"{}"),
        }],
        vec![
            TimerChangeV1::Schedule {
                timer_id: parsed(TIMER),
                generation: generation_two,
                scheduled_for: parsed("2026-08-15T13:00:00Z"),
                canonical_payload: canonical(br"{}"),
            },
            TimerChangeV1::Cancel {
                timer_id: parsed(TIMER),
                generation: generation_two,
            },
        ],
    ] {
        let mut transition = original.clone();
        transition.ordered_timer_changes = changes;
        refresh_transition_hashes(&mut transition);
        assert_candidate_quarantines_before_missing_runtime(recovery_candidate(
            trace.genesis(),
            &[transition],
        ));
    }
}

#[test]
fn recovery_quarantines_rehashed_invalid_genesis_inputs_not_runtime_faults() {
    let trace = counter_trace();
    let registry = builtin_counter_registry()
        .unwrap_or_else(|error| unreachable!("Counter registry: {error}"));

    let mut invalid_configuration = trace.genesis().clone();
    invalid_configuration.configuration = canonical(br"{}");
    refresh_genesis_hashes(&mut invalid_configuration);
    assert_candidate_quarantines(recovery_candidate(&invalid_configuration, &[]), &registry);

    let mut invalid_role = trace.genesis().clone();
    invalid_role
        .initial_core_state
        .memberships
        .get_mut(&parsed(MEMBER))
        .unwrap_or_else(|| unreachable!("Genesis Membership"))
        .role = Some("not-a-counter-role".to_owned());
    refresh_genesis_hashes(&mut invalid_role);
    assert_candidate_quarantines(recovery_candidate(&invalid_role, &[]), &registry);
}
