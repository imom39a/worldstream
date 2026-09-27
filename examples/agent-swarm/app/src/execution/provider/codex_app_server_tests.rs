use super::{
    MAX_FRAME_BYTES, ProviderError, Request, ResourcePolicy, SessionSelection, Transcript, Value,
    json, run,
};

type Exercise = (Result<(), ProviderError>, Vec<Value>, Vec<u8>);

fn request() -> Request {
    Request {
        model: "chosen-model".to_owned(),
        effort: "high".to_owned(),
        cwd: "/work".to_owned(),
        resource_policy: ResourcePolicy::ReadOnly,
        session: SessionSelection::Fresh { requested_id: None },
        prompt: "authorized current room context".to_owned(),
        permission_profile: None,
        output_schema: None,
    }
}

#[test]
fn named_evaluation_profile_is_verified_before_prompt() -> Result<(), Box<dyn std::error::Error>> {
    let mut request = request();
    request.permission_profile = Some("worldstream_swarm_read".to_owned());
    request.output_schema =
        Some(json!({"type":"object","properties":{},"additionalProperties":false}));
    let mut frames = fixture();
    frames[2]["result"]["activePermissionProfile"] = json!({"id":"worldstream_swarm_read"});
    frames[2]["result"]["instructionSources"] = json!([]);
    frames[2]["result"]["runtimeWorkspaceRoots"] = json!(["/work"]);
    let (result, commands, output) = exercise(&request, &frames)?;
    assert!(result.is_ok());
    assert_eq!(
        commands[3]["params"]["permissions"],
        "worldstream_swarm_read"
    );
    assert!(commands[3]["params"].get("sandbox").is_none());
    assert_eq!(
        commands[4]["params"]["outputSchema"],
        request.output_schema.clone().unwrap_or_default()
    );
    let transcript: Transcript = serde_json::from_slice(&output)?;
    assert_eq!(
        transcript.configuration["activePermissionProfile"]["id"],
        "worldstream_swarm_read"
    );
    for (field, value) in [
        (
            "activePermissionProfile",
            json!({"id":":danger-full-access"}),
        ),
        ("instructionSources", json!(["/unrelated/AGENTS.md"])),
        ("runtimeWorkspaceRoots", json!(["/unrelated"])),
    ] {
        let mut changed = frames.clone();
        changed[2]["result"][field] = value;
        let (result, commands, _) = exercise(&request, &changed)?;
        assert!(result.is_err());
        assert!(
            !commands
                .iter()
                .any(|command| command["method"] == "turn/start")
        );
    }
    request.permission_profile = Some(":danger-full-access".to_owned());
    assert!(Request::parse(&serde_json::to_string(&request)?).is_err());
    Ok(())
}

fn fixture() -> Vec<Value> {
    vec![
        json!({"id":1,"result":{"userAgent":"Codex test fixture"}}),
        json!({"id":2,"result":{"account":{"type":"chatgpt","email":"private@example.invalid"},"requiresOpenaiAuth":true}}),
        json!({"id":3,"result":{"model":"chosen-model","reasoningEffort":"high","modelProvider":"openai","cwd":"/work","approvalPolicy":"never","sandbox":{"type":"readOnly","networkAccess":false},"thread":{"id":"thread-a","sessionId":"session-tree-b"}}}),
        json!({"method":"turn/started","params":{"threadId":"thread-a","turn":{"id":"turn-a","status":"inProgress"}}}),
        // Providers may publish items before acknowledging turn/start.
        json!({"method":"item/completed","params":{"threadId":"thread-a","turnId":"turn-a","item":{"id":"message-a","type":"agentMessage","phase":"final_answer","text":"bounded result"}}}),
        json!({"id":4,"result":{"turn":{"id":"turn-a","status":"inProgress"}}}),
        json!({"method":"turn/completed","params":{"threadId":"thread-a","turn":{"id":"turn-a","status":"completed","error":null}}}),
    ]
}

