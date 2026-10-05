//! Compare native operational rows with exact retained Replay witnesses.

use std::collections::BTreeMap;

use worldstream_backup::{
    DigestV1,
    native_sqlite::{NativeSqliteRestoreEvidenceV1, NativeSqliteRowV1, NativeSqliteValueV1},
};
use worldstream_core::{
    CanonicalJsonV1, CanonicalReplayReport, RecoveredObservationConsequenceV1,
    RecoveredTimerStateV1,
};

type VerificationResult<T> = Result<T, &'static str>;

/// Uses the already completed semantic Replay. It performs no Pack callback.
/// Existing native admission still validates receipts, MMR, and raw row shapes.
#[allow(clippy::too_many_lines)] // One closed comparison covers the retained operational domains.
pub(super) fn verify_replayed_operational_rows(
    room_id: &str,
    report: &CanonicalReplayReport,
    evidence: &NativeSqliteRestoreEvidenceV1,
) -> VerificationResult<()> {
    match evidence
        .integrity
        .get(room_id)
        .map(|value| value.0.as_str())
    {
        Some("faulted" | "quarantined") => return Ok(()),
        Some("healthy") => {}
        _ => return Err("Room integrity"),
    }
    if report.final_head.room_id().as_str() != room_id {
        return Err("Replay Room identity");
    }
    let replay = report
        .storage_verification()
        .map_err(|_| "Replay witnesses")?;
    let members = room_rows(evidence, "room_members", room_id)?;
    let mut stored_members = BTreeMap::new();
    for row in members {
        let member = text(row, 1)?;
        if stored_members.insert(member, row).is_some() {
            return Err("duplicate Membership");
        }
    }
    if stored_members.len() != replay.memberships().len() {
        return Err("Membership set");
    }
    let positions = replay
        .observation_positions()
        .iter()
        .map(|position| (position.member_id().as_str(), position))
        .collect::<BTreeMap<_, _>>();
    for expected in replay.memberships() {
        let member = expected.member_id().as_str();
        let row = stored_members.get(member).ok_or("Membership missing")?;
        if blob(row, 7)? != expected.canonical_membership_bytes()
            || report.membership_generations().get(member).copied() != Some(integer(row, 9)?)
        {
            return Err("Membership materialization or generation");
        }
        // The denormalized authority fields must name the same Membership.
        let value: serde_json::Value =
            serde_json::from_slice(expected.canonical_membership_bytes())
                .map_err(|_| "Membership JSON")?;
        for (index, field) in [
            (2, "principal_id"),
            (3, "principal_kind"),
            (4, "standing"),
            (5, "access_mode"),
        ] {
            if value.get(field).and_then(serde_json::Value::as_str) != Some(text(row, index)?) {
                return Err("Membership authority fields");
            }
        }
        if value.get("role").and_then(serde_json::Value::as_str) != optional_text(row, 6)? {
            return Err("Membership Role");
        }
        let position = positions.get(member).ok_or("Replay position missing")?;
        verify_position(
            row,
            position.frame_head(),
            position.reset_required_through(),
        )?;
    }

    let mut expected_timers = replay
        .timers()
        .iter()
        .map(|timer| {
            (
                timer.timer_id().to_string(),
                timer.generation().get(),
                timer.scheduled_for().to_string(),
                timer.canonical_payload_bytes().to_vec(),
                timer_state(timer.state()).to_owned(),
            )
        })
        .collect::<Vec<_>>();
    let mut stored_timers = room_rows(evidence, "timers", room_id)?
        .into_iter()
        .map(|row| {
            Ok((
                text(row, 1)?.to_owned(),
                unsigned(row, 2)?,
                text(row, 3)?.to_owned(),
                blob(row, 4)?.to_vec(),
                text(row, 5)?.to_owned(),
            ))
        })
        .collect::<VerificationResult<Vec<_>>>()?;
    expected_timers.sort_unstable();
    stored_timers.sort_unstable();
    if stored_timers != expected_timers {
        return Err("Timer ledger");
    }

    verify_activation_decisions(room_id, &replay, evidence)?;

    let mut expected_frames = Vec::new();
    for frame in replay.observation_frames() {
        let member = frame.member_id().as_str();
        let row = stored_members.get(member).ok_or("Frame Membership")?;
        if frame.frame_seq() >= unsigned(row, 10)? {
            expected_frames.push((
                member.to_owned(),
                frame.frame_seq(),
                frame.cause_room_seq().get(),
                digest(&frame.payload_hash().to_string())?,
            ));
        }
    }
    let mut stored_frames = room_rows(evidence, "observation_frames", room_id)?
        .into_iter()
        .map(|row| {
            let bytes = blob(row, 5)?;
            let hash = digest(text(row, 4)?)?;
            if hash != DigestV1::hash(bytes)
                || CanonicalJsonV1::from_canonical_bytes(bytes).is_err()
            {
                return Err("Frame payload");
            }
            Ok((
                text(row, 1)?.to_owned(),
                unsigned(row, 2)?,
                unsigned(row, 3)?,
                hash,
            ))
        })
        .collect::<VerificationResult<Vec<_>>>()?;
    expected_frames.sort_unstable();
    stored_frames.sort_unstable();
    if stored_frames != expected_frames {
        return Err("retained Frame witnesses");
    }

    let mut expected_consequences = replay
        .observation_consequences()
        .iter()
        .map(|consequence| match consequence {
            RecoveredObservationConsequenceV1::ResetRequired {
                member_id,
                cause_room_seq,
                projection_hash,
            } => Ok((
                member_id.to_string(),
                cause_room_seq.get(),
                "reset_required".to_owned(),
                Some(digest(&projection_hash.to_string())?),
            )),
            RecoveredObservationConsequenceV1::VisibilityLost {
                member_id,
                cause_room_seq,
            } => Ok((
                member_id.to_string(),
                cause_room_seq.get(),
                "visibility_lost".to_owned(),
                None,
            )),
        })
        .collect::<VerificationResult<Vec<_>>>()?;
    let mut stored_consequences = room_rows(evidence, "observation_consequences", room_id)?
        .into_iter()
        .map(|row| {
            let hash = match text(row, 3)? {
                "reset_required" => {
                    let hash = digest(text(row, 5)?)?;
                    let payload = blob(row, 4)?;
                    if DigestV1::hash(payload) != hash
                        || CanonicalJsonV1::from_canonical_bytes(payload).is_err()
                    {
                        return Err("reset projection");
                    }
                    Some(hash)
                }
                "visibility_lost" => {
                    if !matches!(row.values.get(4), Some(NativeSqliteValueV1::Null))
                        || !matches!(row.values.get(5), Some(NativeSqliteValueV1::Null))
                    {
                        return Err("visibility consequence");
                    }
                    None
                }
                _ => return Err("consequence kind"),
            };
            Ok((
                text(row, 1)?.to_owned(),
                unsigned(row, 2)?,
                text(row, 3)?.to_owned(),
                hash,
            ))
        })
        .collect::<VerificationResult<Vec<_>>>()?;
    expected_consequences.sort_unstable();
    stored_consequences.sort_unstable();
    if stored_consequences != expected_consequences {
        return Err("observation consequences");
    }
    Ok(())
}

