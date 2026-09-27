use std::{
    env,
    fs::{self, OpenOptions},
    io::{Read as _, Write as _},
    path::PathBuf,
    process::{Command, Stdio},
    thread,
    time::Duration,
};

use serde::Deserialize;
use serde_json::{Value, json};

/// Explicit canned output for integration benchmarks, never model execution.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlledArtifact {
    schema: String,
    proposal: Value,
    artifact_text: String,
    delay_ms: u64,
}

fn main() {
    let arguments = env::args().skip(1).collect::<Vec<_>>();
    let result = run(&arguments);
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(2);
    }
}

fn run(arguments: &[String]) -> Result<(), &'static str> {
    match arguments.first().map(String::as_str) {
        Some("--version") => {
            println!("worldstream-controlled-worker 1");
            Ok(())
        }
        Some("--help") | None => {
            println!(
                "worldstream-controlled-worker invoke --invocation-id ID --model MODEL \
                 --effort EFFORT --new-session [ID] --resume ID --behavior BEHAVIOR"
            );
            Ok(())
        }
        Some("invoke") => invoke(&arguments[1..]),
        Some("hold-tree") => hold_tree(&arguments[1..]),
        Some("hold-lock") => hold_lock(&arguments[1..]),
        Some(_) => Err("unknown controlled worker command"),
    }
}

fn invoke(arguments: &[String]) -> Result<(), &'static str> {
    if env::var("WORLDSTREAM_CONTROLLED_PROVIDER").as_deref() != Ok("1") {
        return Err("controlled provider marker is missing");
    }
    let invocation_id = required(arguments, "--invocation-id")?;
    let model = required(arguments, "--model")?;
    let effort = optional(arguments, "--effort");
    let behavior = required(arguments, "--behavior")?;
    let session_id = optional(arguments, "--resume")
        .or_else(|| optional(arguments, "--new-session"))
        .unwrap_or("controlled-session");
    let mut prompt = String::new();
    std::io::stdin()
        .read_to_string(&mut prompt)
        .map_err(|_| "prompt read failed")?;
    if behavior == "delayed" {
        thread::sleep(Duration::from_millis(250));
    }
    if behavior != "unreported" {
        let reported_model = if behavior == "mismatch-model" {
            "substituted-model"
        } else {
            model
        };
        let reported_effort = if behavior == "mismatch-effort" {
            Some("substituted-effort")
        } else {
            effort
        };
        println!(
            "{}",
            json!({
                "type": "configuration",
                "model": reported_model,
                "effort": reported_effort,
                "session_id": session_id
            })
        );
    }
    let result_text = controlled_result(&prompt, invocation_id)?;
    println!(
        "{}",
        json!({
            "type": "result",
            "invocation_id": invocation_id,
            "text": result_text
        })
    );
    Ok(())
}

fn controlled_result(prompt: &str, invocation_id: &str) -> Result<String, &'static str> {
    let Ok(request) = serde_json::from_str::<Value>(prompt) else {
        return Ok(prompt.to_owned());
    };
    if !matches!(
        request.get("schema").and_then(Value::as_str),
        Some(
            "worldstream/agent-swarm-worker-invocation@2"
                | "worldstream/agent-swarm-planning-invocation@1"
        )
    ) {
        return Ok(prompt.to_owned());
    }
    let instruction = request
        .get("instruction")
        .and_then(Value::as_str)
        .ok_or("controlled invocation instruction is missing")?;
    if let Some(output) = controlled_fixture_result(&request, instruction, invocation_id)? {
        return Ok(output);
    }
    if let Ok(review) = serde_json::from_str::<Value>(instruction)
        && review.get("schema").and_then(Value::as_str)
            == Some("worldstream/agent-swarm-progress-review-instruction@1")
    {
        let target = review
            .get("target")
            .and_then(Value::as_object)
            .ok_or("controlled Progress Review target is missing")?;
        let review_id = target
            .get("review_id")
            .and_then(Value::as_str)
            .ok_or("controlled Progress Review id is missing")?;
        let review_revision = target
            .get("review_revision")
            .and_then(Value::as_u64)
            .ok_or("controlled Progress Review revision is missing")?;
        let work_id = target
            .get("work_id")
            .and_then(Value::as_str)
            .ok_or("controlled Progress Review work id is missing")?;
        let work_revision = target
            .get("work_revision")
            .and_then(Value::as_u64)
            .ok_or("controlled Progress Review work revision is missing")?;
        let proposal = match review.get("task").and_then(Value::as_str) {
            Some("claim") => json!({
                "schema": "worldstream/agent-swarm-action-proposal@1",
                "action_type": "claim_work_item",
                "payload": {
                    "attempt_id": format!("attempt-{invocation_id}"),
                    "expected_work_revision": work_revision,
                    "work_id": work_id,
                }
            }),
            Some("report") => json!({
                "schema": "worldstream/agent-swarm-action-proposal@1",
                "action_type": "report_progress_review",
                "payload": {
                    "assessment": "aligned",
                    "corrective_work_ids": [],
                    "evidence_refs": [],
                    "expected_review_revision": review_revision,
                    "review_id": review_id,
                    "summary": "Current Swarm progress remains aligned with the accepted goal."
                }
            }),
            _ => return Err("controlled Progress Review task is invalid"),
        };
        return serde_json::to_string(&proposal)
            .map_err(|_| "controlled Progress Review proposal serialization failed");
    }
    if let Ok(proposal) = serde_json::from_str::<Value>(instruction)
        && let Some(local_path) = proposal
            .get("artifact")
            .and_then(Value::as_object)
            .and_then(|artifact| artifact.get("local_path"))
            .and_then(Value::as_str)
    {
        let path =
            controlled_artifact_path(local_path).ok_or("controlled artifact path is invalid")?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|_| "controlled artifact parent creation failed")?;
        }
        fs::write(
            path,
            format!("controlled contribution from {invocation_id}\n"),
        )
        .map_err(|_| "controlled artifact write failed")?;
    }
    Ok(instruction.to_owned())
}

