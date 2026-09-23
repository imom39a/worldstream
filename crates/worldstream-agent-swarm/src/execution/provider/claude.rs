use std::{collections::BTreeMap, path::Path};

use serde_json::Value;

use super::{
    InvocationOutput, InvocationRequest, OutputContract, PreparedInvocation, ProviderAdapter,
    ProviderCapabilities, ProviderError, ProviderInspection, ProviderKind, ProviderProbe,
    ResourcePolicy, SessionSelection, base_prepared, direct_api_environment, inspect_contract,
};

/// Installed Claude Code CLI adapter.
#[derive(Clone, Copy, Debug, Default)]
pub struct ClaudeAdapter;

impl ProviderAdapter for ClaudeAdapter {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Claude
    }

    fn inspect(&self, probe: &dyn ProviderProbe, executable: &Path) -> ProviderInspection {
        inspect_contract(
            ProviderKind::Claude,
            probe,
            executable,
            &["--version"],
            &["--help"],
            |version, help| {
                let version = version
                    .lines()
                    .find_map(|line| line.trim().strip_suffix(" (Claude Code)"))?
                    .trim()
                    .to_owned();
                Some((
                    version,
                    help.contains("--model <model>"),
                    help.contains("--effort <level>"),
                    help.contains("--session-id <uuid>") && help.contains("--resume"),
                ))
            },
        )
    }

    fn prepare(
        &self,
        request: &InvocationRequest,
        capabilities: &ProviderCapabilities,
    ) -> Result<PreparedInvocation, ProviderError> {
        request.validate()?;
        let mut arguments = vec![
            "--print".to_owned(),
            "--output-format".to_owned(),
            "stream-json".to_owned(),
            "--verbose".to_owned(),
            "--model".to_owned(),
            request.model.clone(),
            "--setting-sources".to_owned(),
            String::new(),
            "--settings".to_owned(),
            "{}".to_owned(),
            "--strict-mcp-config".to_owned(),
            "--disable-slash-commands".to_owned(),
            "--no-chrome".to_owned(),
            "--permission-mode".to_owned(),
            permission_mode(request.resource_policy).to_owned(),
        ];
        if let Some(effort) = &request.effort {
            arguments.push("--effort".to_owned());
            arguments.push(effort.clone());
        }
        match &request.session {
            SessionSelection::Fresh {
                requested_id: Some(session_id),
            } => {
                if !is_uuid(session_id) {
                    return Err(ProviderError::InvalidRequest);
                }
                arguments.push("--session-id".to_owned());
                arguments.push(session_id.clone());
            }
            SessionSelection::Fresh { requested_id: None } => {}
            SessionSelection::Resume { session_id } => {
                if !capabilities.session_reuse {
                    return Err(ProviderError::Unsupported);
                }
                arguments.push("--resume".to_owned());
                arguments.push(session_id.clone());
            }
        }
        arguments.push("--tools".to_owned());
        arguments.push(request.allowed_tools.join(","));
        // Keep this last because Claude accepts a variable number of values.
        arguments.push("--disallowedTools".to_owned());
        arguments.push("Agent,Task".to_owned());
        let mut environment_remove = direct_api_environment();
        // Claude documents this environment value as taking precedence over
        // every interactive, flag, and settings effort selection. Thinking
        // toggles can likewise defeat the selected effort. Removing them only
        // for the owned child keeps concurrent members isolated without
        // modifying the user's shell or global preferences.
        environment_remove.extend(
            [
                "ANTHROPIC_DEFAULT_HAIKU_MODEL",
                "ANTHROPIC_DEFAULT_OPUS_MODEL",
                "ANTHROPIC_DEFAULT_SONNET_MODEL",
                "ANTHROPIC_MODEL",
                "CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING",
                "CLAUDE_CODE_DISABLE_THINKING",
                "CLAUDE_CODE_EFFORT_LEVEL",
                "CLAUDE_CODE_SUBAGENT_MODEL",
                "MAX_THINKING_TOKENS",
            ]
            .into_iter()
            .map(str::to_owned),
        );
        let mut environment_set = BTreeMap::new();
        environment_set.insert("NO_COLOR".to_owned(), "1".to_owned());
        base_prepared(
            ProviderKind::Claude,
            request,
            capabilities,
            arguments,
            environment_remove,
            environment_set,
            OutputContract::ClaudeStreamJson,
        )
    }

    fn decode(
        &self,
        prepared: &PreparedInvocation,
        stdout: &[u8],
    ) -> Result<InvocationOutput, ProviderError> {
        if prepared.output_contract != OutputContract::ClaudeStreamJson {
            return Err(ProviderError::InvalidOutput);
        }
        let text = std::str::from_utf8(stdout).map_err(|_| ProviderError::InvalidOutput)?;
        let mut reported_model = None;
        let mut reported_effort = None;
        let mut session_id = None;
        let mut result = None;
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let event: Value =
                serde_json::from_str(line).map_err(|_| ProviderError::InvalidOutput)?;
            match event.get("type").and_then(Value::as_str) {
                Some("system") if event.get("subtype").and_then(Value::as_str) == Some("init") => {
                    reported_model = event
                        .get("model")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    reported_effort = optional_string(&event, "effort")?;
                    session_id = event
                        .get("session_id")
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
                Some("result") => {
                    result = event
                        .get("result")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    if session_id.is_none() {
                        session_id = event
                            .get("session_id")
                            .and_then(Value::as_str)
                            .map(str::to_owned);
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
            text: result.unwrap_or_default(),
        })
    }
}

const fn permission_mode(policy: ResourcePolicy) -> &'static str {
    match policy {
        ResourcePolicy::ReadOnly => "plan",
        ResourcePolicy::WorkspaceWrite => "dontAsk",
    }
}

fn optional_string(value: &Value, key: &str) -> Result<Option<String>, ProviderError> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(ProviderError::InvalidOutput),
    }
}

fn is_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_hexdigit(),
        })
}
