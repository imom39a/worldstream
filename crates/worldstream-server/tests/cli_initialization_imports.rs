//! Explicit prerequisite review and approval through the shipped CLI process.

use std::{
    env, fs,
    io::Write as _,
    path::Path,
    process::{Command, Output, Stdio},
};

fn control(directory: &Path, arguments: &[&str]) -> std::io::Result<Output> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_worldstreamctl"));
    command
        .current_dir(directory)
        .args(arguments)
        .stdin(Stdio::null());
    for (key, _) in env::vars_os() {
        if key.to_string_lossy().starts_with("WORLDSTREAM") {
            command.env_remove(key);
        }
    }
    command.output()
}

#[test]
fn import_rejections_explain_base_initialization_and_exact_review_remediation()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    fs::write(directory.path().join("profile.json"), br#"{"schema":"worldstream/studio-agent-profile-publish/v2","profile_id":"external-agent","revision":"1","display_name":"External agent","non_secret_configuration":{},"host_contract":{"kind":"generic_mcp"}}"#)?;
    let missing = control(
        directory.path(),
        &[
            "init",
            "--agent-profile",
            "profile.json",
            "--preview",
            "--json",
        ],
    )?;
    assert_eq!(missing.status.code(), Some(1));
    let missing: serde_json::Value = serde_json::from_slice(&missing.stdout)?;
    assert_eq!(missing["code"], "initialization_required");
    assert_eq!(
        missing["next_action"],
        "Run worldstreamctl init with the selected configuration and state directory, then repeat import preview."
    );
    assert!(!directory.path().join(".worldstream").exists());
    assert_eq!(
        control(directory.path(), &["init", "--json"])?
            .status
            .code(),
        Some(0)
    );
    let stale = control(
        directory.path(),
        &[
            "init",
            "--agent-profile",
            "profile.json",
            "--approve-imports",
            "blake3:0000000000000000000000000000000000000000000000000000000000000000",
            "--json",
        ],
    )?;
    assert_eq!(stale.status.code(), Some(1));
    let stale: serde_json::Value = serde_json::from_slice(&stale.stdout)?;
    assert_eq!(stale["code"], "import_approval_required");
    assert_eq!(
        stale["next_action"],
        "Repeat import preview and supply its exact digest with --approve-imports and the same declaration files."
    );
    assert!(
        !directory
            .path()
            .join(".worldstream/studio/agent-profiles")
            .exists()
    );
    Ok(())
}

#[test]
fn explicit_profile_review_and_exact_approval_work_without_starting_services()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    assert_eq!(
        control(directory.path(), &["init", "--json"])?
            .status
            .code(),
        Some(0)
    );
    fs::write(directory.path().join("profile.json"), br#"{"schema":"worldstream/studio-agent-profile-publish/v2","profile_id":"external-agent","revision":"1","display_name":"External agent","non_secret_configuration":{},"host_contract":{"kind":"generic_mcp"}}"#)?;
    let preview = control(
        directory.path(),
        &[
            "init",
            "--agent-profile",
            "profile.json",
            "--preview",
            "--json",
        ],
    )?;
    assert_eq!(preview.status.code(), Some(0));
    let review: serde_json::Value = serde_json::from_slice(&preview.stdout)?;
    assert_eq!(review["status"], "complete");
    assert_eq!(
        review["import_review"]["agent_profiles"][0]["profile_id"],
        "external-agent"
    );
    assert_eq!(review["import_review"]["services_started"], false);
    assert!(
        !directory
            .path()
            .join(".worldstream/studio/agent-profiles")
            .exists()
    );
    let digest = review["import_review"]["digest"]
        .as_str()
        .ok_or("missing review digest")?;
    let applied = control(
        directory.path(),
        &[
            "init",
            "--agent-profile",
            "profile.json",
            "--approve-imports",
            digest,
            "--json",
        ],
    )?;
    assert_eq!(applied.status.code(), Some(0));
    let applied: serde_json::Value = serde_json::from_slice(&applied.stdout)?;
    assert_eq!(
        applied["import_apply"]["created_agent_profiles"][0]["profile_id"],
        "external-agent"
    );
    assert_eq!(applied["import_apply"]["services_started"], false);
    let repeated = control(
        directory.path(),
        &[
            "init",
            "--agent-profile",
            "profile.json",
            "--approve-imports",
            digest,
            "--json",
        ],
    )?;
    assert_eq!(repeated.status.code(), Some(0));
    let repeated: serde_json::Value = serde_json::from_slice(&repeated.stdout)?;
    assert_eq!(
        repeated["import_apply"]["reused_agent_profiles"][0]["profile_id"],
        "external-agent"
    );
    assert!(
        !directory
            .path()
            .join(".worldstream/data/worldstream.sqlite3")
            .exists()
    );
    Ok(())
}

