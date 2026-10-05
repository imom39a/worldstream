use anyhow::{Context, Result};
use serde_json::{Value, json};
use worldstream_core::{
    Blake3DigestV1, CanonicalHistoryFormat, CanonicalJsonV1, GenesisInputV1, GenesisRecord,
    GenesisV1, GenesisV2, LineageCodecError, PAYLOAD_BUDGET_V1, PAYLOAD_BUDGET_V1_ID,
    RoomCreationRequestWithFormat, TransitionRecord, TransitionV1, TransitionV2, TransitionV2Input,
};

fn corpus() -> Result<Value> {
    Ok(serde_json::from_str(include_str!(
        "../../../tests/fixtures/core_v2_golden.json"
    ))?)
}

fn legacy_corpus() -> Result<Value> {
    Ok(serde_json::from_str(include_str!(
        "../../../tests/fixtures/core_v1_golden.json"
    ))?)
}

fn bytes<'a>(value: &'a Value, key: &str) -> Result<&'a [u8]> {
    Ok(value[key]
        .as_str()
        .context("fixture byte string")?
        .as_bytes())
}

fn canonical(value: &Value) -> Result<Vec<u8>> {
    Ok(CanonicalJsonV1::parse(&serde_json::to_vec(value)?)?.to_bytes()?)
}

fn records() -> Result<(GenesisV2, TransitionV2)> {
    let legacy = legacy_corpus()?;
    let genesis = GenesisV1::from_canonical_bytes(bytes(&legacy, "genesis_record_bytes")?)?;
    let source = TransitionV1::from_canonical_bytes(bytes(&legacy, "transition_record_bytes")?)?;
    let compact = GenesisV2::new(
        GenesisInputV1::new(
            genesis.room_id().clone(),
            genesis.pack_digest().clone(),
            genesis.configuration().clone(),
            genesis.room_seed().clone(),
            genesis.created_at().clone(),
            genesis.initial_core_state().clone(),
            genesis.initial_activity_state().clone(),
        )
        .with_initial_timers(genesis.initial_timers().to_vec()),
    )?;
    let transition = TransitionV2::new(TransitionV2Input {
        room_id: compact.room_id().clone(),
        room_seq: source.room_seq(),
        pack_digest: compact.pack_digest().clone(),
        previous_lineage_hash: compact.genesis_hash().clone(),
        recorded_stimulus: source.recorded_stimulus().clone(),
        ordered_domain_events: source.ordered_domain_events().to_vec(),
        ordered_timer_changes: source.ordered_timer_changes().to_vec(),
        ordered_attention_signals: source.ordered_attention_signals().to_vec(),
        resulting_core_state: source.resulting_core_state(),
        resulting_activity_state: source.resulting_activity_state(),
    })?;
    Ok((compact, transition))
}

