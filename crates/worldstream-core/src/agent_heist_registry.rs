//! Embedded exact-revision registry row for Agent Heist.

use std::str::FromStr;

use crate::{
    AccessModeV1, CanonicalJsonV1, CoreRoomStateV1, MembershipStandingV1, MembershipV1,
    PackDigestV1, PackGenesisRequestV1, PackGoldenActionV1, PackGoldenCorpusV1,
    PackGoldenViewerKindV1, PackGoldenViewerV1, PackRegistryErrorV1, PackRegistryStatusV1,
    PackRegistryV1, PrincipalKindV1,
    activity_pack::{CanonicalPackCodecV1, PackRegistryArtifactsV1, PackRegistryEntryV1},
    agent_heist::{agent_heist_legacy_revision, agent_heist_revision},
};

const NAVIGATOR_MEMBER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FC0";
const INSIDER_MEMBER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FC1";
const BROKER_MEMBER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FC2";
const SPECTATOR_MEMBER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FC3";
const OPERATOR_MEMBER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FC4";

// Authored after the executor and corpus are stable. The first construction
// test prints the value while this row is being introduced; it is then fixed
// here so registry startup verifies behavior rather than self-generating it.
const TRANSCRIPT_DIGEST: &str =
    "blake3:6d5436e75793e167b7ef50efe05fd59310c67c29e84afcdb82a4db03b90850ed";
const LEGACY_TRANSCRIPT_DIGEST: &str =
    "blake3:77a13c04178c6a0110d4b30ae3e6683730f2f6a5991e2a8e71d6db2d361d0d63";
#[cfg(test)]
const CORPUS_DIGEST: &str =
    "blake3:c79d1e0c37eb32e54924b4b42f9d3d2d790e456d7fc888698c4d0b2b467ad958";
#[cfg(test)]
const LEGACY_CORPUS_DIGEST: &str =
    "blake3:046e533959dce4a9da3e51f4459fd5cec158df7cef9b9d641451f96ed005141e";

/// Builds the selectable, retained-runnable Agent Heist registry.
///
/// # Errors
///
/// Returns a registry error if the exact revision lock, codec bundle,
/// executor provenance, or golden corpus does not verify.
pub fn builtin_agent_heist_registry() -> Result<PackRegistryV1, PackRegistryErrorV1> {
    let (descriptor, lock, schemas, codecs, artifact_digest) = agent_heist_revision();
    let corpus = golden_corpus(descriptor.revision_digest.clone(), TRANSCRIPT_DIGEST);
    let artifacts = PackRegistryArtifactsV1 {
        expected_revision_digest: descriptor.revision_digest.clone(),
        schemas: Some(schemas.clone()),
        codecs: Some(codecs.clone()),
        codec_implementation: Some(CanonicalPackCodecV1::canonical_v1()),
        executor_artifact_digest: artifact_digest.clone(),
        golden_corpus_digest: corpus.digest()?,
        golden_corpus: Some(corpus),
    };
    let status = PackRegistryStatusV1 {
        selectable_for_new_rooms: true,
        runnable_for_retained_rooms: true,
    };
    let (legacy_descriptor, legacy_lock, legacy_schemas, legacy_codecs, legacy_artifact_digest) =
        agent_heist_legacy_revision();
    let legacy_corpus = golden_corpus(
        legacy_descriptor.revision_digest.clone(),
        LEGACY_TRANSCRIPT_DIGEST,
    );
    let legacy_artifacts = PackRegistryArtifactsV1 {
        expected_revision_digest: legacy_descriptor.revision_digest.clone(),
        schemas: Some(legacy_schemas.clone()),
        codecs: Some(legacy_codecs.clone()),
        codec_implementation: Some(CanonicalPackCodecV1::canonical_v1()),
        executor_artifact_digest: legacy_artifact_digest.clone(),
        golden_corpus_digest: legacy_corpus.digest()?,
        golden_corpus: Some(legacy_corpus),
    };
    let legacy_status = PackRegistryStatusV1 {
        selectable_for_new_rooms: false,
        runnable_for_retained_rooms: true,
    };
    PackRegistryV1::try_new([
        PackRegistryEntryV1::agent_heist_v1(lock.clone(), descriptor, artifacts, status),
        PackRegistryEntryV1::agent_heist_v0(
            legacy_lock.clone(),
            legacy_descriptor,
            legacy_artifacts,
            legacy_status,
        ),
    ])
}

