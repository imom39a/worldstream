//! One bounded Codex app-server turn inside the native owned-process guard.
//!
//! This protocol support is not provider qualification. In particular, the
//! installed CLI must separately prove configuration isolation, tool and
//! resource confinement, and native cancellation before admission.

use std::{
    collections::{BTreeSet, VecDeque},
    io::{BufRead, Read, Write},
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{
    InvocationOutput, InvocationRequest, PreparedInvocation, ProviderError, ResourcePolicy,
    SessionSelection,
};

const MAX_FRAME_BYTES: usize = 1024 * 1024;
const MAX_TRANSCRIPT_BYTES: usize = 4 * 1024 * 1024;
const MAX_WIRE_BYTES: usize = 32 * 1024 * 1024;
const TRANSCRIPT_SCHEMA: &str = "worldstream/codex-app-server-turn@1";

/// The guard receives only this bounded, non-secret application request. It
/// retains provider stdin until the protocol reaches a terminal turn.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Request {
    model: String,
    effort: String,
    cwd: String,
    resource_policy: ResourcePolicy,
    session: SessionSelection,
    prompt: String,
    /// Explicitly selected named profile for bounded native evaluation. Normal
    /// provider admission still requires its separately verified qualification.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    permission_profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    output_schema: Option<Value>,
}

impl Request {
    pub(crate) fn encode(request: &InvocationRequest) -> Result<String, ProviderError> {
        serde_json::to_string(&Self {
            model: request.model.clone(),
            effort: request
                .effort
                .clone()
                .ok_or(ProviderError::InvalidRequest)?,
            cwd: request
                .working_area
                .canonicalize()
                .map_err(|_| ProviderError::InvalidRequest)?
                .to_string_lossy()
                .into_owned(),
            resource_policy: request.resource_policy,
            session: request.session.clone(),
            prompt: request.prompt.clone(),
            permission_profile: None,
            output_schema: None,
        })
        .map_err(|_| ProviderError::InvalidRequest)
    }

    pub(crate) fn parse(encoded: &str) -> Result<Self, ProviderError> {
        let request: Self =
            serde_json::from_str(encoded).map_err(|_| ProviderError::InvalidRequest)?;
        if request.model.is_empty()
            || request.model.len() > 256
            || request.effort.is_empty()
            || request.effort.len() > 256
            || request.prompt.len() > super::MAX_PROMPT_BYTES
            || !std::path::Path::new(&request.cwd).is_absolute()
            || request
                .permission_profile
                .as_deref()
                .is_some_and(|profile| {
                    profile != "worldstream_swarm_read"
                        || request.resource_policy != ResourcePolicy::ReadOnly
                })
            || request
                .output_schema
                .as_ref()
                .is_some_and(|schema| !schema.is_object())
            || matches!(
                request.session,
                SessionSelection::Fresh {
                    requested_id: Some(_)
                }
            )
        {
            return Err(ProviderError::InvalidRequest);
        }
        Ok(request)
    }

    pub(crate) fn validate_prepared(prepared: &PreparedInvocation) -> Result<(), ProviderError> {
        let request = Self::parse(&prepared.stdin)?;
        let canonical = prepared
            .working_area
            .canonicalize()
            .map_err(|_| ProviderError::InvalidRequest)?;
        if let Some(profile) = prepared
            .qualification
            .as_ref()
            .and_then(|binding| binding.local_codex_profile.as_ref())
        {
            profile
                .validate(&canonical)
                .map_err(|_| ProviderError::InvalidRequest)?;
            if request.permission_profile.as_deref() != Some("worldstream_swarm_read")
                || prepared
                    .environment_set
                    .get("CODEX_HOME")
                    .map(String::as_str)
                    != profile.profile_root.to_str()
            {
                return Err(ProviderError::InvalidRequest);
            }
        }
        if request.model != prepared.requested_model
            || Some(request.effort.as_str()) != prepared.requested_effort.as_deref()
            || request.session != prepared.requested_session
            || std::path::Path::new(&request.cwd) != canonical
        {
            return Err(ProviderError::InvalidRequest);
        }
        Ok(())
    }