#[test]
fn lineage_v2_golden_records_and_independent_hash_preimages() -> Result<()> {
    let fixture = corpus()?;
    let (genesis, transition) = records()?;
    assert_eq!(
        genesis.canonical_bytes()?,
        bytes(&fixture, "genesis_record_bytes")?
    );
    assert_eq!(
        transition.canonical_bytes()?,
        bytes(&fixture, "transition_record_bytes")?
    );
    for (name, preimage, hash) in [
        (
            "genesis",
            genesis.hash_preimage_bytes()?,
            genesis.genesis_hash(),
        ),
        (
            "transition",
            transition.hash_preimage_bytes()?,
            transition.transition_hash(),
        ),
    ] {
        let expected = &fixture["hash_vectors"][name];
        assert_eq!(preimage, bytes(expected, "canonical_bytes")?);
        assert_eq!(hash.to_string(), expected["digest"]);
        assert_eq!(*hash, Blake3DigestV1::hash(&preimage));
        let input: Value = serde_json::from_slice(&preimage)?;
        assert!(input.get("genesis_hash").is_none());
        assert!(input.get("transition_hash").is_none());
        assert!(input.get("initial_core_state").is_none());
        assert!(input.get("initial_activity_state").is_none());
        assert!(input.get("resulting_core_state").is_none());
        assert!(input.get("resulting_activity_state").is_none());
    }
    let stored: Value = serde_json::from_slice(&transition.canonical_bytes()?)?;
    assert!(stored.get("resulting_core_state").is_none());
    assert!(stored.get("resulting_activity_state").is_none());
    assert_eq!(stored.as_object().context("record object")?.len(), 16);
    let stored_genesis: Value = serde_json::from_slice(&genesis.canonical_bytes()?)?;
    assert_eq!(stored_genesis["payload_budget_id"], PAYLOAD_BUDGET_V1_ID);
    assert_eq!(
        stored_genesis["transition_version"],
        "worldstream/transition/v2"
    );
    assert_eq!(
        stored_genesis["transition_codec_id"],
        "worldstream/transition-record/v2"
    );
    assert_eq!(
        stored_genesis["transition_hash_suite"],
        "blake3-canonical-json-v2"
    );
    let decoded = GenesisRecord::from_canonical_bytes(&genesis.canonical_bytes()?)?;
    assert_eq!(decoded.format(), CanonicalHistoryFormat::V2);
    assert_eq!(decoded.payload_budget_id(), Some(PAYLOAD_BUDGET_V1_ID));
    let decoded_transition = decoded.decode_transition(&transition.canonical_bytes()?)?;
    decoded_transition.verify_successor(&decoded.complete_head())?;
    assert_eq!(
        decoded_transition.canonical_bytes()?,
        transition.canonical_bytes()?
    );
    Ok(())
}

#[test]
fn lineage_v2_preserves_v1_records_hashes_and_shared_state_domains() -> Result<()> {
    let legacy = legacy_corpus()?;
    let genesis = GenesisRecord::from_canonical_bytes(bytes(&legacy, "genesis_record_bytes")?)?;
    let transition = genesis.decode_transition(bytes(&legacy, "transition_record_bytes")?)?;
    transition.verify_successor(&genesis.complete_head())?;
    assert_eq!(genesis.format(), CanonicalHistoryFormat::V1);
    assert_eq!(genesis.payload_budget_id(), None);
    assert_eq!(
        genesis.canonical_bytes()?,
        bytes(&legacy, "genesis_record_bytes")?
    );
    assert_eq!(
        transition.canonical_bytes()?,
        bytes(&legacy, "transition_record_bytes")?
    );
    let (compact_genesis, compact_transition) = records()?;
    assert_eq!(
        genesis.initial_core_state_hash(),
        compact_genesis.initial_core_state_hash()
    );
    assert_eq!(
        genesis.initial_activity_state_hash(),
        compact_genesis.initial_activity_state_hash()
    );
    assert_eq!(
        genesis.initial_authoritative_state_hash(),
        compact_genesis.initial_authoritative_state_hash()
    );
    assert_eq!(
        transition.resulting_core_state_hash(),
        compact_transition.resulting_core_state_hash()
    );
    assert_eq!(
        transition.resulting_activity_state_hash(),
        compact_transition.resulting_activity_state_hash()
    );
    assert_eq!(
        transition.resulting_authoritative_state_hash(),
        compact_transition.resulting_authoritative_state_hash()
    );
    assert_ne!(genesis.genesis_hash(), compact_genesis.genesis_hash());
    assert_ne!(
        transition.transition_hash(),
        compact_transition.transition_hash()
    );
    Ok(())
}

