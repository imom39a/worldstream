//! Native transport fixtures, not live provider qualification.
#![cfg(all(feature = "managed-local-runtime", unix))]

use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};
use worldstream_agent_swarm::execution::{
    ConfigurationResolution, NativeProcessSpawner, PreparedInvocation, ProcessError, ProviderKind,
    SessionSelection,
    provider::{CodexAdapter, OutputContract, ProviderAdapter},
};

const GUARD: &str = env!("CARGO_BIN_EXE_worldstream-agent-swarm-process-guard");

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn emit(value: &Value) -> String {
    format!("printf '%s\\n' {}\n", quote(&value.to_string()))
}

fn fixture(directory: &Path, finish: bool) -> Result<PreparedInvocation, Box<dyn Error>> {
    let cwd = directory.canonicalize()?;
    let mut script = String::from("#!/bin/sh\nset -eu\nIFS= read -r request\n");
    script.push_str(&emit(&json!({"id":1,"result":{}})));
    script.push_str("IFS= read -r initialized\nIFS= read -r request\n");
    script.push_str(&emit(
        &json!({"id":2,"result":{"account":{"type":"chatgpt"}}}),
    ));
    script.push_str("IFS= read -r request\n");
    script.push_str(&emit(&json!({"id":3,"result":{
        "model":"fixture-model","reasoningEffort":"high","modelProvider":"openai",
        "cwd":cwd,"approvalPolicy":"never","sandbox":{"type":"readOnly","networkAccess":false},
        "thread":{"id":"fixture-thread","sessionId":"fixture-session"}}})));
    script.push_str("IFS= read -r request\n");
    script.push_str(&emit(
        &json!({"id":4,"result":{"turn":{"id":"fixture-turn","status":"inProgress"}}}),
    ));
    writeln!(
        script,
        "printf ready > {}",
        quote(&cwd.join("ready").to_string_lossy())
    )?;
    if finish {
        script.push_str(&emit(&json!({"method":"item/completed","params":{
            "threadId":"fixture-thread","turnId":"fixture-turn",
            "item":{"id":"message","type":"agentMessage","phase":"final_answer","text":"fixture result"}}})));
        script.push_str(&emit(&json!({"method":"turn/completed","params":{
            "threadId":"fixture-thread","turn":{"id":"fixture-turn","status":"completed","error":null}}})));
    }
    // Keep the provider alive beyond its final output, just like app-server.
    // A clean guard exit therefore proves it terminated the long-lived peer.
    script.push_str("while :; do sleep 1; done\n");
    let script_path = cwd.join("fake-app-server.sh");
    fs::write(&script_path, script)?;
    let program = PathBuf::from("/bin/sh").canonicalize()?;
    Ok(PreparedInvocation {
        // Controlled marks this as a model-free transport fixture and never
        // manufactures installed Codex admission/qualification evidence.
        provider: ProviderKind::Controlled,
        invocation_id: "native-app-server-fixture".to_owned(),
        member_id: "fixture-member".to_owned(),
        configuration_revision: 1,
        executable_digest: format!("blake3:{}", blake3::hash(&fs::read(&program)?)),
        program,
        qualification: None,
        arguments: vec![script_path.to_string_lossy().into_owned()],
        working_area: cwd.clone(),
        environment_remove: BTreeSet::new(),
        environment_set: BTreeMap::new(),
        ephemeral_profile: None,
        stdin: json!({"model":"fixture-model","effort":"high","cwd":cwd,
            "resource_policy":"read_only","session":{"mode":"fresh","requested_id":null},
            "prompt":"authorized fixture context"})
        .to_string(),
        requested_model: "fixture-model".to_owned(),
        requested_effort: Some("high".to_owned()),
        requested_session: SessionSelection::Fresh { requested_id: None },
        resolution: ConfigurationResolution::PendingReport,
        output_contract: OutputContract::CodexAppServer,
    })
}

#[test]
fn protocol_selection_cannot_diverge_from_prepared_binding() -> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let mut prepared = fixture(temporary.path(), true)?;
    let mut request: Value = serde_json::from_str(&prepared.stdin)?;
    request["model"] = json!("unapproved-model");
    prepared.stdin = serde_json::to_string(&request)?;
    let spawner = NativeProcessSpawner::with_default_capture(Path::new(GUARD))?;
    assert!(matches!(
        spawner.spawn(&prepared),
        Err(ProcessError::InvalidRequest)
    ));
    assert!(!temporary.path().join("ready").exists());
    Ok(())
}

#[test]
fn native_guard_drives_full_duplex_and_reaps_long_lived_server() -> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let prepared = fixture(temporary.path(), true)?;
    let spawner = NativeProcessSpawner::with_default_capture(Path::new(GUARD))?;
    let mut process = spawner.spawn(&prepared)?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let exit = loop {
        if let Some(exit) = process.poll()? {
            break exit;
        }
        if Instant::now() >= deadline {
            return Err("fixture turn did not terminate".into());
        }
        thread::sleep(Duration::from_millis(10));
    };
    assert!(exit.success, "{}", String::from_utf8_lossy(&exit.stderr));
    let output = CodexAdapter.decode(&prepared, &exit.stdout)?;
    output.verify(&prepared)?;
    assert_eq!(output.text, "fixture result");
    assert_eq!(output.session_id.as_deref(), Some("fixture-thread"));
    assert!(!exit.stdout_truncated);
    Ok(())
}

#[test]
fn provider_exit_on_protocol_eof_is_reaped_without_a_false_containment_failure()
-> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let prepared = fixture(temporary.path(), true)?;
    let script = Path::new(&prepared.arguments[0]);
    fs::write(
        script,
        fs::read_to_string(script)?.replace(
            "while :; do sleep 1; done",
            "while IFS= read -r request; do :; done\nexit 0",
        ),
    )?;
    let spawner = NativeProcessSpawner::with_default_capture(Path::new(GUARD))?;
    for _ in 0..12 {
        let mut process = spawner.spawn(&prepared)?;
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(exit) = process.poll()? {
                assert!(exit.success, "{}", String::from_utf8_lossy(&exit.stderr));
                CodexAdapter
                    .decode(&prepared, &exit.stdout)?
                    .verify(&prepared)?;
                break;
            }
            if Instant::now() >= deadline {
                return Err("EOF-exiting fixture did not terminate".into());
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
    Ok(())
}

#[test]
fn native_guard_cancels_while_app_server_is_waiting_for_output() -> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let prepared = fixture(temporary.path(), false)?;
    let spawner = NativeProcessSpawner::with_default_capture(Path::new(GUARD))?;
    let mut process = spawner.spawn(&prepared)?;
    let deadline = Instant::now() + Duration::from_secs(10);
    while !temporary.path().join("ready").is_file() {
        if let Some(exit) = process.poll()? {
            return Err(format!(
                "fixture failed before ready: {}",
                String::from_utf8_lossy(&exit.stderr)
            )
            .into());
        }
        if Instant::now() >= deadline {
            return Err("fixture did not reach waiting turn".into());
        }
        thread::sleep(Duration::from_millis(10));
    }
    let stopped = process.stop(Duration::from_secs(5))?;
    assert!(!stopped.success);
    assert!(
        stopped.stdout.is_empty(),
        "unfinished turn must not produce accepted transcript"
    );
    Ok(())
}
