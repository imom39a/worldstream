//! Freeze legacy Pack receipts at the command-line process boundary.
//!
//! The fixtures capture the unchanged operator behavior from the exact retained
//! Negotiate Bundle. Only the temporary deployment binding is normalized.

use std::{
    env, fs,
    io::Write as _,
    path::Path,
    process::{Command, Stdio},
};

use serde_json::Value;

const BUNDLE_DIGEST: &str =
    "blake3:9033a1aa10ca37c301660b7427d79c4e71d7af59006bc7d51edc4b470c8c2db5";
const REVISION_DIGEST: &str =
    "blake3:a62585c88ffebe0b2222f5f93e17de1e9cbb003593eca4891225f75dca985589";
type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn receipt(directory: &Path, config: &Path, arguments: &[&str]) -> TestResult<Value> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_worldstreamctl"));
    command.current_dir(directory).stdin(Stdio::null());
    for (key, _) in env::vars_os() {
        if key.to_string_lossy().starts_with("WORLDSTREAM") {
            command.env_remove(key);
        }
    }
    let output = command
        .arg("--config")
        .arg(config)
        .arg("pack")
        .args(arguments)
        .output()?;
    assert!(
        output.status.success(),
        "{arguments:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty(), "{arguments:?}");
    let stdout = String::from_utf8(output.stdout)?;
    assert!(
        !stdout.contains("xxxxxxxx"),
        "credential appeared in {arguments:?}"
    );
    assert!(
        !stdout.contains(directory.to_string_lossy().as_ref()),
        "temporary path appeared in {arguments:?}"
    );
    Ok(serde_json::from_str(&stdout)?)
}

fn check(receipts: &Value, key: &str, actual: Value) {
    assert_eq!(actual, receipts[key], "legacy Pack receipt: {key}");
}

#[test]
#[allow(clippy::too_many_lines)]
fn exact_bundle_keeps_legacy_receipts_through_the_offline_lifecycle() -> TestResult {
    let directory = tempfile::tempdir()?;
    let protected =
        worldstream_runtime::prepare_data_directory(&directory.path().join("protected"))?;
    let authority_path = protected.join("authority.secret");
    let mut authority = worldstream_runtime::create_owner_only_file(&authority_path)?;
    authority.write_all(&[b'x'; 32])?;
    authority.sync_all()?;
    drop(authority);
    let config = directory.path().join("worldstream.toml");
    fs::write(
        &config,
        format!(
            "config_version = 1\n[server]\nbind = \"127.0.0.1:65500\"\n[storage]\nprofile = \"sqlite-bundled\"\ndata_dir = {}\ndeployment_lineage = \"test/cli-pack-receipts\"\nstorage_epoch = 1\n[authority.bootstrap]\nsecret_file = {}\n",
            serde_json::to_string(&directory.path().join("data"))?,
            serde_json::to_string(&authority_path)?,
        ),
    )?;
    let bundle = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs/negotiate/releases/0.1.0/worldstream-negotiate-9033a1aa10ca37c301660b7427d79c4e71d7af59006bc7d51edc4b470c8c2db5.wspack").canonicalize()?;
    let original = fs::read(&bundle)?;
    assert_eq!(format!("blake3:{}", blake3::hash(&original)), BUNDLE_DIGEST);
    let bundle = bundle.to_str().ok_or("Bundle path is not UTF-8")?;
    let fixtures: Value =
        serde_json::from_str(include_str!("fixtures/cli-legacy/pack-receipts.json"))?;
    let receipts = &fixtures["receipts"];
    let inspected = receipt(directory.path(), &config, &["inspect", "--bundle", bundle])?;
    assert_eq!(inspected["bundle_digest"], BUNDLE_DIGEST);
    assert_eq!(inspected["revision_digest"], REVISION_DIGEST);
    check(receipts, "inspect", inspected);
    check(
        receipts,
        "approve",
        receipt(
            directory.path(),
            &config,
            &[
                "approve",
                "--bundle",
                bundle,
                "--operator-id",
                "test-operator",
                "--decided-at",
                "2026-08-30T12:00:00Z",
            ],
        )?,
    );
    check(
        receipts,
        "install",
        receipt(
            directory.path(),
            &config,
            &[
                "install",
                "--bundle",
                bundle,
                "--installed-at",
                "2026-08-30T12:00:01Z",
            ],
        )?,
    );
    check(
        receipts,
        "retained_inventory",
        receipt(directory.path(), &config, &["inventory"])?,
    );
    check(
        receipts,
        "set_selectable",
        receipt(
            directory.path(),
            &config,
            &[
                "set-selectable",
                "--bundle-digest",
                BUNDLE_DIGEST,
                "--selectable",
                "true",
            ],
        )?,
    );
    check(
        receipts,
        "selectable_inventory",
        receipt(directory.path(), &config, &["inventory"])?,
    );
    let mut readiness = receipt(directory.path(), &config, &["restart-readiness"])?;
    let binding = readiness["deployment_binding"]
        .as_str()
        .ok_or("readiness binding is missing")?;
    let hex = binding
        .strip_prefix("blake3:")
        .ok_or("readiness binding is not BLAKE3")?;
    assert_eq!(hex.len(), 64);
    assert!(
        hex.bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    );
    readiness["deployment_binding"] = Value::String("<temporary-deployment-binding>".to_owned());
    check(receipts, "restart_readiness", readiness);
    let exports = worldstream_runtime::prepare_data_directory(&directory.path().join("exports"))?;
    let exported = exports.join("exported.wspack");
    check(
        receipts,
        "export",
        receipt(
            directory.path(),
            &config,
            &[
                "export",
                "--bundle-digest",
                BUNDLE_DIGEST,
                "--output",
                exported.to_str().ok_or("Export path is not UTF-8")?,
            ],
        )?,
    );
    assert_eq!(
        fs::read(&exported)?,
        original,
        "export retains exact original Bundle bytes"
    );
    check(
        receipts,
        "revoke",
        receipt(
            directory.path(),
            &config,
            &[
                "revoke",
                "--bundle-digest",
                BUNDLE_DIGEST,
                "--operator-id",
                "test-operator",
                "--decided-at",
                "2026-08-30T12:00:02Z",
            ],
        )?,
    );
    check(
        receipts,
        "revoked_inventory",
        receipt(directory.path(), &config, &["inventory"])?,
    );
    Ok(())
}