#[test]
fn lineage_v2_creation_defaults_and_request_hash_vectors() -> Result<()> {
    let fixture = corpus()?;
    let omitted = RoomCreationRequestWithFormat::from_canonical_bytes(bytes(
        &fixture,
        "legacy_creation_wire_bytes",
    )?)?;
    let mut explicit_value: Value =
        serde_json::from_slice(bytes(&fixture, "legacy_creation_wire_bytes")?)?;
    explicit_value["canonical_history_format"] = json!("worldstream/transition/v1");
    let explicit =
        RoomCreationRequestWithFormat::from_canonical_bytes(&canonical(&explicit_value)?)?;
    assert_eq!(omitted, explicit);
    assert_eq!(
        omitted.canonical_request_hash()?,
        omitted.legacy_request().canonical_request_hash()?
    );
    assert_eq!(
        explicit.canonical_bytes()?,
        bytes(&fixture, "legacy_creation_wire_bytes")?
    );
    let compact = RoomCreationRequestWithFormat::from_canonical_bytes(bytes(
        &fixture,
        "v2_creation_wire_bytes",
    )?)?;
    assert_eq!(compact.payload_budget_id(), Some(PAYLOAD_BUDGET_V1_ID));
    assert_eq!(compact.format(), CanonicalHistoryFormat::V2);
    assert_ne!(
        compact.canonical_request_hash()?,
        omitted.canonical_request_hash()?
    );
    assert_eq!(
        compact.canonical_bytes()?,
        bytes(&fixture, "v2_creation_wire_bytes")?
    );
    for (name, request) in [("creation_v1", omitted), ("creation_v2", compact)] {
        let expected = &fixture["hash_vectors"][name];
        assert_eq!(
            request.hash_preimage_bytes()?,
            bytes(expected, "canonical_bytes")?
        );
        assert_eq!(
            request.canonical_request_hash()?.to_string(),
            expected["digest"]
        );
        let input: Value = serde_json::from_slice(&request.hash_preimage_bytes()?)?;
        for absent in [
            "room_id",
            "member_id",
            "room_seed",
            "created_at",
            "commit_time",
        ] {
            assert!(input.get(absent).is_none());
        }
    }
    explicit_value["canonical_history_format"] = json!("worldstream/transition/v3");
    assert!(
        RoomCreationRequestWithFormat::from_canonical_bytes(&canonical(&explicit_value)?).is_err()
    );
    explicit_value["canonical_history_format"] = Value::Null;
    assert!(
        RoomCreationRequestWithFormat::from_canonical_bytes(&canonical(&explicit_value)?).is_err()
    );
    Ok(())
}

#[test]
fn lineage_v2_rejects_unknown_and_mixed_tuples() -> Result<()> {
    let fixture = corpus()?;
    let legacy = legacy_corpus()?;
    let genesis = GenesisRecord::from_canonical_bytes(bytes(&fixture, "genesis_record_bytes")?)?;
    let old_genesis = GenesisRecord::from_canonical_bytes(bytes(&legacy, "genesis_record_bytes")?)?;
    assert!(matches!(
        genesis.decode_transition(bytes(&legacy, "transition_record_bytes")?),
        Err(LineageCodecError::MixedFormat)
    ));
    assert!(matches!(
        old_genesis.decode_transition(bytes(&fixture, "transition_record_bytes")?),
        Err(LineageCodecError::MixedFormat)
    ));
    for field in [
        "genesis_version",
        "codec_id",
        "hash_suite",
        "core_schema_version",
        "transition_version",
        "transition_codec_id",
        "transition_hash_suite",
        "payload_budget_id",
    ] {
        let mut value: Value = serde_json::from_slice(bytes(&fixture, "genesis_record_bytes")?)?;
        value[field] = json!("unknown");
        assert!(
            matches!(
                GenesisRecord::from_canonical_bytes(&canonical(&value)?),
                Err(LineageCodecError::UnsupportedIdentity)
            ),
            "{field}"
        );
    }
    for field in [
        "transition_version",
        "codec_id",
        "hash_suite",
        "core_schema_version",
    ] {
        let mut value: Value = serde_json::from_slice(bytes(&fixture, "transition_record_bytes")?)?;
        value[field] = json!("unknown");
        assert!(
            matches!(
                genesis.decode_transition(&canonical(&value)?),
                Err(LineageCodecError::UnsupportedIdentity)
            ),
            "{field}"
        );
    }
    Ok(())
}

