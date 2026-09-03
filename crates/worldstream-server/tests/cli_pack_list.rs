//! Installed intent and the exact frozen running Pack registry stay distinct.

use serde_json::Value;
use std::{
    env, fs,
    path::Path,
    process::{Command, Output, Stdio},
};
use worldstream_studio_supervisor::process_ownership::{ProcessOwnership, ProcessRole};

const BUNDLE_DIGEST: &str =
    "blake3:9033a1aa10ca37c301660b7427d79c4e71d7af59006bc7d51edc4b470c8c2db5";

fn cli(root: &Path, args: &[&str]) -> std::io::Result<Output> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_worldstreamctl"));
    command.current_dir(root).args(args).stdin(Stdio::null());
    for (key, _) in env::vars_os() {
        if key.to_string_lossy().starts_with("WORLDSTREAM") {
            command.env_remove(key);
        }
    }
    command.output()
}

struct Installation {
    root: Option<tempfile::TempDir>,
    controller: String,
}
impl Drop for Installation {
    fn drop(&mut self) {
        let Some(root) = &self.root else {
            return;
        };
        for operation in ["stop", "controller-stop"] {
            let _ = cli(
                root.path(),
                &[
                    "server",
                    operation,
                    "--controller",
                    &self.controller,
                    "--timeout-seconds",
                    "10",
                    "--json",
                ],
            );
        }
        let released =
            ProcessOwnership::open(&root.path().join(".worldstream/studio")).is_ok_and(|owner| {
                [ProcessRole::Controller, ProcessRole::Runtime]
                    .into_iter()
                    .all(|role| owner.is_leased(role) == Ok(false))
            });
        if !released && let Some(root) = self.root.take() {
            eprintln!(
                "Owned Pack-list installation retained for authenticated cleanup: {}",
                root.keep().display()
            );
        }
    }
}

fn prepare_offline_pack(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let bundle = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs/negotiate/releases/0.1.0/worldstream-negotiate-9033a1aa10ca37c301660b7427d79c4e71d7af59006bc7d51edc4b470c8c2db5.wspack").canonicalize()?;
    let bundle = bundle.to_str().ok_or("non-UTF8 test bundle")?;
    for arguments in [
        vec![
            "pack",
            "approve",
            "--bundle",
            bundle,
            "--operator-id",
            "pack-list-test",
            "--decided-at",
            "2026-09-03T00:00:00Z",
        ],
        vec![
            "pack",
            "install",
            "--bundle",
            bundle,
            "--installed-at",
            "2026-09-03T00:00:01Z",
        ],
        vec![
            "pack",
            "set-selectable",
            "--bundle-digest",
            BUNDLE_DIGEST,
            "--selectable",
            "true",
        ],
        vec!["pack", "restart-readiness"],
    ] {
        let mut configured = vec!["--config", ".worldstream/worldstream.toml"];
        configured.extend(arguments);
        assert!(
            cli(root, &configured)?.status.success(),
            "offline Pack prerequisite failed"
        );
    }
    Ok(())
}

#[test]
fn approved_installed_selected_pack_lists_exact_frozen_running_identity()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let runtime = std::net::TcpListener::bind("127.0.0.1:0")?;
    let controller = std::net::TcpListener::bind("127.0.0.1:0")?;
    assert!(
        cli(
            root.path(),
            &[
                "init",
                "--bind",
                &runtime.local_addr()?.to_string(),
                "--json"
            ]
        )?
        .status
        .success()
    );
    let installation = Installation {
        root: Some(root),
        controller: controller.local_addr()?.to_string(),
    };
    let root = installation.root.as_ref().ok_or("missing fixture")?.path();
    prepare_offline_pack(root)?;
    drop(runtime);
    drop(controller);
    assert!(
        cli(
            root,
            &[
                "server",
                "start",
                "--controller",
                &installation.controller,
                "--timeout-seconds",
                "300",
                "--json"
            ]
        )?
        .status
        .success()
    );
    let output = cli(
        root,
        &[
            "pack",
            "list",
            "--controller",
            &installation.controller,
            "--timeout-seconds",
            "300",
            "--json",
        ],
    )?;
    assert_eq!(
        output.status.code(),
        Some(0),
        "pack list must report the running installation"
    );
    let report: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(report["schema"], "worldstream/operator-command/v1");
    let packs = &report["packs"];
    assert_eq!(packs["installed"]["evidence"], "unverified_metadata");
    assert_eq!(
        packs["installed"]["entries"][0]["bundle_digest"],
        BUNDLE_DIGEST
    );
    assert_eq!(
        packs["installed"]["entries"][0]["install_state"],
        "selectable"
    );
    assert_eq!(packs["running"]["availability"], "available");
    assert_eq!(
        packs["next_start"]["selectable_bundle_digests"],
        serde_json::json!([BUNDLE_DIGEST])
    );
    assert_eq!(
        packs["running"]["facts"]["installed"][0]["bundle_digest"],
        BUNDLE_DIGEST
    );
    assert_eq!(
        packs["running"]["facts"]["installed"][0]["revision_digest"],
        "blake3:a62585c88ffebe0b2222f5f93e17de1e9cbb003593eca4891225f75dca985589"
    );
    assert!(
        !packs["running"]["facts"]["embedded_revisions"]
            .as_array()
            .ok_or("missing embedded facts")?
            .is_empty()
    );
    assert!(
        packs["running"]["facts"]["embedded_revisions"]
            .as_array()
            .ok_or("missing embedded facts")?
            .iter()
            .all(|entry| entry["digest"]
                != "blake3:a62585c88ffebe0b2222f5f93e17de1e9cbb003593eca4891225f75dca985589")
    );
    assert_eq!(packs["pending_changes"], false);

    let human = cli(
        root,
        &[
            "pack",
            "list",
            "--controller",
            &installation.controller,
            "--timeout-seconds",
            "300",
        ],
    )?;
    assert!(human.status.success());
    assert!(String::from_utf8(human.stdout)?.contains("next-start=selectable"));

    fs::write(
        root.join(".worldstream/data/activity-packs/restart-readiness-v1.json"),
        b"not-json",
    )?;
    let malformed_readiness = cli(
        root,
        &[
            "pack",
            "list",
            "--controller",
            &installation.controller,
            "--timeout-seconds",
            "300",
            "--json",
        ],
    )?;
    assert!(malformed_readiness.status.success());
    let malformed_readiness: Value = serde_json::from_slice(&malformed_readiness.stdout)?;
    assert_eq!(
        malformed_readiness["packs"]["installed"]["availability"],
        "available"
    );
    assert_eq!(malformed_readiness["packs"]["pending_changes"], true);

    fs::remove_file(
        root.join(".worldstream/data/activity-packs/inventory")
            .join(format!(
                "{}.json",
                BUNDLE_DIGEST.trim_start_matches("blake3:")
            )),
    )?;
    let changed = cli(
        root,
        &[
            "pack",
            "list",
            "--controller",
            &installation.controller,
            "--timeout-seconds",
            "300",
            "--json",
        ],
    )?;
    assert!(changed.status.success());
    let changed: Value = serde_json::from_slice(&changed.stdout)?;
    assert_eq!(changed["packs"]["pending_changes"], true);
    Ok(())
}