/// Exact semantic digest selected for new and retained Heist Rooms.
#[must_use]
pub fn agent_heist_digest() -> PackDigestV1 {
    agent_heist_revision().0.revision_digest.clone()
}

/// Exact semantic digest of the retained-only Agent Heist revision.
#[must_use]
pub fn agent_heist_retained_digest() -> PackDigestV1 {
    agent_heist_legacy_revision().0.revision_digest.clone()
}

fn golden_corpus(
    pack_digest: PackDigestV1,
    expected_transcript_digest: &str,
) -> PackGoldenCorpusV1 {
    let navigator = membership(
        NAVIGATOR_MEMBER,
        "01ARZ3NDEKTSV4RRFFQ69G5FD0",
        AccessModeV1::Participant,
        Some("navigator"),
    );
    let insider = membership(
        INSIDER_MEMBER,
        "01ARZ3NDEKTSV4RRFFQ69G5FD1",
        AccessModeV1::Participant,
        Some("insider"),
    );
    let broker = membership(
        BROKER_MEMBER,
        "01ARZ3NDEKTSV4RRFFQ69G5FD2",
        AccessModeV1::Participant,
        Some("broker"),
    );
    let spectator = membership(
        SPECTATOR_MEMBER,
        "01ARZ3NDEKTSV4RRFFQ69G5FD3",
        AccessModeV1::Spectator,
        None,
    );
    let operator = membership(
        OPERATOR_MEMBER,
        "01ARZ3NDEKTSV4RRFFQ69G5FD4",
        AccessModeV1::Operator,
        None,
    );
    let core = CoreRoomStateV1::active([navigator, insider, broker, spectator, operator])
        .unwrap_or_else(|error| unreachable!("Heist golden Core: {error}"));
    let descriptor = agent_heist_revision().0;
    let inspect_payload = canonical(br#"{"clue_id":"route"}"#);
    PackGoldenCorpusV1 {
        corpus_id: "worldstream/pack-golden-corpus/v1".to_owned(),
        genesis: PackGenesisRequestV1 {
            room_id: parsed("01ARZ3NDEKTSV4RRFFQ69G5FC5"),
            pack_digest,
            configuration: canonical(br#"{"briefing_duration_seconds":30,"commitment_duration_seconds":30,"commitment_reminder_seconds_before_deadline":10,"maximum_open_offers_per_role":4,"maximum_plans":12,"negotiation_duration_seconds":90,"pack_id":"worldstream.agent-heist","pack_schema":1,"result_duration_seconds":20,"roles":["navigator","insider","broker"]}"#),
            room_seed: parsed("hex:000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f"),
            created_at: parsed("2026-08-15T12:00:00Z"),
            initial_core_state: core,
        },
        viewers: vec![
            viewer(PackGoldenViewerKindV1::Public, SPECTATOR_MEMBER, 0, None),
            viewer(PackGoldenViewerKindV1::Participant, NAVIGATOR_MEMBER, 0, None),
            viewer(PackGoldenViewerKindV1::Operator, OPERATOR_MEMBER, 0, None),
            viewer(PackGoldenViewerKindV1::Historical, SPECTATOR_MEMBER, 0, None),
            viewer(PackGoldenViewerKindV1::Historical, NAVIGATOR_MEMBER, 0, None),
            viewer(PackGoldenViewerKindV1::Historical, OPERATOR_MEMBER, 0, None),
            // The corpus runner is Action-only; Complete is exercised by the
            // focused timer/replay tests, so this viewer remains deferred.
            viewer(PackGoldenViewerKindV1::FinalReveal, NAVIGATOR_MEMBER, 2, Some("Agent Heist final reveal is unavailable before completion")),
        ],
        actions: vec![PackGoldenActionV1 {
            member_id: parsed(NAVIGATOR_MEMBER),
            action_id: parsed("01ARZ3NDEKTSV4RRFFQ69G5FC6"),
            action_type: "inspect_clue".to_owned(),
            payload_schema_digest: descriptor.actions[0].payload_schema.schema_digest.clone(),
            canonical_payload: inspect_payload,
            admitted_at: parsed("2026-08-15T12:00:01Z"),
        }],
        expected_transcript_digest: parsed(expected_transcript_digest),
    }
}

fn membership(
    member_id: &str,
    principal_id: &str,
    access_mode: AccessModeV1,
    role: Option<&str>,
) -> MembershipV1 {
    MembershipV1::new(
        parsed(member_id),
        parsed(principal_id),
        PrincipalKindV1::Agent,
        MembershipStandingV1::Enabled,
        access_mode,
        role.map(str::to_owned),
    )
    .unwrap_or_else(|error| unreachable!("Heist golden Membership: {error}"))
}

fn viewer(
    kind: PackGoldenViewerKindV1,
    member_id: &str,
    available_after_action: u32,
    denied_before_detail: Option<&str>,
) -> PackGoldenViewerV1 {
    PackGoldenViewerV1 {
        kind,
        member_id: parsed(member_id),
        available_after_action,
        denied_before_detail: denied_before_detail.map(str::to_owned),
    }
}

fn canonical(bytes: &[u8]) -> CanonicalJsonV1 {
    CanonicalJsonV1::parse(bytes).unwrap_or_else(|error| unreachable!("Heist golden JSON: {error}"))
}

fn parsed<T>(value: &str) -> T
where
    T: FromStr,
    T::Err: std::fmt::Display,
{
    value
        .parse()
        .unwrap_or_else(|error| unreachable!("Heist golden value {value}: {error}"))
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;
    use crate::activity_pack::author_golden_transcript_digest_for_test;
    use crate::{
        AGENT_HEIST_RETAINED_VERSION, AGENT_HEIST_VERSION, AccessModeV1, ActionAdmittedAt,
        ActionId, Blake3DigestV1, CanonicalJsonV1, CoreTraceV1, CreationRecordedAt, MemberId,
        PackRevisionLockV1, ParticipantActionV1, PrincipalId, PrincipalKindV1, RecordedStimulusV1,
        RoomId, RoomSeedV1, TimerFiredV1, agent_heist_retained_digest,
    };
    use std::collections::BTreeMap;

    const CHECKED_IN_RETAINED_CORPUS: &str =
        include_str!("../../../examples/heist/retained_corpus.json");

    #[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
    #[serde(deny_unknown_fields)]
    struct CorpusFileV1 {
        schema: String,
        canonical_encoding: String,
        purpose: String,
        genesis: CorpusFileGenesisV1,
        action: CorpusFileActionV1,
        viewers: Vec<CorpusFileViewerV1>,
        revisions: Vec<CorpusFileRevisionV1>,
    }

    #[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
    #[serde(deny_unknown_fields)]
    struct CorpusFileGenesisV1 {
        room_id: RoomId,
        room_seed: RoomSeedV1,
        created_at: CreationRecordedAt,
        configuration: CanonicalJsonV1,
        memberships: Vec<CorpusFileMembershipV1>,
    }

    #[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
    #[serde(deny_unknown_fields)]
    struct CorpusFileMembershipV1 {
        member_id: MemberId,
        principal_id: PrincipalId,
        principal_kind: PrincipalKindV1,
        standing: MembershipStandingV1,
        access_mode: AccessModeV1,
        role: Option<String>,
    }

    #[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
    #[serde(deny_unknown_fields)]
    struct CorpusFileActionV1 {
        member_id: MemberId,
        action_id: ActionId,
        action_type: String,
        payload_schema_digest: Blake3DigestV1,
        canonical_payload: CanonicalJsonV1,
        admitted_at: ActionAdmittedAt,
    }

    #[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
    #[serde(deny_unknown_fields)]
    struct CorpusFileViewerV1 {
        kind: String,
        member_id: MemberId,
        available_after_action: u32,
        denied_before_detail: Option<String>,
    }

    #[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
    #[serde(deny_unknown_fields)]
    struct CorpusFileRevisionV1 {
        explanatory_version: String,
        revision_digest: PackDigestV1,
        descriptor_digest: Blake3DigestV1,
        executor_artifact_digest: Blake3DigestV1,
        schema_bundle_digest: Blake3DigestV1,
        codec_bundle_digest: Blake3DigestV1,
        selectable_for_new_rooms: bool,
        runnable_for_retained_rooms: bool,
        golden_corpus_digest: Blake3DigestV1,
        transcript_digest: Blake3DigestV1,
    }

    fn viewer_kind_name(kind: PackGoldenViewerKindV1) -> &'static str {
        match kind {
            PackGoldenViewerKindV1::Public => "public",
            PackGoldenViewerKindV1::Participant => "participant",
            PackGoldenViewerKindV1::Operator => "operator",
            PackGoldenViewerKindV1::Historical => "historical",
            PackGoldenViewerKindV1::FinalReveal => "final_reveal",
        }
    }

    #[allow(clippy::too_many_lines)]
    fn verify_checked_in_retained_corpus(
        registry: &PackRegistryV1,
        bytes: &str,
    ) -> Result<(), String> {
        let canonical_input = bytes
            .as_bytes()
            .strip_suffix(b"\n")
            .unwrap_or(bytes.as_bytes());
        let parsed_corpus = CanonicalJsonV1::parse(canonical_input)
            .map_err(|error| format!("retained corpus is not canonical JSON: {error}"))?;
        let canonical_bytes = parsed_corpus
            .to_bytes()
            .map_err(|error| format!("retained corpus cannot be canonicalized: {error}"))?;
        if canonical_bytes != canonical_input {
            let differing_byte = canonical_bytes
                .iter()
                .zip(canonical_input)
                .position(|(expected, actual)| expected != actual)
                .unwrap_or_else(|| canonical_bytes.len().min(canonical_input.len()));
            return Err(format!(
                "retained corpus is not canonical JSON at byte {differing_byte}"
            ));
        }
        let file: CorpusFileV1 = serde_json::from_str(bytes)
            .map_err(|error| format!("retained corpus is not the expected typed shape: {error}"))?;
        if file.schema != "worldstream/agent-heist-retained-corpus/v1"
            || file.canonical_encoding != "worldstream canonical JSON v1"
            || file.purpose
                != "Cross-platform identity and behavior vectors for exact retained Agent Heist lookup."
        {
            return Err("retained corpus header identity differs".to_owned());
        }
        if file.revisions.len() != 2 {
            return Err("retained corpus must contain exactly two revision rows".to_owned());
        }

        let expected_revisions = [
            (
                AGENT_HEIST_RETAINED_VERSION,
                false,
                agent_heist_legacy_revision(),
                LEGACY_TRANSCRIPT_DIGEST,
            ),
            (
                AGENT_HEIST_VERSION,
                true,
                agent_heist_revision(),
                TRANSCRIPT_DIGEST,
            ),
        ];
        for (version, selectable, revision, transcript_digest) in expected_revisions {
            let (descriptor, lock, schemas, codecs, artifact) = revision;
            let row = file
                .revisions
                .iter()
                .find(|row| row.explanatory_version == version)
                .ok_or_else(|| format!("retained corpus is missing revision row {version}"))?;
            let retained = registry
                .load_retained(&row.revision_digest)
                .map_err(|error| format!("retained corpus revision lookup failed: {error}"))?;
            if row.revision_digest != descriptor.revision_digest
                || retained.descriptor().revision_digest != descriptor.revision_digest
                || row.descriptor_digest != lock.descriptor_digest
                || row.executor_artifact_digest != *artifact
                || row.schema_bundle_digest != lock.schema_bundle_digest
                || row.codec_bundle_digest != lock.codec_bundle_digest
                || row.selectable_for_new_rooms != selectable
                || row.runnable_for_retained_rooms != retained.status().runnable_for_retained_rooms
                || retained.status().selectable_for_new_rooms != selectable
            {
                return Err(format!(
                    "retained corpus revision identity drifted for {version}"
                ));
            }
            let corpus = golden_corpus(descriptor.revision_digest.clone(), transcript_digest);
            if row.golden_corpus_digest != corpus.digest().map_err(|error| error.to_string())?
                || row.transcript_digest != corpus.expected_transcript_digest
            {
                return Err(format!(
                    "retained corpus behavior identity drifted for {version}"
                ));
            }

            if file.genesis.room_id != corpus.genesis.room_id {
                return Err(format!("retained corpus Room ID drifted for {version}"));
            }
            if file.genesis.room_seed != corpus.genesis.room_seed {
                return Err(format!("retained corpus Room seed drifted for {version}"));
            }
            if file.genesis.created_at != corpus.genesis.created_at {
                return Err(format!(
                    "retained corpus creation time drifted for {version}"
                ));
            }
            if file.genesis.configuration != corpus.genesis.configuration {
                return Err(format!(
                    "retained corpus configuration drifted for {version}"
                ));
            }
            let expected_memberships = corpus
                .genesis
                .initial_core_state
                .memberships()
                .values()
                .map(|membership| CorpusFileMembershipV1 {
                    member_id: membership.member_id().clone(),
                    principal_id: membership.principal_id().clone(),
                    principal_kind: membership.principal_kind(),
                    standing: membership.standing(),
                    access_mode: membership.access_mode(),
                    role: membership.role().map(str::to_owned),
                })
                .collect::<Vec<_>>();
            if file.genesis.memberships != expected_memberships {
                return Err(format!(
                    "retained corpus Membership vector drifted for {version}"
                ));
            }
            let expected_action = corpus
                .actions
                .first()
                .ok_or_else(|| format!("retained corpus has no Action for {version}"))?;
            let actual_action = CorpusFileActionV1 {
                member_id: expected_action.member_id.clone(),
                action_id: expected_action.action_id.clone(),
                action_type: expected_action.action_type.clone(),
                payload_schema_digest: expected_action.payload_schema_digest.clone(),
                canonical_payload: expected_action.canonical_payload.clone(),
                admitted_at: expected_action.admitted_at.clone(),
            };
            if file.action != actual_action {
                return Err(format!(
                    "retained corpus Action vector drifted for {version}"
                ));
            }
            let expected_viewers = corpus
                .viewers
                .iter()
                .map(|viewer| CorpusFileViewerV1 {
                    kind: viewer_kind_name(viewer.kind).to_owned(),
                    member_id: viewer.member_id.clone(),
                    available_after_action: viewer.available_after_action,
                    denied_before_detail: viewer.denied_before_detail.clone(),
                })
                .collect::<Vec<_>>();
            if file.viewers != expected_viewers {
                return Err(format!(
                    "retained corpus viewer vector drifted for {version}"
                ));
            }
            let _ = (schemas, codecs);
        }
        Ok(())
    }

    #[test]
    fn checked_in_retained_corpus_matches_production_registry_values() {
        let registry = builtin_agent_heist_registry()
            .unwrap_or_else(|error| unreachable!("Agent Heist registry: {error}"));
        let result = verify_checked_in_retained_corpus(&registry, CHECKED_IN_RETAINED_CORPUS);
        assert!(result.is_ok(), "checked-in retained corpus: {result:?}");
    }

    #[test]
    fn checked_in_retained_corpus_rejects_tampered_vector() {
        let registry = builtin_agent_heist_registry()
            .unwrap_or_else(|error| unreachable!("Agent Heist registry: {error}"));
        let tampered = CHECKED_IN_RETAINED_CORPUS.replacen("inspect_clue", "tampered_clue", 1);
        assert!(verify_checked_in_retained_corpus(&registry, &tampered).is_err());
    }

    #[test]
    fn checked_in_retained_corpus_rejects_missing_revision_row() {
        let registry = builtin_agent_heist_registry()
            .unwrap_or_else(|error| unreachable!("Agent Heist registry: {error}"));
        let mut value: serde_json::Value = serde_json::from_str(CHECKED_IN_RETAINED_CORPUS)
            .unwrap_or_else(|error| unreachable!("checked-in retained corpus JSON: {error}"));
        value["revisions"]
            .as_array_mut()
            .unwrap_or_else(|| unreachable!("retained corpus revisions array"))
            .pop();
        let missing = serde_json::to_string(&value)
            .unwrap_or_else(|error| unreachable!("tampered retained corpus JSON: {error}"));
        assert!(verify_checked_in_retained_corpus(&registry, &missing).is_err());
    }

    #[test]
    fn registry_golden_transcript_is_fixed() {
        let (descriptor, lock, schemas, codecs, artifact) = agent_heist_revision();
        let corpus = golden_corpus(descriptor.revision_digest.clone(), TRANSCRIPT_DIGEST);
        assert_eq!(corpus.digest(), Ok(parsed(CORPUS_DIGEST)));
        let digest = author_golden_transcript_digest_for_test(
            lock.clone(),
            descriptor,
            schemas.clone(),
            codecs.clone(),
            artifact.clone(),
            &corpus,
            crate::agent_heist::AgentHeistV1,
        );
        assert_eq!(digest, Ok(parsed(TRANSCRIPT_DIGEST)));
        assert!(builtin_agent_heist_registry().is_ok());
    }

    #[test]
    fn legacy_registry_golden_transcript_is_fixed() {
        let (descriptor, lock, schemas, codecs, artifact) = agent_heist_legacy_revision();
        let corpus = golden_corpus(descriptor.revision_digest.clone(), LEGACY_TRANSCRIPT_DIGEST);
        assert_eq!(corpus.digest(), Ok(parsed(LEGACY_CORPUS_DIGEST)));
        let digest = author_golden_transcript_digest_for_test(
            lock.clone(),
            descriptor,
            schemas.clone(),
            codecs.clone(),
            artifact.clone(),
            &corpus,
            crate::agent_heist::AgentHeistV0,
        )
        .unwrap_or_else(|()| unreachable!("Heist legacy transcript"));
        assert_eq!(digest.to_string(), LEGACY_TRANSCRIPT_DIGEST);
        assert!(builtin_agent_heist_registry().is_ok());
    }

    #[test]
    fn production_registry_replay_folds_genesis_states_events_timers_heads_and_hashes() {
        let registry = builtin_agent_heist_registry()
            .unwrap_or_else(|error| unreachable!("Agent Heist registry: {error}"));
        let digest = agent_heist_digest();
        let corpus = golden_corpus(digest.clone(), TRANSCRIPT_DIGEST);
        let prepared = registry
            .prepare_genesis_for_new_room(&corpus.genesis)
            .unwrap_or_else(|error| unreachable!("Agent Heist production Genesis: {error}"));
        let mut trace = CoreTraceV1::create_uncommitted(prepared)
            .unwrap_or_else(|error| unreachable!("Agent Heist production trace: {error}"));
        let genesis_bytes = trace
            .genesis_bytes()
            .unwrap_or_else(|error| unreachable!("Agent Heist Genesis bytes: {error}"));
        assert_eq!(trace.genesis().initial_timers().len(), 1);

        let golden_action = corpus
            .actions
            .first()
            .unwrap_or_else(|| unreachable!("Agent Heist golden Action"));
        let action = ParticipantActionV1 {
            member_id: golden_action.member_id.clone(),
            action_id: golden_action.action_id.clone(),
            action_type: golden_action.action_type.clone(),
            payload_schema_digest: golden_action.payload_schema_digest.clone(),
            canonical_payload: golden_action.canonical_payload.clone(),
            exact_basis_head: trace.head().clone(),
            admitted_at: golden_action.admitted_at.clone(),
        };
        trace
            .advance(RecordedStimulusV1::ParticipantAction(action))
            .unwrap_or_else(|error| unreachable!("Agent Heist golden Action: {error}"));

        let scheduled = trace
            .scheduled_timers()
            .values()
            .next()
            .cloned()
            .unwrap_or_else(|| unreachable!("Agent Heist initial phase Timer"));
        trace
            .advance(RecordedStimulusV1::TimerFired(TimerFiredV1 {
                timer_id: scheduled.timer_id.clone(),
                generation: scheduled.generation,
                scheduled_for: scheduled.scheduled_for.clone(),
                canonical_payload: scheduled.canonical_payload.clone(),
            }))
            .unwrap_or_else(|error| unreachable!("Agent Heist phase Timer: {error}"));

        let transition_bytes = trace
            .transition_bytes()
            .unwrap_or_else(|error| unreachable!("Agent Heist Transition bytes: {error}"));
        assert_eq!(transition_bytes.len(), 2);
        assert_eq!(trace.transitions()[0].ordered_domain_events().len(), 1);
        assert!(!trace.transitions()[1].ordered_domain_events().is_empty());
        assert!(!trace.transitions()[1].ordered_timer_changes().is_empty());

        let replay = CoreTraceV1::replay(&registry, &genesis_bytes, &transition_bytes)
            .unwrap_or_else(|error| unreachable!("Agent Heist production Replay: {error:?}"));
        assert_eq!(replay.steps.len(), 3);
        assert_eq!(replay.activity_callback_count, 2);
        assert_eq!(replay.final_head, *trace.head());
        assert_eq!(replay.final_state().head(), trace.head());
        assert_eq!(replay.final_state().core_state(), trace.core_state());
        assert_eq!(
            replay.final_state().activity_state(),
            trace.activity_state()
        );
        assert_eq!(
            replay.steps[0].canonical_lineage_record_bytes,
            genesis_bytes
        );
        assert_eq!(replay.steps[0].head, trace.genesis().complete_head());
        assert_eq!(
            replay.steps[1].canonical_lineage_record_bytes,
            transition_bytes[0]
        );
        assert_eq!(
            replay.steps[2].canonical_lineage_record_bytes,
            transition_bytes[1]
        );
        assert_eq!(
            replay.steps[2].canonical_core_bytes,
            trace
                .core_state()
                .canonical_bytes()
                .unwrap_or_else(|error| unreachable!("replayed Core bytes: {error}"))
        );
        assert_eq!(
            replay.steps[2].canonical_activity_bytes,
            trace
                .activity_state()
                .to_bytes()
                .unwrap_or_else(|error| unreachable!("replayed Activity bytes: {error}"))
        );

        let replay_trace = replay.into_trace();
        assert_eq!(
            replay_trace
                .genesis_bytes()
                .unwrap_or_else(|error| unreachable!("replayed Genesis: {error}")),
            genesis_bytes
        );
        assert_eq!(replay_trace.transitions(), trace.transitions());
        assert_eq!(replay_trace.scheduled_timers(), trace.scheduled_timers());
        assert_eq!(replay_trace.head(), trace.head());
    }

    #[test]
    fn retained_legacy_revision_replays_by_exact_digest_and_is_not_selectable() {
        let registry = builtin_agent_heist_registry()
            .unwrap_or_else(|error| unreachable!("Agent Heist registry: {error}"));
        let digest = agent_heist_retained_digest();
        assert_ne!(digest, agent_heist_digest());
        assert!(
            registry
                .select_for_new_room(&digest)
                .is_err_and(|error| matches!(error, PackRegistryErrorV1::NotSelectable(_)))
        );

        let corpus = golden_corpus(digest.clone(), LEGACY_TRANSCRIPT_DIGEST);
        let prepared = registry
            .prepare_genesis_for_retained_room(&corpus.genesis)
            .unwrap_or_else(|error| unreachable!("Agent Heist retained Genesis: {error}"));
        assert_eq!(
            prepared.retained_pack().descriptor().revision_digest,
            digest
        );
        let mut trace = CoreTraceV1::create_uncommitted_retained_for_test(&prepared)
            .unwrap_or_else(|error| unreachable!("Agent Heist retained trace: {error}"));
        let golden_action = corpus
            .actions
            .first()
            .unwrap_or_else(|| unreachable!("Agent Heist retained Action"));
        trace
            .advance(RecordedStimulusV1::ParticipantAction(ParticipantActionV1 {
                member_id: golden_action.member_id.clone(),
                action_id: golden_action.action_id.clone(),
                action_type: golden_action.action_type.clone(),
                payload_schema_digest: golden_action.payload_schema_digest.clone(),
                canonical_payload: golden_action.canonical_payload.clone(),
                exact_basis_head: trace.head().clone(),
                admitted_at: golden_action.admitted_at.clone(),
            }))
            .unwrap_or_else(|error| unreachable!("Agent Heist retained Action: {error}"));
        let scheduled = trace
            .scheduled_timers()
            .values()
            .next()
            .cloned()
            .unwrap_or_else(|| unreachable!("Agent Heist retained phase Timer"));
        trace
            .advance(RecordedStimulusV1::TimerFired(TimerFiredV1 {
                timer_id: scheduled.timer_id.clone(),
                generation: scheduled.generation,
                scheduled_for: scheduled.scheduled_for.clone(),
                canonical_payload: scheduled.canonical_payload.clone(),
            }))
            .unwrap_or_else(|error| unreachable!("Agent Heist retained phase Timer: {error}"));

        let genesis_bytes = trace
            .genesis_bytes()
            .unwrap_or_else(|error| unreachable!("Agent Heist retained Genesis bytes: {error}"));
        let transition_bytes = trace
            .transition_bytes()
            .unwrap_or_else(|error| unreachable!("Agent Heist retained Transition bytes: {error}"));
        let replay = CoreTraceV1::replay(&registry, &genesis_bytes, &transition_bytes)
            .unwrap_or_else(|error| unreachable!("Agent Heist retained Replay: {error:?}"));
        assert_eq!(replay.final_head, *trace.head());
        assert_eq!(
            replay.final_state().activity_state(),
            trace.activity_state()
        );

        let mut tampered_genesis = genesis_bytes.clone();
        let last = tampered_genesis
            .last_mut()
            .unwrap_or_else(|| unreachable!("nonempty Genesis bytes"));
        *last = if *last == b'}' { b' ' } else { b'}' };
        assert!(CoreTraceV1::replay(&registry, &tampered_genesis, &transition_bytes).is_err());

        let missing = parsed::<PackDigestV1>(
            "blake3:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
        );
        let mut missing_request = corpus.genesis.clone();
        missing_request.pack_digest = missing.clone();
        assert!(matches!(
            registry.prepare_genesis_for_retained_room(&missing_request),
            Err(crate::PackGenesisErrorV1::Registry(
                PackRegistryErrorV1::MissingRevision(found)
            )) if found == missing
        ));

        let mut tampered_lock = registry
            .load_retained(&digest)
            .unwrap_or_else(|error| unreachable!("Agent Heist retained lookup: {error}"))
            .revision_lock()
            .canonical_bytes()
            .unwrap_or_else(|error| unreachable!("Agent Heist retained lock bytes: {error}"));
        let byte = tampered_lock
            .iter_mut()
            .find(|byte| **byte == b'0')
            .unwrap_or_else(|| unreachable!("Heist lock contains a mutable byte"));
        *byte = b'1';
        assert!(PackRevisionLockV1::from_canonical_bytes(&tampered_lock, &digest).is_err());
    }

    #[test]
    fn outcome_matrix_covers_majority_and_score_boundaries() {
        let correct =
            |role: &str, resource: bool| (role.to_owned(), ("correct".to_owned(), resource));
        let wrong = |role: &str| (role.to_owned(), ("wrong".to_owned(), false));

        assert_eq!(
            crate::agent_heist::outcome_for_matrix(&BTreeMap::new(), None),
            ("failure".to_owned(), 0)
        );
        assert_eq!(
            crate::agent_heist::outcome_for_matrix(
                &BTreeMap::from([correct("navigator", true)]),
                None
            ),
            ("failure".to_owned(), 0)
        );
        assert_eq!(
            crate::agent_heist::outcome_for_matrix(
                &BTreeMap::from([correct("navigator", true), correct("insider", true)]),
                None,
            ),
            ("success".to_owned(), 5)
        );
        assert_eq!(
            crate::agent_heist::outcome_for_matrix(
                &BTreeMap::from([correct("navigator", true), wrong("insider")]),
                None,
            ),
            ("failure".to_owned(), 0)
        );
        assert_eq!(
            crate::agent_heist::outcome_for_matrix(
                &BTreeMap::from([
                    correct("navigator", true),
                    correct("insider", true),
                    correct("broker", true),
                ]),
                None,
            ),
            ("success".to_owned(), 5)
        );
        assert_eq!(
            crate::agent_heist::outcome_for_matrix(
                &BTreeMap::from([
                    correct("navigator", false),
                    correct("insider", false),
                    wrong("broker"),
                ]),
                None,
            ),
            ("partial_failure".to_owned(), 4)
        );
        assert_eq!(
            crate::agent_heist::outcome_for_matrix(
                &BTreeMap::from([
                    correct("navigator", true),
                    wrong("insider"),
                    ("broker".to_owned(), ("another".to_owned(), false)),
                ]),
                None,
            ),
            ("failure".to_owned(), 0)
        );
    }
}
