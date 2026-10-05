//! Writes additive V2 vectors from independently assembled JSON preimages.
//! The retained V1 fixture is read only and supplies shared logical facts.
use std::{fs, path::PathBuf};

use anyhow::{Context, Result};
use serde_json::{Value, json};
use worldstream_core::{Blake3DigestV1, CanonicalJsonV1};

fn canonical(value: &Value) -> Result<String> {
    let source = serde_json::to_vec(value)?;
    let bytes = CanonicalJsonV1::parse(&source)?.to_bytes()?;
    Ok(String::from_utf8(bytes)?)
}

fn vector(value: &Value) -> Result<Value> {
    let bytes = canonical(value)?;
    Ok(
        json!({"canonical_bytes": bytes, "digest": Blake3DigestV1::hash(bytes.as_bytes()).to_string()}),
    )
}

fn preimage(record: &Value, domain: &str, fields: &[&str]) -> Value {
    let mut object = serde_json::Map::new();
    object.insert("domain".to_owned(), json!(domain));
    object.insert(
        "core_schema".to_owned(),
        record["core_schema_version"].clone(),
    );
    for field in fields {
        object.insert((*field).to_owned(), record[*field].clone());
    }
    Value::Object(object)
}

fn main() -> Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let legacy: Value = serde_json::from_str(&fs::read_to_string(
        root.join("tests/fixtures/core_v1_golden.json"),
    )?)?;
    let mut genesis: Value = serde_json::from_str(
        legacy["genesis_record_bytes"]
            .as_str()
            .context("Genesis bytes")?,
    )?;
    genesis["genesis_version"] = json!("worldstream/genesis/v2");
    genesis["codec_id"] = json!("worldstream/genesis-record/v2");
    genesis["hash_suite"] = json!("blake3-canonical-json-v2");
    genesis["transition_version"] = json!("worldstream/transition/v2");
    genesis["transition_codec_id"] = json!("worldstream/transition-record/v2");
    genesis["transition_hash_suite"] = json!("blake3-canonical-json-v2");
    genesis["payload_budget_id"] = json!("worldstream/payload-budget/v1");
    let genesis_preimage = preimage(
        &genesis,
        "worldstream/genesis/v2",
        &[
            "codec_id",
            "hash_suite",
            "room_id",
            "pack_digest",
            "configuration",
            "room_seed",
            "created_at",
            "initial_timers",
            "initial_core_state_hash",
            "initial_activity_state_hash",
            "initial_authoritative_state_hash",
            "transition_version",
            "transition_codec_id",
            "transition_hash_suite",
            "payload_budget_id",
        ],
    );
    let genesis_vector = vector(&genesis_preimage)?;
    genesis["genesis_hash"] = genesis_vector["digest"].clone();
    let mut transition: Value = serde_json::from_str(
        legacy["transition_record_bytes"]
            .as_str()
            .context("Transition bytes")?,
    )?;
    transition["transition_version"] = json!("worldstream/transition/v2");
    transition["codec_id"] = json!("worldstream/transition-record/v2");
    transition["hash_suite"] = json!("blake3-canonical-json-v2");
    transition["previous_transition_or_genesis_hash"] = genesis["genesis_hash"].clone();
    let object = transition.as_object_mut().context("Transition object")?;
    object.remove("resulting_core_state");
    object.remove("resulting_activity_state");
    let transition_preimage = preimage(
        &transition,
        "worldstream/transition/v2",
        &[
            "codec_id",
            "hash_suite",
            "room_id",
            "room_seq",
            "pack_digest",
            "previous_transition_or_genesis_hash",
            "recorded_stimulus",
            "ordered_domain_events",
            "ordered_timer_changes",
            "ordered_attention_signals",
            "resulting_core_state_hash",
            "resulting_activity_state_hash",
            "resulting_authoritative_state_hash",
        ],
    );
    let transition_vector = vector(&transition_preimage)?;
    transition["transition_hash"] = transition_vector["digest"].clone();
    let memberships = genesis["initial_core_state"]["memberships"]
        .as_object()
        .context("Membership map")?;
    let proposals: Vec<Value> = memberships
        .values()
        .map(|membership| {
            let mut proposal = membership.clone();
            if let Some(object) = proposal.as_object_mut() {
                object.remove("member_id");
            }
            proposal
        })
        .collect();
    let legacy_request = json!({"pack_digest":genesis["pack_digest"], "configuration":genesis["configuration"],
        "ordered_initial_memberships":proposals});
    let mut v1_preimage = legacy_request.clone();
    v1_preimage["domain"] = json!("worldstream/create-room-request/v1");
    let mut compact_request = legacy_request.clone();
    compact_request["canonical_history_format"] = json!("worldstream/transition/v2");
    let mut v2_preimage = compact_request.clone();
    v2_preimage["domain"] = json!("worldstream/create-room-request/v2");
    v2_preimage["payload_budget_id"] = json!("worldstream/payload-budget/v1");
    let corpus = json!({
        "schema":"worldstream/core-v2-golden-corpus/v1",
        "genesis_record_bytes":canonical(&genesis)?,
        "transition_record_bytes":canonical(&transition)?,
        "legacy_creation_wire_bytes":canonical(&legacy_request)?,
        "v2_creation_wire_bytes":canonical(&compact_request)?,
        "hash_vectors":{"genesis":genesis_vector,"transition":transition_vector,
            "creation_v1":vector(&v1_preimage)?,"creation_v2":vector(&v2_preimage)?},
    });
    fs::write(
        root.join("tests/fixtures/core_v2_golden.json"),
        format!("{}\n", serde_json::to_string_pretty(&corpus)?),
    )?;
    Ok(())
}