fn control_owned(directory: &Path, arguments: &[String]) -> std::io::Result<Output> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_worldstreamctl"));
    command
        .current_dir(directory)
        .args(arguments)
        .stdin(Stdio::null());
    for (key, _) in env::vars_os() {
        if key.to_string_lossy().starts_with("WORLDSTREAM") {
            command.env_remove(key);
        }
    }
    command.output()
}

fn json_report(directory: &Path, arguments: &[String]) -> serde_json::Value {
    let output = control_owned(directory, arguments).expect("worldstreamctl should start");
    assert!(
        output.status.success(),
        "worldstreamctl failed: status={:?}\nstdout={}\nstderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    serde_json::from_slice(&output.stdout).expect("worldstreamctl JSON report")
}

fn import_arguments(
    runner: &str,
    provider: Option<&str>,
    profiles: &[&str],
    final_argument: &str,
) -> Vec<String> {
    let mut arguments = vec![
        "init".to_owned(),
        "--runner-template".to_owned(),
        runner.to_owned(),
    ];
    if let Some(provider) = provider {
        arguments.extend(["--provider-declaration".to_owned(), provider.to_owned()]);
    }
    for profile in profiles {
        arguments.extend(["--agent-profile".to_owned(), (*profile).to_owned()]);
    }
    arguments.extend([final_argument.to_owned(), "--json".to_owned()]);
    arguments
}

fn runner_declaration(
    path: &Path,
    revision: &str,
    executable: &str,
    executable_digest: &str,
    instance: &str,
    health_address: &str,
) {
    let declaration = serde_json::json!({
        "schema": "worldstream/runner-template/v1",
        "template_id": "openrouter-house",
        "revision": revision,
        "display_name": format!("Hosted house Runner {revision}"),
        "executable": {
            "path": executable,
            "blake3": executable_digest
        },
        "compatibility": [{
            "activity_pack_id": "worldstream.agent-heist",
            "exact_revisions": ["0.2.0"]
        }],
        "capacity": {"maximum_concurrent_invocations": 1},
        "health": {"path": "/health", "timeout_ms": 1000, "stale_after_ms": 5000},
        "non_secret_environment": {"WORLDSTREAM_RUNNER_MODE": "hosted-house"},
        "secret_environment": [],
        "instances": [{"instance_id": instance, "health_address": health_address}]
    });
    fs::write(path, serde_json::to_vec(&declaration).expect("runner JSON")).expect("runner file");
}

fn profile_declaration(path: &Path, profile_id: &str, revision: &str, runner_revision: &str) {
    let declaration = serde_json::json!({
        "schema": "worldstream/studio-agent-profile-publish/v2",
        "profile_id": profile_id,
        "revision": revision,
        "display_name": format!("{profile_id} {revision}"),
        "non_secret_configuration": {},
        "host_contract": {
            "kind": "managed_house_openrouter",
            "host_contract_revision": "1",
            "runner_template": {"template_id": "openrouter-house", "revision": runner_revision}
        },
        "managed_provider_credential_id": "hosted-openrouter"
    });
    fs::write(
        path,
        serde_json::to_vec(&declaration).expect("profile JSON"),
    )
    .expect("profile file");
}

fn provider_declaration(path: &Path) {
    let declaration = serde_json::json!({
        "schema": "worldstream/model-provider-credential-import/v1",
        "credential_id": "hosted-openrouter",
        "display_name": "Hosted test provider",
        "provider": "openrouter",
        "secret_file": "private/model-token"
    });
    fs::write(
        path,
        serde_json::to_vec(&declaration).expect("provider JSON"),
    )
    .expect("provider file");
}

fn encoded_component(value: &str) -> String {
    value.bytes().map(|byte| format!("{byte:02x}")).collect()
}

fn import_batch(
    directory: &Path,
    runner: &str,
    provider: Option<&str>,
    profiles: &[&str],
) -> serde_json::Value {
    let mut preview = import_arguments(runner, provider, profiles, "--preview");
    let review = json_report(directory, &preview);
    assert_eq!(review["status"], "complete");
    let digest = review["import_review"]["digest"]
        .as_str()
        .expect("import review digest")
        .to_owned();
    preview.retain(|argument| argument != "--preview");
    preview.extend(["--approve-imports".to_owned(), digest]);
    let applied = json_report(directory, &preview);
    assert_eq!(applied["status"], "complete");
    assert_eq!(applied["import_apply"]["services_started"], false);
    applied
}

#[test]
fn real_cli_imports_r13_successor_while_retaining_populated_r12_installation()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    fs::write(root.join("managed-house-r12.bin"), [0_u8])?;
    fs::write(root.join("managed-house-r13.bin"), [1_u8])?;
    let private = root.join("private");
    worldstream_runtime::prepare_data_directory(&private)?;
    let mut token = worldstream_runtime::create_owner_only_file(&private.join("model-token"))?;
    token.write_all(b"test-only-provider-secret")?;
    drop(token);
    provider_declaration(&root.join("provider.json"));
    assert!(control(root, &["init", "--json"])?.status.success());

    runner_declaration(
        &root.join("r12-runner.json"),
        "12",
        "managed-house-r12.bin",
        "2d3adedff11b61f14c886e35afa036736dcd87a74d27b5c1510225d0f592e213",
        "hosted-house-r12-01",
        "127.0.0.1:9602",
    );
    profile_declaration(
        &root.join("r12-planner.json"),
        "house-cooperative-planner",
        "13",
        "12",
    );
    profile_declaration(
        &root.join("r12-skeptic.json"),
        "house-skeptical-auditor",
        "12",
        "12",
    );
    let r12_result = import_batch(
        root,
        "r12-runner.json",
        Some("provider.json"),
        &["r12-planner.json", "r12-skeptic.json"],
    );
    assert_eq!(
        r12_result["import_apply"]["created_runner_templates"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );

    runner_declaration(
        &root.join("r13-runner.json"),
        "13",
        "managed-house-r13.bin",
        "48fc721fbbc172e0925fa27af1671de225ba927134802998b10a1568a188652b",
        "hosted-house-r13-01",
        "127.0.0.1:9603",
    );
    profile_declaration(
        &root.join("r13-planner.json"),
        "house-cooperative-planner",
        "14",
        "13",
    );
    profile_declaration(
        &root.join("r13-skeptic.json"),
        "house-skeptical-auditor",
        "13",
        "13",
    );
    let r13_result = import_batch(
        root,
        "r13-runner.json",
        None,
        &["r13-planner.json", "r13-skeptic.json"],
    );
    assert_eq!(
        r13_result["import_apply"]["created_runner_templates"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );

    let state = root.join(".worldstream/studio");
    assert!(
        state
            .join("runner-templates/installed/openrouter-house--12.json")
            .is_file()
    );
    assert!(
        state
            .join("runner-templates/installed/openrouter-house--13.json")
            .is_file()
    );
    for (profile, revisions) in [
        ("house-cooperative-planner", ["13", "14"]),
        ("house-skeptical-auditor", ["12", "13"]),
    ] {
        for revision in revisions {
            assert!(
                state
                    .join("agent-profiles/revisions")
                    .join(encoded_component(profile))
                    .join(format!("{}.json", encoded_component(revision)))
                    .is_file(),
                "retained profile {profile}@{revision} is missing"
            );
        }
    }
    Ok(())
}
