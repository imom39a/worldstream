//! Additive commit conformance. Retained executors and legacy fixtures are inputs.
use super::*;
use std::error::Error;

#[path = "canonical_history_tests.rs"]
pub(crate) mod history_tests;

type TestResult = Result<(), Box<dyn Error>>;

#[derive(Clone, Copy)]
enum Reply {
    New,
    Existing,
    Absent,
    Ambiguous,
    WrongKind,
}
struct CanonicalStorage {
    replies: Mutex<VecDeque<Reply>>,
    requests: Mutex<Vec<Vec<u8>>>,
}
impl CanonicalStorage {
    fn new(replies: impl IntoIterator<Item = Reply>) -> Self {
        Self {
            replies: Mutex::new(replies.into_iter().collect()),
            requests: Mutex::new(Vec::new()),
        }
    }
}
impl CanonicalRoomCommitStorage for CanonicalStorage {
    fn commit(&self, write: &PreparedCanonicalRoomWrite) -> RoomCommitResolutionV1 {
        self.requests
            .lock()
            .unwrap_or_else(|_| unreachable!("test lock"))
            .push(write.semantic_result().canonical_receipt_bytes().to_vec());
        let reply = self
            .replies
            .lock()
            .unwrap_or_else(|_| unreachable!("test lock"))
            .pop_front()
            .unwrap_or_else(|| unreachable!("scripted reply"));
        match reply {
            Reply::New => RoomCommitResolutionV1::resolved(
                ResolutionStatusV1::New,
                write.semantic_result().clone(),
            ),
            Reply::Existing => RoomCommitResolutionV1::resolved(
                ResolutionStatusV1::Existing,
                write.semantic_result().clone(),
            ),
            Reply::Absent => RoomCommitResolutionV1::RetryableKnownAbsent,
            Reply::Ambiguous => RoomCommitResolutionV1::Indeterminate,
            Reply::WrongKind => RoomCommitResolutionV1::RejectionRecorded {
                status: ResolutionStatusV1::New,
                result: Box::new(write.semantic_result().clone()),
            },
        }
    }
    fn resolve(&self, _: &OperationIdentityV1, _: &CanonicalRequestHashV1) -> ResolveOutcomeV1 {
        ResolveOutcomeV1::KnownAbsent
    }
}
fn trace(format: CanonicalHistoryFormat) -> Result<CanonicalRoomTrace, TraceErrorV1> {
    CanonicalRoomTrace::create_from_retained_for_conformance(counter_genesis(0), format)
}
fn action(trace: &CanonicalRoomTrace, action_id: &str) -> RecordedStimulusV1 {
    let schema = trace
        .retained_pack()
        .descriptor()
        .actions
        .iter()
        .find(|action| action.action_type == "increment")
        .unwrap_or_else(|| unreachable!("retained Counter action"))
        .payload_schema
        .schema_digest
        .clone();
    RecordedStimulusV1::ParticipantAction(ParticipantActionV1 {
        member_id: parsed(MEMBER),
        action_id: parsed(action_id),
        action_type: "increment".to_owned(),
        payload_schema_digest: schema,
        canonical_payload: canonical(br"{}"),
        exact_basis_head: trace.head().clone(),
        admitted_at: parsed("2026-08-15T12:00:02Z"),
    })
}
fn action_plan(
    trace: &CanonicalRoomTrace,
    action_id: &str,
) -> Result<PreparedCanonicalRoomCommit, Box<dyn Error>> {
    let stimulus = action(trace, action_id);
    let request = ParticipantActionRequestV1::new(
        trace.head().room_id().clone(),
        parsed(MEMBER),
        parsed(action_id),
        trace.head().room_seq(),
        "increment",
        canonical(br"{}"),
    );
    Ok(PreparedCanonicalRoomCommit::for_action_for_conformance(
        trace,
        &request,
        trace.prepare(stimulus)?,
        parsed(HOST_CAPABILITY),
        IntegrityGenerationV1::new(1)?,
        authority(),
        &BTreeMap::from([(parsed(MEMBER), 0)]),
    )?)
}
fn advance(plan: &PreparedCanonicalRoomCommit) -> &PreparedCanonicalAdvancePersistence {
    let PreparedCanonicalExistingIntent::Advance(bundle) = plan.intent() else {
        unreachable!("accepted fixture")
    };
    bundle
}

