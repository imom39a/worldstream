//! Narrow native development admission. This never emits release qualification:
//! one exact executable, read-only profile, working area and model/effort pair
//! are admitted only after the referenced native receipts are reverified.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use super::provider::CODEX_APP_SERVER_CONTRACT;
use super::{
    NativeProviderProbe, ProviderCapabilities, ProviderKind, ProviderQualificationBinding,
    ProviderRegistry, QualifiedSelection,
};

/// A disposable, read-only native trial profile, never portable approval.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LocalCodexProfile {
    pub profile_root: PathBuf,
    pub config_sha256: String,
    pub working_area: PathBuf,
    pub expires_at: u64,
    pub guard_sha256: String,
}

impl LocalCodexProfile {
    pub(crate) fn validate_guard(&self, guard: &Path) -> Result<(), String> {
        if sha256(guard)? != self.guard_sha256 {
            return Err("native trial guard differs from its measured executable".to_owned());
        }
        Ok(())
    }
    pub(crate) fn validate(&self, working_area: &Path) -> Result<(), String> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_secs();
        if now >= self.expires_at
            || self.expires_at > now + 86400
            || fs::canonicalize(working_area).map_err(|e| e.to_string())? != self.working_area
            || fs::canonicalize(&self.profile_root).map_err(|e| e.to_string())? != self.profile_root
            || self.profile_root.starts_with(&self.working_area)
        {
            return Err("local Codex trial scope is invalid or expired".to_owned());
        }
        let config = self.profile_root.join("config.toml");
        worldstream_runtime::validate_owner_only_file(&config).map_err(|e| e.to_string())?;
        if sha256(&config)? != self.config_sha256 {
            return Err("local Codex trial configuration changed".to_owned());
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiptRef {
    path: String,
    sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Evidence {
    schema: String,
    executable: PathBuf,
    executable_sha256: String,
    guard: PathBuf,
    guard_sha256: String,
    model: String,
    effort: String,
    profile: LocalCodexProfile,
    isolation: ReceiptRef,
    fresh: ReceiptRef,
    resumed: ReceiptRef,
    cancellation: ReceiptRef,
    owner_loss: ReceiptRef,
}

/// Loads a native trial receipt graph for the current local host. The normal
/// release qualification loader and multi-platform release gate are unchanged.
///
/// # Errors
/// Rejects missing, drifted, unsafe, incomplete, expired or substituted evidence.
pub fn load_local_codex(path: &Path) -> Result<ProviderCapabilities, String> {
    worldstream_runtime::validate_owner_only_file(path).map_err(|e| e.to_string())?;
    let bytes = read_bounded(path)?;
    let evidence: Evidence = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if evidence.schema != "worldstream/codex-native-local-admission@1"
        || evidence.model.is_empty()
        || evidence.effort.is_empty()
        || sha256(&evidence.executable)? != evidence.executable_sha256
        || sha256(&evidence.guard)? != evidence.guard_sha256
    {
        return Err("native local admission identity does not match".to_owned());
    }
    evidence.profile.validate(&evidence.profile.working_area)?;
    if evidence.profile.guard_sha256 != evidence.guard_sha256 {
        return Err("native trial guard binding differs".to_owned());
    }
    let parent = path.parent().ok_or("missing evidence parent")?;
    let read = |reference: &ReceiptRef| -> Result<Value, String> {
        if reference.path.is_empty()
            || Path::new(&reference.path).components().count() != 1
            || !matches!(
                Path::new(&reference.path).components().next(),
                Some(std::path::Component::Normal(_))
            )
        {
            return Err("invalid native receipt path".to_owned());
        }
        let path = parent.join(&reference.path);
        worldstream_runtime::validate_owner_only_file(&path).map_err(|e| e.to_string())?;
        if sha256(&path)? != reference.sha256 {
            return Err("native receipt hash mismatch".to_owned());
        }
        serde_json::from_slice(&read_bounded(&path)?).map_err(|e| e.to_string())
    };
    validate_isolation(&evidence, &read(&evidence.isolation)?)?;
    validate_turns(
        &evidence,
        &read(&evidence.fresh)?,
        &read(&evidence.resumed)?,
    )?;
    for (reference, mode) in [
        (&evidence.cancellation, "cancellation"),
        (&evidence.owner_loss, "owner_loss"),
    ] {
        validate_cancellation(&evidence, &read(reference)?, mode)?;
    }
    let inspection = ProviderRegistry::new().inspect(
        ProviderKind::Codex,
        &NativeProviderProbe::default(),
        &evidence.executable,
    );
    let mut capabilities = inspection
        .capabilities
        .ok_or("installed native Codex inspection failed")?;
    capabilities.explicit_model = true;
    capabilities.explicit_effort = true;
    capabilities.session_reuse = true;
    capabilities.reports_effective_configuration = true;
    capabilities.delegation_contained = true;
    capabilities.native_cancellation_qualified = true;
    capabilities.resource_confinement_qualified = true;
    capabilities.qualification = Some(ProviderQualificationBinding {
        provider: ProviderKind::Codex,
        executable_digest: capabilities.executable_digest.clone(),
        version: capabilities.version.clone(),
        evidence_sha256: format!("{:x}", Sha256::digest(&bytes)),
        operating_system: std::env::consts::OS.to_owned(),
        architecture: std::env::consts::ARCH.to_owned(),
        resource_confinement_qualified: true,
        invocation_contract: Some(CODEX_APP_SERVER_CONTRACT.to_owned()),
        local_codex_profile: Some(evidence.profile),
        selections: vec![QualifiedSelection {
            model: evidence.model,
            effort: Some(evidence.effort),
        }],
    });
    Ok(capabilities)
}

fn validate_isolation(evidence: &Evidence, isolation: &Value) -> Result<(), String> {
    if isolation["account_type"] != "chatgpt"
        || isolation["mcp_server_count"] != 0
        || isolation["configuration_sha256"] != evidence.profile.config_sha256
        || isolation["working_area"] != evidence.profile.working_area.to_string_lossy().as_ref()
        || isolation["executable_sha256"] != evidence.executable_sha256
        || isolation["inside_read"]["stdout"] != "SYNTHETIC_INSIDE"
        || isolation["inside_read"]["exitCode"] != 0
        || isolation["network_baseline"]["stdout"] != "SYNTHETIC_NETWORK"
        || isolation["network_baseline"]["exit_code"] != 0
        || isolation["thread_configuration"]["activePermissionProfile"]["id"]
            != "worldstream_swarm_read"
        || isolation["thread_configuration"]["instructionSources"] != json!([])
    {
        return Err("native profile isolation evidence is incomplete".to_owned());
    }
    for probe in ["outside_read", "outside_write", "inside_write", "network"] {
        if isolation[probe]["exitCode"]
            .as_i64()
            .is_none_or(|code| code == 0)
            || isolation[probe]["stdout"]
                .as_str()
                .is_none_or(|value| !value.is_empty())
        {
            return Err(format!("native denial probe {probe} did not fail closed"));
        }
        if probe != "network"
            && isolation[probe]["stderr"].as_str().is_none_or(|message| {
                !message.contains("Operation not permitted")
                    && !message.contains("Permission denied")
            })
        {
            return Err("filesystem probe did not record an OS denial".to_owned());
        }
    }
    Ok(())
}

fn validate_turns(evidence: &Evidence, fresh: &Value, resumed: &Value) -> Result<(), String> {
    if fresh["observation"]["session_mode"] != "fresh"
        || resumed["observation"]["session_mode"] != "resume"
        || fresh["transcript"]["turn_id"] == resumed["transcript"]["turn_id"]
    {
        return Err("native fresh/resume receipts do not describe distinct turns".to_owned());
    }
    for receipt in [&fresh, &resumed] {
        let configuration = &receipt["transcript"]["configuration"];
        let transcript = &receipt["transcript"];
        if receipt["transcript"]["schema"] != "worldstream/codex-app-server-turn@1"
            || receipt["executable_sha256"] != evidence.executable_sha256
            || receipt["guard_sha256"] != evidence.guard_sha256
            || receipt["configuration_sha256"] != evidence.profile.config_sha256
            || configuration["model"] != evidence.model
            || configuration["effort"] != evidence.effort
            || configuration["activePermissionProfile"]["id"] != "worldstream_swarm_read"
            || configuration["instructionSources"] != json!([])
            || configuration["runtimeWorkspaceRoots"] != json!([evidence.profile.working_area])
            || transcript["turn_id"].as_str().is_none_or(str::is_empty)
            || transcript["completion"]["turn"]["status"] != "completed"
            || transcript["completion"]["turn"]["error"] != Value::Null
            || transcript["completion"]["turn"]["id"] != transcript["turn_id"]
            || transcript["completion"]["threadId"] != configuration["thread_id"]
            || receipt["observation"]["exit_code"] != 0
            || receipt["observation"]["remaining_observed_live_process_count"] != 0
            || receipt["observation"]["observed_process_groups_reaped"] != true
        {
            return Err(
                "native fresh/resumed turn did not establish the selected contract".to_owned(),
            );
        }
        if receipt["transcript"]["observed_item_types"]
            .as_array()
            .is_none_or(|items| {
                items.iter().any(|item| {
                    !matches!(
                        item.as_str(),
                        Some("userMessage" | "agentMessage" | "reasoning")
                    )
                })
            })
        {
            return Err("native probe used an unallowed tool".to_owned());
        }
    }
    if fresh["transcript"]["configuration"]["thread_id"]
        .as_str()
        .is_none_or(str::is_empty)
        || fresh["transcript"]["configuration"]["thread_id"]
            != resumed["transcript"]["configuration"]["thread_id"]
    {
        return Err("native resume did not preserve its conversation".to_owned());
    }
    Ok(())
}

fn validate_cancellation(evidence: &Evidence, receipt: &Value, mode: &str) -> Result<(), String> {
    if receipt["guard_sha256"] != evidence.guard_sha256
        || receipt["executable_sha256"] != evidence.executable_sha256
        || receipt["configuration_sha256"] != evidence.profile.config_sha256
        || receipt["guard_ready"] != true
        || receipt["running_provider_observed"] != true
        || receipt["remaining_live_process_count"] != 0
        || receipt["exit_code"].as_i64().is_none_or(|code| code == 0)
        || receipt["mode"] != mode
    {
        return Err("native owned-process cleanup was not established".to_owned());
    }
    let guard_pid = receipt["guard_pid"]
        .as_u64()
        .ok_or("missing native guard identity")?;
    let before = receipt["observed_before"]
        .as_object()
        .ok_or("missing pre-cancellation process census")?;
    let after = receipt["observed_after"]
        .as_object()
        .ok_or("missing post-cancellation process census")?;
    if !before
        .values()
        .any(|row| row.get(0).and_then(Value::as_u64) == Some(guard_pid))
        || after.values().any(|row| {
            row.get(2)
                .and_then(Value::as_str)
                .is_none_or(|state| !state.starts_with('Z'))
        })
    {
        return Err("native process census does not prove owned descendant cleanup".to_owned());
    }
    Ok(())
}

fn read_bounded(path: &Path) -> Result<Vec<u8>, String> {
    if fs::metadata(path).map_err(|e| e.to_string())?.len() > 4 * 1024 * 1024 {
        return Err("native receipt is oversized".to_owned());
    }
    fs::read(path).map_err(|e| e.to_string())
}

fn sha256(path: &Path) -> Result<String, String> {
    use std::io::Read as _;
    let mut file = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 65536].into_boxed_slice();
    loop {
        let count = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;

    #[test]
    fn native_turn_receipts_require_distinct_completed_turns_and_exact_selection()
    -> Result<(), Box<dyn std::error::Error>> {
        // These are loader regression fixtures, never native admission files.
        let reference = json!({"path":"fixture.json","sha256":"synthetic"});
        let evidence: Evidence = serde_json::from_value(json!({
            "schema":"worldstream/codex-native-local-admission@1",
            "executable":"/synthetic/codex","executable_sha256":"native",
            "guard":"/synthetic/guard","guard_sha256":"guard",
            "model":"selected-model","effort":"medium",
            "profile":{"profile_root":"/synthetic/profile","working_area":"/synthetic/work",
                "config_sha256":"configuration","guard_sha256":"guard","expires_at":0},
            "isolation":reference,"fresh":reference,"resumed":reference,
            "cancellation":reference,"owner_loss":reference
        }))?;
        let turn = |id: &str, mode: &str| {
            json!({
                "executable_sha256":"native","guard_sha256":"guard","configuration_sha256":"configuration",
                "transcript":{"schema":"worldstream/codex-app-server-turn@1","turn_id":id,
                    "configuration":{"model":"selected-model","effort":"medium","thread_id":"same-thread",
                        "activePermissionProfile":{"id":"worldstream_swarm_read"},"instructionSources":[],
                        "runtimeWorkspaceRoots":["/synthetic/work"]},
                    "completion":{"threadId":"same-thread","turn":{"id":id,"status":"completed","error":null}},
                    "observed_item_types":["agentMessage"]},
                "observation":{"exit_code":0,"session_mode":mode,"remaining_observed_live_process_count":0,
                    "observed_process_groups_reaped":true}
            })
        };
        let fresh = turn("turn-1", "fresh");
        let resumed = turn("turn-2", "resume");
        assert!(validate_turns(&evidence, &fresh, &resumed).is_ok());
        assert!(validate_turns(&evidence, &fresh, &fresh).is_err());
        for (pointer, substituted) in [
            ("/transcript/configuration/model", json!("other-model")),
            (
                "/transcript/configuration/instructionSources",
                json!(["ambient"]),
            ),
            (
                "/transcript/configuration/runtimeWorkspaceRoots",
                json!(["/elsewhere"]),
            ),
            ("/transcript/completion/turn/status", json!("failed")),
            ("/transcript/completion/turn/id", json!("another-turn")),
            (
                "/transcript/observed_item_types",
                json!(["commandExecution"]),
            ),
            (
                "/observation/remaining_observed_live_process_count",
                json!(1),
            ),
        ] {
            let mut changed = resumed.clone();
            *changed.pointer_mut(pointer).ok_or("bad fixture pointer")? = substituted;
            assert!(
                validate_turns(&evidence, &fresh, &changed).is_err(),
                "{pointer}"
            );
        }
        Ok(())
    }

    #[test]
    fn trial_profile_rejects_scope_drift_expiry_and_changed_configuration()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempfile::tempdir()?;
        let root = temporary.path().canonicalize()?;
        let work = root.join("work");
        let profile_root = root.join("profile");
        fs::create_dir(&work)?;
        fs::create_dir(&profile_root)?;
        let config = profile_root.join("config.toml");
        fs::write(&config, b"synthetic test configuration")?;
        fs::set_permissions(&config, fs::Permissions::from_mode(0o600))?;
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let profile = LocalCodexProfile {
            profile_root,
            config_sha256: sha256(&config)?,
            working_area: work.clone(),
            expires_at: now + 3600,
            guard_sha256: "unused in profile-only test".to_owned(),
        };
        assert!(profile.validate(&work).is_ok());
        assert!(profile.validate(&root).is_err());
        let mut changed = profile.clone();
        changed.expires_at = now;
        assert!(changed.validate(&work).is_err());
        changed.expires_at = now + 172_800;
        assert!(changed.validate(&work).is_err());
        changed = profile.clone();
        changed.profile_root = work.clone();
        assert!(changed.validate(&work).is_err());
        fs::write(&config, b"changed configuration")?;
        assert!(profile.validate(&work).is_err());
        Ok(())
    }

    #[test]
    fn trial_profile_rejects_changed_native_guard() -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempfile::tempdir()?;
        let guard = temporary.path().join("guard");
        fs::write(&guard, b"synthetic executable identity")?;
        let profile = LocalCodexProfile {
            profile_root: temporary.path().to_path_buf(),
            config_sha256: String::new(),
            working_area: temporary.path().to_path_buf(),
            expires_at: 0,
            guard_sha256: sha256(&guard)?,
        };
        assert!(profile.validate_guard(&guard).is_ok());
        fs::write(&guard, b"substituted executable identity")?;
        assert!(profile.validate_guard(&guard).is_err());
        Ok(())
    }
}
