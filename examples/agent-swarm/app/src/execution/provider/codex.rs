use std::{collections::BTreeMap, path::Path};

use serde_json::Value;

use super::{
    InvocationOutput, InvocationRequest, OutputContract, PreparedInvocation, ProviderAdapter,
    ProviderCapabilities, ProviderError, ProviderInspection, ProviderKind, ProviderProbe,
    SessionSelection, base_prepared, codex_app_server, direct_api_environment, inspect_contract,
};

/// Installed Codex CLI adapter.
#[derive(Clone, Copy, Debug, Default)]
pub struct CodexAdapter;

impl ProviderAdapter for CodexAdapter {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Codex
    }

    fn inspect(&self, probe: &dyn ProviderProbe, executable: &Path) -> ProviderInspection {
        inspect_contract(
            ProviderKind::Codex,
            probe,
            executable,
            &["--version"],
            &["app-server", "--help"],
            |version, help| {
                let version = version
                    .lines()
                    .find_map(|line| line.trim().strip_prefix("codex-cli "))?
                    .trim()
                    .to_owned();
                let explicit_model = help.contains("--config <key=value>");
                // This key is not promised by generic `--config`; qualification
                // must retain help/schema evidence that names it exactly.
                let explicit_effort = help.contains("model_reasoning_effort");
                // Help establishes only transport availability. Exact schema and
                // session/model/effort controls still require qualification.
                let session_reuse = help.contains("--stdio");
                Some((version, explicit_model, explicit_effort, session_reuse))
            },
        )
    }

    fn prepare(
        &self,
        request: &InvocationRequest,
        capabilities: &ProviderCapabilities,
    ) -> Result<PreparedInvocation, ProviderError> {
        request.validate()?;
        // Codex exposes an invocation sandbox but no exact named-tool
        // allow-list. Empty therefore means its qualified minimal built-in
        // surface under the selected sandbox. Never silently broaden a
        // caller's non-empty named-tool policy.
        if !request.allowed_tools.is_empty() {
            return Err(ProviderError::ToolPolicyUnavailable);
        }
        if matches!(
            request.session,
            SessionSelection::Fresh {
                requested_id: Some(_)
            }
        ) {
            return Err(ProviderError::Unsupported);
        }
        if matches!(request.session, SessionSelection::Resume { .. }) && !capabilities.session_reuse
        {
            return Err(ProviderError::Unsupported);
        }
        let mut arguments = vec![
            "app-server".to_owned(),
            "--stdio".to_owned(),
            "--strict-config".to_owned(),
        ];
        for value in [
            "model_provider=\"openai\"".to_owned(),
            "forced_login_method=\"chatgpt\"".to_owned(),
            "approval_policy=\"never\"".to_owned(),
            "features.multi_agent=false".to_owned(),
            "web_search=\"disabled\"".to_owned(),
        ] {
            arguments.extend(["--config".to_owned(), value]);
        }
        // app-server has no --ignore-user-config in the installed contract.
        // These overrides do not establish isolation; admission remains
        // blocked until the exact invocation's native evidence proves it.
        add_effort(&mut arguments, request.effort.as_deref());
        let mut environment_set = BTreeMap::new();
        environment_set.insert("NO_COLOR".to_owned(), "1".to_owned());
        let mut prepared = base_prepared(
            ProviderKind::Codex,
            request,
            capabilities,
            arguments,
            direct_api_environment(),
            environment_set,
            OutputContract::CodexAppServer,
        )?;
        prepared.stdin = codex_app_server::Request::encode(request)?;
        if let Some(profile) = capabilities
            .qualification
            .as_ref()
            .and_then(|binding| binding.local_codex_profile.as_ref())
        {
            if request.resource_policy != super::ResourcePolicy::ReadOnly {
                return Err(ProviderError::ToolPolicyUnavailable);
            }
            profile
                .validate(&request.working_area)
                .map_err(|_| ProviderError::InvalidRequest)?;
            prepared.environment_set.insert(
                "CODEX_HOME".to_owned(),
                profile.profile_root.to_string_lossy().into_owned(),
            );
            let mut encoded: Value =
                serde_json::from_str(&prepared.stdin).map_err(|_| ProviderError::InvalidRequest)?;
            encoded["permission_profile"] = serde_json::json!("worldstream_swarm_read");
            prepared.stdin =
                serde_json::to_string(&encoded).map_err(|_| ProviderError::InvalidRequest)?;
        }
        Ok(prepared)
    }

    fn decode(
        &self,
        prepared: &PreparedInvocation,
        stdout: &[u8],
    ) -> Result<InvocationOutput, ProviderError> {
        if prepared.output_contract == OutputContract::CodexAppServer {
            return codex_app_server::decode(prepared, stdout);
        }
        if prepared.output_contract != OutputContract::CodexJsonLines {
            return Err(ProviderError::InvalidOutput);
        }
        let text = std::str::from_utf8(stdout).map_err(|_| ProviderError::InvalidOutput)?;
        let mut reported_model = None;
        let mut reported_effort = None;
        let mut session_id = None;
        let mut messages = Vec::new();
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let event: Value =
                serde_json::from_str(line).map_err(|_| ProviderError::InvalidOutput)?;
            match event.get("type").and_then(Value::as_str) {
                Some("thread.started") => {
                    session_id = event
                        .get("thread_id")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                }
                Some("configuration" | "provider.configuration") => {
                    reported_model = event
                        .get("model")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    reported_effort = optional_string(&event, "effort")?;
                }
                Some("item.completed") => {
                    if let Some(message) = event
                        .get("item")
                        .filter(|item| {
                            item.get("type").and_then(Value::as_str) == Some("agent_message")
                        })
                        .and_then(|item| item.get("text"))
                        .and_then(Value::as_str)
                    {
                        messages.push(message.to_owned());
                    }
                }
                _ => {}
            }
        }
        let reported_model = reported_model.ok_or(ProviderError::InvalidOutput)?;
        if prepared.requested_effort.is_some() && reported_effort.is_none() {
            return Err(ProviderError::InvalidOutput);
        }
        Ok(InvocationOutput {
            invocation_id: prepared.invocation_id.clone(),
            reported_model,
            reported_effort,
            session_id,
            text: messages.join("\n"),
        })
    }
}

fn add_effort(arguments: &mut Vec<String>, effort: Option<&str>) {
    if let Some(effort) = effort {
        let quoted = serde_json::to_string(effort).unwrap_or_else(|_| "\"\"".to_owned());
        arguments.push("--config".to_owned());
        arguments.push(format!("model_reasoning_effort={quoted}"));
    }
}

fn optional_string(value: &Value, key: &str) -> Result<Option<String>, ProviderError> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(ProviderError::InvalidOutput),
    }
}
