//! Compatibility at the shipped command-line process boundary.
//!
//! Fixtures freeze the pre-CLI-first legacy contract at revision 1d8e6dc.
//! Only build revision text is normalized; legacy commands keep their own
//! receipts rather than adopting the additive operator result envelope.

use std::{
    env, fs,
    io::Write as _,
    path::Path,
    process::{Command, Stdio},
};

use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
struct HelpFixtureSet {
    fixtures: Vec<HelpFixture>,
}

#[derive(Deserialize)]
struct HelpFixture {
    args: Vec<String>,
    stdout: String,
}

fn command(directory: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_worldstreamctl"));
    command.current_dir(directory).stdin(Stdio::null());
    for (key, _) in env::vars_os() {
        if key.to_string_lossy().starts_with("WORLDSTREAM") {
            command.env_remove(key);
        }
    }
    command
}

#[test]
fn legacy_leaf_help_and_flags_remain_unchanged() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let fixtures: HelpFixtureSet =
        serde_json::from_str(include_str!("fixtures/cli-legacy/help.json"))?;
    for fixture in fixtures.fixtures {
        let output = command(directory.path())
            .args(&fixture.args)
            .arg("--help")
            .output()?;
        assert!(output.status.success(), "{:?}", fixture.args);
        assert!(output.stderr.is_empty(), "{:?}", fixture.args);
        let stdout = String::from_utf8(output.stdout)?;
        let normalized = stdout
            .split_inclusive('\n')
            .filter(|line| !line.starts_with("Source revision: "))
            .collect::<String>();
        assert_eq!(normalized, fixture.stdout, "{:?}", fixture.args);
    }
    assert_eq!(fs::read_dir(directory.path())?.count(), 0);
    Ok(())
}

#[test]
fn legacy_version_keeps_its_machine_document_without_json_flag()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let output = command(directory.path()).arg("version").output()?;
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let mut receipt: Value = serde_json::from_slice(&output.stdout)?;
    assert!(receipt["product_build"]["source_revision"].is_string());
    receipt["product_build"]["source_revision"] = json!("<build-revision>");
    let baseline: Value = serde_json::from_str(include_str!("fixtures/cli-legacy/version.json"))?;
    assert_eq!(receipt, baseline);
    assert_eq!(fs::read_dir(directory.path())?.count(), 0);
    Ok(())
}

#[test]
fn legacy_inventory_keeps_the_closed_offline_receipt() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let protected = directory.path().join("protected");
    worldstream_runtime::prepare_data_directory(&protected)?;
    let authority_path = protected.join("authority.secret");
    let mut authority = worldstream_runtime::create_owner_only_file(&authority_path)?;
    authority.write_all(&[b'x'; 32])?;
    authority.sync_all()?;
    drop(authority);
    let config = directory.path().join("worldstream.toml");
    fs::write(
        &config,
        format!(
            "config_version = 1\n\
             [server]\nbind = \"127.0.0.1:65500\"\n\
             [storage]\nprofile = \"sqlite-bundled\"\n\
             data_dir = {}\ndeployment_lineage = \"test/cli-legacy\"\nstorage_epoch = 1\n\
             [authority.bootstrap]\nsecret_file = {}\n",
            serde_json::to_string(&directory.path().join("data"))?,
            serde_json::to_string(&authority_path)?,
        ),
    )?;
    let output = command(directory.path())
        .arg("--config")
        .arg(&config)
        .args(["pack", "inventory"])
        .output()?;
    assert!(output.status.success(), "{:?}", output.status);
    assert!(output.stderr.is_empty());
    let receipt: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        receipt,
        json!({
            "schema": "worldstream/pack-operator-receipt/v1",
            "status": "complete",
            "operation": "inventory",
            "installed": 0,
            "selectable": 0,
            "retained_only": 0,
            "storage_profile": "sqlite-bundled",
            "inventory_digest": "blake3:db34f4f0ee69b6fd54a823a3e0abfb74d811a4be0998e30ae9bda362f65b66bc",
            "entries": [],
            "restart_required_for_pending_changes": true,
        })
    );
    assert!(!String::from_utf8(output.stdout)?.contains("xxxxxxxx"));
    Ok(())
}
