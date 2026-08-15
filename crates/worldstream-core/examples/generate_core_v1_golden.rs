//! Regenerates the shared IMO-40 canonical/hash golden corpus.
//!
//! This is deliberately separate from assertions. Its hash-input objects are
//! assembled independently from the production private hashing structs.

use std::{collections::BTreeSet, fs, path::PathBuf, str::FromStr};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use worldstream_core::{
    AccessModeV1, ActivityApplyV1, ActivityDispositionV1, ActivityReduceInputV1,
    ActivityRejectionV1, AdministrationOperationIdentityV1, AdvanceDispositionV1,
    CANONICAL_CODEC_ID, CORE_OPERATION_KIND, CORE_SCHEMA_VERSION, CanonicalJsonV1,
    CoreAuthorityAttributionV1, CoreAuthorityKindV1, CoreChangeSetV1, CoreProposedKindV1,
    CoreProposedV1, CoreRoomStateV1, CoreTraceV1, GENESIS_VERSION, GenesisInputV1, HASH_SUITE_ID,
    MembershipChangeV1, MembershipStandingV1, MembershipV1, PackFaultV1, PrincipalKindV1,
    RecordedStimulusV1, RoomStatusV1, TRANSITION_VERSION, TimerGenerationV1, TransitionV1,
};

const ROOM: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const MEMBER_A: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
const MEMBER_B: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAX";
const MEMBER_C: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAY";
const PRINCIPAL_A: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB0";
const PRINCIPAL_B: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB1";
const ADMIN: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB2";
const TIMER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FC0";
const PACK_DIGEST: &str = "blake3:1111111111111111111111111111111111111111111111111111111111111111";
const ROOM_SEED: &str = "hex:000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

fn parse<T>(value: &str) -> Result<T>
where
    T: FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    value.parse().map_err(Into::into)
}

fn canonical_json(value: &str) -> Result<CanonicalJsonV1> {
    CanonicalJsonV1::parse(value.as_bytes()).map_err(Into::into)
}

fn participant(
    member: &str,
    principal: &str,
    kind: PrincipalKindV1,
    role: &str,
) -> Result<MembershipV1> {
    MembershipV1::new(
        parse(member)?,
        parse(principal)?,
        kind,
        MembershipStandingV1::Enabled,
        AccessModeV1::Participant,
        Some(role.to_owned()),
    )
    .map_err(Into::into)
}

fn validate_roles(state: &CoreRoomStateV1) -> Result<(), String> {
    let mut roles = BTreeSet::new();
    for membership in state.memberships().values() {
        if membership.standing() != MembershipStandingV1::Departed
            && membership.access_mode() == AccessModeV1::Participant
        {
            let Some(role) = membership.role() else {
                return Err("participant missing Role".to_owned());
            };
            if !roles.insert(role) {
                return Err("duplicate Role".to_owned());
            }
        }
    }
    Ok(())
}