    fn thread_request(&self) -> Value {
        let mut params = json!({
            "model": self.model,
            "modelProvider": "openai",
            "cwd": self.cwd,
            "approvalPolicy": "never",
            "sandbox": match self.resource_policy {
                ResourcePolicy::ReadOnly => "read-only",
                ResourcePolicy::WorkspaceWrite => "workspace-write",
            },
            "config": {"model_reasoning_effort": self.effort,
                "sandbox_workspace_write": {"writable_roots": [self.cwd],
                    "network_access": false, "exclude_tmpdir_env_var": true,
                    "exclude_slash_tmp": true}},
            "runtimeWorkspaceRoots": [self.cwd],
        });
        let method = match &self.session {
            SessionSelection::Fresh { .. } => {
                params["allowProviderModelFallback"] = json!(false);
                params["selectedCapabilityRoots"] = json!([]);
                "thread/start"
            }
            SessionSelection::Resume { session_id } => {
                params["threadId"] = json!(session_id);
                "thread/resume"
            }
        };
        if let Some(profile) = &self.permission_profile {
            if let Some(object) = params.as_object_mut() {
                object.remove("sandbox");
            }
            if let Some(config) = params["config"].as_object_mut() {
                config.remove("sandbox_workspace_write");
            }
            params["permissions"] = json!(profile);
        }
        json!({"id": 3, "method": method, "params": params})
    }