fn exercise(request: &Request, frames: &[Value]) -> Result<Exercise, Box<dyn std::error::Error>> {
    let mut wire = Vec::new();
    for frame in frames {
        wire.extend(serde_json::to_vec(frame)?);
        wire.push(b'\n');
    }
    let mut sent = Vec::new();
    let mut output = Vec::new();
    let result = run(request, wire.as_slice(), &mut sent, &mut output);
    let commands = sent
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(serde_json::from_slice)
        .collect::<Result<Vec<_>, _>>()?;
    Ok((result, commands, output))
}

#[test]
fn resumed_token_usage_does_not_rebind_or_abort_the_new_turn()
-> Result<(), Box<dyn std::error::Error>> {
    let mut frames = fixture();
    frames.insert(
        6,
        json!({"method":"thread/tokenUsage/updated","params":{
        "threadId":"thread-a","turnId":"previous-completed-turn","tokenUsage":{}}}),
    );
    let (result, _, output) = exercise(&request(), &frames)?;
    assert!(result.is_ok());
    let transcript: Transcript = serde_json::from_slice(&output)?;
    assert_eq!(transcript.turn_id, "turn-a");
    assert_eq!(transcript.messages.len(), 1);
    Ok(())
}

#[test]
fn verifies_subscription_and_effective_settings_before_sending_prompt()
-> Result<(), Box<dyn std::error::Error>> {
    let (result, commands, output) = exercise(&request(), &fixture())?;
    assert_eq!(result, Ok(()));
    let methods: Vec<_> = commands.iter().map(|v| v["method"].as_str()).collect();
    assert_eq!(
        methods,
        vec![
            Some("initialize"),
            Some("initialized"),
            Some("account/read"),
            Some("thread/start"),
            Some("turn/start")
        ]
    );
    assert_eq!(commands[3]["params"]["allowProviderModelFallback"], false);
    assert_eq!(commands[3]["params"]["model"], "chosen-model");
    assert_eq!(
        commands[3]["params"]["config"]["model_reasoning_effort"],
        "high"
    );
    assert!(commands[4]["params"].get("model").is_none());
    assert!(commands[4]["params"].get("effort").is_none());
    let transcript: Transcript = serde_json::from_slice(&output)?;
    assert_eq!(transcript.configuration["thread_id"], "thread-a");
    assert_eq!(
        transcript.configuration["provider_session_id"],
        "session-tree-b"
    );
    assert_eq!(transcript.messages.len(), 1);
    assert!(!String::from_utf8(output)?.contains("private@example.invalid"));
    Ok(())
}

#[test]
fn refuses_missing_or_substituted_settings_before_model_work()
-> Result<(), Box<dyn std::error::Error>> {
    for (key, value) in [
        ("model", json!("substituted-model")),
        ("reasoningEffort", json!("low")),
        ("reasoningEffort", Value::Null),
        ("modelProvider", json!("other-provider")),
        ("approvalPolicy", json!("on-request")),
        ("cwd", json!("/unrelated")),
        ("sandbox", json!({"type":"dangerFullAccess"})),
    ] {
        let mut frames = fixture();
        frames[2]["result"][key] = value;
        let (result, sent, output) = exercise(&request(), &frames)?;
        assert!(result.is_err(), "{key}");
        assert!(!sent.iter().any(|v| v["method"] == "turn/start"));
        assert!(output.is_empty());
    }
    Ok(())
}

#[test]
fn api_key_or_missing_login_never_starts_a_thread() -> Result<(), Box<dyn std::error::Error>> {
    for account in [
        Value::Null,
        json!({"type":"apiKey"}),
        json!({"type":"unknown"}),
    ] {
        let mut frames = fixture();
        frames[1]["result"]["account"] = account;
        let (result, sent, _) = exercise(&request(), &frames)?;
        assert_eq!(result, Err(ProviderError::Unsupported));
        assert!(!sent.iter().any(|v| v["method"] == "thread/start"));
    }
    Ok(())
}

