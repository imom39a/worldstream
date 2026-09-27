#![cfg(feature = "managed-local-runtime")]

use std::{
    io::Write as _,
    path::Path,
    process::{Command, Output, Stdio},
};

use serde_json::{Value, json};

fn instruction(path: &str) -> Value {
    json!({
        "schema": "worldstream/agent-swarm-controlled-artifact@1",
        "proposal": {
            "schema": "worldstream/agent-swarm-action-proposal@1",
            "action_type": "submit_contribution",
            "payload": {},
            "artifact": {
                "artifact_id": "fixture-code",
                "local_path": path,
                "media_type": "text/x-python"
            }
        },
        "artifact_text": "def encode(value):\n    return str(value)\n",
        "delay_ms": 0
    })
}

fn invoke(root: &Path, fixture: &Value) -> Result<Output, Box<dyn std::error::Error>> {
    let mut child = Command::new(env!(
        "CARGO_BIN_EXE_worldstream-agent-swarm-controlled-worker"
    ))
    .args([
        "invoke",
        "--invocation-id",
        "artifact-test",
        "--model",
        "controlled",
        "--behavior",
        "normal",
        "--new-session",
        "artifact-session",
    ])
    .env_clear()
    .env("WORLDSTREAM_CONTROLLED_PROVIDER", "1")
    .current_dir(root)
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()?;
    let request = json!({
        "schema": "worldstream/agent-swarm-worker-invocation@2",
        "instruction": serde_json::to_string(fixture)?
    });
    child
        .stdin
        .take()
        .ok_or("missing stdin")?
        .write_all(&serde_json::to_vec(&request)?)?;
    Ok(child.wait_with_output()?)
}

#[test]
fn controlled_artifact_writes_exact_fixture_and_returns_normal_proposal()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let fixture = instruction("contributions/encode.py");
    let output = invoke(root.path(), &fixture)?;
    assert!(output.status.success());
    assert_eq!(
        std::fs::read_to_string(root.path().join("contributions/encode.py"))?,
        fixture["artifact_text"]
            .as_str()
            .ok_or("missing fixture text")?
    );
    let messages = String::from_utf8(output.stdout)?
        .lines()
        .map(serde_json::from_str::<Value>)
        .collect::<Result<Vec<_>, _>>()?;
    let result = messages.last().ok_or("missing result")?;
    assert_eq!(
        serde_json::from_str::<Value>(result["text"].as_str().ok_or("missing proposal")?)?,
        fixture["proposal"]
    );
    Ok(())
}

#[test]
fn controlled_artifact_rejects_unbounded_or_escaping_fixtures_before_writing()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let work = root.path().join("work");
    std::fs::create_dir(&work)?;
    let mut too_large = instruction("out.py");
    too_large["artifact_text"] = json!("x".repeat(64 * 1024 + 1));
    let mut too_slow = instruction("out.py");
    too_slow["delay_ms"] = json!(5_001);
    let mut unexpected_field = instruction("out.py");
    unexpected_field["command"] = json!("not supported");
    for fixture in [
        instruction("../escaped.py"),
        too_large,
        too_slow,
        unexpected_field,
    ] {
        assert!(!invoke(&work, &fixture)?.status.success());
        assert_eq!(std::fs::read_dir(&work)?.count(), 0);
        assert!(!root.path().join("escaped.py").exists());
    }
    Ok(())
}