#[test]
fn canonical_v1_preparation_preserves_legacy_bytes_and_effects() -> TestResult {
    let legacy = counter_trace();
    let canonical = trace(CanonicalHistoryFormat::V1)?;
    assert_eq!(
        canonical.genesis().canonical_bytes()?,
        legacy.genesis_bytes()?
    );
    let old = legacy.prepare(counter_action_stimulus(
        &legacy,
        MEMBER,
        ACTION,
        "increment",
    ))?;
    let new = canonical.prepare(action(&canonical, ACTION))?;
    let AdvanceDispositionV1::TransitionAccepted {
        transition: old, ..
    } = old.disposition()
    else {
        unreachable!("accepted")
    };
    let CanonicalAdvanceDisposition::TransitionAccepted {
        transition: new, ..
    } = new.disposition()
    else {
        unreachable!("accepted")
    };
    assert_eq!(old.canonical_bytes()?, new.canonical_bytes()?);
    let creation = PreparedCanonicalRoomCreation::from_registry_genesis_for_conformance(
        creation_identity(),
        &RoomCreationRequestWithFormat::new(creation_request(0), CanonicalHistoryFormat::V1),
        authority(),
        counter_genesis(0),
    )?;
    assert_eq!(
        creation.semantic_result(),
        creation_plan(0).semantic_result()
    );
    let converted = match canonical.try_into_legacy() {
        Ok(trace) => trace,
        Err(_) => unreachable!("genuine V1"),
    };
    assert_eq!(converted.head(), legacy.head());
    assert_eq!(converted.activity_state(), legacy.activity_state());
    Ok(())
}

#[test]
fn v2_creation_receipt_binds_selected_request_and_complete_materializations() -> TestResult {
    let request =
        RoomCreationRequestWithFormat::new(creation_request(0), CanonicalHistoryFormat::V2);
    let plan = PreparedCanonicalRoomCreation::from_registry_genesis_for_conformance(
        creation_identity(),
        &request,
        authority(),
        counter_genesis(0),
    )?;
    assert_eq!(plan.request_hash(), &request.canonical_request_hash()?);
    assert_ne!(
        plan.request_hash(),
        &request.legacy_request().canonical_request_hash()?
    );
    assert_eq!(
        plan.persistence().activity_state(),
        plan.persistence().genesis().initial_activity_state()
    );
    let bytes = plan.semantic_result().canonical_receipt_bytes();
    let decoded = StoredSemanticResultV1::from_canonical_receipt_bytes(bytes)?;
    assert_eq!(&decoded, plan.semantic_result());
    let mut wire: serde_json::Value = serde_json::from_slice(bytes)?;
    wire["semantic_input"]["request"]["canonical_history_format"] =
        serde_json::json!("worldstream/transition/v1");
    assert!(
        StoredSemanticResultV1::from_canonical_receipt_bytes(
            &CanonicalJsonV1::from_serialize(&wire)?.to_bytes()?
        )
        .is_err()
    );
    let outcome = commit_canonical_room_creation(&CanonicalStorage::new([Reply::New]), plan);
    assert_eq!(outcome.actor_installation(), ActorInstallationV1::Installed);
    let (_, _, created, _) = outcome.into_parts();
    let created = created.ok_or("committed creation trace")?;
    assert!(matches!(created.genesis(), GenesisRecord::V2(_)));
    assert!(created.try_into_legacy().is_err());
    Ok(())
}