#[test]
fn lineage_v2_detects_commitment_tampering_and_selector_hash_binding() -> Result<()> {
    let fixture = corpus()?;
    for (key, fields) in [
        (
            "genesis_record_bytes",
            vec![
                "configuration",
                "initial_core_state",
                "initial_activity_state",
                "initial_core_state_hash",
                "initial_activity_state_hash",
                "initial_authoritative_state_hash",
                "genesis_hash",
            ],
        ),
        (
            "transition_record_bytes",
            vec![
                "recorded_stimulus",
                "ordered_domain_events",
                "ordered_timer_changes",
                "ordered_attention_signals",
                "resulting_core_state_hash",
                "resulting_activity_state_hash",
                "resulting_authoritative_state_hash",
                "transition_hash",
            ],
        ),
    ] {
        for field in fields {
            let mut value: Value = serde_json::from_slice(bytes(&fixture, key)?)?;
            if field.ends_with("hash") {
                value[field] = json!(Blake3DigestV1::hash(b"altered").to_string());
            } else if field.starts_with("ordered_") {
                value[field] = json!([{"altered":true}]);
            } else {
                value[field] = json!({"altered":true});
            }
            let encoded = canonical(&value)?;
            let rejected = if key == "genesis_record_bytes" {
                GenesisRecord::from_canonical_bytes(&encoded).is_err()
            } else {
                TransitionRecord::from_canonical_bytes(CanonicalHistoryFormat::V2, &encoded)
                    .is_err()
            };
            assert!(rejected, "{field}");
        }
    }
    let original: Value = serde_json::from_slice(bytes(
        &fixture["hash_vectors"]["genesis"],
        "canonical_bytes",
    )?)?;
    let original_hash = Blake3DigestV1::hash(&canonical(&original)?);
    for field in [
        "payload_budget_id",
        "transition_version",
        "transition_codec_id",
        "transition_hash_suite",
    ] {
        let mut changed = original.clone();
        changed[field] = json!("altered");
        assert_ne!(
            Blake3DigestV1::hash(&canonical(&changed)?),
            original_hash,
            "{field}"
        );
        let mut record: Value = serde_json::from_slice(bytes(&fixture, "genesis_record_bytes")?)?;
        record[field] = changed[field].clone();
        record["genesis_hash"] = json!(Blake3DigestV1::hash(&canonical(&changed)?).to_string());
        assert!(GenesisRecord::from_canonical_bytes(&canonical(&record)?).is_err());
    }
    let mut changed_request: Value = serde_json::from_slice(bytes(
        &fixture["hash_vectors"]["creation_v2"],
        "canonical_bytes",
    )?)?;
    let original = Blake3DigestV1::hash(&canonical(&changed_request)?);
    changed_request["payload_budget_id"] = json!("worldstream/payload-budget/v2");
    assert_ne!(
        Blake3DigestV1::hash(&canonical(&changed_request)?),
        original
    );
    Ok(())
}

#[test]
fn lineage_v2_successor_checks_all_head_identity_fields() -> Result<()> {
    let (genesis, transition) = records()?;
    let head = genesis.complete_head();
    let mut fields: Value = serde_json::from_slice(&head.canonical_bytes()?)?;
    for field in [
        "room_id",
        "pack_digest",
        "core_schema_version",
        "room_seq",
        "genesis_or_transition_hash",
    ] {
        let original = fields[field].clone();
        fields[field] = match field {
            "room_id" => json!("01ARZ3NDEKTSV4RRFFQ69G5FAX"),
            "core_schema_version" => json!("wrong/schema"),
            "room_seq" => json!(1),
            _ => json!(Blake3DigestV1::hash(b"different").to_string()),
        };
        let altered = CanonicalJsonV1::decode_canonical(&canonical(&fields)?)?;
        assert!(
            TransitionRecord::V2(transition.clone())
                .verify_successor(&altered)
                .is_err(),
            "{field}"
        );
        fields[field] = original;
    }
    Ok(())
}