fn verify_activation_decisions(
    room_id: &str,
    replay: &worldstream_core::ReplayStorageVerificationV1,
    evidence: &NativeSqliteRestoreEvidenceV1,
) -> VerificationResult<()> {
    let mut expected_decisions = replay
        .activation_decisions()
        .iter()
        .map(|decision| {
            (
                decision.cause_room_seq().get(),
                decision.decision_id().to_owned(),
                decision.target_member_id().map(ToString::to_string),
                decision.canonical_decision_bytes().to_vec(),
            )
        })
        .collect::<Vec<_>>();
    let mut stored_decisions = room_rows(evidence, "activation_decisions", room_id)?
        .into_iter()
        .map(|row| {
            Ok((
                unsigned(row, 1)?,
                text(row, 2)?.to_owned(),
                optional_text(row, 3)?.map(str::to_owned),
                blob(row, 4)?.to_vec(),
            ))
        })
        .collect::<VerificationResult<Vec<_>>>()?;
    expected_decisions.sort_unstable();
    stored_decisions.sort_unstable();
    if stored_decisions != expected_decisions {
        return Err("Activation decisions");
    }

    Ok(())
}

fn verify_position(
    row: &NativeSqliteRowV1,
    frame_head: u64,
    replay_reset: Option<u64>,
) -> VerificationResult<()> {
    let floor = unsigned(row, 10)?;
    let cursor = optional_unsigned(row, 11)?.unwrap_or(0);
    let reset = optional_unsigned(row, 12)?;
    if unsigned(row, 8)? != frame_head
        || floor == 0
        || floor > frame_head.saturating_add(1)
        || cursor > frame_head
        || optional_unsigned(row, 11)? == Some(0)
        || reset.is_some_and(|marker| marker > frame_head)
    {
        return Err("observation position");
    }
    let lost_prefix = (floor.saturating_sub(1) > cursor).then_some(floor.saturating_sub(1));
    let required = lost_prefix
        .into_iter()
        .chain(replay_reset.filter(|marker| *marker > cursor && *marker > 0))
        .max();
    if required.is_some_and(|minimum| reset.is_none_or(|marker| marker < minimum)) {
        return Err("required observation reset");
    }
    Ok(())
}