fn controlled_fixture_result(
    request: &Value,
    instruction: &str,
    invocation_id: &str,
) -> Result<Option<String>, &'static str> {
    let Ok(mut fixture) = serde_json::from_str::<Value>(instruction) else {
        return Ok(None);
    };
    // Adaptive dispatch preserves the immutable action contract alongside the
    // selected instruction. This fixture executes only its own typed model
    // instruction; real provider prompts retain both parts.
    if let Some(selected) = fixture.get("model_instruction").and_then(Value::as_str) {
        fixture = serde_json::from_str(selected)
            .map_err(|_| "controlled model instruction is invalid")?;
    }
    match fixture["schema"].as_str() {
        Some("worldstream/agent-swarm-planning-instruction@1") => {
            controlled_planning(request, &fixture).map(Some)
        }
        Some("worldstream/agent-swarm-autonomy-instruction@1") => {
            controlled_autonomy(request, &fixture, invocation_id).map(Some)
        }
        Some("worldstream/agent-swarm-controlled-artifact@1") => {
            controlled_artifact(fixture).map(Some)
        }
        _ => Ok(None),
    }
}

/// Deterministic provider fixture for the ordinary autonomous planning protocol.
/// It deliberately supplies no model reasoning or external tool execution.
fn controlled_autonomy(
    request: &Value,
    instruction: &Value,
    invocation_id: &str,
) -> Result<String, &'static str> {
    let target = instruction.get("target").ok_or("autonomy target missing")?;
    let work_id = target["work_id"]
        .as_str()
        .ok_or("autonomy work id missing")?;
    let goal = request["authorized_activity"]["goal"]
        .as_str()
        .filter(|value| value.len() <= 16 * 1024)
        .ok_or("autonomy goal missing or oversized")?;
    let proposal = match instruction["task"].as_str() {
        Some("propose") if target["kind"] == "work_proposal" => {
            let index = instruction["proposal_index"]
                .as_u64()
                .filter(|index| *index > 0)
                .ok_or("autonomy proposal index missing")?;
            let goal_revision = target["goal_revision"]
                .as_u64()
                .ok_or("autonomy goal revision missing")?;
            let topic = if index % 2 == 1 {
                "requirements"
            } else {
                "verification"
            };
            json!({
                "schema": "worldstream/agent-swarm-action-proposal@1",
                "action_type": "propose_work_item",
                "payload": {
                    "work_id": work_id,
                    "expected_goal_revision": goal_revision,
                    "title": format!("Independent {topic} investigation"),
                    "description": format!("Investigate {topic} independently for the accepted goal: {goal}"),
                    "kind": "goal",
                    "dependency_ids": [],
                },
            })
        }
        Some("claim") if target["kind"] == "work_claim" => {
            let attempt_id = instruction["attempt_id"]
                .as_str()
                .ok_or("autonomy attempt id missing")?;
            let revision = target["work_revision"]
                .as_u64()
                .ok_or("autonomy work revision missing")?;
            json!({
                "schema": "worldstream/agent-swarm-action-proposal@1",
                "action_type": "claim_work_item",
                "payload": {"attempt_id": attempt_id, "work_id": work_id,
                            "expected_work_revision": revision},
            })
        }
        Some("contribute") if target["kind"] == "work_attempt" => {
            controlled_failure_once(goal, work_id, instruction)?;
            let local_path = instruction["artifact_path"]
                .as_str()
                .ok_or("autonomy artifact path missing")?;
            let work_revision = target["work_revision"]
                .as_u64()
                .ok_or("autonomy work revision missing")?;
            let attempt_revision = target["attempt_revision"]
                .as_u64()
                .ok_or("autonomy attempt revision missing")?;
            return controlled_artifact(json!({
                "schema": "worldstream/agent-swarm-controlled-artifact@1",
                "artifact_text": format!("Controlled autonomous contribution\nGoal: {goal}\nWork: {work_id}\n"),
                "delay_ms": 1_500,
                "proposal": {
                    "schema": "worldstream/agent-swarm-action-proposal@1",
                    "action_type": "submit_contribution",
                    "payload": {
                        "contribution_id": format!("contribution-{work_id}"),
                        "completes_work": true,
                        "expected_work_revision": work_revision,
                        "expected_attempt_revision": attempt_revision,
                        "work_id": work_id,
                        "resource_basis": [],
                        "source_refs": ["controlled:autonomy-fixture"],
                        "summary": "Controlled evidence for one independently owned goal task.",
                    },
                    "artifact": {"artifact_id": format!("artifact-{invocation_id}"),
                                 "local_path": local_path, "media_type": "text/plain"},
                },
            }));
        }
        _ => return Err("autonomy task and target do not match"),
    };
    serde_json::to_string(&proposal).map_err(|_| "autonomy proposal serialization failed")
}

