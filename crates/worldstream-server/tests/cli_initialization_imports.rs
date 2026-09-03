//! Explicit prerequisite review and approval through the shipped CLI process.
#![cfg(feature = "cli-operator-preview")]

use std::{
    env, fs,
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