fn room_rows<'a>(
    evidence: &'a NativeSqliteRestoreEvidenceV1,
    table: &str,
    room: &str,
) -> VerificationResult<Vec<&'a NativeSqliteRowV1>> {
    let mut result = Vec::new();
    for row in evidence.operational.tables.get(table).into_iter().flatten() {
        if text(row, 0)? == room {
            result.push(row);
        }
    }
    Ok(result)
}
fn text(row: &NativeSqliteRowV1, index: usize) -> VerificationResult<&str> {
    match row.values.get(index) {
        Some(NativeSqliteValueV1::Text(value)) => Ok(value),
        _ => Err("text row field"),
    }
}
fn optional_text(row: &NativeSqliteRowV1, index: usize) -> VerificationResult<Option<&str>> {
    match row.values.get(index) {
        Some(NativeSqliteValueV1::Text(value)) => Ok(Some(value)),
        Some(NativeSqliteValueV1::Null) => Ok(None),
        _ => Err("optional text row field"),
    }
}
fn blob(row: &NativeSqliteRowV1, index: usize) -> VerificationResult<&[u8]> {
    match row.values.get(index) {
        Some(NativeSqliteValueV1::Blob(value)) => Ok(value),
        _ => Err("blob row field"),
    }
}
fn integer(row: &NativeSqliteRowV1, index: usize) -> VerificationResult<i64> {
    match row.values.get(index) {
        Some(NativeSqliteValueV1::Integer(value)) => Ok(*value),
        _ => Err("integer row field"),
    }
}
fn unsigned(row: &NativeSqliteRowV1, index: usize) -> VerificationResult<u64> {
    u64::try_from(integer(row, index)?).map_err(|_| "negative row field")
}
fn optional_unsigned(row: &NativeSqliteRowV1, index: usize) -> VerificationResult<Option<u64>> {
    match row.values.get(index) {
        Some(NativeSqliteValueV1::Integer(value)) => Ok(Some(
            u64::try_from(*value).map_err(|_| "negative optional row field")?,
        )),
        Some(NativeSqliteValueV1::Null) => Ok(None),
        _ => Err("optional integer row field"),
    }
}
fn digest(value: &str) -> VerificationResult<DigestV1> {
    DigestV1::parse(value.strip_prefix("blake3:").unwrap_or(value)).map_err(|_| "digest row field")
}
const fn timer_state(state: RecoveredTimerStateV1) -> &'static str {
    match state {
        RecoveredTimerStateV1::Scheduled => "scheduled",
        RecoveredTimerStateV1::Fired => "fired",
        RecoveredTimerStateV1::Cancelled => "cancelled",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use worldstream_backup::native_sqlite::{NativeSqliteLimits, extract_restore_evidence};
    use worldstream_core::{
        CanonicalHistoryFormat, CanonicalRoomTrace, CompleteHeadV1, builtin_worldstream_registry,
    };

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn replay(
        evidence: &NativeSqliteRestoreEvidenceV1,
        room: &str,
    ) -> Result<CanonicalReplayReport, Box<dyn std::error::Error>> {
        let records = &evidence.canonical_records[room];
        let transitions = records
            .iter()
            .skip(1)
            .map(|record| record.bytes.clone())
            .collect::<Vec<_>>();
        let head = CanonicalJsonV1::decode_canonical::<CompleteHeadV1>(
            &evidence.room_heads[room].complete_head_bytes,
        )?;
        let (core, activity) = &evidence.materializations[room];
        Ok(CanonicalRoomTrace::replay_for_storage(
            &builtin_worldstream_registry()?,
            &head,
            &records[0].bytes,
            &transitions,
            Some(core),
            Some(activity),
        )?)
    }

    #[test]
    #[allow(clippy::too_many_lines)] // Exercise independent corruptions on actual provider rows.
    fn actual_both_format_rows_reject_generation_frame_timer_and_decision_tampering() -> TestResult
    {
        let temporary = tempfile::tempdir()?;
        super::super::tests::prepare_test_directory(temporary.path())?;
        let source = temporary.path().join("operational.sqlite3");
        let (_, rooms) = super::super::tests::initialize_room_sources_and_companion(
            &source,
            &[CanonicalHistoryFormat::V1, CanonicalHistoryFormat::V2],
            true,
        )?;
        let evidence = extract_restore_evidence(&source, NativeSqliteLimits::default())?;
        for room in rooms {
            let report = replay(&evidence, &room)?;
            assert_eq!(
                verify_replayed_operational_rows(&room, &report, &evidence),
                Ok(())
            );
            let mut pruned = evidence.clone();
            let row = pruned
                .operational
                .tables
                .get_mut("room_members")
                .ok_or("members")?
                .iter_mut()
                .find(|row| text(row, 0) == Ok(room.as_str()))
                .ok_or("member")?;
            let head = integer(row, 8)?;
            row.values[10] = NativeSqliteValueV1::Integer(head + 1);
            row.values[12] = NativeSqliteValueV1::Integer(head);
            pruned
                .operational
                .tables
                .get_mut("observation_frames")
                .ok_or("frames")?
                .retain(|row| text(row, 0) != Ok(room.as_str()));
            assert_eq!(
                verify_replayed_operational_rows(&room, &report, &pruned),
                Ok(())
            );
            let row = pruned
                .operational
                .tables
                .get_mut("room_members")
                .ok_or("members")?
                .iter_mut()
                .find(|row| text(row, 0) == Ok(room.as_str()))
                .ok_or("member")?;
            row.values[12] = NativeSqliteValueV1::Null;
            assert_eq!(
                verify_replayed_operational_rows(&room, &report, &pruned),
                Err("required observation reset")
            );

            let mut changed = evidence.clone();
            let row = changed
                .operational
                .tables
                .get_mut("room_members")
                .ok_or("members")?
                .iter_mut()
                .find(|row| text(row, 0) == Ok(room.as_str()))
                .ok_or("member")?;
            row.values[9] = NativeSqliteValueV1::Integer(integer(row, 9)? + 1);
            assert_eq!(
                verify_replayed_operational_rows(&room, &report, &changed),
                Err("Membership materialization or generation")
            );

            let mut changed = evidence.clone();
            let row = changed
                .operational
                .tables
                .get_mut("observation_frames")
                .ok_or("frames")?
                .iter_mut()
                .find(|row| text(row, 0) == Ok(room.as_str()))
                .ok_or("frame")?;
            let payload = b"{}".to_vec();
            row.values[4] =
                NativeSqliteValueV1::Text(format!("blake3:{}", DigestV1::hash(&payload).as_str()));
            row.values[5] = NativeSqliteValueV1::Blob(payload);
            assert_eq!(
                verify_replayed_operational_rows(&room, &report, &changed),
                Err("retained Frame witnesses")
            );

            let mut changed = evidence.clone();
            changed
                .operational
                .tables
                .entry("timers".into())
                .or_default()
                .push(NativeSqliteRowV1 {
                    table: "timers".into(),
                    values: vec![
                        NativeSqliteValueV1::Text(room.clone()),
                        NativeSqliteValueV1::Text("injected".into()),
                        NativeSqliteValueV1::Integer(1),
                        NativeSqliteValueV1::Text("2026-08-15T12:00:30Z".into()),
                        NativeSqliteValueV1::Blob(b"{}".to_vec()),
                        NativeSqliteValueV1::Text("scheduled".into()),
                    ],
                });
            assert_eq!(
                verify_replayed_operational_rows(&room, &report, &changed),
                Err("Timer ledger")
            );

            let mut changed = evidence.clone();
            changed
                .operational
                .tables
                .entry("activation_decisions".into())
                .or_default()
                .push(NativeSqliteRowV1 {
                    table: "activation_decisions".into(),
                    values: vec![
                        NativeSqliteValueV1::Text(room.clone()),
                        NativeSqliteValueV1::Integer(1),
                        NativeSqliteValueV1::Text("injected".into()),
                        NativeSqliteValueV1::Null,
                        NativeSqliteValueV1::Blob(b"{}".to_vec()),
                    ],
                });
            assert_eq!(
                verify_replayed_operational_rows(&room, &report, &changed),
                Err("Activation decisions")
            );
            let mut isolated = changed;
            isolated.integrity.get_mut(&room).ok_or("integrity")?.0 = "faulted".into();
            assert_eq!(
                verify_replayed_operational_rows(&room, &report, &isolated),
                Ok(())
            );
        }
        Ok(())
    }

    #[test]
    #[allow(clippy::too_many_lines)] // Build exact selected-format Pack facts for mutation checks.
    fn exact_retained_heist_timer_witness_rejects_time_payload_and_state_changes() -> TestResult {
        use worldstream_core::{
            AccessModeV1, CoreRoomStateV1, MembershipStandingV1, MembershipV1,
            PackGenesisRequestV1, PrincipalKindV1, agent_heist_digest,
        };
        let temporary = tempfile::tempdir()?;
        super::super::tests::prepare_test_directory(temporary.path())?;
        let source = temporary.path().join("timer.sqlite3");
        let (_, rooms) = super::super::tests::initialize_room_sources_and_companion(
            &source,
            &[CanonicalHistoryFormat::V1],
            false,
        )?;
        let mut evidence = extract_restore_evidence(&source, NativeSqliteLimits::default())?;
        let room = &rooms[0];
        let registry = builtin_worldstream_registry()?;
        let memberships = [
            (
                "01ARZ3NDEKTSV4RRFFQ69G5FB1",
                "01ARZ3NDEKTSV4RRFFQ69G5FC1",
                "navigator",
            ),
            (
                "01ARZ3NDEKTSV4RRFFQ69G5FB2",
                "01ARZ3NDEKTSV4RRFFQ69G5FC2",
                "insider",
            ),
            (
                "01ARZ3NDEKTSV4RRFFQ69G5FB3",
                "01ARZ3NDEKTSV4RRFFQ69G5FC3",
                "broker",
            ),
        ]
        .into_iter()
        .map(
            |(member, principal, role)| -> Result<_, Box<dyn std::error::Error>> {
                Ok(MembershipV1::new(
                    member.parse()?,
                    principal.parse()?,
                    if role == "navigator" {
                        PrincipalKindV1::Agent
                    } else {
                        PrincipalKindV1::Human
                    },
                    MembershipStandingV1::Enabled,
                    AccessModeV1::Participant,
                    Some(role.into()),
                )?)
            },
        )
        .collect::<Result<Vec<_>, _>>()?;
        for format in [CanonicalHistoryFormat::V1, CanonicalHistoryFormat::V2] {
            let prepared = registry.prepare_genesis_for_new_room(&PackGenesisRequestV1 {
                room_id: room.parse()?, pack_digest: agent_heist_digest(),
                configuration: CanonicalJsonV1::parse(br#"{"briefing_duration_seconds":30,"commitment_duration_seconds":30,"commitment_reminder_seconds_before_deadline":10,"maximum_open_offers_per_role":4,"maximum_plans":12,"negotiation_duration_seconds":90,"pack_id":"worldstream.agent-heist","pack_schema":1,"result_duration_seconds":20,"roles":["navigator","insider","broker"]}"#)?,
                room_seed: "hex:000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f".parse()?,
                created_at: "2026-08-15T12:00:00Z".parse()?,
                initial_core_state: CoreRoomStateV1::active(memberships.clone())?,
            })?;
            let trace = CanonicalRoomTrace::create_from_retained_for_conformance(prepared, format)?;
            let report = CanonicalRoomTrace::replay_for_storage(
                &registry,
                trace.head(),
                &trace.genesis_bytes()?,
                &[],
                None,
                None,
            )?;
            let witnesses = report.storage_verification()?;
            let genesis_bytes = trace.genesis_bytes()?;
            let mut executor = trace;
            let phase_timer_id = executor.genesis().initial_timers()[0].timer_id.clone();
            let mut records = Vec::new();
            for _ in 0..2 {
                let timer = executor
                    .scheduled_timers()
                    .get(&phase_timer_id)
                    .ok_or("phase Timer")?;
                let prepared =
                    executor.prepare(worldstream_core::RecordedStimulusV1::TimerFired(
                        worldstream_core::TimerFiredV1 {
                            timer_id: timer.timer_id.clone(),
                            generation: timer.generation,
                            scheduled_for: timer.scheduled_for.clone(),
                            canonical_payload: timer.canonical_payload.clone(),
                        },
                    ))?;
                let worldstream_core::CanonicalAdvanceDisposition::TransitionAccepted {
                    transition,
                    ..
                } = prepared.disposition()
                else {
                    return Err("phase Timer rejected".into());
                };
                records.push(transition.canonical_bytes()?);
                executor = CanonicalRoomTrace::replay_for_storage(
                    &registry,
                    &transition.complete_head(),
                    &genesis_bytes,
                    &records,
                    None,
                    None,
                )?
                .into_trace();
            }
            let phase_report = CanonicalRoomTrace::replay_for_storage(
                &registry,
                executor.head(),
                &genesis_bytes,
                &records,
                None,
                None,
            )?;
            let phase_witnesses = phase_report.storage_verification()?;
            assert!(!phase_witnesses.activation_decisions().is_empty());
            let mut decisions = evidence.clone();
            decisions.operational.tables.insert(
                "activation_decisions".into(),
                phase_witnesses
                    .activation_decisions()
                    .iter()
                    .map(|decision| NativeSqliteRowV1 {
                        table: "activation_decisions".into(),
                        values: vec![
                            NativeSqliteValueV1::Text(room.clone()),
                            NativeSqliteValueV1::Integer(
                                i64::try_from(decision.cause_room_seq().get()).unwrap_or(0),
                            ),
                            NativeSqliteValueV1::Text(decision.decision_id().into()),
                            decision
                                .target_member_id()
                                .map_or(NativeSqliteValueV1::Null, |member| {
                                    NativeSqliteValueV1::Text(member.to_string())
                                }),
                            NativeSqliteValueV1::Blob(decision.canonical_decision_bytes().to_vec()),
                        ],
                    })
                    .collect(),
            );
            assert_eq!(
                verify_activation_decisions(room, &phase_witnesses, &decisions),
                Ok(())
            );
            decisions
                .operational
                .tables
                .get_mut("activation_decisions")
                .ok_or("decisions")?[0]
                .values[4] = NativeSqliteValueV1::Blob(b"{}".to_vec());
            assert_eq!(
                verify_activation_decisions(room, &phase_witnesses, &decisions),
                Err("Activation decisions")
            );

            assert!(!witnesses.timers().is_empty());
            for table in [
                "room_members",
                "timers",
                "observation_frames",
                "observation_consequences",
                "activation_decisions",
            ] {
                evidence.operational.tables.insert(table.into(), Vec::new());
            }
            for membership in witnesses.memberships() {
                let decoded: serde_json::Value =
                    serde_json::from_slice(membership.canonical_membership_bytes())?;
                let mut values = vec![
                    NativeSqliteValueV1::Text(room.clone()),
                    NativeSqliteValueV1::Text(membership.member_id().to_string()),
                ];
                for field in ["principal_id", "principal_kind", "standing", "access_mode"] {
                    values.push(NativeSqliteValueV1::Text(
                        decoded[field].as_str().ok_or("Membership field")?.into(),
                    ));
                }
                values.extend([
                    NativeSqliteValueV1::Text(decoded["role"].as_str().ok_or("Role")?.into()),
                    NativeSqliteValueV1::Blob(membership.canonical_membership_bytes().to_vec()),
                    NativeSqliteValueV1::Integer(0),
                    NativeSqliteValueV1::Integer(1),
                    NativeSqliteValueV1::Integer(1),
                    NativeSqliteValueV1::Null,
                    NativeSqliteValueV1::Null,
                    NativeSqliteValueV1::Integer(0),
                ]);
                evidence
                    .operational
                    .tables
                    .get_mut("room_members")
                    .ok_or("members")?
                    .push(NativeSqliteRowV1 {
                        table: "room_members".into(),
                        values,
                    });
            }
            for timer in witnesses.timers() {
                evidence
                    .operational
                    .tables
                    .get_mut("timers")
                    .ok_or("timers")?
                    .push(NativeSqliteRowV1 {
                        table: "timers".into(),
                        values: vec![
                            NativeSqliteValueV1::Text(room.clone()),
                            NativeSqliteValueV1::Text(timer.timer_id().to_string()),
                            NativeSqliteValueV1::Integer(i64::try_from(timer.generation().get())?),
                            NativeSqliteValueV1::Text(timer.scheduled_for().to_string()),
                            NativeSqliteValueV1::Blob(timer.canonical_payload_bytes().to_vec()),
                            NativeSqliteValueV1::Text(timer_state(timer.state()).into()),
                        ],
                    });
            }
            assert_eq!(
                verify_replayed_operational_rows(room, &report, &evidence),
                Ok(())
            );
            for (column, value) in [
                (3, NativeSqliteValueV1::Text("2026-08-15T12:00:31Z".into())),
                (4, NativeSqliteValueV1::Blob(b"{}".to_vec())),
                (5, NativeSqliteValueV1::Text("fired".into())),
            ] {
                let mut changed = evidence.clone();
                changed
                    .operational
                    .tables
                    .get_mut("timers")
                    .ok_or("timers")?[0]
                    .values[column] = value;
                assert_eq!(
                    verify_replayed_operational_rows(room, &report, &changed),
                    Err("Timer ledger")
                );
            }
        }
        Ok(())
    }
}