fn reduce(input: &ActivityReduceInputV1<'_>) -> Result<ActivityDispositionV1, PackFaultV1> {
    let reason = match input.recorded_stimulus {
        RecordedStimulusV1::CoreProposed(proposal) => proposal.reason_code(),
        RecordedStimulusV1::ParticipantAction(action) => action.action_type.as_str(),
        RecordedStimulusV1::TimerFired(_) => "timer_fired",
        RecordedStimulusV1::ExternalInput(input) => input.input_type.as_str(),
    };
    if reason.starts_with("reject.") {
        return Ok(ActivityDispositionV1::Reject(ActivityRejectionV1 {
            declared_code: "fixture_veto".to_owned(),
            bounded_safe_details: canonical_json(r#"{"safe":"fixture"}"#)
                .map_err(|error| PackFaultV1::Callback(error.to_string()))?,
        }));
    }
    Ok(ActivityDispositionV1::Apply(ActivityApplyV1 {
        next_activity_state: CanonicalJsonV1::parse(
            format!(
                r#"{{"last_reason":"{reason}","last_room_seq":{}}}"#,
                input.next_room_seq.get()
            )
            .as_bytes(),
        )
        .map_err(|error| PackFaultV1::Callback(error.to_string()))?,
        ordered_domain_events: if reason == "operator.role_swap" {
            vec![
                CanonicalJsonV1::parse(br#"{"event":"roles_swap_started"}"#)
                    .map_err(|error| PackFaultV1::Callback(error.to_string()))?,
                CanonicalJsonV1::parse(br#"{"event":"roles_swap_completed"}"#)
                    .map_err(|error| PackFaultV1::Callback(error.to_string()))?,
            ]
        } else {
            vec![
                canonical_json(&format!(r#"{{"event":"{reason}"}}"#))
                    .map_err(|error| PackFaultV1::Callback(error.to_string()))?,
            ]
        },
        timer_requests: Vec::new(),
        ordered_attention_signals: vec![
            canonical_json(&format!(r#"{{"attention":"{reason}"}}"#))
                .map_err(|error| PackFaultV1::Callback(error.to_string()))?,
        ],
    }))
}

fn canonical(value: Value) -> Result<String> {
    serde_json::to_string(&sort(value)).map_err(Into::into)
}

fn sort(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(sort).collect()),
        Value::Object(values) => {
            let mut entries: Vec<_> = values.into_iter().collect();
            entries.sort_unstable_by(|left, right| left.0.cmp(&right.0));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, sort(value)))
                    .collect(),
            )
        }
        scalar => scalar,
    }
}

fn vector(canonical_input: &str) -> Value {
    let digest = blake3::hash(canonical_input.as_bytes());
    json!({
        "canonical_bytes": canonical_input,
        "digest": format!("blake3:{digest}"),
        "digest_bytes_hex": digest.to_hex().to_string(),
    })
}

fn genesis_input() -> Result<GenesisInputV1> {
    let core = CoreRoomStateV1::active([
        participant(MEMBER_A, PRINCIPAL_A, PrincipalKindV1::Agent, "navigator")?,
        participant(MEMBER_B, PRINCIPAL_B, PrincipalKindV1::Human, "insider")?,
    ])?;
    Ok(GenesisInputV1::new(
        parse(ROOM)?,
        parse(PACK_DIGEST)?,
        canonical_json(r#"{"scenario":"imo-40","variant":1}"#)?,
        parse(ROOM_SEED)?,
        parse("2026-08-15T12:00:00Z")?,
        core,
        canonical_json(r#"{"last_reason":"genesis","last_room_seq":0}"#)?,
    )
    .with_initial_timers(vec![worldstream_core::ScheduledTimerV1 {
        timer_id: parse(TIMER)?,
        generation: TimerGenerationV1::new(1)?,
        scheduled_for: parse("2026-08-15T12:30:00Z")?,
        canonical_payload: canonical_json(r#"{"kind":"deadline"}"#)?,
    }]))
}

fn new_trace() -> Result<CoreTraceV1> {
    CoreTraceV1::create_for_conformance(genesis_input()?, validate_roles, reduce)
        .map_err(Into::into)
}

fn core_stimulus(
    trace: &CoreTraceV1,
    kind: CoreProposedKindV1,
    changeset: CoreChangeSetV1,
    reason: &str,
    idempotency_key: &str,
    recorded_at: &str,
) -> Result<RecordedStimulusV1> {
    Ok(RecordedStimulusV1::CoreProposed(CoreProposedV1::new(
        kind,
        CoreAuthorityAttributionV1 {
            principal_id: parse(ADMIN)?,
            authority_kind: CoreAuthorityKindV1::RoomAdministrator,
        },
        AdministrationOperationIdentityV1 {
            authenticated_principal: parse(ADMIN)?,
            versioned_operation_kind: CORE_OPERATION_KIND.to_owned(),
            idempotency_key: idempotency_key.to_owned(),
        },
        trace.head().room_seq(),
        reason,
        parse(recorded_at)?,
        changeset,
    )))
}

fn accepted(trace: &mut CoreTraceV1, stimulus: RecordedStimulusV1) -> Result<()> {
    if !matches!(
        trace.advance_for_conformance(stimulus)?,
        AdvanceDispositionV1::TransitionAccepted {
            existing: false,
            ..
        }
    ) {
        bail!("golden transition was not accepted");
    }
    Ok(())
}

fn transition_hash_input(transition: &TransitionV1) -> Result<String> {
    canonical(json!({
        "domain": TRANSITION_VERSION,
        "codec_id": CANONICAL_CODEC_ID,
        "hash_suite": HASH_SUITE_ID,
        "room_id": transition.complete_head().room_id(),
        "room_seq": transition.room_seq(),
        "core_schema": CORE_SCHEMA_VERSION,
        "pack_digest": transition.complete_head().pack_digest(),
        "previous_transition_or_genesis_hash": transition.previous_lineage_hash(),
        "recorded_stimulus": transition.recorded_stimulus(),
        "ordered_domain_events": transition.ordered_domain_events(),
        "ordered_timer_changes": transition.ordered_timer_changes(),
        "ordered_attention_signals": transition.ordered_attention_signals(),
        "resulting_core_state_hash": transition.resulting_core_state_hash(),
        "resulting_activity_state_hash": transition.resulting_activity_state_hash(),
        "resulting_authoritative_state_hash": transition.resulting_authoritative_state_hash(),
    }))
}

fn transition_vector(name: &str, transition: &TransitionV1) -> Result<Value> {
    Ok(json!({
        "name": name,
        "hash_input_bytes": transition_hash_input(transition)?,
        "digest": transition.transition_hash().to_string(),
        "record_bytes": String::from_utf8(transition.canonical_bytes()?)?,
        "complete_head_bytes": canonical(serde_json::to_value(transition.complete_head())?)?,
    }))
}

#[allow(clippy::too_many_lines)]
fn main() -> Result<()> {
    let mut trace = new_trace()?;
    let member_a = trace
        .core_state()
        .membership(&parse(MEMBER_A)?)
        .context("missing member A")?
        .clone();
    let member_b = trace
        .core_state()
        .membership(&parse(MEMBER_B)?)
        .context("missing member B")?
        .clone();
    let changeset = CoreChangeSetV1::atomic(vec![
        MembershipChangeV1::role_change(member_a, "insider")?,
        MembershipChangeV1::role_change(member_b, "navigator")?,
    ])?;
    let stimulus = core_stimulus(
        &trace,
        CoreProposedKindV1::MembershipChangeSet,
        changeset,
        "operator.role_swap",
        "swap-1",
        "2026-08-15T12:01:00Z",
    )?;
    accepted(&mut trace, stimulus)?;

    let genesis = trace.genesis();
    let transition = &trace.transitions()[0];
    let core_value = serde_json::to_value(genesis.initial_core_state())?;
    let activity_value = serde_json::to_value(genesis.initial_activity_state())?;
    let core_hash_input = canonical(json!({
        "domain": "worldstream/core-state/v1",
        "core_schema": CORE_SCHEMA_VERSION,
        "core": core_value,
    }))?;
    let activity_hash_input = canonical(json!({
        "domain": "worldstream/activity-state/v1",
        "pack_digest": genesis.pack_digest(),
        "activity": activity_value,
    }))?;
    let authoritative_hash_input = canonical(json!({
        "domain": "worldstream/authoritative-state/v1",
        "core_schema": CORE_SCHEMA_VERSION,
        "pack_digest": genesis.pack_digest(),
        "core_state_hash": genesis.initial_core_state_hash(),
        "activity_state_hash": genesis.initial_activity_state_hash(),
    }))?;
    let genesis_hash_input = canonical(json!({
        "domain": GENESIS_VERSION,
        "codec_id": CANONICAL_CODEC_ID,
        "hash_suite": HASH_SUITE_ID,
        "room_id": genesis.room_id(),
        "core_schema": CORE_SCHEMA_VERSION,
        "pack_digest": genesis.pack_digest(),
        "configuration": genesis.configuration(),
        "room_seed": genesis.room_seed(),
        "created_at": genesis.created_at(),
        "initial_timers": genesis.initial_timers(),
        "initial_core_state_hash": genesis.initial_core_state_hash(),
        "initial_activity_state_hash": genesis.initial_activity_state_hash(),
        "initial_authoritative_state_hash": genesis.initial_authoritative_state_hash(),
    }))?;
    let transition_hash_input = canonical(json!({
        "domain": TRANSITION_VERSION,
        "codec_id": CANONICAL_CODEC_ID,
        "hash_suite": HASH_SUITE_ID,
        "room_id": transition.complete_head().room_id(),
        "room_seq": transition.room_seq(),
        "core_schema": CORE_SCHEMA_VERSION,
        "pack_digest": transition.complete_head().pack_digest(),
        "previous_transition_or_genesis_hash": transition.previous_lineage_hash(),
        "recorded_stimulus": transition.recorded_stimulus(),
        "ordered_domain_events": transition.ordered_domain_events(),
        "ordered_timer_changes": transition.ordered_timer_changes(),
        "ordered_attention_signals": transition.ordered_attention_signals(),
        "resulting_core_state_hash": transition.resulting_core_state_hash(),
        "resulting_activity_state_hash": transition.resulting_activity_state_hash(),
        "resulting_authoritative_state_hash": transition.resulting_authoritative_state_hash(),
    }))?;

    let genesis_record = String::from_utf8(trace.genesis_bytes()?)?;
    let transition_record = String::from_utf8(trace.transition_bytes()?.remove(0))?;
    let prefix_heads = vec![
        canonical(serde_json::to_value(genesis.complete_head())?)?,
        canonical(serde_json::to_value(transition.complete_head())?)?,
    ];
    let mut lifecycle_transitions = vec![transition_vector(
        "atomic_role_swap",
        &trace.transitions()[0],
    )?];

    let member_a = trace
        .core_state()
        .membership(&parse(MEMBER_A)?)
        .context("missing member A")?
        .clone();
    let stimulus = core_stimulus(
        &trace,
        CoreProposedKindV1::Suspend,
        CoreChangeSetV1::one(MembershipChangeV1::suspend(member_a)),
        "operator.suspend",
        "suspend-1",
        "2026-08-15T12:02:00Z",
    )?;
    accepted(&mut trace, stimulus)?;
    lifecycle_transitions.push(transition_vector(
        "enabled_to_suspended",
        trace.transitions().last().context("missing suspend")?,
    )?);

    let member_a = trace
        .core_state()
        .membership(&parse(MEMBER_A)?)
        .context("missing member A")?
        .clone();
    let stimulus = core_stimulus(
        &trace,
        CoreProposedKindV1::Resume,
        CoreChangeSetV1::one(MembershipChangeV1::resume(member_a)),
        "operator.resume",
        "resume-1",
        "2026-08-15T12:03:00Z",
    )?;
    accepted(&mut trace, stimulus)?;
    lifecycle_transitions.push(transition_vector(
        "resume",
        trace.transitions().last().context("missing resume")?,
    )?);

    let member_b = trace
        .core_state()
        .membership(&parse(MEMBER_B)?)
        .context("missing member B")?
        .clone();
    let change = MembershipChangeV1::access_mode_change(member_b, AccessModeV1::Spectator, None)?;
    let stimulus = core_stimulus(
        &trace,
        CoreProposedKindV1::AccessModeChange,
        CoreChangeSetV1::one(change),
        "operator.to_spectator",
        "access-1",
        "2026-08-15T12:04:00Z",
    )?;
    accepted(&mut trace, stimulus)?;
    lifecycle_transitions.push(transition_vector(
        "access_to_spectator",
        trace.transitions().last().context("missing spectator")?,
    )?);

    let member_b = trace
        .core_state()
        .membership(&parse(MEMBER_B)?)
        .context("missing member B")?
        .clone();
    let change = MembershipChangeV1::access_mode_change(
        member_b,
        AccessModeV1::Participant,
        Some("navigator".to_owned()),
    )?;
    let stimulus = core_stimulus(
        &trace,
        CoreProposedKindV1::AccessModeChange,
        CoreChangeSetV1::one(change),
        "operator.to_participant",
        "access-2",
        "2026-08-15T12:05:00Z",
    )?;
    accepted(&mut trace, stimulus)?;
    lifecycle_transitions.push(transition_vector(
        "access_to_participant",
        trace.transitions().last().context("missing participant")?,
    )?);

    let member_a = trace
        .core_state()
        .membership(&parse(MEMBER_A)?)
        .context("missing member A")?
        .clone();
    let rejected = core_stimulus(
        &trace,
        CoreProposedKindV1::RoleChange,
        CoreChangeSetV1::one(MembershipChangeV1::role_change(member_a.clone(), "scout")?),
        "reject.role",
        "reject-1",
        "2026-08-15T12:06:00Z",
    )?;
    let rejection_new = trace.advance_for_conformance(rejected.clone())?;
    let rejection_existing = trace.advance_for_conformance(rejected)?;
    let rejection_new_bytes = match rejection_new {
        AdvanceDispositionV1::RejectionRecorded {
            existing: false,
            canonical_receipt_bytes,
            ..
        } => String::from_utf8(canonical_receipt_bytes)?,
        _ => bail!("expected new rejection"),
    };
    let rejection_existing_bytes = match rejection_existing {
        AdvanceDispositionV1::RejectionRecorded {
            existing: true,
            canonical_receipt_bytes,
            ..
        } => String::from_utf8(canonical_receipt_bytes)?,
        _ => bail!("expected existing rejection"),
    };

    let no_change = core_stimulus(
        &trace,
        CoreProposedKindV1::RoleChange,
        CoreChangeSetV1::one(MembershipChangeV1::role_change(member_a, "insider")?),
        "operator.role_already_set",
        "nochange-1",
        "2026-08-15T12:07:00Z",
    )?;
    let no_change_new = trace.advance_for_conformance(no_change.clone())?;
    let no_change_existing = trace.advance_for_conformance(no_change)?;
    let no_change_new_bytes = match no_change_new {
        AdvanceDispositionV1::NoChangeRecorded {
            existing: false,
            canonical_receipt_bytes,
        } => String::from_utf8(canonical_receipt_bytes)?,
        _ => bail!("expected new NoChange"),
    };
    let no_change_existing_bytes = match no_change_existing {
        AdvanceDispositionV1::NoChangeRecorded {
            existing: true,
            canonical_receipt_bytes,
        } => String::from_utf8(canonical_receipt_bytes)?,
        _ => bail!("expected existing NoChange"),
    };

    let member_b = trace
        .core_state()
        .membership(&parse(MEMBER_B)?)
        .context("missing member B")?
        .clone();
    let stimulus = core_stimulus(
        &trace,
        CoreProposedKindV1::Depart,
        CoreChangeSetV1::one(MembershipChangeV1::depart(member_b)),
        "operator.depart",
        "depart-1",
        "2026-08-15T12:08:00Z",
    )?;
    accepted(&mut trace, stimulus)?;
    lifecycle_transitions.push(transition_vector(
        "depart",
        trace.transitions().last().context("missing depart")?,
    )?);

    let rejoined = participant(MEMBER_C, PRINCIPAL_B, PrincipalKindV1::Human, "navigator")?;
    let stimulus = core_stimulus(
        &trace,
        CoreProposedKindV1::Join,
        CoreChangeSetV1::one(MembershipChangeV1::join(rejoined)),
        "operator.rejoin",
        "join-1",
        "2026-08-15T12:09:00Z",
    )?;
    accepted(&mut trace, stimulus)?;
    lifecycle_transitions.push(transition_vector(
        "rejoin_new_member_id",
        trace.transitions().last().context("missing rejoin")?,
    )?);

    let stimulus = core_stimulus(
        &trace,
        CoreProposedKindV1::Archive,
        CoreChangeSetV1::archive(RoomStatusV1::Active),
        "operator.archive",
        "archive-1",
        "2026-08-15T12:10:00Z",
    )?;
    accepted(&mut trace, stimulus)?;
    lifecycle_transitions.push(transition_vector(
        "archive_with_host_timer_cancellation",
        trace.transitions().last().context("missing archive")?,
    )?);

    let member_a = trace
        .core_state()
        .membership(&parse(MEMBER_A)?)
        .context("missing member A")?
        .clone();
    let stimulus = core_stimulus(
        &trace,
        CoreProposedKindV1::Suspend,
        CoreChangeSetV1::one(MembershipChangeV1::suspend(member_a)),
        "operator.post_archive_suspend",
        "suspend-2",
        "2026-08-15T12:11:00Z",
    )?;
    accepted(&mut trace, stimulus)?;
    lifecycle_transitions.push(transition_vector(
        "post_archive_suspend",
        trace
            .transitions()
            .last()
            .context("missing post-archive suspend")?,
    )?);

    let lifecycle_prefix_heads = std::iter::once(trace.genesis().complete_head())
        .chain(trace.transitions().iter().map(TransitionV1::complete_head))
        .map(|head| canonical(serde_json::to_value(head)?))
        .collect::<Result<Vec<_>>>()?;

    let mut role_trace = new_trace()?;
    let member_a = role_trace
        .core_state()
        .membership(&parse(MEMBER_A)?)
        .context("missing member A")?
        .clone();
    let stimulus = core_stimulus(
        &role_trace,
        CoreProposedKindV1::RoleChange,
        CoreChangeSetV1::one(MembershipChangeV1::role_change(member_a, "scout")?),
        "operator.role_change",
        "role-accepted",
        "2026-08-15T12:01:00Z",
    )?;
    accepted(&mut role_trace, stimulus)?;
    let individual_role =
        transition_vector("individual_role_change", &role_trace.transitions()[0])?;

    let mut mandatory_trace = new_trace()?;
    let member_a = mandatory_trace
        .core_state()
        .membership(&parse(MEMBER_A)?)
        .context("missing member A")?
        .clone();
    let member_b = mandatory_trace
        .core_state()
        .membership(&parse(MEMBER_B)?)
        .context("missing member B")?
        .clone();
    let changeset = CoreChangeSetV1::atomic(vec![
        MembershipChangeV1::suspend(member_a),
        MembershipChangeV1::depart(member_b),
    ])?;
    let stimulus = core_stimulus(
        &mandatory_trace,
        CoreProposedKindV1::MembershipChangeSet,
        changeset,
        "operator.atomic_mandatory",
        "mandatory-atomic",
        "2026-08-15T12:01:00Z",
    )?;
    accepted(&mut mandatory_trace, stimulus)?;
    let atomic_mandatory =
        transition_vector("atomic_all_mandatory", &mandatory_trace.transitions()[0])?;

    let mut swap_reject_trace = new_trace()?;
    let member_a = swap_reject_trace
        .core_state()
        .membership(&parse(MEMBER_A)?)
        .context("missing member A")?
        .clone();
    let member_b = swap_reject_trace
        .core_state()
        .membership(&parse(MEMBER_B)?)
        .context("missing member B")?
        .clone();
    let changeset = CoreChangeSetV1::atomic(vec![
        MembershipChangeV1::role_change(member_a, "insider")?,
        MembershipChangeV1::role_change(member_b, "navigator")?,
    ])?;
    let stimulus = core_stimulus(
        &swap_reject_trace,
        CoreProposedKindV1::MembershipChangeSet,
        changeset,
        "reject.role_swap",
        "swap-reject",
        "2026-08-15T12:01:00Z",
    )?;
    let atomic_reject_bytes = match swap_reject_trace.advance_for_conformance(stimulus)? {
        AdvanceDispositionV1::RejectionRecorded {
            existing: false,
            canonical_receipt_bytes,
            ..
        } => String::from_utf8(canonical_receipt_bytes)?,
        _ => bail!("expected atomic rejection"),
    };

    let mut archive_reject_trace = new_trace()?;
    let archive_reject_head = canonical(serde_json::to_value(archive_reject_trace.head())?)?;
    let stimulus = core_stimulus(
        &archive_reject_trace,
        CoreProposedKindV1::Archive,
        CoreChangeSetV1::archive(RoomStatusV1::Active),
        "reject.archive",
        "archive-reject",
        "2026-08-15T12:01:00Z",
    )?;
    let archive_reject = archive_reject_trace.advance_for_conformance(stimulus);
    if !matches!(
        archive_reject,
        Err(worldstream_core::TraceErrorV1::Pack(
            PackFaultV1::MandatoryStimulusRejected
        ))
    ) {
        bail!("expected Archive mandatory PackFault");
    }

    let corpus = json!({
        "schema": "worldstream/core-v1-golden-corpus/v1",
        "blake3_empty_known_hex": "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262",
        "hash_vectors": {
            "core": vector(&core_hash_input),
            "activity": vector(&activity_hash_input),
            "authoritative": vector(&authoritative_hash_input),
            "genesis": vector(&genesis_hash_input),
            "transition": vector(&transition_hash_input),
        },
        "genesis_record_bytes": genesis_record,
        "transition_record_bytes": transition_record,
        "prefix_complete_head_bytes": prefix_heads,
        "lifecycle_vectors": {
            "accepted_transitions": lifecycle_transitions,
            "prefix_complete_head_bytes": lifecycle_prefix_heads,
            "independent_transitions": {
                "individual_role_change": individual_role,
                "atomic_all_mandatory": atomic_mandatory,
            },
            "durable_dispositions": {
                "clean_rejection_new_bytes": rejection_new_bytes,
                "clean_rejection_existing_bytes": rejection_existing_bytes,
                "no_change_new_bytes": no_change_new_bytes,
                "no_change_existing_bytes": no_change_existing_bytes,
                "atomic_role_swap_rejection_bytes": atomic_reject_bytes,
            },
            "archive_mandatory_reject": {
                "classification": "mandatory_stimulus_rejected_pack_fault",
                "head_bytes_after": canonical(serde_json::to_value(archive_reject_trace.head())?)?,
                "head_bytes_before": archive_reject_head,
                "receipt_count": archive_reject_trace.administrative_receipt_count(),
                "transition_count": archive_reject_trace.transition_count(),
            },
        },
    });

    let target =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/core_v1_golden.json");
    let parent = target.parent().context("golden target has no parent")?;
    fs::create_dir_all(parent)?;
    let mut bytes = serde_json::to_vec_pretty(&corpus)?;
    bytes.push(b'\n');
    fs::write(&target, bytes).with_context(|| format!("write {}", target.display()))?;
    println!("wrote {}", target.display());
    Ok(())
}