#[test]
fn compact_action_seals_full_state_observations_and_install_fences() -> TestResult {
    let one = trace(CanonicalHistoryFormat::V1)?;
    let mut two = trace(CanonicalHistoryFormat::V2)?;
    let old = action_plan(&one, ACTION)?;
    let new = action_plan(&two, ACTION)?;
    let old = advance(&old);
    let bundle = advance(&new);
    assert_eq!(old.resulting_core_state(), bundle.resulting_core_state());
    assert_eq!(
        old.resulting_activity_state(),
        bundle.resulting_activity_state()
    );
    assert_eq!(
        old.transition().ordered_domain_events(),
        bundle.transition().ordered_domain_events()
    );
    assert_eq!(
        old.transition().ordered_timer_changes(),
        bundle.transition().ordered_timer_changes()
    );
    assert_eq!(
        old.activation_decisions()
            .iter()
            .map(PreparedActivationDecisionV1::canonical_decision_bytes)
            .collect::<Vec<_>>(),
        bundle
            .activation_decisions()
            .iter()
            .map(PreparedActivationDecisionV1::canonical_decision_bytes)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        old.delivery_consequences().len(),
        bundle.delivery_consequences().len()
    );
    assert!(!bundle.delivery_consequences().is_empty());
    let record: serde_json::Value = serde_json::from_slice(bundle.canonical_transition_bytes())?;
    assert!(record.get("resulting_core_state").is_none());
    assert!(record.get("resulting_activity_state").is_none());
    let expected = bundle.resulting_complete_head().clone();
    let expected_projection = one
        .retained_pack()
        .host()
        .view(&ViewInputV1 {
            core: bundle.resulting_core_state(),
            activity_state: bundle.resulting_activity_state(),
            complete_head: bundle.resulting_complete_head(),
            viewer: &PackViewerV1::Participant(parsed(MEMBER)),
        })?
        .projection()
        .clone();
    let stale = action_plan(&two, REPLAY_CAPABILITY)?;
    let outcome =
        commit_canonical_existing_room(&CanonicalStorage::new([Reply::New]), &mut two, new);
    assert_eq!(outcome.actor_installation(), ActorInstallationV1::Installed);
    assert_eq!(two.head(), &expected);
    let current_view = two.view(&PackViewerV1::Participant(parsed(MEMBER)))?;
    assert_eq!(current_view.complete_head(), two.head());
    assert_eq!(current_view.projection(), &expected_projection);
    let stale =
        commit_canonical_existing_room(&CanonicalStorage::new([Reply::New]), &mut two, stale);
    assert_eq!(
        stale.actor_installation(),
        ActorInstallationV1::ReloadRequired
    );
    assert_eq!(two.head(), &expected);
    let callbacks = two.activity_callback_count();
    two.discard_persisted_history();
    assert!(two.transitions().is_empty());
    assert_eq!(two.activity_callback_count(), callbacks);
    assert_eq!(two.head(), &expected);
    Ok(())
}

#[test]
fn compact_commit_withholds_existing_ambiguous_and_incoherent_results() -> TestResult {
    for reply in [Reply::Existing, Reply::WrongKind] {
        let mut trace = trace(CanonicalHistoryFormat::V2)?;
        let head = trace.head().clone();
        let plan = action_plan(&trace, ACTION)?;
        let outcome =
            commit_canonical_existing_room(&CanonicalStorage::new([reply]), &mut trace, plan);
        assert_eq!(trace.head(), &head);
        assert_eq!(
            outcome.actor_installation(),
            if matches!(reply, Reply::Existing) {
                ActorInstallationV1::ReloadRequired
            } else {
                ActorInstallationV1::QuarantineRequired
            }
        );
    }
    let mut trace = trace(CanonicalHistoryFormat::V2)?;
    let before = trace.head().clone();
    let plan = action_plan(&trace, ACTION)?;
    let callbacks = trace.activity_callback_count();
    let storage = CanonicalStorage::new([Reply::Ambiguous, Reply::Absent, Reply::New]);
    let outcome = commit_canonical_existing_room(&storage, &mut trace, plan);
    assert_eq!(outcome.actor_installation(), ActorInstallationV1::Withheld);
    assert_eq!(trace.head(), &before);
    let (_, _, _, Some(CanonicalRoomPendingAttempt::ResolveOnly(resolve))) = outcome.into_parts()
    else {
        unreachable!("resolve-only")
    };
    let (_, _, _, Some(CanonicalRoomPendingAttempt::Retryable(retry))) =
        resolve.resolve(&storage).into_parts()
    else {
        unreachable!("known absence")
    };
    let (_, _, _, Some(CanonicalRoomPendingAttempt::Retryable(retry))) =
        retry.retry(&storage, Some(&mut trace)).into_parts()
    else {
        unreachable!("retry still absent")
    };
    let outcome = retry.retry(&storage, Some(&mut trace));
    assert_eq!(outcome.actor_installation(), ActorInstallationV1::Installed);
    assert_eq!(trace.activity_callback_count(), callbacks);
    let requests = storage
        .requests
        .lock()
        .unwrap_or_else(|_| unreachable!("test lock"));
    assert_eq!(requests.len(), 3);
    assert!(requests.windows(2).all(|pair| pair[0] == pair[1]));
    Ok(())
}

