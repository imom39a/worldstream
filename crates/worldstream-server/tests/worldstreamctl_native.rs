use std::process::{Command, Stdio};

#[test]
fn packaged_control_hidden_native_worker_fails_closed_outside_supervision() {
    for marker in [
        "--worldstream-native-postgres-admission-v1",
        "--worldstream-native-postgres-worker-v1",
        "--worldstream-native-postgres-repair-v1",
        "--worldstream-native-postgres-commit-v1",
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_worldstreamctl"))
            .arg(marker)
            .env_remove("WORLDSTREAM_NATIVE_POSTGRES_CONTAINED_V1")
            .stdin(Stdio::null())
            .output()
            .unwrap_or_else(|error| unreachable!("packaged worldstreamctl: {error}"));
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8(output.stderr)
            .unwrap_or_else(|error| unreachable!("classified stderr: {error}"));
        assert!(stderr.contains("failed closed"));
        assert!(!stderr.contains("password"));
        assert!(!stderr.contains("postgresql://"));
    }
}

#[test]
fn packaged_control_reports_canonical_native_artifact_directory_identity() {
    let parent =
        tempfile::tempdir().unwrap_or_else(|error| unreachable!("temporary directory: {error}"));
    let protected = parent.path().join("native-artifacts");
    worldstream_runtime::prepare_data_directory(&protected)
        .unwrap_or_else(|error| unreachable!("protected artifact directory: {error}"));
    let output = Command::new(env!("CARGO_BIN_EXE_worldstreamctl"))
        .args(["postgres", "native", "directory-identity", "--path"])
        .arg(&protected)
        .output()
        .unwrap_or_else(|error| unreachable!("packaged worldstreamctl: {error}"));
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let receipt: serde_json::Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| unreachable!("directory identity receipt: {error}"));
    assert_eq!(
        receipt.get("schema").and_then(serde_json::Value::as_str),
        Some("worldstream/postgres-native-artifact-directory-identity/v1")
    );
    assert_eq!(
        receipt.get("status").and_then(serde_json::Value::as_str),
        Some("observed")
    );
    assert_eq!(
        receipt
            .get("secrets_emitted")
            .and_then(serde_json::Value::as_bool),
        Some(false)
    );
    let identity = receipt
        .get("artifact_directory_identity")
        .and_then(serde_json::Value::as_object)
        .unwrap_or_else(|| unreachable!("artifact identity object"));
    let storage_id = identity
        .get("storage_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| unreachable!("storage identity"));
    let file_id = identity
        .get("file_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| unreachable!("file identity"));
    assert_eq!(storage_id.len(), 16);
    assert_eq!(file_id.len(), 32);
    assert!(
        storage_id
            .bytes()
            .chain(file_id.bytes())
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;

        let metadata = std::fs::metadata(&protected)
            .unwrap_or_else(|error| unreachable!("artifact metadata: {error}"));
        assert_eq!(storage_id, format!("{:016x}", metadata.dev()));
        assert_eq!(file_id, format!("{:032x}", u128::from(metadata.ino())));
    }
}