    fn turn_request(&self, thread_id: &str) -> Value {
        // These settings were just resolved and verified by thread/start or
        // thread/resume. Inherit them unchanged; a second override would have
        // no corresponding effective-configuration response to verify.
        let mut request = json!({"id": 4, "method": "turn/start", "params": {
            "threadId": thread_id,
            "input": [{"type": "text", "text": self.prompt, "text_elements": []}],
        }});
        if let Some(schema) = &self.output_schema {
            request["params"]["outputSchema"] = schema.clone();
        }
        request
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Transcript {
    schema: String,
    /// Selected fields from the provider response, never echoed arguments.
    configuration: Value,
    turn_id: String,
    completion: Value,
    messages: Vec<Value>,
    #[serde(default)]
    observed_item_types: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    timing: Option<Value>,
}

/// Drives the provider pipes. The caller must terminate/reap the owned process
/// group when this returns, including on success: app-server is long lived.
pub(crate) fn run(
    request: &Request,
    reader: impl Read,
    mut writer: impl Write,
    mut output: impl Write,
) -> Result<(), ProviderError> {
    let mut reader = std::io::BufReader::new(reader);
    let mut wire_bytes = 0_usize;
    send(
        &mut writer,
        &json!({"id": 1, "method": "initialize", "params": {
            "clientInfo": {"name": "worldstream_agent_swarm", "version": env!("CARGO_PKG_VERSION")},
            "capabilities": {"experimentalApi": true},
        }}),
    )?;
    response(&mut reader, 1, &mut wire_bytes)?;
    send(&mut writer, &json!({"method": "initialized"}))?;
    send(
        &mut writer,
        &json!({"id": 2, "method": "account/read", "params": {"refreshToken": false}}),
    )?;
    let (account, _) = response(&mut reader, 2, &mut wire_bytes)?;
    if account.pointer("/account/type").and_then(Value::as_str) != Some("chatgpt") {
        return Err(ProviderError::Unsupported);
    }
    // Do not retain account details (email, plan, identifiers) in the output.
    send(&mut writer, &request.thread_request())?;
    let (reported, _) = response(&mut reader, 3, &mut wire_bytes)?;
    let configuration = verified_configuration(request, &reported)?;
    let thread_id = required_string(&configuration, "thread_id")?;
    let turn_started_unix_ms = unix_milliseconds()?;
    send(&mut writer, &request.turn_request(thread_id))?;
    let (started, pending) = response(&mut reader, 4, &mut wire_bytes)?;
    let turn_id = started
        .pointer("/turn/id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.len() <= 256)
        .ok_or(ProviderError::InvalidOutput)?
        .to_owned();
    let (completion, messages, observed_item_types) = collect_turn(
        &mut reader,
        thread_id,
        &turn_id,
        request,
        &mut wire_bytes,
        pending,
    )?;
    let transcript = Transcript {
        schema: TRANSCRIPT_SCHEMA.to_owned(),
        configuration,
        turn_id,
        completion,
        messages,
        observed_item_types,
        timing: Some(json!({"turn_started_unix_ms":turn_started_unix_ms,
            "turn_completed_unix_ms":unix_milliseconds()?})),
    };
    let bytes = serde_json::to_vec(&transcript).map_err(|_| ProviderError::InvalidOutput)?;
    if bytes.len() > MAX_TRANSCRIPT_BYTES {
        return Err(ProviderError::InvalidOutput);
    }
    output
        .write_all(&bytes)
        .and_then(|()| output.flush())
        .map_err(|_| ProviderError::InvalidOutput)
}

fn unix_milliseconds() -> Result<u64, ProviderError> {
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| ProviderError::InvalidOutput)?;
    u64::try_from(elapsed.as_millis()).map_err(|_| ProviderError::InvalidOutput)
}

fn verified_configuration(request: &Request, reported: &Value) -> Result<Value, ProviderError> {
    let model = required_string(reported, "model")?;
    let effort = required_string(reported, "reasoningEffort")?;
    let thread = reported.get("thread").ok_or(ProviderError::InvalidOutput)?;
    let thread_id = required_string(thread, "id")?;
    let session_id = required_string(thread, "sessionId")?;
    if model != request.model
        || effort != request.effort
        || required_string(reported, "modelProvider")? != "openai"
        || required_string(reported, "cwd")? != request.cwd
        || reported.get("approvalPolicy").and_then(Value::as_str) != Some("never")
        || !sandbox_matches(request, reported.get("sandbox"))
        || request.permission_profile.as_ref().is_some_and(|profile| {
            reported
                .pointer("/activePermissionProfile/id")
                .and_then(Value::as_str)
                != Some(profile)
                || reported.get("instructionSources") != Some(&json!([]))
                || reported.get("runtimeWorkspaceRoots") != Some(&json!([request.cwd]))
        })
        || matches!(&request.session, SessionSelection::Resume { session_id } if session_id != thread_id)
    {
        return Err(ProviderError::InvalidOutput);
    }
    // SessionSelection stores the exact resumable thread identity. Preserve
    // the independently reported session-tree ID as evidence, never derive it.
    let mut configuration = json!({"model": model, "effort": effort, "thread_id": thread_id,
        "provider_session_id": session_id});
    if request.permission_profile.is_some() {
        for field in [
            "activePermissionProfile",
            "instructionSources",
            "runtimeWorkspaceRoots",
            "sandbox",
        ] {
            configuration[field] = reported[field].clone();
        }
    }
    Ok(configuration)
}

fn sandbox_matches(request: &Request, sandbox: Option<&Value>) -> bool {
    let Some(sandbox) = sandbox else {
        return false;
    };
    if sandbox.get("networkAccess").and_then(Value::as_bool) != Some(false) {
        return false;
    }
    match request.resource_policy {
        ResourcePolicy::ReadOnly => sandbox.get("type").and_then(Value::as_str) == Some("readOnly"),
        ResourcePolicy::WorkspaceWrite => {
            sandbox.get("type").and_then(Value::as_str) == Some("workspaceWrite")
                && sandbox.get("excludeSlashTmp").and_then(Value::as_bool) == Some(true)
                && sandbox.get("excludeTmpdirEnvVar").and_then(Value::as_bool) == Some(true)
                && sandbox
                    .get("writableRoots")
                    .and_then(Value::as_array)
                    .is_some_and(|roots| {
                        roots
                            .iter()
                            .all(|root| root.as_str() == Some(request.cwd.as_str()))
                    })
        }
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "keep bounded provider event validation in one visible sequence"
)]
fn collect_turn(
    reader: &mut impl BufRead,
    thread_id: &str,
    turn_id: &str,
    request: &Request,
    wire_bytes: &mut usize,
    mut pending: VecDeque<Value>,
) -> Result<(Value, Vec<Value>, BTreeSet<String>), ProviderError> {
    let mut messages = Vec::new();
    let mut observed_item_types = BTreeSet::new();
    let mut message_bytes = 0_usize;
    loop {
        let event = match pending.pop_front() {
            Some(event) => event,
            None => frame(reader, wire_bytes)?,
        };
        let method = event
            .get("method")
            .and_then(Value::as_str)
            .ok_or(ProviderError::InvalidOutput)?;
        let params = event.get("params").ok_or(ProviderError::InvalidOutput)?;
        if params
            .get("threadId")
            .and_then(Value::as_str)
            .is_some_and(|id| id != thread_id)
        {
            return Err(ProviderError::InvalidOutput);
        }
        // Resume replays the previous turn's cumulative token usage after its
        // response. This telemetry is not an item or completion of our turn.
        if method != "thread/tokenUsage/updated"
            && params
                .get("turnId")
                .and_then(Value::as_str)
                .is_some_and(|id| id != turn_id)
        {
            return Err(ProviderError::InvalidOutput);
        }
        match method {
            "thread/settings/updated" => {
                let settings = params
                    .get("threadSettings")
                    .ok_or(ProviderError::InvalidOutput)?;
                if settings.get("model").and_then(Value::as_str) != Some(request.model.as_str())
                    || settings.get("effort").and_then(Value::as_str)
                        != Some(request.effort.as_str())
                    || settings.get("modelProvider").and_then(Value::as_str) != Some("openai")
                    || settings.get("approvalPolicy").and_then(Value::as_str) != Some("never")
                    || settings.get("cwd").and_then(Value::as_str) != Some(request.cwd.as_str())
                    || !sandbox_matches(request, settings.get("sandboxPolicy"))
                    || request.permission_profile.as_ref().is_some_and(|profile| {
                        settings
                            .pointer("/activePermissionProfile/id")
                            .and_then(Value::as_str)
                            != Some(profile)
                    })
                {
                    return Err(ProviderError::InvalidOutput);
                }
            }
            "item/started" | "item/completed" => {
                if params.get("threadId").and_then(Value::as_str) != Some(thread_id)
                    || params.get("turnId").and_then(Value::as_str) != Some(turn_id)
                {
                    return Err(ProviderError::InvalidOutput);
                }
                let item = params.get("item").ok_or(ProviderError::InvalidOutput)?;
                let item_type = required_string(item, "type")?;
                if request.permission_profile.is_some()
                    && !matches!(item_type, "userMessage" | "agentMessage" | "reasoning")
                {
                    return Err(ProviderError::Unsupported);
                }
                if observed_item_types.len() >= 64 {
                    return Err(ProviderError::InvalidOutput);
                }
                observed_item_types.insert(item_type.to_owned());
                match item.get("type").and_then(Value::as_str) {
                    Some("agentMessage") if method == "item/completed" => {
                        let text = item
                            .get("text")
                            .and_then(Value::as_str)
                            .ok_or(ProviderError::InvalidOutput)?;
                        let item_id = required_string(item, "id")?;
                        if messages.iter().any(|message: &Value| {
                            message.get("id").and_then(Value::as_str) == Some(item_id)
                        }) {
                            return Err(ProviderError::InvalidOutput);
                        }
                        message_bytes = message_bytes
                            .checked_add(text.len())
                            .ok_or(ProviderError::InvalidOutput)?;
                        if message_bytes > MAX_TRANSCRIPT_BYTES || messages.len() >= 1024 {
                            return Err(ProviderError::InvalidOutput);
                        }
                        messages.push(item.clone());
                    }
                    Some("collabAgentToolCall" | "dynamicToolCall" | "mcpToolCall") => {
                        return Err(ProviderError::InvalidOutput);
                    }
                    _ => {}
                }
            }
            "turn/completed" => {
                if params.get("threadId").and_then(Value::as_str) != Some(thread_id)
                    || params.pointer("/turn/id").and_then(Value::as_str) != Some(turn_id)
                    || params.pointer("/turn/status").and_then(Value::as_str) != Some("completed")
                    || params
                        .pointer("/turn/error")
                        .is_some_and(|error| !error.is_null())
                {
                    return Err(ProviderError::InvalidOutput);
                }
                return Ok((params.clone(), messages, observed_item_types));
            }
            _ => {}
        }
    }
}

fn response(
    reader: &mut impl BufRead,
    id: u64,
    wire_bytes: &mut usize,
) -> Result<(Value, VecDeque<Value>), ProviderError> {
    // Notifications can precede their request's response (thread/started is
    // one example). They cannot replace the correlated response.
    let mut pending = VecDeque::new();
    loop {
        let event = frame(reader, wire_bytes)?;
        match event.get("id") {
            Some(value) if value.as_u64() == Some(id) => {
                return event
                    .get("result")
                    .filter(|value| value.is_object())
                    .cloned()
                    .map(|result| (result, pending))
                    .ok_or(ProviderError::InvalidOutput);
            }
            Some(_) => return Err(ProviderError::InvalidOutput),
            None => {
                if pending.len() >= 4096 {
                    return Err(ProviderError::InvalidOutput);
                }
                pending.push_back(event);
            }
        }
    }
}

fn frame(reader: &mut impl BufRead, wire_bytes: &mut usize) -> Result<Value, ProviderError> {
    let mut bytes = Vec::new();
    reader
        .take((MAX_FRAME_BYTES + 1) as u64)
        .read_until(b'\n', &mut bytes)
        .map_err(|_| ProviderError::InvalidOutput)?;
    *wire_bytes = wire_bytes
        .checked_add(bytes.len())
        .ok_or(ProviderError::InvalidOutput)?;
    if bytes.is_empty()
        || bytes.len() > MAX_FRAME_BYTES
        || *wire_bytes > MAX_WIRE_BYTES
        || bytes.last() != Some(&b'\n')
    {
        return Err(ProviderError::InvalidOutput);
    }
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| ProviderError::InvalidOutput)?;
    // Never grant approvals, invoke dynamic tools, or service authentication
    // requests. The guard terminates the provider tree on any such request.
    if !value.is_object()
        || value.get("error").is_some()
        || (value.get("id").is_some() && value.get("method").is_some())
        || value.get("method").and_then(Value::as_str) == Some("error")
    {
        return Err(ProviderError::InvalidOutput);
    }
    Ok(value)
}

fn send(writer: &mut impl Write, value: &Value) -> Result<(), ProviderError> {
    serde_json::to_writer(&mut *writer, value).map_err(|_| ProviderError::InvalidOutput)?;
    writer
        .write_all(b"\n")
        .and_then(|()| writer.flush())
        .map_err(|_| ProviderError::InvalidOutput)
}

fn required_string<'a>(value: &'a Value, key: &str) -> Result<&'a str, ProviderError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.len() <= 4096 && !value.contains('\0'))
        .ok_or(ProviderError::InvalidOutput)
}