#[test]
fn compact_preparation_rejects_foreign_executor_and_prepares_core_change() -> TestResult {
    let first = trace(CanonicalHistoryFormat::V2)?;
    let second = trace(CanonicalHistoryFormat::V2)?;
    let prepared = first.prepare(action(&first, ACTION))?;
    let request = ParticipantActionRequestV1::new(
        parsed(ROOM),
        parsed(MEMBER),
        parsed(ACTION),
        second.head().room_seq(),
        "increment",
        canonical(br"{}"),
    );
    assert!(
        PreparedCanonicalRoomCommit::for_action_for_conformance(
            &second,
            &request,
            prepared,
            parsed(HOST_CAPABILITY),
            IntegrityGenerationV1::new(1)?,
            authority(),
            &BTreeMap::from([(parsed(MEMBER), 0)])
        )
        .is_err()
    );
    let mut effects = Vec::new();
    for format in [CanonicalHistoryFormat::V1, CanonicalHistoryFormat::V2] {
        let trace = trace(format)?;
        let request = CoreAdministrationRequestV1::new(
            parsed(ROOM),
            AdministrationOperationIdentityV1 {
                authenticated_principal: parsed(PRINCIPAL),
                versioned_operation_kind: CORE_OPERATION_KIND.to_owned(),
                idempotency_key: "canonical-archive".into(),
            },
            CoreProposedKindV1::Archive,
            trace.head().room_seq(),
            "fixture_archive",
            CoreChangeSetV1::archive(trace.core_state().room_status()),
        )?;
        let (host, capability) = room_host_authority(ROOM);
        let grant = host.authorize_core_administration(
            &capability,
            &request,
            parsed("2026-08-15T12:00:01Z"),
        )?;
        let plan = PreparedCanonicalRoomCommit::for_authorized_core_administration(
            &trace,
            &request,
            parsed("2026-08-15T12:00:02Z"),
            parsed(HOST_CAPABILITY),
            IntegrityGenerationV1::new(1)?,
            grant,
            &BTreeMap::from([(parsed(MEMBER), 0)]),
        )?;
        let bundle = advance(&plan);
        effects.push((
            bundle.resulting_core_state().clone(),
            bundle.resulting_activity_state().clone(),
            bundle.transition().ordered_domain_events().to_vec(),
            bundle.transition().ordered_timer_changes().to_vec(),
        ));
    }
    assert_eq!(effects[0], effects[1]);
    Ok(())
}

pub(crate) fn heist_trace(
    format: CanonicalHistoryFormat,
) -> Result<CanonicalRoomTrace, Box<dyn Error>> {
    let roles = ["navigator", "insider", "broker"];
    let members = [MEMBER, LATER_MEMBER, ACTION];
    let core = CoreRoomStateV1::active(
        members
            .into_iter()
            .zip(roles)
            .map(|(id, role)| {
                MembershipV1::new(
                    parsed(id),
                    parsed(id),
                    PrincipalKindV1::Agent,
                    MembershipStandingV1::Enabled,
                    AccessModeV1::Participant,
                    Some(role.into()),
                )
            })
            .collect::<Result<Vec<_>, _>>()?,
    )?;
    let configuration = CanonicalJsonV1::from_serialize(&serde_json::json!({
        "pack_id":"worldstream.agent-heist","pack_schema":1,"roles":roles,
        "briefing_duration_seconds":30,"negotiation_duration_seconds":90,
        "commitment_duration_seconds":30,"commitment_reminder_seconds_before_deadline":10,
        "result_duration_seconds":20,"maximum_open_offers_per_role":4,"maximum_plans":12,
    }))?;
    let prepared =
        builtin_agent_heist_registry()?.prepare_genesis_for_new_room(&PackGenesisRequestV1 {
            room_id: parsed(ROOM),
            pack_digest: agent_heist_schema_safe_digest(),
            configuration,
            room_seed: parsed(SEED),
            created_at: parsed("2026-08-23T12:00:00Z"),
            initial_core_state: core,
        })?;
    Ok(CanonicalRoomTrace::create_from_retained_for_conformance(
        prepared, format,
    )?)
}

