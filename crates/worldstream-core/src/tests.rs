use std::{
    collections::BTreeSet,
    fmt::Display,
    str::FromStr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use serde::{Serialize, Serializer, ser::SerializeMap};

use super::*;
use crate::{
    canonical::encode,
    lineage::{
        activity_state_hash_bytes, authoritative_state_hash_bytes, core_state_hash_bytes,
        hash_activity_state, hash_authoritative_state, hash_core_state,
    },
    trace::hash_administration_request,
};

const ROOM: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const MEMBER_A: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
const MEMBER_B: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAX";
const MEMBER_C: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAY";
const PRINCIPAL_A: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB0";
const PRINCIPAL_B: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB1";
const ADMIN: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB2";
const TIMER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FC0";
const ACTION: &str = "01ARZ3NDEKTSV4RRFFQ69G5FD0";
const SOURCE: &str = "01ARZ3NDEKTSV4RRFFQ69G5FE0";
const INPUT: &str = "01ARZ3NDEKTSV4RRFFQ69G5FF0";
const PACK_DIGEST: &str = "blake3:1111111111111111111111111111111111111111111111111111111111111111";
const PAYLOAD_DIGEST: &str =
    "blake3:2222222222222222222222222222222222222222222222222222222222222222";
const ROOM_SEED: &str = "hex:000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

fn parsed<T>(value: &str) -> T
where
    T: FromStr,
    T::Err: Display,
{
    value
        .parse()
        .unwrap_or_else(|error| unreachable!("fixture parse failed: {error}"))
}

#[test]
fn historical_evidence_reference_is_bounded_and_contains_no_payload() {
    let reference = HistoricalEvidenceReferenceV1::new(
        7,
        "transition-7".to_owned(),
        "blake3:abc".to_owned(),
        "blake3:def".to_owned(),
        "room/01ARZ3NDEKTSV4RRFFQ69G5FAV/transition/7".to_owned(),
    )
    .unwrap_or_else(|| unreachable!("valid evidence reference"));
    assert_eq!(reference.room_seq(), 7);
    assert!(reference.encoded_bytes() < MAX_HISTORICAL_EVIDENCE_BYTES_PER_PAGE_V1);
    assert!(
        HistoricalEvidenceReferenceV1::new(
            0,
            "transition-0".to_owned(),
            "hash".to_owned(),
            "prior".to_owned(),
            "room/r/transition/0".to_owned(),
        )
        .is_none()
    );
    assert!(
        HistoricalEvidenceReferenceV1::new(
            1,
            "transition-1".to_owned(),
            "hash\nwith-secret".to_owned(),
            "prior".to_owned(),
            "room/r/transition/1".to_owned(),
        )
        .is_none()
    );
}

fn json(value: &str) -> CanonicalJsonV1 {
    CanonicalJsonV1::parse(value.as_bytes())
        .unwrap_or_else(|error| unreachable!("fixture JSON failed: {error}"))
}

fn participant(
    member_id: &str,
    principal_id: &str,
    kind: PrincipalKindV1,
    role: &str,
) -> MembershipV1 {
    MembershipV1::new(
        parsed(member_id),
        parsed(principal_id),
        kind,
        MembershipStandingV1::Enabled,
        AccessModeV1::Participant,
        Some(role.to_owned()),
    )
    .unwrap_or_else(|error| unreachable!("fixture Membership failed: {error}"))
}

fn fixture_role_validator(state: &CoreRoomStateV1) -> Result<(), String> {
    let mut roles = BTreeSet::new();
    for membership in state.memberships().values() {
        if membership.standing() != MembershipStandingV1::Departed
            && membership.access_mode() == AccessModeV1::Participant
        {
            let role = membership
                .role()
                .ok_or_else(|| "participant has no role".to_owned())?;
            if !roles.insert(role.to_owned()) {
                return Err(format!("duplicate live participant Role {role}"));
            }
        }
    }
    Ok(())
}

#[allow(clippy::unnecessary_wraps)]
fn fixture_activity_reducer(
    input: &ActivityReduceInputV1<'_>,
) -> Result<ActivityDispositionV1, PackFaultV1> {
    let reason = match input.recorded_stimulus {
        RecordedStimulusV1::CoreProposed(proposal) => proposal.reason_code(),
        RecordedStimulusV1::ParticipantAction(action) => action.action_type.as_str(),
        RecordedStimulusV1::TimerFired(_) => "timer_fired",
        RecordedStimulusV1::ExternalInput(input) => input.input_type.as_str(),
    };
    if reason.starts_with("reject.") {
        return Ok(ActivityDispositionV1::Reject(ActivityRejectionV1 {
            declared_code: "fixture_veto".to_owned(),
            bounded_safe_details: json(r#"{"safe":"fixture"}"#),
        }));
    }

    let next_activity_state = json(&format!(
        r#"{{"last_reason":"{reason}","last_room_seq":{}}}"#,
        input.next_room_seq.get()
    ));
    let ordered_domain_events = if reason == "operator.role_swap" {
        vec![
            json(r#"{"event":"roles_swap_started"}"#),
            json(r#"{"event":"roles_swap_completed"}"#),
        ]
    } else {
        vec![json(&format!(r#"{{"event":"{reason}"}}"#))]
    };
    Ok(ActivityDispositionV1::Apply(ActivityApplyV1 {
        next_activity_state,
        ordered_domain_events,
        timer_requests: Vec::new(),
        ordered_attention_signals: vec![json(&format!(r#"{{"attention":"{reason}"}}"#))],
    }))
}

fn genesis_input() -> GenesisInputV1 {
    let core = CoreRoomStateV1::active([
        participant(MEMBER_A, PRINCIPAL_A, PrincipalKindV1::Agent, "navigator"),
        participant(MEMBER_B, PRINCIPAL_B, PrincipalKindV1::Human, "insider"),
    ])
    .unwrap_or_else(|error| unreachable!("fixture Core failed: {error}"));
    GenesisInputV1::new(
        parsed(ROOM),
        parsed(PACK_DIGEST),
        json(r#"{"scenario":"imo-40","variant":1}"#),
        parsed(ROOM_SEED),
        parsed("2026-08-15T12:00:00Z"),
        core,
        json(r#"{"last_reason":"genesis","last_room_seq":0}"#),
    )
    .with_initial_timers(vec![ScheduledTimerV1 {
        timer_id: parsed(TIMER),
        generation: TimerGenerationV1::new(1)
            .unwrap_or_else(|error| unreachable!("fixture generation failed: {error}")),
        scheduled_for: parsed("2026-08-15T12:30:00Z"),
        canonical_payload: json(r#"{"kind":"deadline"}"#),
    }])
}

fn new_trace() -> CoreTraceV1 {
    CoreTraceV1::create_for_conformance(
        genesis_input(),
        fixture_role_validator,
        fixture_activity_reducer,
    )
    .unwrap_or_else(|error| unreachable!("fixture Genesis failed: {error}"))
}

fn trace_with_timer_requests(requests: Vec<TimerRequestV1>) -> CoreTraceV1 {
    CoreTraceV1::create_for_conformance(genesis_input(), fixture_role_validator, move |input| {
        Ok(ActivityDispositionV1::Apply(ActivityApplyV1 {
            next_activity_state: input.prior_activity_state.clone(),
            ordered_domain_events: Vec::new(),
            timer_requests: requests.clone(),
            ordered_attention_signals: Vec::new(),
        }))
    })
    .unwrap_or_else(|error| unreachable!("fixture Genesis failed: {error}"))
}

fn role_change_stimulus(trace: &CoreTraceV1, key: &str) -> RecordedStimulusV1 {
    let member_a = trace
        .core_state()
        .membership(&parsed(MEMBER_A))
        .unwrap_or_else(|| unreachable!("member A missing"))
        .clone();
    core_stimulus(
        trace,
        CoreProposedKindV1::RoleChange,
        CoreChangeSetV1::one(
            MembershipChangeV1::role_change(member_a, "scout")
                .unwrap_or_else(|error| unreachable!("Role change failed: {error}")),
        ),
        "operator.timer_request",
        key,
        "2026-08-15T12:01:00Z",
    )
}

fn core_stimulus(
    trace: &CoreTraceV1,
    kind: CoreProposedKindV1,
    changeset: CoreChangeSetV1,
    reason: &str,
    idempotency_key: &str,
    recorded_at: &str,
) -> RecordedStimulusV1 {
    core_stimulus_at_seq(
        kind,
        changeset,
        trace.head().room_seq(),
        reason,
        idempotency_key,
        recorded_at,
    )
}

fn core_stimulus_at_seq(
    kind: CoreProposedKindV1,
    changeset: CoreChangeSetV1,
    expected_room_seq: RoomSequenceV1,
    reason: &str,
    idempotency_key: &str,
    recorded_at: &str,
) -> RecordedStimulusV1 {
    RecordedStimulusV1::CoreProposed(CoreProposedV1::new(
        kind,
        CoreAuthorityAttributionV1 {
            principal_id: parsed(ADMIN),
            authority_kind: CoreAuthorityKindV1::RoomAdministrator,
        },
        AdministrationOperationIdentityV1 {
            authenticated_principal: parsed(ADMIN),
            versioned_operation_kind: CORE_OPERATION_KIND.to_owned(),
            idempotency_key: idempotency_key.to_owned(),
        },
        expected_room_seq,
        reason,
        parsed(recorded_at),
        changeset,
    ))
}

fn committed(trace: &mut CoreTraceV1, stimulus: RecordedStimulusV1) -> TransitionV1 {
    match trace
        .advance(stimulus)
        .unwrap_or_else(|error| unreachable!("fixture advance failed: {error}"))
    {
        AdvanceDispositionV1::TransitionAccepted {
            existing: false,
            transition,
        } => *transition,
        disposition => unreachable!("expected new Transition, got {disposition:?}"),
    }
}

#[allow(clippy::too_many_lines)]
fn build_lifecycle_trace() -> CoreTraceV1 {
    let mut trace = new_trace();

    let member_a = trace
        .core_state()
        .membership(&parsed(MEMBER_A))
        .unwrap_or_else(|| unreachable!("member A missing"))
        .clone();
    let member_b = trace
        .core_state()
        .membership(&parsed(MEMBER_B))
        .unwrap_or_else(|| unreachable!("member B missing"))
        .clone();
    let swap = CoreChangeSetV1::atomic(vec![
        MembershipChangeV1::role_change(member_a, "insider")
            .unwrap_or_else(|error| unreachable!("swap A failed: {error}")),
        MembershipChangeV1::role_change(member_b, "navigator")
            .unwrap_or_else(|error| unreachable!("swap B failed: {error}")),
    ])
    .unwrap_or_else(|error| unreachable!("swap changeset failed: {error}"));
    let stimulus = core_stimulus(
        &trace,
        CoreProposedKindV1::MembershipChangeSet,
        swap,
        "operator.role_swap",
        "swap-1",
        "2026-08-15T12:01:00Z",
    );
    committed(&mut trace, stimulus);

    let member_a = trace
        .core_state()
        .membership(&parsed(MEMBER_A))
        .unwrap_or_else(|| unreachable!("member A missing"))
        .clone();
    let stimulus = core_stimulus(
        &trace,
        CoreProposedKindV1::Suspend,
        CoreChangeSetV1::one(MembershipChangeV1::suspend(member_a)),
        "operator.suspend",
        "suspend-1",
        "2026-08-15T12:02:00Z",
    );
    committed(&mut trace, stimulus);

    let member_a = trace
        .core_state()
        .membership(&parsed(MEMBER_A))
        .unwrap_or_else(|| unreachable!("member A missing"))
        .clone();
    let stimulus = core_stimulus(
        &trace,
        CoreProposedKindV1::Resume,
        CoreChangeSetV1::one(MembershipChangeV1::resume(member_a)),
        "operator.resume",
        "resume-1",
        "2026-08-15T12:03:00Z",
    );
    committed(&mut trace, stimulus);

    let member_b = trace
        .core_state()
        .membership(&parsed(MEMBER_B))
        .unwrap_or_else(|| unreachable!("member B missing"))
        .clone();
    let to_spectator =
        MembershipChangeV1::access_mode_change(member_b, AccessModeV1::Spectator, None)
            .unwrap_or_else(|error| unreachable!("spectator change failed: {error}"));
    let stimulus = core_stimulus(
        &trace,
        CoreProposedKindV1::AccessModeChange,
        CoreChangeSetV1::one(to_spectator),
        "operator.to_spectator",
        "access-1",
        "2026-08-15T12:04:00Z",
    );
    committed(&mut trace, stimulus);

    let member_b = trace
        .core_state()
        .membership(&parsed(MEMBER_B))
        .unwrap_or_else(|| unreachable!("member B missing"))
        .clone();
    let to_participant = MembershipChangeV1::access_mode_change(
        member_b,
        AccessModeV1::Participant,
        Some("navigator".to_owned()),
    )
    .unwrap_or_else(|error| unreachable!("participant change failed: {error}"));
    let stimulus = core_stimulus(
        &trace,
        CoreProposedKindV1::AccessModeChange,
        CoreChangeSetV1::one(to_participant),
        "operator.to_participant",
        "access-2",
        "2026-08-15T12:05:00Z",
    );
    committed(&mut trace, stimulus);

    let member_a = trace
        .core_state()
        .membership(&parsed(MEMBER_A))
        .unwrap_or_else(|| unreachable!("member A missing"))
        .clone();
    let rejected_change = MembershipChangeV1::role_change(member_a, "scout")
        .unwrap_or_else(|error| unreachable!("rejected Role candidate failed: {error}"));
    let rejected_stimulus = core_stimulus(
        &trace,
        CoreProposedKindV1::RoleChange,
        CoreChangeSetV1::one(rejected_change),
        "reject.role",
        "reject-1",
        "2026-08-15T12:06:00Z",
    );
    let first_rejection = trace
        .advance(rejected_stimulus.clone())
        .unwrap_or_else(|error| unreachable!("clean rejection failed: {error}"));
    let callbacks_after_rejection = trace.activity_callback_count();
    let repeated_rejection = trace
        .advance(rejected_stimulus)
        .unwrap_or_else(|error| unreachable!("rejection retry failed: {error}"));
    let first_rejection_bytes = match first_rejection {
        AdvanceDispositionV1::RejectionRecorded {
            existing: false,
            canonical_receipt_bytes,
            ..
        } => canonical_receipt_bytes,
        other => unreachable!("expected new Rejection, got {other:?}"),
    };
    let repeated_rejection_bytes = match repeated_rejection {
        AdvanceDispositionV1::RejectionRecorded {
            existing: true,
            canonical_receipt_bytes,
            ..
        } => canonical_receipt_bytes,
        other => unreachable!("expected existing Rejection, got {other:?}"),
    };
    assert_eq!(first_rejection_bytes, repeated_rejection_bytes);
    assert_eq!(trace.activity_callback_count(), callbacks_after_rejection);

    let member_a = trace
        .core_state()
        .membership(&parsed(MEMBER_A))
        .unwrap_or_else(|| unreachable!("member A missing"))
        .clone();
    let current_role = member_a
        .role()
        .unwrap_or_else(|| unreachable!("member A Role missing"))
        .to_owned();
    let no_change = MembershipChangeV1::role_change(member_a, current_role)
        .unwrap_or_else(|error| unreachable!("NoChange candidate failed: {error}"));
    let no_change_stimulus = core_stimulus(
        &trace,
        CoreProposedKindV1::RoleChange,
        CoreChangeSetV1::one(no_change),
        "operator.role_already_set",
        "nochange-1",
        "2026-08-15T12:07:00Z",
    );
    let callbacks_before_no_change = trace.activity_callback_count();
    let first_no_change = trace
        .advance(no_change_stimulus.clone())
        .unwrap_or_else(|error| unreachable!("NoChange failed: {error}"));
    let repeated_no_change = trace
        .advance(no_change_stimulus)
        .unwrap_or_else(|error| unreachable!("NoChange retry failed: {error}"));
    let first_no_change_bytes = match first_no_change {
        AdvanceDispositionV1::NoChangeRecorded {
            existing: false,
            canonical_receipt_bytes,
        } => canonical_receipt_bytes,
        other => unreachable!("expected new NoChange, got {other:?}"),
    };
    let repeated_no_change_bytes = match repeated_no_change {
        AdvanceDispositionV1::NoChangeRecorded {
            existing: true,
            canonical_receipt_bytes,
        } => canonical_receipt_bytes,
        other => unreachable!("expected existing NoChange, got {other:?}"),
    };
    assert_eq!(first_no_change_bytes, repeated_no_change_bytes);
    assert_eq!(trace.activity_callback_count(), callbacks_before_no_change);

    let member_b = trace
        .core_state()
        .membership(&parsed(MEMBER_B))
        .unwrap_or_else(|| unreachable!("member B missing"))
        .clone();
    let stimulus = core_stimulus(
        &trace,
        CoreProposedKindV1::Depart,
        CoreChangeSetV1::one(MembershipChangeV1::depart(member_b)),
        "operator.depart",
        "depart-1",
        "2026-08-15T12:08:00Z",
    );
    committed(&mut trace, stimulus);

    let rejoined = participant(MEMBER_C, PRINCIPAL_B, PrincipalKindV1::Human, "navigator");
    let stimulus = core_stimulus(
        &trace,
        CoreProposedKindV1::Join,
        CoreChangeSetV1::one(MembershipChangeV1::join(rejoined)),
        "operator.rejoin",
        "join-1",
        "2026-08-15T12:09:00Z",
    );
    committed(&mut trace, stimulus);

    let stimulus = core_stimulus(
        &trace,
        CoreProposedKindV1::Archive,
        CoreChangeSetV1::archive(trace.core_state().room_status()),
        "operator.archive",
        "archive-1",
        "2026-08-15T12:10:00Z",
    );
    committed(&mut trace, stimulus);

    let member_a = trace
        .core_state()
        .membership(&parsed(MEMBER_A))
        .unwrap_or_else(|| unreachable!("member A missing"))
        .clone();
    let stimulus = core_stimulus(
        &trace,
        CoreProposedKindV1::Suspend,
        CoreChangeSetV1::one(MembershipChangeV1::suspend(member_a)),
        "operator.post_archive_suspend",
        "suspend-2",
        "2026-08-15T12:11:00Z",
    );
    committed(&mut trace, stimulus);

    trace
}

#[test]
fn canonical_codec_is_strict_and_idempotent() {
    let value = CanonicalJsonV1::parse(br#"{"z":1,"a":[3,2,1],"nested":{"b":2,"a":1}}"#)
        .unwrap_or_else(|error| unreachable!("canonical parse failed: {error}"));
    assert_eq!(
        value
            .to_bytes()
            .unwrap_or_else(|error| unreachable!("canonical write failed: {error}")),
        br#"{"a":[3,2,1],"nested":{"a":1,"b":2},"z":1}"#
    );
    assert!(CanonicalJsonV1::parse(br#"{"a":1,"\u0061":2}"#).is_err());
    assert!(CanonicalJsonV1::parse(&[0xff]).is_err());
    assert!(CanonicalJsonV1::parse(b"1.0").is_err());
    assert!(CanonicalJsonV1::parse(b"-9007199254740991").is_ok());
    assert!(CanonicalJsonV1::parse(b"9007199254740991").is_ok());
    assert!(CanonicalJsonV1::parse(b"-9007199254740992").is_err());
    assert!(CanonicalJsonV1::parse(b"9007199254740992").is_err());
    assert_eq!(
        CanonicalJsonV1::from_canonical_bytes(br#"{"b":2,"a":1}"#),
        Err(CanonicalJsonError::NonCanonicalBytes)
    );
    assert!(CanonicalJsonV1::from_canonical_bytes(br#"{"a":1,"b":2}"#).is_ok());
}

struct DuplicateSerializeMap;

struct RawBytes;

impl Serialize for DuplicateSerializeMap {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(Some(2))?;
        map.serialize_entry("same", &1)?;
        map.serialize_entry("same", &2)?;
        map.end()
    }
}

impl Serialize for RawBytes {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_bytes(b"bytes")
    }
}

#[test]
fn canonical_writer_rejects_floats_duplicates_and_implicit_bytes() {
    for float in [0.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(CanonicalJsonV1::from_serialize(&float).is_err());
    }
    assert!(CanonicalJsonV1::from_serialize(&DuplicateSerializeMap).is_err());
    assert!(CanonicalJsonV1::from_serialize(&RawBytes).is_err());
}

#[test]
fn typed_primitives_freeze_digest_time_and_identifier_forms() {
    assert!(PACK_DIGEST.parse::<Blake3DigestV1>().is_ok());
    assert!(
        PACK_DIGEST
            .to_uppercase()
            .parse::<Blake3DigestV1>()
            .is_err()
    );
    assert!("01arz3ndektsv4rrffq69g5fav".parse::<RoomId>().is_err());
    assert!("2026-08-15T12:00:00Z".parse::<CoreRecordedAt>().is_ok());
    assert!("2026-08-15T12:00Z".parse::<CoreRecordedAt>().is_err());
    assert!(
        "2026-08-15T12:00:00+00:00"
            .parse::<CoreRecordedAt>()
            .is_err()
    );
    assert!("2026-08-15T12:00:00.10Z".parse::<CoreRecordedAt>().is_err());
    let tenth: CoreRecordedAt = parsed("2026-08-15T12:00:00.1Z");
    let eleven_hundredths: CoreRecordedAt = parsed("2026-08-15T12:00:00.11Z");
    assert!(tenth < eleven_hundredths);
}

#[test]
fn core_and_complete_head_shapes_are_exact_and_commands_deny_unknown_fields() {
    let trace = new_trace();
    let core: serde_json::Value = serde_json::from_slice(
        &encode(trace.core_state())
            .unwrap_or_else(|error| unreachable!("Core encode failed: {error}")),
    )
    .unwrap_or_else(|error| unreachable!("Core decode failed: {error}"));
    let core_keys: BTreeSet<_> = core
        .as_object()
        .unwrap_or_else(|| unreachable!("Core is not an object"))
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(core_keys, BTreeSet::from(["memberships", "room_status"]));
    for (member_id, membership) in core["memberships"]
        .as_object()
        .unwrap_or_else(|| unreachable!("Membership map missing"))
    {
        assert_eq!(membership["member_id"], member_id.as_str());
        let keys: BTreeSet<_> = membership
            .as_object()
            .unwrap_or_else(|| unreachable!("Membership is not an object"))
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            BTreeSet::from([
                "access_mode",
                "member_id",
                "principal_id",
                "principal_kind",
                "role",
                "standing",
            ])
        );
    }

    let head: serde_json::Value = serde_json::from_slice(
        &encode(trace.head()).unwrap_or_else(|error| unreachable!("Head encode failed: {error}")),
    )
    .unwrap_or_else(|error| unreachable!("Head decode failed: {error}"));
    let head_keys: BTreeSet<_> = head
        .as_object()
        .unwrap_or_else(|| unreachable!("Head is not an object"))
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        head_keys,
        BTreeSet::from([
            "activity_state_hash",
            "authoritative_state_hash",
            "core_schema_version",
            "core_state_hash",
            "genesis_or_transition_hash",
            "pack_digest",
            "room_id",
            "room_seq",
        ])
    );

    let member_a = trace
        .core_state()
        .membership(&parsed(MEMBER_A))
        .unwrap_or_else(|| unreachable!("member A missing"))
        .clone();
    let stimulus = core_stimulus(
        &trace,
        CoreProposedKindV1::Suspend,
        CoreChangeSetV1::one(MembershipChangeV1::suspend(member_a)),
        "operator.suspend",
        "unknown-field",
        "2026-08-15T12:01:00Z",
    );
    let mut value = serde_json::to_value(stimulus)
        .unwrap_or_else(|error| unreachable!("Stimulus encode failed: {error}"));
    value
        .as_object_mut()
        .unwrap_or_else(|| unreachable!("Stimulus is not an object"))
        .insert("unknown_command_field".to_owned(), serde_json::Value::Null);
    let bytes = CanonicalJsonV1::from_serialize(&value)
        .and_then(|value| value.to_bytes())
        .unwrap_or_else(|error| unreachable!("unknown field encode failed: {error}"));
    assert!(CanonicalJsonV1::decode_canonical::<RecordedStimulusV1>(&bytes).is_err());
}

#[test]
fn timer_request_v1_has_one_strict_canonical_wire_form_per_variant() {
    let timer_id: TimerId = parsed(TIMER);
    let generation = TimerGenerationV1::new(7)
        .unwrap_or_else(|error| unreachable!("generation failed: {error}"));
    let cases = [
        (
            TimerRequestV1::ScheduleNext {
                timer_id: timer_id.clone(),
                due: parsed("2026-08-15T12:00:30Z"),
                canonical_payload: json(r#"{"kind":"schedule"}"#),
            },
            br#"{"canonical_payload":{"kind":"schedule"},"due":"2026-08-15T12:00:30Z","timer_id":"01ARZ3NDEKTSV4RRFFQ69G5FC0","timer_request_type":"schedule_next"}"#
                .as_slice(),
        ),
        (
            TimerRequestV1::CancelCurrent {
                timer_id: timer_id.clone(),
                expected_generation: generation,
            },
            br#"{"expected_generation":7,"timer_id":"01ARZ3NDEKTSV4RRFFQ69G5FC0","timer_request_type":"cancel_current"}"#.as_slice(),
        ),
        (
            TimerRequestV1::RescheduleCurrent {
                timer_id,
                expected_generation: generation,
                new_due: parsed("2026-08-15T12:01:00Z"),
                new_canonical_payload: json(r#"{"kind":"reschedule"}"#),
            },
            br#"{"expected_generation":7,"new_canonical_payload":{"kind":"reschedule"},"new_due":"2026-08-15T12:01:00Z","timer_id":"01ARZ3NDEKTSV4RRFFQ69G5FC0","timer_request_type":"reschedule_current"}"#
                .as_slice(),
        ),
    ];

    for (value, expected_bytes) in cases {
        let encoded = encode(&value)
            .unwrap_or_else(|error| unreachable!("Timer request encode failed: {error}"));
        assert_eq!(encoded, expected_bytes);
        let decoded = CanonicalJsonV1::decode_canonical::<TimerRequestV1>(expected_bytes)
            .unwrap_or_else(|error| unreachable!("Timer request decode failed: {error}"));
        assert_eq!(decoded, value);
        assert_eq!(
            encode(&decoded)
                .unwrap_or_else(|error| unreachable!("Timer request re-encode failed: {error}")),
            expected_bytes
        );
    }

    assert!(CanonicalJsonV1::decode_canonical::<TimerRequestV1>(
        br#"{"expected_generation":7,"extra":true,"timer_id":"01ARZ3NDEKTSV4RRFFQ69G5FC0","timer_request_type":"cancel_current"}"#
    )
    .is_err());
}

#[test]
fn activity_disposition_v1_has_strict_canonical_apply_and_reject_wires() {
    let apply = ActivityDispositionV1::Apply(ActivityApplyV1 {
        next_activity_state: json(r#"{"count":2}"#),
        ordered_domain_events: vec![json(r#"{"event":"increment"}"#)],
        timer_requests: vec![TimerRequestV1::ScheduleNext {
            timer_id: parsed(TIMER),
            due: parsed("2026-08-15T12:00:30Z"),
            canonical_payload: json(r#"{"kind":"schedule"}"#),
        }],
        ordered_attention_signals: vec![json(r#"{"kind":"alert"}"#)],
    });
    let reject = ActivityDispositionV1::Reject(ActivityRejectionV1 {
        declared_code: "counter_rejected".to_owned(),
        bounded_safe_details: json(r#"{"reason":"closed"}"#),
    });
    let cases = [
        (
            apply,
            br#"{"activity_disposition_type":"apply","next_activity_state":{"count":2},"ordered_attention_signals":[{"kind":"alert"}],"ordered_domain_events":[{"event":"increment"}],"timer_requests":[{"canonical_payload":{"kind":"schedule"},"due":"2026-08-15T12:00:30Z","timer_id":"01ARZ3NDEKTSV4RRFFQ69G5FC0","timer_request_type":"schedule_next"}]}"#
                .as_slice(),
        ),
        (
            reject,
            br#"{"activity_disposition_type":"reject","bounded_safe_details":{"reason":"closed"},"declared_code":"counter_rejected"}"#.as_slice(),
        ),
    ];

    for (value, expected_bytes) in cases {
        let encoded = encode(&value)
            .unwrap_or_else(|error| unreachable!("Activity disposition encode failed: {error}"));
        assert_eq!(encoded, expected_bytes);
        let decoded = CanonicalJsonV1::decode_canonical::<ActivityDispositionV1>(expected_bytes)
            .unwrap_or_else(|error| unreachable!("Activity disposition decode failed: {error}"));
        assert_eq!(decoded, value);
        assert_eq!(
            encode(&decoded).unwrap_or_else(|error| unreachable!(
                "Activity disposition re-encode failed: {error}"
            )),
            expected_bytes
        );
    }

    assert!(CanonicalJsonV1::decode_canonical::<ActivityDispositionV1>(
        br#"{"activity_disposition_type":"reject","bounded_safe_details":{"reason":"closed"},"declared_code":"counter_rejected","extra":true}"#
    )
    .is_err());
}

#[test]
fn genesis_requires_active_core_for_creation_and_replay() {
    let mut input = genesis_input();
    input.initial_core_state.room_status = RoomStatusV1::Archived;
    assert!(matches!(
        CoreTraceV1::create_for_conformance(
            input,
            fixture_role_validator,
            fixture_activity_reducer
        ),
        Err(TraceErrorV1::GenesisMustBeActive)
    ));

    let trace = new_trace();
    let mut genesis = trace.genesis().clone();
    genesis.initial_core_state.room_status = RoomStatusV1::Archived;
    let bytes = genesis
        .canonical_bytes()
        .unwrap_or_else(|error| unreachable!("Genesis encode failed: {error}"));
    let failure = replay_failure_for(&bytes, &[]);
    assert_eq!(failure.class, ReplayFailureClassV1::Stimulus);
    assert!(failure.last_verified_head.is_none());
}

#[test]
fn lifecycle_trace_covers_atomic_core_semantics() {
    let trace = build_lifecycle_trace();
    assert_eq!(trace.transition_count(), 9);
    assert_eq!(trace.activity_callback_count(), 10);
    assert_eq!(trace.administrative_receipt_count(), 11);
    assert_eq!(trace.external_effect_count(), 0);
    assert_eq!(trace.head().room_seq().get(), 9);
    assert_eq!(trace.core_state().room_status(), RoomStatusV1::Archived);

    let departed = trace
        .core_state()
        .membership(&parsed(MEMBER_B))
        .unwrap_or_else(|| unreachable!("departed Membership missing"));
    let replacement = trace
        .core_state()
        .membership(&parsed(MEMBER_C))
        .unwrap_or_else(|| unreachable!("replacement Membership missing"));
    assert_eq!(departed.standing(), MembershipStandingV1::Departed);
    assert_eq!(replacement.standing(), MembershipStandingV1::Enabled);
    assert_eq!(departed.principal_id(), replacement.principal_id());
    assert_ne!(departed.member_id(), replacement.member_id());

    let archive = &trace.transitions()[7];
    assert_eq!(archive.ordered_timer_changes().len(), 1);
    assert!(matches!(
        archive.ordered_timer_changes()[0],
        TimerChangeV1::Cancel { .. }
    ));
}

#[test]
fn invalid_core_proposals_fail_before_activity_or_receipt() {
    let callbacks = Arc::new(AtomicUsize::new(0));
    let callback_counter = Arc::clone(&callbacks);
    let mut trace = CoreTraceV1::create_for_conformance(
        genesis_input(),
        fixture_role_validator,
        move |input| {
            callback_counter.fetch_add(1, Ordering::SeqCst);
            fixture_activity_reducer(input)
        },
    )
    .unwrap_or_else(|error| unreachable!("fixture Genesis failed: {error}"));

    let member_a = trace
        .core_state()
        .membership(&parsed(MEMBER_A))
        .unwrap_or_else(|| unreachable!("member A missing"))
        .clone();
    let member_b = trace
        .core_state()
        .membership(&parsed(MEMBER_B))
        .unwrap_or_else(|| unreachable!("member B missing"))
        .clone();
    let mixed = CoreChangeSetV1::atomic(vec![
        MembershipChangeV1::role_change(member_a, "scout")
            .unwrap_or_else(|error| unreachable!("Role change failed: {error}")),
        MembershipChangeV1::depart(member_b),
    ])
    .unwrap_or_else(|error| unreachable!("mixed changeset build failed: {error}"));
    let mixed_stimulus = core_stimulus(
        &trace,
        CoreProposedKindV1::MembershipChangeSet,
        mixed,
        "operator.mixed",
        "mixed-1",
        "2026-08-15T12:01:00Z",
    );
    assert!(matches!(
        trace.advance(mixed_stimulus),
        Err(TraceErrorV1::Core(CoreValidationErrorV1::MixedVetoClasses))
    ));
    assert_eq!(callbacks.load(Ordering::SeqCst), 0);
    assert_eq!(trace.transition_count(), 0);
    assert_eq!(trace.administrative_receipt_count(), 0);

    let duplicate_principal = participant(MEMBER_C, PRINCIPAL_A, PrincipalKindV1::Agent, "scout");
    let join = core_stimulus(
        &trace,
        CoreProposedKindV1::Join,
        CoreChangeSetV1::one(MembershipChangeV1::join(duplicate_principal)),
        "operator.bad_join",
        "join-bad",
        "2026-08-15T12:02:00Z",
    );
    assert!(matches!(
        trace.advance(join),
        Err(TraceErrorV1::Core(
            CoreValidationErrorV1::DuplicateLivePrincipal(_)
        ))
    ));
    assert_eq!(callbacks.load(Ordering::SeqCst), 0);
}

#[test]
fn homogeneous_mandatory_changes_apply_atomically_and_individual_role_applies() {
    let mut trace = new_trace();
    let member_a = trace
        .core_state()
        .membership(&parsed(MEMBER_A))
        .unwrap_or_else(|| unreachable!("member A missing"))
        .clone();
    let member_b = trace
        .core_state()
        .membership(&parsed(MEMBER_B))
        .unwrap_or_else(|| unreachable!("member B missing"))
        .clone();
    let changes = CoreChangeSetV1::atomic(vec![
        MembershipChangeV1::suspend(member_a),
        MembershipChangeV1::depart(member_b),
    ])
    .unwrap_or_else(|error| unreachable!("mandatory changeset failed: {error}"));
    let stimulus = core_stimulus(
        &trace,
        CoreProposedKindV1::MembershipChangeSet,
        changes,
        "operator.atomic_mandatory",
        "mandatory-atomic",
        "2026-08-15T12:01:00Z",
    );
    committed(&mut trace, stimulus);
    assert_eq!(trace.transition_count(), 1);
    assert_eq!(
        trace
            .core_state()
            .membership(&parsed(MEMBER_A))
            .unwrap_or_else(|| unreachable!("member A missing"))
            .standing(),
        MembershipStandingV1::Suspended
    );
    assert_eq!(
        trace
            .core_state()
            .membership(&parsed(MEMBER_B))
            .unwrap_or_else(|| unreachable!("member B missing"))
            .standing(),
        MembershipStandingV1::Departed
    );

    let mut role_trace = new_trace();
    let member_a = role_trace
        .core_state()
        .membership(&parsed(MEMBER_A))
        .unwrap_or_else(|| unreachable!("member A missing"))
        .clone();
    let change = MembershipChangeV1::role_change(member_a, "scout")
        .unwrap_or_else(|error| unreachable!("Role change failed: {error}"));
    let stimulus = core_stimulus(
        &role_trace,
        CoreProposedKindV1::RoleChange,
        CoreChangeSetV1::one(change),
        "operator.role_change",
        "role-accepted",
        "2026-08-15T12:01:00Z",
    );
    committed(&mut role_trace, stimulus);
    assert_eq!(
        role_trace
            .core_state()
            .membership(&parsed(MEMBER_A))
            .unwrap_or_else(|| unreachable!("member A missing"))
            .role(),
        Some("scout")
    );
}

#[test]
fn whole_atomic_swap_may_reject_but_archive_reject_is_pack_fault() {
    let mut trace = new_trace();
    let member_a = trace
        .core_state()
        .membership(&parsed(MEMBER_A))
        .unwrap_or_else(|| unreachable!("member A missing"))
        .clone();
    let member_b = trace
        .core_state()
        .membership(&parsed(MEMBER_B))
        .unwrap_or_else(|| unreachable!("member B missing"))
        .clone();
    let swap = CoreChangeSetV1::atomic(vec![
        MembershipChangeV1::role_change(member_a, "insider")
            .unwrap_or_else(|error| unreachable!("Role change A failed: {error}")),
        MembershipChangeV1::role_change(member_b, "navigator")
            .unwrap_or_else(|error| unreachable!("Role change B failed: {error}")),
    ])
    .unwrap_or_else(|error| unreachable!("swap failed: {error}"));
    let stimulus = core_stimulus(
        &trace,
        CoreProposedKindV1::MembershipChangeSet,
        swap,
        "reject.role_swap",
        "swap-reject",
        "2026-08-15T12:01:00Z",
    );
    assert!(matches!(
        trace.advance(stimulus),
        Ok(AdvanceDispositionV1::RejectionRecorded {
            existing: false,
            ..
        })
    ));
    assert_eq!(trace.transition_count(), 0);
    assert_eq!(trace.head().room_seq().get(), 0);
    assert_eq!(trace.activity_callback_count(), 1);
    assert_eq!(trace.administrative_receipt_count(), 1);

    let mut archive_trace = new_trace();
    let before = archive_trace.head().clone();
    let archive = core_stimulus(
        &archive_trace,
        CoreProposedKindV1::Archive,
        CoreChangeSetV1::archive(RoomStatusV1::Active),
        "reject.archive",
        "archive-reject",
        "2026-08-15T12:01:00Z",
    );
    assert_eq!(
        archive_trace.advance(archive),
        Err(TraceErrorV1::Pack(PackFaultV1::MandatoryStimulusRejected))
    );
    assert_eq!(archive_trace.head(), &before);
    assert_eq!(archive_trace.transition_count(), 0);
    assert_eq!(archive_trace.administrative_receipt_count(), 0);
}

#[test]
#[allow(clippy::too_many_lines)]
fn archive_repeat_is_no_change_and_all_other_forbidden_classes_fail_closed() {
    let mut trace = build_lifecycle_trace();
    let before_head = trace.head().clone();
    let callbacks = trace.activity_callback_count();
    let repeat_archive = core_stimulus(
        &trace,
        CoreProposedKindV1::Archive,
        CoreChangeSetV1::archive(RoomStatusV1::Archived),
        "operator.archive_repeat",
        "archive-repeat",
        "2026-08-15T12:12:00Z",
    );
    assert!(matches!(
        trace.advance(repeat_archive),
        Ok(AdvanceDispositionV1::NoChangeRecorded {
            existing: false,
            ..
        })
    ));
    assert_eq!(trace.head(), &before_head);
    assert_eq!(trace.activity_callback_count(), callbacks);

    let replacement = trace
        .core_state()
        .membership(&parsed(MEMBER_C))
        .unwrap_or_else(|| unreachable!("replacement missing"))
        .clone();
    let role_same = MembershipChangeV1::role_change(
        replacement.clone(),
        replacement
            .role()
            .unwrap_or_else(|| unreachable!("replacement Role missing")),
    )
    .unwrap_or_else(|error| unreachable!("Role NoChange failed: {error}"));
    let role = core_stimulus(
        &trace,
        CoreProposedKindV1::RoleChange,
        CoreChangeSetV1::one(role_same),
        "operator.archived_role",
        "archived-role",
        "2026-08-15T12:13:00Z",
    );
    assert!(matches!(
        trace.advance(role),
        Err(TraceErrorV1::Core(
            CoreValidationErrorV1::ArchivedMutationForbidden
        ))
    ));

    let access_same = MembershipChangeV1::access_mode_change(
        replacement.clone(),
        AccessModeV1::Participant,
        replacement.role().map(ToOwned::to_owned),
    )
    .unwrap_or_else(|error| unreachable!("Access NoChange failed: {error}"));
    let access = core_stimulus(
        &trace,
        CoreProposedKindV1::AccessModeChange,
        CoreChangeSetV1::one(access_same),
        "operator.archived_access",
        "archived-access",
        "2026-08-15T12:14:00Z",
    );
    assert!(trace.advance(access).is_err());

    let suspended = trace
        .core_state()
        .membership(&parsed(MEMBER_A))
        .unwrap_or_else(|| unreachable!("suspended member missing"))
        .clone();
    let resume = core_stimulus(
        &trace,
        CoreProposedKindV1::Resume,
        CoreChangeSetV1::one(MembershipChangeV1::resume(suspended)),
        "operator.archived_resume",
        "archived-resume",
        "2026-08-15T12:15:00Z",
    );
    assert!(trace.advance(resume).is_err());

    let new_member = participant(
        "01ARZ3NDEKTSV4RRFFQ69G5FAZ",
        "01ARZ3NDEKTSV4RRFFQ69G5FB3",
        PrincipalKindV1::Agent,
        "scout",
    );
    let join = core_stimulus(
        &trace,
        CoreProposedKindV1::Join,
        CoreChangeSetV1::one(MembershipChangeV1::join(new_member)),
        "operator.archived_join",
        "archived-join",
        "2026-08-15T12:16:00Z",
    );
    assert!(trace.advance(join).is_err());

    let action = RecordedStimulusV1::ParticipantAction(ParticipantActionV1 {
        member_id: parsed(MEMBER_C),
        action_id: parsed(ACTION),
        action_type: "fixture_action".to_owned(),
        payload_schema_digest: parsed(PAYLOAD_DIGEST),
        canonical_payload: json(r#"{"move":1}"#),
        exact_basis_head: trace.head().clone(),
        admitted_at: parsed("2026-08-15T12:17:00Z"),
    });
    assert_eq!(
        trace.advance(action),
        Err(TraceErrorV1::ArchivedStimulusForbidden)
    );
    let external = RecordedStimulusV1::ExternalInput(ExternalInputV1 {
        source_id: parsed(SOURCE),
        input_id: parsed(INPUT),
        input_type: "fixture_input".to_owned(),
        recorded_at: parsed("2026-08-15T12:18:00Z"),
        canonical_payload: json(r#"{"input":true}"#),
        immutable_resource_references: Vec::new(),
    });
    assert_eq!(
        trace.advance(external),
        Err(TraceErrorV1::ArchivedStimulusForbidden)
    );
    assert_eq!(trace.head(), &before_head);
}

#[test]
fn mandatory_reject_is_pack_fault_and_malformed_no_change_is_not_recorded() {
    let mut trace = new_trace();
    let member_a = trace
        .core_state()
        .membership(&parsed(MEMBER_A))
        .unwrap_or_else(|| unreachable!("member A missing"))
        .clone();
    let reject = core_stimulus(
        &trace,
        CoreProposedKindV1::Suspend,
        CoreChangeSetV1::one(MembershipChangeV1::suspend(member_a)),
        "reject.mandatory",
        "mandatory-reject",
        "2026-08-15T12:01:00Z",
    );
    assert_eq!(
        trace.advance(reject),
        Err(TraceErrorV1::Pack(PackFaultV1::MandatoryStimulusRejected))
    );
    assert_eq!(trace.transition_count(), 0);
    assert_eq!(trace.administrative_receipt_count(), 0);
    assert_eq!(trace.activity_callback_count(), 1);

    let member_a = trace
        .core_state()
        .membership(&parsed(MEMBER_A))
        .unwrap_or_else(|| unreachable!("member A missing"))
        .clone();
    let malformed = MembershipChangeV1 {
        kind: MembershipChangeKindV1::Suspend,
        member_id: member_a.member_id.clone(),
        before: Some(member_a.clone()),
        after: member_a,
    };
    let malformed = core_stimulus(
        &trace,
        CoreProposedKindV1::Suspend,
        CoreChangeSetV1::one(malformed),
        "operator.malformed_nochange",
        "malformed-nochange",
        "2026-08-15T12:02:00Z",
    );
    assert!(matches!(
        trace.advance(malformed),
        Err(TraceErrorV1::Core(
            CoreValidationErrorV1::InvalidNoChangeTarget(_)
        ))
    ));
    assert_eq!(trace.administrative_receipt_count(), 0);
}

#[test]
fn participant_role_shape_and_departed_terminal_are_enforced() {
    assert!(
        MembershipV1::new(
            parsed(MEMBER_C),
            parsed(PRINCIPAL_A),
            PrincipalKindV1::Agent,
            MembershipStandingV1::Enabled,
            AccessModeV1::Participant,
            None,
        )
        .is_err()
    );
    assert!(
        MembershipV1::new(
            parsed(MEMBER_C),
            parsed(PRINCIPAL_A),
            PrincipalKindV1::Agent,
            MembershipStandingV1::Enabled,
            AccessModeV1::Spectator,
            Some("forbidden".to_owned()),
        )
        .is_err()
    );

    let trace = build_lifecycle_trace();
    let departed = trace
        .core_state()
        .membership(&parsed(MEMBER_B))
        .unwrap_or_else(|| unreachable!("departed member missing"))
        .clone();
    let mut departed_trace = trace;
    let attempt = core_stimulus(
        &departed_trace,
        CoreProposedKindV1::Resume,
        CoreChangeSetV1::one(MembershipChangeV1::resume(departed)),
        "operator.invalid_resume",
        "resume-departed",
        "2026-08-15T12:12:00Z",
    );
    assert!(departed_trace.advance(attempt).is_err());
}

#[test]
fn replay_reproduces_every_prefix_and_no_operational_effects() {
    let trace = build_lifecycle_trace();
    let genesis_bytes = trace
        .genesis_bytes()
        .unwrap_or_else(|error| unreachable!("Genesis encode failed: {error}"));
    let transition_bytes = trace
        .transition_bytes()
        .unwrap_or_else(|error| unreachable!("Transition encode failed: {error}"));
    let report = CoreTraceV1::replay_for_conformance(
        &genesis_bytes,
        &transition_bytes,
        fixture_role_validator,
        fixture_activity_reducer,
    )
    .unwrap_or_else(|failure| unreachable!("Replay failed: {failure:?}"));
    assert_eq!(report.final_head, *trace.head());
    assert_eq!(report.steps.len(), trace.transition_count() + 1);
    assert_eq!(report.activity_callback_count, trace.transition_count());
    assert_eq!(report.external_effect_count, 0);
    assert_eq!(report.receipt_count, 0);
    for (index, step) in report.steps.iter().enumerate() {
        assert_eq!(step.head.room_seq().get(), index as u64);
    }
    assert_eq!(
        report.steps[0].canonical_lineage_record_bytes,
        genesis_bytes
    );
    for (step, expected) in report.steps[1..].iter().zip(&transition_bytes) {
        assert_eq!(&step.canonical_lineage_record_bytes, expected);
    }
    assert_eq!(report.final_state().head(), trace.head());
    let member_c = report
        .final_state()
        .core_state()
        .membership(&parsed(MEMBER_C))
        .unwrap_or_else(|| unreachable!("replacement member missing"))
        .clone();
    let continuation = core_stimulus_at_seq(
        CoreProposedKindV1::Suspend,
        CoreChangeSetV1::one(MembershipChangeV1::suspend(member_c)),
        report.final_head.room_seq(),
        "operator.replay_continuation",
        "replay-continuation",
        "2026-08-15T12:12:00Z",
    );
    let prepared = report
        .continuation_preparer()
        .prepare(report.final_state(), continuation)
        .unwrap_or_else(|error| unreachable!("continuation prepare failed: {error}"));
    assert!(matches!(
        prepared.disposition(),
        AdvanceDispositionV1::TransitionAccepted { .. }
    ));
}

fn replay_failure_for(genesis: &[u8], transitions: &[Vec<u8>]) -> ReplayFailureV1 {
    match CoreTraceV1::replay_for_conformance(
        genesis,
        transitions,
        fixture_role_validator,
        fixture_activity_reducer,
    ) {
        Ok(_) => unreachable!("tampered Replay unexpectedly succeeded"),
        Err(failure) => failure,
    }
}

fn refresh_transition_lineage(transition: &mut TransitionV1) {
    transition.transition_hash = transition
        .calculate_hash()
        .unwrap_or_else(|error| unreachable!("Transition rehash failed: {error}"));
}

fn refresh_transition_state_and_lineage(transition: &mut TransitionV1) {
    transition.resulting_core_state_hash = hash_core_state(&transition.resulting_core_state)
        .unwrap_or_else(|error| unreachable!("Core rehash failed: {error}"));
    transition.resulting_activity_state_hash = hash_activity_state(
        &transition.pack_digest,
        &transition.resulting_activity_state,
    )
    .unwrap_or_else(|error| unreachable!("Activity rehash failed: {error}"));
    transition.resulting_authoritative_state_hash = hash_authoritative_state(
        &transition.pack_digest,
        &transition.resulting_core_state_hash,
        &transition.resulting_activity_state_hash,
    )
    .unwrap_or_else(|error| unreachable!("aggregate rehash failed: {error}"));
    refresh_transition_lineage(transition);
}

#[test]
#[allow(clippy::too_many_lines)]
fn replay_detects_state_lineage_order_version_digest_and_stimulus_mutations() {
    let trace = build_lifecycle_trace();
    let genesis = trace
        .genesis_bytes()
        .unwrap_or_else(|error| unreachable!("Genesis encode failed: {error}"));
    let original = trace
        .transition_bytes()
        .unwrap_or_else(|error| unreachable!("Transition encode failed: {error}"));

    let mut mutation = trace.transitions()[1].clone();
    mutation.room_seq =
        RoomSequenceV1::new(42).unwrap_or_else(|error| unreachable!("sequence failed: {error}"));
    let mut bytes = original.clone();
    bytes[1] = mutation
        .canonical_bytes()
        .unwrap_or_else(|error| unreachable!("tamper encode failed: {error}"));
    let failure = replay_failure_for(&genesis, &bytes);
    assert_eq!(failure.class, ReplayFailureClassV1::Sequence);
    assert_eq!(
        failure
            .last_verified_head
            .as_ref()
            .unwrap_or_else(|| unreachable!("last Head missing"))
            .room_seq()
            .get(),
        1
    );

    let mut mutation = trace.transitions()[1].clone();
    mutation.previous_transition_or_genesis_hash = parsed(PAYLOAD_DIGEST);
    bytes = original.clone();
    bytes[1] = mutation
        .canonical_bytes()
        .unwrap_or_else(|error| unreachable!("tamper encode failed: {error}"));
    assert_eq!(
        replay_failure_for(&genesis, &bytes).class,
        ReplayFailureClassV1::PriorHash
    );

    let mut mutation = trace.transitions()[1].clone();
    mutation.codec_id = "worldstream/canonical-json/v2".to_owned();
    bytes = original.clone();
    bytes[1] = mutation
        .canonical_bytes()
        .unwrap_or_else(|error| unreachable!("tamper encode failed: {error}"));
    assert_eq!(
        replay_failure_for(&genesis, &bytes).class,
        ReplayFailureClassV1::VersionOrDigest
    );

    let mut mutation = trace.transitions()[1].clone();
    mutation.pack_digest = parsed(PAYLOAD_DIGEST);
    bytes = original.clone();
    bytes[1] = mutation
        .canonical_bytes()
        .unwrap_or_else(|error| unreachable!("tamper encode failed: {error}"));
    assert_eq!(
        replay_failure_for(&genesis, &bytes).class,
        ReplayFailureClassV1::VersionOrDigest
    );

    let mut mutation = trace.transitions()[1].clone();
    if let RecordedStimulusV1::CoreProposed(proposal) = &mut mutation.recorded_stimulus {
        proposal.expected_room_seq = RoomSequenceV1::new(99)
            .unwrap_or_else(|error| unreachable!("sequence failed: {error}"));
    } else {
        unreachable!("expected Core Stimulus");
    }
    refresh_transition_lineage(&mut mutation);
    bytes = original.clone();
    bytes[1] = mutation
        .canonical_bytes()
        .unwrap_or_else(|error| unreachable!("tamper encode failed: {error}"));
    assert_eq!(
        replay_failure_for(&genesis, &bytes).class,
        ReplayFailureClassV1::Stimulus
    );

    let mut mutation = trace.transitions()[0].clone();
    mutation.ordered_domain_events.reverse();
    refresh_transition_lineage(&mut mutation);
    bytes = original.clone();
    bytes[0] = mutation
        .canonical_bytes()
        .unwrap_or_else(|error| unreachable!("tamper encode failed: {error}"));
    assert_eq!(
        replay_failure_for(&genesis, &bytes).class,
        ReplayFailureClassV1::DomainEvents
    );

    let mut mutation = trace.transitions()[0].clone();
    mutation.resulting_activity_state = json(r#"{"mutated":true}"#);
    refresh_transition_state_and_lineage(&mut mutation);
    bytes = original.clone();
    bytes[0] = mutation
        .canonical_bytes()
        .unwrap_or_else(|error| unreachable!("tamper encode failed: {error}"));
    assert_eq!(
        replay_failure_for(&genesis, &bytes).class,
        ReplayFailureClassV1::ActivityState
    );

    let mut mutation = trace.transitions()[7].clone();
    mutation.ordered_timer_changes.clear();
    refresh_transition_lineage(&mut mutation);
    bytes = original.clone();
    bytes[7] = mutation
        .canonical_bytes()
        .unwrap_or_else(|error| unreachable!("tamper encode failed: {error}"));
    assert_eq!(
        replay_failure_for(&genesis, &bytes).class,
        ReplayFailureClassV1::TimerChanges
    );

    let mut mutation = trace.transitions()[0].clone();
    mutation.resulting_authoritative_state_hash = parsed(PAYLOAD_DIGEST);
    bytes = original.clone();
    bytes[0] = mutation
        .canonical_bytes()
        .unwrap_or_else(|error| unreachable!("tamper encode failed: {error}"));
    assert_eq!(
        replay_failure_for(&genesis, &bytes).class,
        ReplayFailureClassV1::StateHashes
    );

    let mut mutation = trace.transitions()[0].clone();
    mutation.transition_hash = parsed(PAYLOAD_DIGEST);
    bytes = original;
    bytes[0] = mutation
        .canonical_bytes()
        .unwrap_or_else(|error| unreachable!("tamper encode failed: {error}"));
    assert_eq!(
        replay_failure_for(&genesis, &bytes).class,
        ReplayFailureClassV1::LineageHash
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn replay_detects_direct_core_attention_component_hash_and_genesis_mutations() {
    let trace = build_lifecycle_trace();
    let genesis_bytes = trace
        .genesis_bytes()
        .unwrap_or_else(|error| unreachable!("Genesis encode failed: {error}"));
    let original = trace
        .transition_bytes()
        .unwrap_or_else(|error| unreachable!("Transition encode failed: {error}"));

    let mut mutation = trace.transitions()[0].clone();
    mutation
        .resulting_core_state
        .memberships
        .get_mut(&parsed(MEMBER_A))
        .unwrap_or_else(|| unreachable!("member A missing"))
        .standing = MembershipStandingV1::Suspended;
    let mut bytes = original.clone();
    bytes[0] = mutation
        .canonical_bytes()
        .unwrap_or_else(|error| unreachable!("Core tamper encode failed: {error}"));
    assert_eq!(
        replay_failure_for(&genesis_bytes, &bytes).class,
        ReplayFailureClassV1::StateHashes
    );

    let mut mutation = trace.transitions()[0].clone();
    mutation
        .ordered_attention_signals
        .push(json(r#"{"attention":"mutated"}"#));
    refresh_transition_lineage(&mut mutation);
    bytes = original.clone();
    bytes[0] = mutation
        .canonical_bytes()
        .unwrap_or_else(|error| unreachable!("Attention tamper encode failed: {error}"));
    assert_eq!(
        replay_failure_for(&genesis_bytes, &bytes).class,
        ReplayFailureClassV1::AttentionSignals
    );

    for mutate in [
        |transition: &mut TransitionV1| {
            transition.resulting_core_state_hash = parsed(PAYLOAD_DIGEST);
        },
        |transition: &mut TransitionV1| {
            transition.resulting_activity_state_hash = parsed(PAYLOAD_DIGEST);
        },
    ] {
        let mut mutation = trace.transitions()[0].clone();
        mutate(&mut mutation);
        bytes = original.clone();
        bytes[0] = mutation
            .canonical_bytes()
            .unwrap_or_else(|error| unreachable!("hash tamper encode failed: {error}"));
        assert_eq!(
            replay_failure_for(&genesis_bytes, &bytes).class,
            ReplayFailureClassV1::StateHashes
        );
    }

    let mut genesis = trace.genesis().clone();
    genesis.configuration = json(r#"{"mutated":"configuration"}"#);
    let bytes = genesis
        .canonical_bytes()
        .unwrap_or_else(|error| unreachable!("Genesis config tamper encode failed: {error}"));
    let role_calls = Arc::new(AtomicUsize::new(0));
    let activity_calls = Arc::new(AtomicUsize::new(0));
    let role_counter = Arc::clone(&role_calls);
    let activity_counter = Arc::clone(&activity_calls);
    let Err(failure) = CoreTraceV1::replay_for_conformance(
        &bytes,
        &original,
        move |_| {
            role_counter.fetch_add(1, Ordering::SeqCst);
            Ok(())
        },
        move |input| {
            activity_counter.fetch_add(1, Ordering::SeqCst);
            fixture_activity_reducer(input)
        },
    ) else {
        unreachable!("tampered Genesis unexpectedly replayed")
    };
    assert_eq!(failure.class, ReplayFailureClassV1::LineageHash);
    assert_eq!(role_calls.load(Ordering::SeqCst), 0);
    assert_eq!(activity_calls.load(Ordering::SeqCst), 0);

    let mut genesis = trace.genesis().clone();
    genesis.room_seed =
        parsed("hex:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff");
    let bytes = genesis
        .canonical_bytes()
        .unwrap_or_else(|error| unreachable!("Genesis seed tamper encode failed: {error}"));
    assert_eq!(
        replay_failure_for(&bytes, &original).class,
        ReplayFailureClassV1::LineageHash
    );

    let mut genesis = trace.genesis().clone();
    genesis.created_at = parsed("2026-08-15T12:00:01Z");
    let bytes = genesis
        .canonical_bytes()
        .unwrap_or_else(|error| unreachable!("Genesis time tamper encode failed: {error}"));
    assert_eq!(
        replay_failure_for(&bytes, &original).class,
        ReplayFailureClassV1::LineageHash
    );

    let mut genesis = trace.genesis().clone();
    genesis.genesis_hash = parsed(PAYLOAD_DIGEST);
    let bytes = genesis
        .canonical_bytes()
        .unwrap_or_else(|error| unreachable!("Genesis hash tamper encode failed: {error}"));
    assert_eq!(
        replay_failure_for(&bytes, &original).class,
        ReplayFailureClassV1::LineageHash
    );
}

#[test]
fn replay_checks_transition_hashes_before_pack_owned_callbacks() {
    let trace = build_lifecycle_trace();
    let genesis = trace
        .genesis_bytes()
        .unwrap_or_else(|error| unreachable!("Genesis encode failed: {error}"));
    let mut transitions = trace
        .transition_bytes()
        .unwrap_or_else(|error| unreachable!("Transition encode failed: {error}"));
    let mut mutation = trace.transitions()[0].clone();
    mutation
        .ordered_attention_signals
        .push(json(r#"{"stale_hash":true}"#));
    transitions[0] = mutation
        .canonical_bytes()
        .unwrap_or_else(|error| unreachable!("tamper encode failed: {error}"));

    let role_calls = Arc::new(AtomicUsize::new(0));
    let activity_calls = Arc::new(AtomicUsize::new(0));
    let role_counter = Arc::clone(&role_calls);
    let activity_counter = Arc::clone(&activity_calls);
    let Err(failure) = CoreTraceV1::replay_for_conformance(
        &genesis,
        &transitions,
        move |state| {
            role_counter.fetch_add(1, Ordering::SeqCst);
            fixture_role_validator(state)
        },
        move |input| {
            activity_counter.fetch_add(1, Ordering::SeqCst);
            fixture_activity_reducer(input)
        },
    ) else {
        unreachable!("stale Transition hash unexpectedly replayed")
    };
    assert_eq!(failure.class, ReplayFailureClassV1::LineageHash);
    assert_eq!(role_calls.load(Ordering::SeqCst), 1);
    assert_eq!(activity_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn correctly_rehashed_semantically_invalid_core_still_fails_replay() {
    let trace = build_lifecycle_trace();
    let genesis = trace
        .genesis_bytes()
        .unwrap_or_else(|error| unreachable!("Genesis encode failed: {error}"));
    let mut transitions = trace
        .transition_bytes()
        .unwrap_or_else(|error| unreachable!("Transition encode failed: {error}"));
    let mut forged = trace.transitions()[0].clone();
    let member_a = forged
        .resulting_core_state
        .memberships
        .get_mut(&parsed(MEMBER_A))
        .unwrap_or_else(|| unreachable!("member A missing"));
    member_a.role = Some("navigator".to_owned());
    forged.resulting_core_state_hash = hash_core_state(&forged.resulting_core_state)
        .unwrap_or_else(|error| unreachable!("Core hash failed: {error}"));
    forged.resulting_authoritative_state_hash = hash_authoritative_state(
        &forged.pack_digest,
        &forged.resulting_core_state_hash,
        &forged.resulting_activity_state_hash,
    )
    .unwrap_or_else(|error| unreachable!("aggregate hash failed: {error}"));
    forged.transition_hash = forged
        .calculate_hash()
        .unwrap_or_else(|error| unreachable!("Transition hash failed: {error}"));
    transitions[0] = forged
        .canonical_bytes()
        .unwrap_or_else(|error| unreachable!("forged encode failed: {error}"));
    assert_eq!(
        replay_failure_for(&genesis, &transitions).class,
        ReplayFailureClassV1::CoreInvariant
    );
}

#[test]
fn replay_rejects_noncanonical_history_unknown_fields_and_reused_admin_identity() {
    let trace = build_lifecycle_trace();
    let genesis = trace
        .genesis_bytes()
        .unwrap_or_else(|error| unreachable!("Genesis encode failed: {error}"));
    let transitions = trace
        .transition_bytes()
        .unwrap_or_else(|error| unreachable!("Transition encode failed: {error}"));
    let mut noncanonical = b" ".to_vec();
    noncanonical.extend_from_slice(&genesis);
    assert_eq!(
        replay_failure_for(&noncanonical, &transitions).class,
        ReplayFailureClassV1::NonCanonicalRecord
    );

    let mut value: serde_json::Value = serde_json::from_slice(&transitions[0])
        .unwrap_or_else(|error| unreachable!("fixture decode failed: {error}"));
    value
        .as_object_mut()
        .unwrap_or_else(|| unreachable!("Transition is not object"))
        .insert("unknown".to_owned(), serde_json::Value::Bool(true));
    let unknown = CanonicalJsonV1::from_serialize(&value)
        .and_then(|value| value.to_bytes())
        .unwrap_or_else(|error| unreachable!("unknown-field encode failed: {error}"));
    let mut mutated = transitions.clone();
    mutated[0] = unknown;
    assert_eq!(
        replay_failure_for(&genesis, &mutated).class,
        ReplayFailureClassV1::NonCanonicalRecord
    );

    let mut second = trace.transitions()[1].clone();
    let first_identity = match trace.transitions()[0].recorded_stimulus() {
        RecordedStimulusV1::CoreProposed(proposal) => proposal.operation_identity.clone(),
        _ => unreachable!("expected Core Stimulus"),
    };
    if let RecordedStimulusV1::CoreProposed(proposal) = &mut second.recorded_stimulus {
        proposal.operation_identity = first_identity;
    }
    refresh_transition_lineage(&mut second);
    mutated = transitions;
    mutated[1] = second
        .canonical_bytes()
        .unwrap_or_else(|error| unreachable!("identity tamper encode failed: {error}"));
    assert_eq!(
        replay_failure_for(&genesis, &mutated).class,
        ReplayFailureClassV1::Stimulus
    );
}

#[test]
fn admin_request_hash_excludes_recorded_time_and_authority_but_binds_semantics() {
    let mut trace = new_trace();
    let member_a = trace
        .core_state()
        .membership(&parsed(MEMBER_A))
        .unwrap_or_else(|| unreachable!("member A missing"))
        .clone();
    let stimulus = core_stimulus(
        &trace,
        CoreProposedKindV1::RoleChange,
        CoreChangeSetV1::one(
            MembershipChangeV1::role_change(member_a, "scout")
                .unwrap_or_else(|error| unreachable!("Role change failed: {error}")),
        ),
        "reject.role",
        "request-hash-1",
        "2026-08-15T12:01:00Z",
    );
    let first = trace
        .advance(stimulus.clone())
        .unwrap_or_else(|error| unreachable!("first rejection failed: {error}"));
    let AdvanceDispositionV1::RejectionRecorded {
        canonical_receipt_bytes: first_bytes,
        ..
    } = first
    else {
        unreachable!("expected rejection")
    };

    let mut retry = stimulus.clone();
    if let RecordedStimulusV1::CoreProposed(proposal) = &mut retry {
        proposal.recorded_at = parsed("2026-08-15T12:01:59Z");
        proposal.canonical_authority_attribution.authority_kind = CoreAuthorityKindV1::HostOperator;
    }
    let retry = trace
        .advance(retry)
        .unwrap_or_else(|error| unreachable!("semantic retry failed: {error}"));
    match retry {
        AdvanceDispositionV1::RejectionRecorded {
            existing: true,
            canonical_receipt_bytes,
            ..
        } => assert_eq!(canonical_receipt_bytes, first_bytes),
        _ => unreachable!("expected existing rejection"),
    }

    let mut conflict = stimulus.clone();
    if let RecordedStimulusV1::CoreProposed(proposal) = &mut conflict {
        proposal.reason_code = "reject.changed_reason".to_owned();
    }
    assert_eq!(
        trace.advance(conflict),
        Err(TraceErrorV1::IdempotencyConflict)
    );

    let RecordedStimulusV1::CoreProposed(proposal) = &stimulus else {
        unreachable!("expected Core proposal")
    };
    let basis = trace.genesis().complete_head();
    let expected = hash_administration_request(&basis, proposal)
        .unwrap_or_else(|error| unreachable!("request hash failed: {error}"));
    let mut changed = basis.clone();
    changed.room_id = parsed("01ARZ3NDEKTSV4RRFFQ69G5FB3");
    assert_ne!(
        hash_administration_request(&changed, proposal)
            .unwrap_or_else(|error| unreachable!("request hash failed: {error}")),
        expected
    );

    let mut host_head_variations = Vec::new();
    let mut changed = basis.clone();
    changed.genesis_or_transition_hash = parsed(PAYLOAD_DIGEST);
    host_head_variations.push(changed);
    let mut changed = basis.clone();
    changed.core_schema_version = "worldstream.core-room-state.v2".to_owned();
    host_head_variations.push(changed);
    let mut changed = basis.clone();
    changed.pack_digest = parsed(PAYLOAD_DIGEST);
    host_head_variations.push(changed);
    let mut changed = basis.clone();
    changed.core_state_hash = parsed(PAYLOAD_DIGEST);
    host_head_variations.push(changed);
    let mut changed = basis.clone();
    changed.activity_state_hash = parsed(PAYLOAD_DIGEST);
    host_head_variations.push(changed);
    let mut changed = basis;
    changed.authoritative_state_hash = parsed(PAYLOAD_DIGEST);
    host_head_variations.push(changed);
    for changed_basis in host_head_variations {
        assert_eq!(
            hash_administration_request(&changed_basis, proposal)
                .unwrap_or_else(|error| unreachable!("request hash failed: {error}")),
            expected
        );
    }
}

#[test]
fn reducer_preparer_and_install_seams_are_provenance_bound_and_nonmutating() {
    let mut trace = new_trace();
    let initial_head = trace.head().clone();
    let member_a = trace
        .core_state()
        .membership(&parsed(MEMBER_A))
        .unwrap_or_else(|| unreachable!("member A missing"))
        .clone();
    let stimulus = core_stimulus(
        &trace,
        CoreProposedKindV1::Suspend,
        CoreChangeSetV1::one(MembershipChangeV1::suspend(member_a)),
        "operator.prepare_suspend",
        "prepare-suspend",
        "2026-08-15T12:01:00Z",
    );
    let prepared = trace
        .prepare(stimulus.clone())
        .unwrap_or_else(|error| unreachable!("prepare failed: {error}"));
    assert!(prepared.is_new());
    assert_eq!(prepared.basis_complete_head(), &initial_head);
    assert_eq!(trace.head(), &initial_head);
    assert_eq!(trace.transition_count(), 0);
    assert_eq!(trace.administrative_receipt_count(), 0);
    assert_eq!(trace.activity_callback_count(), 1);

    let state = trace.verified_transition_state();
    let cloned_preparer = trace.transition_preparer().clone();
    assert!(cloned_preparer.prepare(&state, stimulus.clone()).is_ok());
    let foreign_preparer = RoomTransitionPreparerV1::new(
        CoreReducerV1::new(fixture_role_validator),
        fixture_activity_reducer,
    );
    assert!(matches!(
        foreign_preparer.prepare(&state, stimulus.clone()),
        Err(TraceErrorV1::TransitionPreparerProvenanceMismatch)
    ));

    let candidate = genesis_input().initial_core_state;
    let permissive = CoreReducerV1::new(fixture_role_validator);
    let verified = permissive
        .validate_state(candidate)
        .unwrap_or_else(|error| unreachable!("Core validation failed: {error}"));
    let strict = CoreReducerV1::new(|_| Err("strict policy".to_owned()));
    let RecordedStimulusV1::CoreProposed(proposal) = &stimulus else {
        unreachable!("expected Core proposal")
    };
    assert!(matches!(
        strict.reduce(&verified, proposal),
        Err(TraceErrorV1::CoreReducerProvenanceMismatch)
    ));

    let mut corrupt_preparer_state = trace
        .prepare(stimulus.clone())
        .unwrap_or_else(|error| unreachable!("prepare failed: {error}"));
    corrupt_preparer_state.corrupt_resulting_provenance_for_test(false);
    assert_eq!(
        trace.install_prepared(corrupt_preparer_state),
        Err(TraceErrorV1::TransitionPreparerProvenanceMismatch)
    );
    let mut corrupt_core_state = trace
        .prepare(stimulus.clone())
        .unwrap_or_else(|error| unreachable!("prepare failed: {error}"));
    corrupt_core_state.corrupt_resulting_provenance_for_test(true);
    assert_eq!(
        trace.install_prepared(corrupt_core_state),
        Err(TraceErrorV1::CoreReducerProvenanceMismatch)
    );

    let foreign_trace = new_trace();
    let foreign_plan = foreign_trace
        .prepare(stimulus)
        .unwrap_or_else(|error| unreachable!("foreign prepare failed: {error}"));
    assert_eq!(
        trace.install_prepared(foreign_plan),
        Err(TraceErrorV1::TransitionPreparerProvenanceMismatch)
    );
    assert!(matches!(
        trace.install_prepared(prepared),
        Ok(AdvanceDispositionV1::TransitionAccepted {
            existing: false,
            ..
        })
    ));
    assert_eq!(trace.head().room_seq().get(), 1);
}

#[test]
#[allow(clippy::too_many_lines)]
fn install_reresolves_same_head_administration_without_overwrite() {
    let mut trace = new_trace();
    let member_a = trace
        .core_state()
        .membership(&parsed(MEMBER_A))
        .unwrap_or_else(|| unreachable!("member A missing"))
        .clone();
    let rejection = core_stimulus(
        &trace,
        CoreProposedKindV1::RoleChange,
        CoreChangeSetV1::one(
            MembershipChangeV1::role_change(member_a.clone(), "scout")
                .unwrap_or_else(|error| unreachable!("Role change failed: {error}")),
        ),
        "reject.race",
        "race-key",
        "2026-08-15T12:01:00Z",
    );
    let first = trace
        .prepare(rejection.clone())
        .unwrap_or_else(|error| unreachable!("first prepare failed: {error}"));
    let identical = trace
        .prepare(rejection.clone())
        .unwrap_or_else(|error| unreachable!("identical prepare failed: {error}"));
    let mut conflicting = rejection;
    if let RecordedStimulusV1::CoreProposed(proposal) = &mut conflicting {
        proposal.reason_code = "reject.changed".to_owned();
    }
    let conflicting = trace
        .prepare(conflicting)
        .unwrap_or_else(|error| unreachable!("conflicting prepare failed: {error}"));

    let original_bytes = match trace
        .install_prepared(first)
        .unwrap_or_else(|error| unreachable!("first install failed: {error}"))
    {
        AdvanceDispositionV1::RejectionRecorded {
            existing: false,
            canonical_receipt_bytes,
            ..
        } => canonical_receipt_bytes,
        other => unreachable!("expected new rejection, got {other:?}"),
    };
    match trace
        .install_prepared(identical)
        .unwrap_or_else(|error| unreachable!("identical install failed: {error}"))
    {
        AdvanceDispositionV1::RejectionRecorded {
            existing: true,
            canonical_receipt_bytes,
            ..
        } => assert_eq!(canonical_receipt_bytes, original_bytes),
        other => unreachable!("expected existing rejection, got {other:?}"),
    }
    assert_eq!(
        trace.install_prepared(conflicting),
        Err(TraceErrorV1::IdempotencyConflict)
    );
    assert_eq!(trace.administrative_receipt_count(), 1);
    assert_eq!(trace.transition_count(), 0);

    let mut no_change_trace = new_trace();
    let member_a = no_change_trace
        .core_state()
        .membership(&parsed(MEMBER_A))
        .unwrap_or_else(|| unreachable!("member A missing"))
        .clone();
    let no_change = core_stimulus(
        &no_change_trace,
        CoreProposedKindV1::RoleChange,
        CoreChangeSetV1::one(
            MembershipChangeV1::role_change(member_a, "navigator")
                .unwrap_or_else(|error| unreachable!("NoChange failed: {error}")),
        ),
        "operator.nochange_race",
        "nochange-race",
        "2026-08-15T12:01:00Z",
    );
    let first = no_change_trace
        .prepare(no_change.clone())
        .unwrap_or_else(|error| unreachable!("NoChange prepare failed: {error}"));
    let second = no_change_trace
        .prepare(no_change)
        .unwrap_or_else(|error| unreachable!("NoChange retry prepare failed: {error}"));
    let first_bytes = match no_change_trace
        .install_prepared(first)
        .unwrap_or_else(|error| unreachable!("NoChange install failed: {error}"))
    {
        AdvanceDispositionV1::NoChangeRecorded {
            existing: false,
            canonical_receipt_bytes,
        } => canonical_receipt_bytes,
        other => unreachable!("expected new NoChange, got {other:?}"),
    };
    match no_change_trace
        .install_prepared(second)
        .unwrap_or_else(|error| unreachable!("NoChange retry install failed: {error}"))
    {
        AdvanceDispositionV1::NoChangeRecorded {
            existing: true,
            canonical_receipt_bytes,
        } => assert_eq!(canonical_receipt_bytes, first_bytes),
        other => unreachable!("expected existing NoChange, got {other:?}"),
    }
    assert_eq!(no_change_trace.administrative_receipt_count(), 1);
}

#[test]
fn install_reresolves_concurrent_advance_after_head_moves() {
    let mut trace = new_trace();
    let member_a = trace
        .core_state()
        .membership(&parsed(MEMBER_A))
        .unwrap_or_else(|| unreachable!("member A missing"))
        .clone();
    let stimulus = core_stimulus(
        &trace,
        CoreProposedKindV1::Suspend,
        CoreChangeSetV1::one(MembershipChangeV1::suspend(member_a)),
        "operator.concurrent_suspend",
        "concurrent-advance",
        "2026-08-15T12:01:00Z",
    );
    let first = trace
        .prepare(stimulus.clone())
        .unwrap_or_else(|error| unreachable!("first prepare failed: {error}"));
    let identical = trace
        .prepare(stimulus.clone())
        .unwrap_or_else(|error| unreachable!("identical prepare failed: {error}"));
    let mut changed = stimulus;
    if let RecordedStimulusV1::CoreProposed(proposal) = &mut changed {
        proposal.reason_code = "operator.changed_suspend".to_owned();
    }
    let conflicting = trace
        .prepare(changed)
        .unwrap_or_else(|error| unreachable!("conflicting prepare failed: {error}"));

    let first_transition = match trace
        .install_prepared(first)
        .unwrap_or_else(|error| unreachable!("first install failed: {error}"))
    {
        AdvanceDispositionV1::TransitionAccepted {
            existing: false,
            transition,
        } => transition,
        other => unreachable!("expected new Transition, got {other:?}"),
    };
    let existing_transition = match trace
        .install_prepared(identical)
        .unwrap_or_else(|error| unreachable!("identical install failed: {error}"))
    {
        AdvanceDispositionV1::TransitionAccepted {
            existing: true,
            transition,
        } => transition,
        other => unreachable!("expected existing Transition, got {other:?}"),
    };
    assert_eq!(
        first_transition
            .canonical_bytes()
            .unwrap_or_else(|error| unreachable!("Transition encode failed: {error}")),
        existing_transition
            .canonical_bytes()
            .unwrap_or_else(|error| unreachable!("Transition encode failed: {error}"))
    );
    assert_eq!(
        trace.install_prepared(conflicting),
        Err(TraceErrorV1::IdempotencyConflict)
    );
    assert_eq!(trace.head().room_seq().get(), 1);
    assert_eq!(trace.transition_count(), 1);
    assert_eq!(trace.administrative_receipt_count(), 1);
    assert_eq!(trace.activity_callback_count(), 3);
}

#[test]
#[allow(clippy::panic)]
fn role_validator_panic_is_typed_and_never_aliases_policy_text() {
    const FORMER_SENTINEL: &str = "worldstream.internal.role-validator-panicked";
    assert!(matches!(
        CoreTraceV1::create_for_conformance(
            genesis_input(),
            |_| Err(FORMER_SENTINEL.to_owned()),
            fixture_activity_reducer,
        ),
        Err(TraceErrorV1::Core(CoreValidationErrorV1::RolePolicy(detail)))
            if detail == FORMER_SENTINEL
    ));
    assert!(matches!(
        CoreTraceV1::create_for_conformance(
            genesis_input(),
            |_| -> Result<(), String> { panic!("validator panic") },
            fixture_activity_reducer,
        ),
        Err(TraceErrorV1::Pack(PackFaultV1::RoleValidatorPanicked))
    ));

    let valid = new_trace();
    let genesis = valid
        .genesis_bytes()
        .unwrap_or_else(|error| unreachable!("Genesis encode failed: {error}"));
    let Err(replay_failure) = CoreTraceV1::replay_for_conformance(
        &genesis,
        &[],
        |_| -> Result<(), String> { panic!("replay validator panic") },
        fixture_activity_reducer,
    ) else {
        unreachable!("panicking validator unexpectedly replayed")
    };
    assert_eq!(replay_failure.class, ReplayFailureClassV1::RuntimeFault);

    let calls = Arc::new(AtomicUsize::new(0));
    let validator_calls = Arc::clone(&calls);
    let mut trace = CoreTraceV1::create_for_conformance(
        genesis_input(),
        move |_| {
            if validator_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                Ok(())
            } else {
                panic!("live validator panic")
            }
        },
        fixture_activity_reducer,
    )
    .unwrap_or_else(|error| unreachable!("Genesis failed: {error}"));
    let before = trace.head().clone();
    let member_a = trace
        .core_state()
        .membership(&parsed(MEMBER_A))
        .unwrap_or_else(|| unreachable!("member A missing"))
        .clone();
    let stimulus = core_stimulus(
        &trace,
        CoreProposedKindV1::RoleChange,
        CoreChangeSetV1::one(
            MembershipChangeV1::role_change(member_a, "scout")
                .unwrap_or_else(|error| unreachable!("Role change failed: {error}")),
        ),
        "operator.role_change",
        "role-panic",
        "2026-08-15T12:01:00Z",
    );
    assert_eq!(
        trace.advance(stimulus),
        Err(TraceErrorV1::Pack(PackFaultV1::RoleValidatorPanicked))
    );
    assert_eq!(trace.head(), &before);
    assert_eq!(trace.activity_callback_count(), 0);
    assert_eq!(trace.administrative_receipt_count(), 0);
}

#[test]
fn timer_requests_allocate_host_owned_successor_generations() {
    let mut cancel_then_schedule =
        CoreTraceV1::create_for_conformance(genesis_input(), fixture_role_validator, |input| {
            let RecordedStimulusV1::CoreProposed(proposal) = input.recorded_stimulus else {
                unreachable!("expected Core proposal")
            };
            let timer_requests = match proposal.reason_code() {
                "operator.cancel_timer" => vec![TimerRequestV1::CancelCurrent {
                    timer_id: parsed(TIMER),
                    expected_generation: TimerGenerationV1::new(1)
                        .unwrap_or_else(|error| unreachable!("generation failed: {error}")),
                }],
                "operator.schedule_timer" => vec![TimerRequestV1::ScheduleNext {
                    timer_id: parsed(TIMER),
                    due: parsed("2026-08-15T13:00:00Z"),
                    canonical_payload: json(r#"{"kind":"second"}"#),
                }],
                other => unreachable!("unexpected reason {other}"),
            };
            Ok(ActivityDispositionV1::Apply(ActivityApplyV1 {
                next_activity_state: input.prior_activity_state.clone(),
                ordered_domain_events: Vec::new(),
                timer_requests,
                ordered_attention_signals: Vec::new(),
            }))
        })
        .unwrap_or_else(|error| unreachable!("Genesis failed: {error}"));
    let member_a = cancel_then_schedule
        .core_state()
        .membership(&parsed(MEMBER_A))
        .unwrap_or_else(|| unreachable!("member A missing"))
        .clone();
    let cancel = core_stimulus(
        &cancel_then_schedule,
        CoreProposedKindV1::RoleChange,
        CoreChangeSetV1::one(
            MembershipChangeV1::role_change(member_a, "scout")
                .unwrap_or_else(|error| unreachable!("Role change failed: {error}")),
        ),
        "operator.cancel_timer",
        "timer-cancel",
        "2026-08-15T12:01:00Z",
    );
    committed(&mut cancel_then_schedule, cancel);
    let member_a = cancel_then_schedule
        .core_state()
        .membership(&parsed(MEMBER_A))
        .unwrap_or_else(|| unreachable!("member A missing"))
        .clone();
    let schedule = core_stimulus(
        &cancel_then_schedule,
        CoreProposedKindV1::RoleChange,
        CoreChangeSetV1::one(
            MembershipChangeV1::role_change(member_a, "navigator")
                .unwrap_or_else(|error| unreachable!("Role change failed: {error}")),
        ),
        "operator.schedule_timer",
        "timer-schedule",
        "2026-08-15T12:02:00Z",
    );
    committed(&mut cancel_then_schedule, schedule);
    assert!(matches!(
        cancel_then_schedule.transitions()[0].ordered_timer_changes()[0],
        TimerChangeV1::Cancel { generation, .. } if generation.get() == 1
    ));
    assert!(matches!(
        cancel_then_schedule.transitions()[1].ordered_timer_changes()[0],
        TimerChangeV1::Schedule { generation, .. } if generation.get() == 2
    ));

    let mut fired_then_schedule =
        CoreTraceV1::create_for_conformance(genesis_input(), fixture_role_validator, |input| {
            assert!(matches!(
                input.recorded_stimulus,
                RecordedStimulusV1::TimerFired(_)
            ));
            Ok(ActivityDispositionV1::Apply(ActivityApplyV1 {
                next_activity_state: input.prior_activity_state.clone(),
                ordered_domain_events: Vec::new(),
                timer_requests: vec![TimerRequestV1::ScheduleNext {
                    timer_id: parsed(TIMER),
                    due: parsed("2026-08-15T13:00:00Z"),
                    canonical_payload: json(r#"{"kind":"after_fire"}"#),
                }],
                ordered_attention_signals: Vec::new(),
            }))
        })
        .unwrap_or_else(|error| unreachable!("Genesis failed: {error}"));
    let fired = RecordedStimulusV1::TimerFired(TimerFiredV1 {
        timer_id: parsed(TIMER),
        generation: TimerGenerationV1::new(1)
            .unwrap_or_else(|error| unreachable!("generation failed: {error}")),
        scheduled_for: parsed("2026-08-15T12:30:00Z"),
        canonical_payload: json(r#"{"kind":"deadline"}"#),
    });
    committed(&mut fired_then_schedule, fired);
    assert!(matches!(
        fired_then_schedule.transitions()[0].ordered_timer_changes()[0],
        TimerChangeV1::Schedule { generation, .. } if generation.get() == 2
    ));
}

#[test]
fn timer_request_witness_duplicates_and_time_fail_as_pack_faults() {
    let generation_one = TimerGenerationV1::new(1)
        .unwrap_or_else(|error| unreachable!("generation failed: {error}"));
    let generation_two = TimerGenerationV1::new(2)
        .unwrap_or_else(|error| unreachable!("generation failed: {error}"));
    let cases = vec![
        vec![TimerRequestV1::CancelCurrent {
            timer_id: parsed(TIMER),
            expected_generation: generation_two,
        }],
        vec![
            TimerRequestV1::CancelCurrent {
                timer_id: parsed(TIMER),
                expected_generation: generation_one,
            },
            TimerRequestV1::RescheduleCurrent {
                timer_id: parsed(TIMER),
                expected_generation: generation_one,
                new_due: parsed("2026-08-15T13:00:00Z"),
                new_canonical_payload: json(r#"{"kind":"duplicate"}"#),
            },
        ],
        vec![TimerRequestV1::RescheduleCurrent {
            timer_id: parsed(TIMER),
            expected_generation: generation_one,
            new_due: parsed("2026-08-15T12:01:00Z"),
            new_canonical_payload: json(r#"{"kind":"equal_time"}"#),
        }],
        vec![TimerRequestV1::CancelCurrent {
            timer_id: parsed("01ARZ3NDEKTSV4RRFFQ69G5FC1"),
            expected_generation: generation_one,
        }],
    ];
    for (index, requests) in cases.into_iter().enumerate() {
        let mut trace = trace_with_timer_requests(requests);
        let stimulus = role_change_stimulus(&trace, &format!("invalid-timer-{index}"));
        assert!(matches!(
            trace.advance(stimulus),
            Err(TraceErrorV1::Pack(PackFaultV1::InvalidTimerOutput(_)))
        ));
        assert_eq!(trace.transition_count(), 0);
        assert_eq!(trace.administrative_receipt_count(), 0);
        assert_eq!(trace.activity_callback_count(), 1);
    }

    let mut reschedule = trace_with_timer_requests(vec![TimerRequestV1::RescheduleCurrent {
        timer_id: parsed(TIMER),
        expected_generation: generation_one,
        new_due: parsed("2026-08-15T13:00:00Z"),
        new_canonical_payload: json(r#"{"kind":"rescheduled"}"#),
    }]);
    let stimulus = role_change_stimulus(&reschedule, "valid-reschedule");
    committed(&mut reschedule, stimulus);
    assert!(matches!(
        reschedule.transitions()[0].ordered_timer_changes()[0],
        TimerChangeV1::Reschedule {
            previous_generation,
            generation,
            ..
        } if previous_generation.get() == 1 && generation.get() == 2
    ));
}

#[test]
#[allow(clippy::panic)]
fn caught_activity_panic_cannot_commit() {
    let mut trace = CoreTraceV1::create_for_conformance(
        genesis_input(),
        fixture_role_validator,
        |_input| -> Result<ActivityDispositionV1, PackFaultV1> { panic!("fixture panic") },
    )
    .unwrap_or_else(|error| unreachable!("fixture Genesis failed: {error}"));
    let member_a = trace
        .core_state()
        .membership(&parsed(MEMBER_A))
        .unwrap_or_else(|| unreachable!("member A missing"))
        .clone();
    let stimulus = core_stimulus(
        &trace,
        CoreProposedKindV1::Suspend,
        CoreChangeSetV1::one(MembershipChangeV1::suspend(member_a)),
        "operator.suspend",
        "panic-1",
        "2026-08-15T12:01:00Z",
    );
    assert_eq!(
        trace.advance(stimulus),
        Err(TraceErrorV1::Pack(PackFaultV1::CallbackPanicked))
    );
    assert_eq!(trace.transition_count(), 0);
    assert_eq!(trace.head().room_seq().get(), 0);
    assert_eq!(trace.administrative_receipt_count(), 0);
    assert_eq!(trace.activity_callback_count(), 1);
}

#[test]
fn operational_integrity_metadata_cannot_affect_any_canonical_hash() {
    let trace = new_trace();
    let before = (
        trace.head().core_state_hash().clone(),
        trace.head().activity_state_hash().clone(),
        trace.head().authoritative_state_hash().clone(),
        trace.head().genesis_or_transition_hash().clone(),
    );
    let healthy = RoomIntegrityStateV1::new(
        RoomIntegrityStatusV1::Healthy,
        IntegrityGenerationV1::new(1)
            .unwrap_or_else(|error| unreachable!("generation failed: {error}")),
    );
    let quarantined = RoomIntegrityStateV1::new(
        RoomIntegrityStatusV1::Quarantined,
        IntegrityGenerationV1::new(99)
            .unwrap_or_else(|error| unreachable!("generation failed: {error}")),
    );
    assert_ne!(healthy, quarantined);
    let after = (
        hash_core_state(trace.core_state())
            .unwrap_or_else(|error| unreachable!("Core hash failed: {error}")),
        hash_activity_state(trace.head().pack_digest(), trace.activity_state())
            .unwrap_or_else(|error| unreachable!("Activity hash failed: {error}")),
        trace.head().authoritative_state_hash().clone(),
        trace
            .genesis()
            .calculate_hash()
            .unwrap_or_else(|error| unreachable!("Genesis hash failed: {error}")),
    );
    assert_eq!(before, after);
}

#[test]
fn typed_stimuli_have_only_variant_specific_semantic_time() {
    let trace = new_trace();
    let action = RecordedStimulusV1::ParticipantAction(ParticipantActionV1 {
        member_id: parsed(MEMBER_A),
        action_id: parsed(ACTION),
        action_type: "fixture_action".to_owned(),
        payload_schema_digest: parsed(PAYLOAD_DIGEST),
        canonical_payload: json(r#"{"move":1}"#),
        exact_basis_head: trace.head().clone(),
        admitted_at: parsed("2026-08-15T12:00:01Z"),
    });
    let timer = RecordedStimulusV1::TimerFired(TimerFiredV1 {
        timer_id: parsed(TIMER),
        generation: TimerGenerationV1::new(1)
            .unwrap_or_else(|error| unreachable!("generation failed: {error}")),
        scheduled_for: parsed("2026-08-15T12:30:00Z"),
        canonical_payload: json(r#"{"kind":"deadline"}"#),
    });
    let external = RecordedStimulusV1::ExternalInput(ExternalInputV1 {
        source_id: parsed(SOURCE),
        input_id: parsed(INPUT),
        input_type: "fixture_input".to_owned(),
        recorded_at: parsed("2026-08-15T12:00:02Z"),
        canonical_payload: json(r#"{"input":true}"#),
        immutable_resource_references: Vec::new(),
    });
    assert_eq!(action.semantic_time(), "2026-08-15T12:00:01Z");
    assert_eq!(timer.semantic_time(), "2026-08-15T12:30:00Z");
    assert_eq!(external.semantic_time(), "2026-08-15T12:00:02Z");
    for stimulus in [action, timer, external] {
        let bytes = encode(&stimulus)
            .unwrap_or_else(|error| unreachable!("Stimulus encode failed: {error}"));
        let value: serde_json::Value = serde_json::from_slice(&bytes)
            .unwrap_or_else(|error| unreachable!("Stimulus decode failed: {error}"));
        assert!(value.get("transition_at").is_none());
        assert!(value.get("commit_time").is_none());
    }
}

#[test]
fn timer_identity_binds_the_exact_scheduled_generation_witness() {
    let first = TimerFiredRequestV1::new(
        parsed(ROOM),
        parsed(TIMER),
        TimerGenerationV1::new(1)
            .unwrap_or_else(|error| unreachable!("generation failed: {error}")),
        parsed("2026-08-15T12:30:00Z"),
        json(r#"{"kind":"deadline"}"#),
    );
    let later = TimerFiredRequestV1::new(
        parsed(ROOM),
        parsed(TIMER),
        TimerGenerationV1::new(1)
            .unwrap_or_else(|error| unreachable!("generation failed: {error}")),
        parsed("2026-08-15T12:31:00Z"),
        json(r#"{"kind":"deadline"}"#),
    );
    let OperationIdentityV1::TimerFired(identity) = first.operation_identity() else {
        unreachable!("Timer request did not produce Timer identity")
    };
    assert_eq!(identity.room_id, parsed(ROOM));
    assert_eq!(identity.timer_id, parsed(TIMER));
    assert_eq!(identity.generation.get(), 1);
    assert_eq!(identity.scheduled_for.as_str(), "2026-08-15T12:30:00Z");
    assert_ne!(
        first
            .canonical_request_hash()
            .unwrap_or_else(|error| unreachable!("Timer hash failed: {error}")),
        later
            .canonical_request_hash()
            .unwrap_or_else(|error| unreachable!("Timer hash failed: {error}"))
    );
}

fn assert_transition_golden(transition: &TransitionV1, vector: &serde_json::Value) {
    assert_eq!(
        transition
            .hash_input_bytes()
            .unwrap_or_else(|error| unreachable!("Transition hash input failed: {error}")),
        vector["hash_input_bytes"]
            .as_str()
            .unwrap_or_else(|| unreachable!("lifecycle hash input missing"))
            .as_bytes()
    );
    assert_eq!(
        transition.transition_hash().to_string(),
        vector["digest"]
            .as_str()
            .unwrap_or_else(|| unreachable!("lifecycle digest missing"))
    );
    assert_eq!(
        transition
            .canonical_bytes()
            .unwrap_or_else(|error| unreachable!("Transition encode failed: {error}")),
        vector["record_bytes"]
            .as_str()
            .unwrap_or_else(|| unreachable!("lifecycle record missing"))
            .as_bytes()
    );
    assert_eq!(
        encode(&transition.complete_head())
            .unwrap_or_else(|error| unreachable!("Head encode failed: {error}")),
        vector["complete_head_bytes"]
            .as_str()
            .unwrap_or_else(|| unreachable!("lifecycle Head missing"))
            .as_bytes()
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn shared_literal_golden_freezes_all_five_hash_objects_records_and_heads() {
    let corpus: serde_json::Value =
        serde_json::from_str(include_str!("../../../tests/fixtures/core_v1_golden.json"))
            .unwrap_or_else(|error| unreachable!("golden corpus is invalid: {error}"));
    assert_eq!(corpus["schema"], "worldstream/core-v1-golden-corpus/v1");
    assert_eq!(
        Blake3DigestV1::hash(b"").to_string(),
        format!(
            "blake3:{}",
            corpus["blake3_empty_known_hex"]
                .as_str()
                .unwrap_or_else(|| unreachable!("known BLAKE3 vector is not a string"))
        )
    );

    let mut trace = build_lifecycle_trace();
    let genesis = trace.genesis();
    let transition = &trace.transitions()[0];
    let vectors = &corpus["hash_vectors"];
    let cases = [
        (
            "core",
            core_state_hash_bytes(genesis.initial_core_state())
                .unwrap_or_else(|error| unreachable!("Core hash input failed: {error}")),
            genesis.initial_core_state_hash(),
        ),
        (
            "activity",
            activity_state_hash_bytes(genesis.pack_digest(), genesis.initial_activity_state())
                .unwrap_or_else(|error| unreachable!("Activity hash input failed: {error}")),
            genesis.initial_activity_state_hash(),
        ),
        (
            "authoritative",
            authoritative_state_hash_bytes(
                genesis.pack_digest(),
                genesis.initial_core_state_hash(),
                genesis.initial_activity_state_hash(),
            )
            .unwrap_or_else(|error| unreachable!("aggregate hash input failed: {error}")),
            genesis.initial_authoritative_state_hash(),
        ),
        (
            "genesis",
            genesis
                .hash_input_bytes()
                .unwrap_or_else(|error| unreachable!("Genesis hash input failed: {error}")),
            genesis.genesis_hash(),
        ),
        (
            "transition",
            transition
                .hash_input_bytes()
                .unwrap_or_else(|error| unreachable!("Transition hash input failed: {error}")),
            transition.transition_hash(),
        ),
    ];
    let mut domains = BTreeSet::new();
    for (name, bytes, digest) in cases {
        let vector = &vectors[name];
        assert_eq!(
            bytes,
            vector["canonical_bytes"]
                .as_str()
                .unwrap_or_else(|| unreachable!("golden bytes missing"))
                .as_bytes(),
            "{name} hash-input bytes drifted"
        );
        assert_eq!(
            digest.to_string(),
            vector["digest"]
                .as_str()
                .unwrap_or_else(|| unreachable!("golden digest missing")),
            "{name} digest drifted"
        );
        assert_eq!(
            &digest.to_string()["blake3:".len()..],
            vector["digest_bytes_hex"]
                .as_str()
                .unwrap_or_else(|| unreachable!("golden raw digest missing"))
        );
        assert!(domains.insert(digest.to_string()));
    }

    assert_eq!(
        trace
            .genesis_bytes()
            .unwrap_or_else(|error| unreachable!("Genesis encode failed: {error}")),
        corpus["genesis_record_bytes"]
            .as_str()
            .unwrap_or_else(|| unreachable!("golden Genesis record missing"))
            .as_bytes()
    );
    assert_eq!(
        transition
            .canonical_bytes()
            .unwrap_or_else(|error| unreachable!("Transition encode failed: {error}")),
        corpus["transition_record_bytes"]
            .as_str()
            .unwrap_or_else(|| unreachable!("golden Transition record missing"))
            .as_bytes()
    );
    let expected_heads = corpus["prefix_complete_head_bytes"]
        .as_array()
        .unwrap_or_else(|| unreachable!("golden Heads missing"));
    assert_eq!(
        encode(&genesis.complete_head())
            .unwrap_or_else(|error| unreachable!("Genesis Head encode failed: {error}")),
        expected_heads[0]
            .as_str()
            .unwrap_or_else(|| unreachable!("golden Head zero missing"))
            .as_bytes()
    );
    assert_eq!(
        encode(&transition.complete_head())
            .unwrap_or_else(|error| unreachable!("Transition Head encode failed: {error}")),
        expected_heads[1]
            .as_str()
            .unwrap_or_else(|| unreachable!("golden Head one missing"))
            .as_bytes()
    );

    let lifecycle = &corpus["lifecycle_vectors"];
    let accepted = lifecycle["accepted_transitions"]
        .as_array()
        .unwrap_or_else(|| unreachable!("accepted lifecycle vectors missing"));
    assert_eq!(accepted.len(), trace.transitions().len());
    for (transition, vector) in trace.transitions().iter().zip(accepted) {
        assert_transition_golden(transition, vector);
    }
    let lifecycle_heads = lifecycle["prefix_complete_head_bytes"]
        .as_array()
        .unwrap_or_else(|| unreachable!("lifecycle Heads missing"));
    assert_eq!(lifecycle_heads.len(), trace.transitions().len() + 1);
    assert_eq!(
        encode(&trace.genesis().complete_head())
            .unwrap_or_else(|error| unreachable!("Genesis Head encode failed: {error}")),
        lifecycle_heads[0]
            .as_str()
            .unwrap_or_else(|| unreachable!("lifecycle Head zero missing"))
            .as_bytes()
    );
    for (transition, expected) in trace.transitions().iter().zip(&lifecycle_heads[1..]) {
        assert_eq!(
            encode(&transition.complete_head())
                .unwrap_or_else(|error| unreachable!("Head encode failed: {error}")),
            expected
                .as_str()
                .unwrap_or_else(|| unreachable!("lifecycle Head missing"))
                .as_bytes()
        );
    }

    let dispositions = &lifecycle["durable_dispositions"];
    assert_eq!(
        dispositions["clean_rejection_new_bytes"],
        dispositions["clean_rejection_existing_bytes"]
    );
    assert_eq!(
        dispositions["no_change_new_bytes"],
        dispositions["no_change_existing_bytes"]
    );
    let historical_a = participant(MEMBER_A, PRINCIPAL_A, PrincipalKindV1::Agent, "insider");
    let rejected = core_stimulus_at_seq(
        CoreProposedKindV1::RoleChange,
        CoreChangeSetV1::one(
            MembershipChangeV1::role_change(historical_a.clone(), "scout")
                .unwrap_or_else(|error| unreachable!("Role change failed: {error}")),
        ),
        RoomSequenceV1::new(5).unwrap_or_else(|error| unreachable!("sequence failed: {error}")),
        "reject.role",
        "reject-1",
        "2026-08-15T12:06:00Z",
    );
    let rejection_bytes = match trace
        .advance(rejected)
        .unwrap_or_else(|error| unreachable!("rejection resolution failed: {error}"))
    {
        AdvanceDispositionV1::RejectionRecorded {
            existing: true,
            canonical_receipt_bytes,
            ..
        } => canonical_receipt_bytes,
        other => unreachable!("expected existing rejection, got {other:?}"),
    };
    assert_eq!(
        rejection_bytes,
        dispositions["clean_rejection_new_bytes"]
            .as_str()
            .unwrap_or_else(|| unreachable!("golden rejection missing"))
            .as_bytes()
    );
    let no_change = core_stimulus_at_seq(
        CoreProposedKindV1::RoleChange,
        CoreChangeSetV1::one(
            MembershipChangeV1::role_change(historical_a, "insider")
                .unwrap_or_else(|error| unreachable!("NoChange failed: {error}")),
        ),
        RoomSequenceV1::new(5).unwrap_or_else(|error| unreachable!("sequence failed: {error}")),
        "operator.role_already_set",
        "nochange-1",
        "2026-08-15T12:07:00Z",
    );
    let no_change_bytes = match trace
        .advance(no_change)
        .unwrap_or_else(|error| unreachable!("NoChange resolution failed: {error}"))
    {
        AdvanceDispositionV1::NoChangeRecorded {
            existing: true,
            canonical_receipt_bytes,
        } => canonical_receipt_bytes,
        other => unreachable!("expected existing NoChange, got {other:?}"),
    };
    assert_eq!(
        no_change_bytes,
        dispositions["no_change_new_bytes"]
            .as_str()
            .unwrap_or_else(|| unreachable!("golden NoChange missing"))
            .as_bytes()
    );

    let mut role_trace = new_trace();
    let member_a = role_trace
        .core_state()
        .membership(&parsed(MEMBER_A))
        .unwrap_or_else(|| unreachable!("member A missing"))
        .clone();
    let stimulus = core_stimulus(
        &role_trace,
        CoreProposedKindV1::RoleChange,
        CoreChangeSetV1::one(
            MembershipChangeV1::role_change(member_a, "scout")
                .unwrap_or_else(|error| unreachable!("Role change failed: {error}")),
        ),
        "operator.role_change",
        "role-accepted",
        "2026-08-15T12:01:00Z",
    );
    committed(&mut role_trace, stimulus);
    assert_transition_golden(
        &role_trace.transitions()[0],
        &lifecycle["independent_transitions"]["individual_role_change"],
    );

    let mut mandatory_trace = new_trace();
    let member_a = mandatory_trace
        .core_state()
        .membership(&parsed(MEMBER_A))
        .unwrap_or_else(|| unreachable!("member A missing"))
        .clone();
    let member_b = mandatory_trace
        .core_state()
        .membership(&parsed(MEMBER_B))
        .unwrap_or_else(|| unreachable!("member B missing"))
        .clone();
    let changes = CoreChangeSetV1::atomic(vec![
        MembershipChangeV1::suspend(member_a),
        MembershipChangeV1::depart(member_b),
    ])
    .unwrap_or_else(|error| unreachable!("mandatory changes failed: {error}"));
    let stimulus = core_stimulus(
        &mandatory_trace,
        CoreProposedKindV1::MembershipChangeSet,
        changes,
        "operator.atomic_mandatory",
        "mandatory-atomic",
        "2026-08-15T12:01:00Z",
    );
    committed(&mut mandatory_trace, stimulus);
    assert_transition_golden(
        &mandatory_trace.transitions()[0],
        &lifecycle["independent_transitions"]["atomic_all_mandatory"],
    );

    let mut swap_reject_trace = new_trace();
    let member_a = swap_reject_trace
        .core_state()
        .membership(&parsed(MEMBER_A))
        .unwrap_or_else(|| unreachable!("member A missing"))
        .clone();
    let member_b = swap_reject_trace
        .core_state()
        .membership(&parsed(MEMBER_B))
        .unwrap_or_else(|| unreachable!("member B missing"))
        .clone();
    let swap = CoreChangeSetV1::atomic(vec![
        MembershipChangeV1::role_change(member_a, "insider")
            .unwrap_or_else(|error| unreachable!("Role change failed: {error}")),
        MembershipChangeV1::role_change(member_b, "navigator")
            .unwrap_or_else(|error| unreachable!("Role change failed: {error}")),
    ])
    .unwrap_or_else(|error| unreachable!("swap failed: {error}"));
    let stimulus = core_stimulus(
        &swap_reject_trace,
        CoreProposedKindV1::MembershipChangeSet,
        swap,
        "reject.role_swap",
        "swap-reject",
        "2026-08-15T12:01:00Z",
    );
    let atomic_rejection = match swap_reject_trace
        .advance(stimulus)
        .unwrap_or_else(|error| unreachable!("swap reject failed: {error}"))
    {
        AdvanceDispositionV1::RejectionRecorded {
            canonical_receipt_bytes,
            ..
        } => canonical_receipt_bytes,
        other => unreachable!("expected atomic rejection, got {other:?}"),
    };
    assert_eq!(
        atomic_rejection,
        dispositions["atomic_role_swap_rejection_bytes"]
            .as_str()
            .unwrap_or_else(|| unreachable!("golden atomic rejection missing"))
            .as_bytes()
    );

    let mut archive_reject_trace = new_trace();
    let before = encode(archive_reject_trace.head())
        .unwrap_or_else(|error| unreachable!("Head encode failed: {error}"));
    let stimulus = core_stimulus(
        &archive_reject_trace,
        CoreProposedKindV1::Archive,
        CoreChangeSetV1::archive(RoomStatusV1::Active),
        "reject.archive",
        "archive-reject",
        "2026-08-15T12:01:00Z",
    );
    assert_eq!(
        archive_reject_trace.advance(stimulus),
        Err(TraceErrorV1::Pack(PackFaultV1::MandatoryStimulusRejected))
    );
    let archive_vector = &lifecycle["archive_mandatory_reject"];
    assert_eq!(
        before,
        archive_vector["head_bytes_before"]
            .as_str()
            .unwrap_or_else(|| unreachable!("archive Head before missing"))
            .as_bytes()
    );
    assert_eq!(
        encode(archive_reject_trace.head())
            .unwrap_or_else(|error| unreachable!("Head encode failed: {error}")),
        archive_vector["head_bytes_after"]
            .as_str()
            .unwrap_or_else(|| unreachable!("archive Head after missing"))
            .as_bytes()
    );
    assert_eq!(archive_reject_trace.transition_count(), 0);
    assert_eq!(archive_reject_trace.administrative_receipt_count(), 0);
}