#[test]
fn lineage_v2_requires_strict_original_canonical_bytes() -> Result<()> {
    let fixture = corpus()?;
    for key in ["genesis_record_bytes", "transition_record_bytes"] {
        let original = bytes(&fixture, key)?;
        let value: Value = serde_json::from_slice(original)?;
        let mut unknown = value.clone();
        unknown["extra"] = Value::Null;
        let mut malformed = vec![canonical(&unknown)?, serde_json::to_vec_pretty(&value)?];
        let mut trailing = original.to_vec();
        trailing.push(b' ');
        malformed.push(trailing);
        let duplicate = format!(
            "{{\"codec_id\":\"duplicate\",{}",
            std::str::from_utf8(original)?
                .get(1..)
                .context("record contents")?
        );
        malformed.push(duplicate.into_bytes());
        let mut missing = value.clone();
        missing
            .as_object_mut()
            .context("object")?
            .remove("hash_suite");
        malformed.push(canonical(&missing)?);
        let mut unsafe_integer = value.clone();
        unsafe_integer["extra"] = json!(9_007_199_254_740_992_u64);
        malformed.push(serde_json::to_vec(&unsafe_integer)?);
        let mut float = value;
        float["extra"] = json!(1.0);
        malformed.push(serde_json::to_vec(&float)?);
        malformed.push(vec![0xff]);
        for bytes in malformed {
            let rejected = if key == "genesis_record_bytes" {
                GenesisRecord::from_canonical_bytes(&bytes).is_err()
            } else {
                TransitionRecord::from_canonical_bytes(CanonicalHistoryFormat::V2, &bytes).is_err()
            };
            assert!(rejected, "{key}");
        }
    }
    Ok(())
}

#[test]
fn lineage_v2_fixed_policy_keeps_distinct_body_and_record_ceilings() {
    assert_eq!(PAYLOAD_BUDGET_V1.creation_configuration, 32_768);
    assert_eq!(PAYLOAD_BUDGET_V1.action_payload, 32_768);
    assert_eq!(PAYLOAD_BUDGET_V1.external_input_payload, 32_768);
    assert_eq!(PAYLOAD_BUDGET_V1.domain_event_item, 8_192);
    assert_eq!(PAYLOAD_BUDGET_V1.domain_events_array, 262_144);
    assert_eq!(PAYLOAD_BUDGET_V1.effects, 393_216);
    assert_eq!(PAYLOAD_BUDGET_V1.transition, 524_288);
    assert_eq!(PAYLOAD_BUDGET_V1.genesis, 786_432);
    assert_eq!(PAYLOAD_BUDGET_V1.core_state, 131_072);
    assert_eq!(PAYLOAD_BUDGET_V1.activity_state, 262_144);
    assert_eq!(PAYLOAD_BUDGET_V1.authoritative_state, 524_288);
    assert_eq!(PAYLOAD_BUDGET_V1.observation, 32_768);
    assert_eq!(PAYLOAD_BUDGET_V1.projection, 262_144);
}

#[test]
fn lineage_v2_validates_initial_timer_and_room_facts() -> Result<()> {
    let fixture = corpus()?;
    let original: Value = serde_json::from_slice(bytes(&fixture, "genesis_record_bytes")?)?;
    let mut archived = original.clone();
    archived["initial_core_state"]["room_status"] = json!("archived");
    let mut generation = original.clone();
    generation["initial_timers"][0]["generation"] = json!(2);
    let mut equal_time = original.clone();
    equal_time["initial_timers"][0]["scheduled_for"] = original["created_at"].clone();
    let mut duplicate = original.clone();
    let timer = duplicate["initial_timers"][0].clone();
    duplicate["initial_timers"]
        .as_array_mut()
        .context("Timer array")?
        .push(timer);
    for invalid in [archived, generation, equal_time, duplicate] {
        assert!(matches!(
            GenesisRecord::from_canonical_bytes(&canonical(&invalid)?),
            Err(LineageCodecError::InvalidCoreOrGenesis)
        ));
    }
    let mut valid_stimulus_tamper: Value =
        serde_json::from_slice(bytes(&fixture, "transition_record_bytes")?)?;
    valid_stimulus_tamper["recorded_stimulus"]["reason_code"] = json!("operator.other_reason");
    assert!(matches!(
        TransitionRecord::from_canonical_bytes(
            CanonicalHistoryFormat::V2,
            &canonical(&valid_stimulus_tamper)?
        ),
        Err(LineageCodecError::CommitmentMismatch)
    ));
    Ok(())
}