fn controlled_failure_once(
    goal: &str,
    work_id: &str,
    instruction: &Value,
) -> Result<(), &'static str> {
    if !goal.contains("CONTROLLED_FAIL_ONCE") {
        return Ok(());
    }
    let marker = ".swarm-autonomy-fixture-failure";
    match OpenOptions::new().write(true).create_new(true).open(marker) {
        Ok(mut file) => {
            file.write_all(work_id.as_bytes())
                .map_err(|_| "controlled failure marker write failed")?;
            Err("intentional controlled contribution failure")
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let failed_work =
                fs::read_to_string(marker).map_err(|_| "controlled failure marker read failed")?;
            if failed_work == work_id
                && !instruction["instructions"]
                    .as_str()
                    .is_some_and(|text| text.contains("Retained process failure"))
            {
                return Err("controlled recovery requires a revised planning instruction");
            }
            Ok(())
        }
        Err(_) => Err("controlled failure marker unavailable"),
    }
}

/// A state-sensitive protocol fixture: the product never uses these choices
/// for real providers. A retained failure changes the next emitted instruction.
fn controlled_planning(request: &Value, instruction: &Value) -> Result<String, &'static str> {
    let options = instruction["options"]
        .as_array()
        .ok_or("controlled planning options missing")?;
    let work = request["authorized_activity"]["work_items"]
        .as_array()
        .ok_or("controlled planning work context missing")?;
    let failed = instruction["recent_outcomes"]
        .as_array()
        .is_some_and(|outcomes| {
            outcomes
                .iter()
                .any(|item| item["coordinator_outcome"]["reason"] == "process_failed")
        });
    let wanted = if work.len() < 2 {
        "work_proposal"
    } else if work.iter().any(|item| item["status"] == "open") {
        "work_claim"
    } else {
        "work_attempt"
    };
    let mut eligible = options
        .iter()
        .filter(|option| option["semantic_target"]["kind"] == wanted)
        .collect::<Vec<_>>();
    let mut members = eligible
        .iter()
        .filter_map(|option| option["member_key"].as_str())
        .collect::<Vec<_>>();
    members.sort_unstable();
    members.dedup();
    if wanted != "work_attempt" && !members.is_empty() {
        let ordinal = if wanted == "work_proposal" {
            work.len()
        } else {
            work.iter()
                .position(|item| item["status"] == "open")
                .unwrap_or(0)
        };
        let selected = members[ordinal % members.len()];
        eligible.retain(|option| option["member_key"] == selected);
        eligible.truncate(1);
    }
    let mut seen = std::collections::BTreeSet::new();
    eligible.retain(|option| seen.insert(option["semantic_target"]["work_id"].as_str()));
    let steps = eligible.into_iter().map(|option| {
        let mut task: Value = serde_json::from_str(option["instruction"].as_str().ok_or("controlled planning option instruction missing")?)
            .map_err(|_| "controlled planning option instruction invalid")?;
        if failed {
            task["instructions"] = json!("Retained process failure: retry the exact owned work with corrected execution instructions.");
        }
        Ok(json!({
            "target_id": option["target_id"], "member_key": option["member_key"],
            "instruction": serde_json::to_string(&task).map_err(|_| "controlled planning step serialization failed")?,
        }))
    }).collect::<Result<Vec<_>, &'static str>>()?;
    let decision = if steps.is_empty() {
        json!({"kind": "wait", "reason": "The fixture's two independent contributions are ready for integration and review."})
    } else {
        json!({"kind": "dispatch", "steps": steps, "reason": if failed {
            "Retained process failure requires a revised execution instruction; preserve accepted independent work."
        } else { "Choose the next eligible work from the current authoritative context." }})
    };
    serde_json::to_string(
        &json!({"schema": "worldstream/agent-swarm-planning-decision@1", "decision": decision}),
    )
    .map_err(|_| "controlled planning decision serialization failed")
}