pub(crate) fn decode(
    prepared: &PreparedInvocation,
    bytes: &[u8],
) -> Result<InvocationOutput, ProviderError> {
    if bytes.len() > MAX_TRANSCRIPT_BYTES {
        return Err(ProviderError::InvalidOutput);
    }
    let transcript: Transcript =
        serde_json::from_slice(bytes).map_err(|_| ProviderError::InvalidOutput)?;
    let thread_id = required_string(&transcript.configuration, "thread_id")?;
    required_string(&transcript.configuration, "provider_session_id")?;
    if transcript.schema != TRANSCRIPT_SCHEMA
        || transcript.turn_id.is_empty()
        || transcript
            .completion
            .get("threadId")
            .and_then(Value::as_str)
            != Some(thread_id)
        || transcript
            .completion
            .pointer("/turn/id")
            .and_then(Value::as_str)
            != Some(transcript.turn_id.as_str())
        || transcript
            .completion
            .pointer("/turn/status")
            .and_then(Value::as_str)
            != Some("completed")
        || transcript
            .completion
            .pointer("/turn/error")
            .is_some_and(|error| !error.is_null())
        || transcript.messages.iter().any(|item| {
            item.get("type").and_then(Value::as_str) != Some("agentMessage")
                || required_string(item, "id").is_err()
        })
    {
        return Err(ProviderError::InvalidOutput);
    }
    let messages = transcript
        .messages
        .iter()
        .filter(|item| item.get("phase").and_then(Value::as_str) != Some("commentary"))
        .map(|item| {
            item.get("text")
                .and_then(Value::as_str)
                .ok_or(ProviderError::InvalidOutput)
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(InvocationOutput {
        invocation_id: prepared.invocation_id.clone(),
        reported_model: required_string(&transcript.configuration, "model")?.to_owned(),
        reported_effort: Some(required_string(&transcript.configuration, "effort")?.to_owned()),
        session_id: Some(thread_id.to_owned()),
        text: messages.join("\n"),
    })
}

#[cfg(test)]
#[path = "codex_app_server_tests.rs"]
mod tests;