#[test]
fn compact_external_and_timer_seal_equal_effects_activation_and_observations() -> TestResult {
    let mut resulting = Vec::new();
    for format in [CanonicalHistoryFormat::V1, CanonicalHistoryFormat::V2] {
        let mut trace = heist_trace(format)?;
        let input = ExternalInputV1 {
            source_id: parsed(HOST_LOBBY_LAUNCH_SOURCE),
            input_id: parsed(REPLAY_CAPABILITY),
            input_type: HOST_LAUNCH_INPUT_TYPE.into(),
            recorded_at: parsed("2026-08-23T12:01:00Z"),
            canonical_payload: canonical(br"{}"),
            immutable_resource_references: vec![],
        };
        let prepared = trace.prepare(RecordedStimulusV1::ExternalInput(input.clone()))?;
        let request_hash =
            external_input_request_hash(trace.head().room_id(), trace.head().room_seq(), &input)?;
        let (host, capability) = room_host_authority(ROOM);
        let grant = host.authorize_external_input(
            &capability,
            parsed(ROOM),
            request_hash,
            parsed("2026-08-23T12:00:59Z"),
        )?;
        let frames = trace
            .core_state()
            .memberships()
            .keys()
            .cloned()
            .map(|member| (member, 0))
            .collect::<BTreeMap<_, _>>();
        let plan = PreparedCanonicalRoomCommit::for_authorized_external_input(
            &trace,
            &parsed(ROOM),
            trace.head().room_seq(),
            &input,
            prepared,
            parsed(HOST_CAPABILITY),
            IntegrityGenerationV1::new(1)?,
            grant,
            &frames,
        )?;
        let launch = advance(&plan);
        assert!(!launch.timer_changes().is_empty());
        assert!(!launch.activation_decisions().is_empty());
        assert!(!launch.delivery_consequences().is_empty());
        let launch_effects = (
            launch.resulting_activity_state().clone(),
            launch.transition().ordered_domain_events().to_vec(),
            launch.transition().ordered_timer_changes().to_vec(),
            launch.transition().ordered_attention_signals().to_vec(),
            launch
                .activation_decisions()
                .iter()
                .map(|decision| decision.canonical_decision_bytes().to_vec())
                .collect::<Vec<_>>(),
            launch.delivery_consequences().len(),
        );
        assert_eq!(
            commit_canonical_existing_room(&CanonicalStorage::new([Reply::New]), &mut trace, plan)
                .actor_installation(),
            ActorInstallationV1::Installed
        );
        let timer = trace
            .scheduled_timers()
            .values()
            .min_by_key(|timer| timer.scheduled_for.as_str())
            .ok_or("scheduled retained Timer")?
            .clone();
        let request = TimerFiredRequestV1::new(
            parsed(ROOM),
            timer.timer_id.clone(),
            timer.generation,
            timer.scheduled_for.clone(),
            timer.canonical_payload.clone(),
        );
        let prepared = trace.prepare(RecordedStimulusV1::TimerFired(TimerFiredV1 {
            timer_id: timer.timer_id,
            generation: timer.generation,
            scheduled_for: timer.scheduled_for,
            canonical_payload: timer.canonical_payload,
        }))?;
        let grant =
            host.authorize_timer_fired(&capability, &request, parsed("2026-08-23T12:01:31Z"))?;
        let plan = PreparedCanonicalRoomCommit::for_authorized_timer_fired(
            &trace,
            &request,
            prepared,
            parsed(ACTION_CAPABILITY),
            IntegrityGenerationV1::new(1)?,
            grant,
            &frames,
        )?;
        let fired = advance(&plan);
        let timer_effects = (
            fired.resulting_activity_state().clone(),
            fired.transition().ordered_domain_events().to_vec(),
            fired.transition().ordered_timer_changes().to_vec(),
            fired.transition().ordered_attention_signals().to_vec(),
            fired
                .activation_decisions()
                .iter()
                .map(|decision| decision.canonical_decision_bytes().to_vec())
                .collect::<Vec<_>>(),
            fired.delivery_consequences().len(),
        );
        assert_eq!(
            commit_canonical_existing_room(&CanonicalStorage::new([Reply::New]), &mut trace, plan)
                .actor_installation(),
            ActorInstallationV1::Installed
        );
        assert!(!trace.scheduled_timers().is_empty());
        resulting.push((launch_effects, timer_effects, trace.core_state().clone()));
    }
    assert_eq!(resulting[0], resulting[1]);
    Ok(())
}