#[test]
fn resume_uses_exact_thread_identity_and_keeps_session_tree_separate()
-> Result<(), Box<dyn std::error::Error>> {
    let mut request = request();
    request.session = SessionSelection::Resume {
        session_id: "thread-a".to_owned(),
    };
    let (result, sent, _) = exercise(&request, &fixture())?;
    assert_eq!(result, Ok(()));
    assert_eq!(sent[3]["method"], "thread/resume");
    assert_eq!(sent[3]["params"]["threadId"], "thread-a");
    assert!(
        sent[3]["params"]
            .get("allowProviderModelFallback")
            .is_none()
    );
    let mut frames = fixture();
    frames[2]["result"]["thread"]["id"] = json!("other-thread");
    let (result, sent, _) = exercise(&request, &frames)?;
    assert!(result.is_err());
    assert!(!sent.iter().any(|v| v["method"] == "turn/start"));
    Ok(())
}

#[test]
fn rejects_interactive_requests_wrong_turns_and_unsuccessful_completions()
-> Result<(), Box<dyn std::error::Error>> {
    for event in [
        json!({"id":51,"method":"item/commandExecution/requestApproval","params":{}}),
        json!({"method":"turn/completed","params":{"threadId":"other-thread","turn":{"id":"turn-a","status":"completed"}}}),
        json!({"method":"turn/completed","params":{"threadId":"thread-a","turn":{"id":"other-turn","status":"completed"}}}),
        json!({"method":"turn/completed","params":{"threadId":"thread-a","turn":{"id":"turn-a","status":"failed"}}}),
        json!({"method":"turn/completed","params":{"threadId":"thread-a","turn":{"id":"turn-a","status":"interrupted"}}}),
        json!({"method":"item/started","params":{"threadId":"thread-a","turnId":"turn-a","item":{"type":"collabAgentToolCall"}}}),
    ] {
        let mut frames = fixture();
        frames[6] = event;
        let (result, _, output) = exercise(&request(), &frames)?;
        assert!(result.is_err());
        assert!(output.is_empty());
    }
    Ok(())
}

#[test]
fn eof_duplicate_messages_and_oversized_frames_fail_closed()
-> Result<(), Box<dyn std::error::Error>> {
    let mut frames = fixture();
    frames.pop();
    assert!(exercise(&request(), &frames)?.0.is_err());
    let mut frames = fixture();
    frames.insert(6, frames[4].clone());
    assert!(exercise(&request(), &frames)?.0.is_err());
    let input = vec![b'x'; MAX_FRAME_BYTES + 1];
    assert!(run(&request(), input.as_slice(), Vec::new(), Vec::new()).is_err());
    Ok(())
}

#[test]
fn workspace_write_rejects_additional_roots_and_ambient_tmp()
-> Result<(), Box<dyn std::error::Error>> {
    let mut request = request();
    request.resource_policy = ResourcePolicy::WorkspaceWrite;
    let sandbox = json!({"type":"workspaceWrite","networkAccess":false,
        "writableRoots":["/work"],"excludeSlashTmp":true,"excludeTmpdirEnvVar":true});
    let mut frames = fixture();
    frames[2]["result"]["sandbox"] = sandbox.clone();
    assert!(exercise(&request, &frames)?.0.is_ok());
    for (key, value) in [
        ("writableRoots", json!(["/work", "/outside"])),
        ("excludeSlashTmp", json!(false)),
        ("excludeTmpdirEnvVar", json!(false)),
        ("networkAccess", json!(true)),
    ] {
        let mut frames = fixture();
        frames[2]["result"]["sandbox"] = sandbox.clone();
        frames[2]["result"]["sandbox"][key] = value;
        let (result, sent, _) = exercise(&request, &frames)?;
        assert!(result.is_err());
        assert!(!sent.iter().any(|v| v["method"] == "turn/start"));
    }
    Ok(())
}