fn controlled_artifact(value: Value) -> Result<String, &'static str> {
    let fixture: ControlledArtifact =
        serde_json::from_value(value).map_err(|_| "controlled artifact fixture is invalid")?;
    if fixture.schema != "worldstream/agent-swarm-controlled-artifact@1"
        || fixture.artifact_text.len() > 64 * 1024
        || fixture.delay_ms > 5_000
        || fixture.proposal.get("schema").and_then(Value::as_str)
            != Some("worldstream/agent-swarm-action-proposal@1")
    {
        return Err("controlled artifact fixture exceeds its contract");
    }
    let local_path = fixture
        .proposal
        .get("artifact")
        .and_then(|artifact| artifact.get("local_path"))
        .and_then(Value::as_str)
        .ok_or("controlled artifact fixture path is missing")?;
    let path = controlled_artifact_path(local_path)
        .ok_or("controlled artifact fixture path is invalid")?;
    let output = serde_json::to_string(&fixture.proposal)
        .map_err(|_| "controlled artifact proposal serialization failed")?;
    thread::sleep(Duration::from_millis(fixture.delay_ms));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|_| "controlled artifact parent creation failed")?;
    }
    fs::write(path, fixture.artifact_text).map_err(|_| "controlled artifact write failed")?;
    Ok(output)
}

fn controlled_artifact_path(value: &str) -> Option<PathBuf> {
    if value.is_empty()
        || value.starts_with('/')
        || value.starts_with('\\')
        || value.contains(':')
        || value.contains('\0')
    {
        return None;
    }
    let mut path = PathBuf::new();
    for segment in value.split('/') {
        if segment.is_empty() || matches!(segment, "." | "..") || segment.contains('\\') {
            return None;
        }
        path.push(segment);
    }
    Some(path)
}

fn hold_tree(arguments: &[String]) -> Result<(), &'static str> {
    let lock_path = PathBuf::from(required(arguments, "--lock")?);
    let ready_path = PathBuf::from(required(arguments, "--ready")?);
    let executable = env::current_exe().map_err(|_| "current executable unavailable")?;
    let child = Command::new(executable)
        .arg("hold-lock")
        .arg("--lock")
        .arg(&lock_path)
        .arg("--ready")
        .arg(&ready_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "grandchild spawn failed")?;
    let _child = child;
    loop {
        thread::sleep(Duration::from_secs(1));
    }
}

fn hold_lock(arguments: &[String]) -> Result<(), &'static str> {
    let lock_path = PathBuf::from(required(arguments, "--lock")?);
    let ready_path = PathBuf::from(required(arguments, "--ready")?);
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path)
        .map_err(|_| "lock file unavailable")?;
    file.lock().map_err(|_| "lock acquisition failed")?;
    fs::write(ready_path, b"ready").map_err(|_| "ready publication failed")?;
    let _file = file;
    loop {
        thread::sleep(Duration::from_secs(1));
    }
}

fn required<'a>(arguments: &'a [String], name: &str) -> Result<&'a str, &'static str> {
    optional(arguments, name).ok_or("required argument is missing")
}

fn optional<'a>(arguments: &'a [String], name: &str) -> Option<&'a str> {
    arguments
        .iter()
        .position(|value| value == name)
        .and_then(|index| arguments.get(index.saturating_add(1)))
        .filter(|value| !value.starts_with("--"))
        .map(String::as_str)
}
