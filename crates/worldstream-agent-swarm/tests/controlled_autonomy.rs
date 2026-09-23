#![cfg(feature = "managed-local-runtime")]

use std::{
    io::Write as _,
    path::Path,
    process::{Command, Output, Stdio},
};

use serde_json::{Value, json};

fn invoke(root: &Path, instruction: &Value) -> Result<Output, Box<dyn std::error::Error>> {
    invoke_with_context(
        root,
        instruction,
        &json!({"goal": "Validate CSV parsing and formatting.", "work_items": []}),
    )
}

fn invoke_with_context(
    root: &Path,
    instruction: &Value,
    activity: &Value,
) -> Result<Output, Box<dyn std::error::Error>> {
    let mut child = Command::new(env!(
        "CARGO_BIN_EXE_worldstream-agent-swarm-controlled-worker"
    ))
    .args([
        "invoke",
        "--invocation-id",
        "autonomy-fixture",
        "--model",
        "controlled",
        "--behavior",
        "normal",
        "--new-session",
    ])
    .env_clear()
    .env("WORLDSTREAM_CONTROLLED_PROVIDER", "1")
    .current_dir(root)
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()?;
    let request = json!({
        "schema": if instruction["schema"] == "worldstream/agent-swarm-planning-instruction@1" {
            "worldstream/agent-swarm-planning-invocation@1"
        } else {
            "worldstream/agent-swarm-worker-invocation@2"
        },
        "authorized_activity": activity,
        "instruction": serde_json::to_string(instruction)?,
    });
    child
        .stdin
        .take()
        .ok_or("missing stdin")?
        .write_all(&serde_json::to_vec(&request)?)?;
    Ok(child.wait_with_output()?)
}

#[test]
fn planning_fixture_changes_its_decision_instruction_after_a_retained_worker_failure()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let activity = json!({"goal": "CONTROLLED_FAIL_ONCE: validate two independent CSV tasks.",
        "work_items": [{"work_id":"work-1","status":"claimed"}, {"work_id":"work-2","status":"completed"}]});
    let task = json!({
        "schema": "worldstream/agent-swarm-autonomy-instruction@1", "task": "contribute",
        "artifact_path": ".swarm-contributions/fixture/work-1.txt",
        "target": {"kind": "work_attempt", "work_id": "work-1", "work_revision": 2, "attempt_revision": 1},
        "instructions": "Complete this exact task.",
    });
    assert!(
        !invoke_with_context(root.path(), &task, &activity)?
            .status
            .success()
    );
    assert!(
        !invoke_with_context(root.path(), &task, &activity)?
            .status
            .success()
    );
    let planning = json!({
        "schema": "worldstream/agent-swarm-planning-instruction@1",
        "options": [{"target_id":"target-1", "member_key":"worker-1", "semantic_target":task["target"],
                     "allowed_action_types":["submit_contribution"], "instruction":serde_json::to_string(&task)?}],
        "recent_outcomes": [{"coordinator_outcome":{"outcome":"not_submitted","reason":"process_failed"}}],
    });
    let decision = proposal(invoke_with_context(root.path(), &planning, &activity)?)?;
    assert_eq!(
        decision["schema"],
        "worldstream/agent-swarm-planning-decision@1"
    );
    assert_eq!(decision["decision"]["kind"], "dispatch");
    let step = &decision["decision"]["steps"][0];
    assert_eq!(step["target_id"], "target-1");
    let revised: Value = serde_json::from_str(
        step["instruction"]
            .as_str()
            .ok_or("missing revised instruction")?,
    )?;
    assert_ne!(revised["instructions"], task["instructions"]);
    let dispatched = json!({
        "action_contract": serde_json::to_string(&task)?,
        "model_instruction": serde_json::to_string(&revised)?,
    });
    let result = proposal(invoke_with_context(root.path(), &dispatched, &activity)?)?;
    assert_eq!(result["action_type"], "submit_contribution");
    Ok(())
}

fn proposal(output: Output) -> Result<Value, Box<dyn std::error::Error>> {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout)?;
    let result: Value = serde_json::from_str(text.lines().last().ok_or("missing result")?)?;
    Ok(serde_json::from_str(
        result["text"].as_str().ok_or("missing proposal")?,
    )?)
}

#[test]
fn autonomous_fixture_proposes_goal_bound_independent_work_and_claims_exact_target()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let mut titles = Vec::new();
    for index in 1..=2 {
        let work_id = format!("work-{index}");
        let result = proposal(invoke(
            root.path(),
            &json!({
                "schema": "worldstream/agent-swarm-autonomy-instruction@1", "task": "propose",
                "proposal_index": index,
                "target": {"kind": "work_proposal", "goal_revision": 7, "work_id": work_id},
            }),
        )?)?;
        assert_eq!(result["action_type"], "propose_work_item");
        assert_eq!(result["payload"]["work_id"], work_id);
        assert_eq!(result["payload"]["expected_goal_revision"], 7);
        assert_eq!(result["payload"]["dependency_ids"], json!([]));
        assert!(
            result["payload"]["description"]
                .as_str()
                .ok_or("missing description")?
                .contains("CSV parsing and formatting")
        );
        titles.push(result["payload"]["title"].clone());
    }
    assert_ne!(titles[0], titles[1]);
    let result = proposal(invoke(
        root.path(),
        &json!({
            "schema": "worldstream/agent-swarm-autonomy-instruction@1", "task": "claim",
            "attempt_id": "exact-attempt", "target": {"kind": "work_claim", "work_revision": 3, "work_id": "work-1"},
        }),
    )?)?;
    assert_eq!(
        result["payload"],
        json!({"attempt_id": "exact-attempt", "work_id": "work-1", "expected_work_revision": 3})
    );
    Ok(())
}

#[test]
fn autonomous_fixture_emits_owned_contribution_and_rejects_target_or_path_escape()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let fixture = json!({
        "schema": "worldstream/agent-swarm-autonomy-instruction@1", "task": "contribute",
        "artifact_path": ".swarm-contributions/fixture/work-1.txt",
        "target": {"kind": "work_attempt", "work_id": "work-1", "work_revision": 2, "attempt_revision": 1},
    });
    let result = proposal(invoke(root.path(), &fixture)?)?;
    assert_eq!(result["action_type"], "submit_contribution");
    assert_eq!(result["payload"]["completes_work"], true);
    assert_eq!(result["payload"]["expected_attempt_revision"], 1);
    let text =
        std::fs::read_to_string(root.path().join(".swarm-contributions/fixture/work-1.txt"))?;
    assert!(text.contains("Goal: Validate CSV parsing and formatting."));
    assert!(text.contains("Work: work-1"));
    let mut escaping = fixture.clone();
    escaping["artifact_path"] = json!("../escaped.txt");
    assert!(!invoke(root.path(), &escaping)?.status.success());
    let mut mismatched = fixture;
    mismatched["target"]["kind"] = json!("work_proposal");
    assert!(!invoke(root.path(), &mismatched)?.status.success());
    Ok(())
}
