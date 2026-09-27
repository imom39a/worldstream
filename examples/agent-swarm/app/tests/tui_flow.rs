use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use worldstream_agent_swarm::fixture::FixtureFileBackend;
use worldstream_agent_swarm::{
    AcceptanceCriterion, BackendError, CreateSwarm, ExactSwarmAction, MemberConfiguration,
    ProviderConfigurationState, SwarmActionOffer, SwarmActionReceipt, SwarmActor, SwarmApplication,
    SwarmBackend, SwarmId, SwarmObservation, SwarmSummary, SwarmView, ValidatedCreateSwarm,
    tui::{
        ExecutionControls, ExecutionPolicy, ExecutionView, Input, ScriptedSession,
        WorkspaceArtifactInspector, run, run_with_actions, run_with_actions_and_execution,
        run_with_plans_and_execution, run_with_plans_execution_and_artifacts,
    },
};

fn request() -> CreateSwarm {
    CreateSwarm {
        goal: "Exercise the interactive path".to_owned(),
        constraints: vec!["stay local".to_owned()],
        acceptance_criteria: vec![AcceptanceCriterion {
            text: "reopens".to_owned(),
        }],
        working_area: std::env::temp_dir(),
        roster: vec![MemberConfiguration {
            member_key: "worker".to_owned(),
            label: "Worker".to_owned(),
            provider: "codex".to_owned(),
            requested_model: "requested".to_owned(),
            requested_effort: None,
            configuration_revision: 1,
            moving_alias_acknowledged: true,
            configuration_state: ProviderConfigurationState::ResolutionUnreported,
        }],
        progress_review_interval_seconds: 300,
        correction_failure_limit: 3,
    }
}

struct ActionBackend {
    inner: FixtureFileBackend,
    submissions: Arc<Mutex<usize>>,
    captured: Arc<Mutex<Vec<ExactSwarmAction>>>,
    reject_stale: bool,
}

struct FakeExecution {
    registered: Option<SwarmId>,
    phase: String,
    commands: Arc<Mutex<Vec<&'static str>>>,
    policies: Arc<Mutex<Vec<ExecutionPolicy>>>,
}

impl FakeExecution {
    fn view(&self) -> ExecutionView {
        ExecutionView {
            desired: self.phase.clone(),
            phase: self.phase.clone(),
            active: usize::from(self.phase == "pausing"),
            queued: 1,
            invocations_started: 2,
            unknown_effects: 0,
            priority: 1,
            invocation_limit: None,
            active_time_limit_ms: None,
            provider_caps: BTreeMap::from([("controlled".to_owned(), 1)]),
            coordinator: None,
        }
    }

    fn mutate(&mut self, phase: &'static str) -> Result<ExecutionView, String> {
        phase.clone_into(&mut self.phase);
        self.commands
            .lock()
            .map_err(|_| "poisoned execution fixture".to_owned())?
            .push(phase);
        Ok(self.view())
    }
}

impl ExecutionControls for FakeExecution {
    fn register(&mut self, swarm: &SwarmView) -> Result<(), String> {
        self.registered = Some(swarm.swarm_id.clone());
        Ok(())
    }

    fn status(&self, swarm_id: &SwarmId) -> Result<Option<ExecutionView>, String> {
        Ok((self.registered.as_ref() == Some(swarm_id)).then(|| self.view()))
    }

    fn pause(&mut self, _swarm_id: &SwarmId) -> Result<ExecutionView, String> {
        self.mutate("paused")
    }

    fn stop(&mut self, _swarm_id: &SwarmId) -> Result<ExecutionView, String> {
        self.mutate("stopped")
    }

    fn resume(&mut self, _swarm_id: &SwarmId) -> Result<ExecutionView, String> {
        self.mutate("running")
    }

    fn apply_policy(
        &mut self,
        _swarm_id: &SwarmId,
        policy: &ExecutionPolicy,
    ) -> Result<ExecutionView, String> {
        self.policies
            .lock()
            .map_err(|_| "poisoned".to_owned())?
            .push(policy.clone());
        match policy {
            ExecutionPolicy::SetProviderCap { provider, limit } => {
                self.commands
                    .lock()
                    .map_err(|_| "poisoned".to_owned())?
                    .push("policy");
                let mut view = self.view();
                view.provider_caps.insert(provider.clone(), *limit);
                Ok(view)
            }
            ExecutionPolicy::SetPriority { priority } => {
                self.commands
                    .lock()
                    .map_err(|_| "poisoned".to_owned())?
                    .push("policy");
                let mut view = self.view();
                view.priority = *priority;
                Ok(view)
            }
            ExecutionPolicy::SetBudget {
                invocation_limit,
                active_time_limit_ms,
            } => {
                self.commands
                    .lock()
                    .map_err(|_| "poisoned".to_owned())?
                    .push("policy");
                let mut view = self.view();
                view.invocation_limit = *invocation_limit;
                view.active_time_limit_ms = *active_time_limit_ms;
                Ok(view)
            }
        }
    }
}

impl SwarmBackend for ActionBackend {
    fn create(&mut self, request: ValidatedCreateSwarm) -> Result<SwarmView, BackendError> {
        self.inner.create(request)
    }

    fn list(&self) -> Result<Vec<SwarmSummary>, BackendError> {
        self.inner.list()
    }

    fn open(&self, swarm_id: &SwarmId) -> Result<SwarmView, BackendError> {
        self.inner.open(swarm_id)
    }

    fn observe(
        &self,
        swarm_id: &SwarmId,
        actor: &SwarmActor,
    ) -> Result<SwarmObservation, BackendError> {
        let swarm = self.inner.open(swarm_id)?;
        let artifact_path = swarm
            .working_area
            .join("artifacts/report-世界.md")
            .display()
            .to_string();
        let artifact_digest = std::fs::read(&artifact_path).map_or_else(
            |_| format!("blake3:{}", "e".repeat(64)),
            |bytes| format!("blake3:{}", blake3::hash(&bytes).to_hex()),
        );
        let optional_artifact = |file: &str, artifact_id: &str, media_type: &str| {
            let path = swarm.working_area.join("artifacts").join(file);
            std::fs::read(&path).ok().map(|bytes| {
                serde_json::json!({
                    "artifact_id": artifact_id,
                    "local_path": path.display().to_string(),
                    "digest": format!("blake3:{}", blake3::hash(&bytes).to_hex()),
                    "media_type": media_type,
                })
            })
        };
        let candidates = optional_artifact("change.diff", "artifact-change", "text/x-diff")
            .into_iter()
            .map(|artifact| serde_json::json!({"candidate_id":"candidate-a", "artifact":artifact}))
            .collect::<Vec<_>>();
        let checks = optional_artifact("check.json", "artifact-check", "application/json")
            .into_iter()
            .map(|artifact| {
                serde_json::json!({
                    "check_id":"check-a",
                    "status":"passed",
                    "evidence_refs":[{"artifact":artifact}]
                })
            })
            .collect::<Vec<_>>();
        Ok(SwarmObservation {
            swarm,
            actor: actor.clone(),
            member_id: "fixture-human".to_owned(),
            room_seq: 0,
            authoritative_state_hash: format!("blake3:{}", "a".repeat(64)),
            action_offers: vec![
                SwarmActionOffer {
                    offer_id: "direction-offer".to_owned(),
                    action_type: "issue_direction".to_owned(),
                    payload_schema_digest: format!("blake3:{}", "b".repeat(64)),
                },
                SwarmActionOffer {
                    offer_id: "suggestion-offer".to_owned(),
                    action_type: "submit_suggestion".to_owned(),
                    payload_schema_digest: format!("blake3:{}", "c".repeat(64)),
                },
                SwarmActionOffer {
                    offer_id: "configuration-offer".to_owned(),
                    action_type: "update_member_configuration".to_owned(),
                    payload_schema_digest: format!("blake3:{}", "d".repeat(64)),
                },
            ],
            activity: serde_json::json!({"phase":"open", "roster":[{
                "member_key":"worker", "member_id":"01ARZ3NDEKTSV4RRFFQ69G5FZZ",
                "configuration_revision":1
            }], "work_items":[{
                "work_id":"work-a", "owner_member_id":"01ARZ3NDEKTSV4RRFFQ69G5FZZ", "status":"active"
            }], "candidates":candidates, "checks":checks, "contributions":[{"artifact":{
                "artifact_id":"artifact-report",
                "local_path":artifact_path,
                "digest":artifact_digest,
                "media_type":"text/markdown"
            }}]}),
        })
    }

    fn submit(
        &mut self,
        _swarm_id: &SwarmId,
        request: &ExactSwarmAction,
    ) -> Result<SwarmActionReceipt, BackendError> {
        if request.actor != SwarmActor::HumanCoordinator
            || request.based_on_room_seq != 0
            || !matches!(
                request.action_type.as_str(),
                "issue_direction" | "submit_suggestion" | "update_member_configuration"
            )
        {
            return Err(BackendError::InvalidData);
        }
        self.captured
            .lock()
            .map_err(|_| BackendError::StorageUnavailable)?
            .push(request.clone());
        *self
            .submissions
            .lock()
            .map_err(|_| BackendError::StorageUnavailable)? += 1;
        if self.reject_stale {
            Ok(SwarmActionReceipt::Rejected {
                action_id: request.action_id.clone(),
                code: "stale_room_head".to_owned(),
                current_room_seq: 1,
                retryable_with_same_action_id: false,
                may_submit_revised_action: true,
                duplicate: false,
            })
        } else {
            Ok(SwarmActionReceipt::Accepted {
                action_id: request.action_id.clone(),
                room_seq: 1,
                duplicate: false,
            })
        }
    }
}

#[test]
fn scripted_new_resize_list_open_refresh_and_detach_restores_terminal()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("swarms.json");
    let mut application = SwarmApplication::new(FixtureFileBackend::new(path.clone()));
    let mut session = ScriptedSession::new(
        vec![
            Input::New,
            Input::Open,
            Input::Open,
            Input::Resize {
                width: 72,
                height: 20,
            },
            Input::Home,
            Input::Open,
            Input::Refresh,
            Input::Quit,
        ],
        100,
        30,
    )?;
    run(&mut application, &mut session, Some(&request()))?;
    assert!(session.restored());
    assert!(session.cursor_shown());
    assert_eq!(session.draws(), 8);
    let reopened = SwarmApplication::new(FixtureFileBackend::new(path));
    let summaries = reopened.list()?;
    assert_eq!(summaries.len(), 1);
    assert_eq!(
        reopened.open(&summaries[0].swarm_id)?.goal,
        "Exercise the interactive path"
    );
    Ok(())
}

#[test]
fn human_authors_previews_cancels_and_confirms_a_complete_swarm_without_json()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let working_area = directory.path().join("authored-work");
    std::fs::create_dir(&working_area)?;
    let path = directory.path().join("authored-swarms.json");
    let mut application = SwarmApplication::new(FixtureFileBackend::new(path.clone()));
    let mut session = ScriptedSession::new(
        vec![
            Input::New,
            Input::Paste {
                text: "discard this draft".to_owned(),
            },
            Input::Home,
            Input::New,
            Input::Paste {
                text: "Ship the Unicode résumé 世界".to_owned(),
            },
            Input::ToggleTarget,
            Input::Paste {
                text: "Stay local | Preserve evidence".to_owned(),
            },
            Input::ToggleTarget,
            Input::Paste {
                text: "Tests pass | Diff is reviewable".to_owned(),
            },
            Input::ToggleTarget,
            Input::Paste {
                text: working_area.display().to_string(),
            },
            Input::ToggleTarget,
            Input::Paste {
                text: "builder | Builder | codex | gpt-5.6 | high | unack ; reviewer | Reviewer | claude | claude-4 | - | unack".to_owned(),
            },
            Input::Open,
            Input::Open,
            Input::Quit,
        ],
        100,
        30,
    )?;

    let receipt = run(&mut application, &mut session, None)?;
    assert!(receipt.created);
    assert!(session.statuses().iter().any(|status| {
        status.contains("AUTHORING NEW SWARM · GOAL") && status.contains("plain text")
    }));
    assert!(session.statuses().iter().any(|status| {
        status.contains("AUTHORING NEW SWARM · ROSTER") && status.contains("members separated by ;")
    }));
    assert!(session.statuses().iter().any(|status| {
        status.contains("STAGED NEW SWARM")
            && status.contains("constraints 2")
            && status.contains("criteria 2")
            && status.contains("members 2")
    }));
    assert!(
        session
            .statuses()
            .iter()
            .any(|status| status == "staged mutation cancelled")
    );
    assert!(session.frames().iter().any(|frame| {
        frame.contains("NEW SWARM FORM")
            && frame.contains("GOAL")
            && frame.contains("CONSTRAINTS")
            && frame.contains("ACCEPTANCE CRITERIA")
            && frame.contains("WORKING AREA")
            && frame.contains("ROSTER")
    }));
    assert!(session.frames().iter().any(|frame| {
        frame.contains("STAGED NEW SWARM") && frame.contains("Ship the Unicode résumé")
    }));
    let reopened = SwarmApplication::new(FixtureFileBackend::new(path));
    let summaries = reopened.list()?;
    assert_eq!(summaries.len(), 1);
    let view = reopened.open(&summaries[0].swarm_id)?;
    assert_eq!(view.goal, "Ship the Unicode résumé 世界");
    assert_eq!(view.constraints, ["Stay local", "Preserve evidence"]);
    assert_eq!(view.acceptance_criteria.len(), 2);
    assert_eq!(view.working_area, working_area);
    assert_eq!(view.roster.len(), 2);
    assert_eq!(view.roster[0].member_key, "builder");
    assert_eq!(view.roster[1].requested_effort, None);
    assert!(session.restored());
    Ok(())
}

#[test]
fn script_error_still_restores_terminal() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let mut application = SwarmApplication::new(FixtureFileBackend::new(
        directory.path().join("swarms.json"),
    ));
    let mut session = ScriptedSession::new(vec![], 100, 30)?;
    assert!(run(&mut application, &mut session, None).is_err());
    assert!(session.restored());
    assert!(session.cursor_shown());
    Ok(())
}

#[test]
fn staged_human_action_requires_a_separate_confirmation_input()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let submissions = Arc::new(Mutex::new(0));
    let backend = ActionBackend {
        inner: FixtureFileBackend::new(directory.path().join("actions.json")),
        submissions: Arc::clone(&submissions),
        captured: Arc::new(Mutex::new(Vec::new())),
        reject_stale: false,
    };
    let mut application = SwarmApplication::new(backend);
    let mut session = ScriptedSession::new(
        vec![
            Input::New,
            Input::Open,
            Input::Open,
            Input::StageNextAction,
            Input::Open,
            Input::Quit,
        ],
        100,
        30,
    )?;
    let action = ExactSwarmAction {
        actor: SwarmActor::HumanCoordinator,
        action_id: "01ARZ3NDEKTSV4RRFFQ69G5FC2".to_owned(),
        based_on_room_seq: 0,
        offer_id: "direction-offer".to_owned(),
        action_type: "issue_direction".to_owned(),
        payload_schema_digest: format!("blake3:{}", "b".repeat(64)),
        payload: serde_json::json!({
            "direction_id":"direction-a",
            "expected_direction_revision":0,
            "instruction":"Use the revised outline",
            "target_scope":"work",
            "target_work_ids":["work-a"]
        }),
    };

    let receipt = run_with_actions(
        &mut application,
        &mut session,
        Some(&request()),
        vec![action],
    )?;
    assert_eq!(receipt.actions_submitted, 1);
    assert_eq!(*submissions.lock().map_err(|_| "poisoned")?, 1);
    assert_eq!(session.draws(), 6);
    assert!(session.restored());
    Ok(())
}

#[test]
fn human_authors_unicode_direction_targets_current_work_and_discovers_artifact()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let submissions = Arc::new(Mutex::new(0));
    let captured = Arc::new(Mutex::new(Vec::new()));
    let backend = ActionBackend {
        inner: FixtureFileBackend::new(directory.path().join("authored.json")),
        submissions: Arc::clone(&submissions),
        captured: Arc::clone(&captured),
        reject_stale: false,
    };
    let mut application = SwarmApplication::new(backend);
    let mut session = ScriptedSession::new(
        vec![
            Input::New,
            Input::Open,
            Input::Open,
            Input::BeginSuggestion,
            Input::Paste {
                text: "cancel this suggestion".to_owned(),
            },
            Input::ToggleTarget,
            Input::Home,
            Input::BeginDirection,
            Input::Text {
                text: "Réviser ".to_owned(),
            },
            Input::Paste {
                text: "世界 sans replay".to_owned(),
            },
            Input::ToggleTarget,
            Input::Open,
            Input::Resize {
                width: 48,
                height: 16,
            },
            Input::Open,
            Input::InspectArtifact,
            Input::Quit,
        ],
        100,
        30,
    )?;

    let receipt = run_with_actions(&mut application, &mut session, Some(&request()), Vec::new())?;
    assert_eq!(receipt.actions_submitted, 1);
    assert_eq!(*submissions.lock().map_err(|_| "poisoned submissions")?, 1);
    let actions = captured.lock().map_err(|_| "poisoned actions")?;
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].action_type, "issue_direction");
    assert_eq!(actions[0].based_on_room_seq, 0);
    assert_eq!(
        actions[0].payload["instruction"],
        "Réviser 世界 sans replay"
    );
    assert_eq!(actions[0].payload["target_scope"], "work");
    assert_eq!(
        actions[0].payload["target_work_ids"],
        serde_json::json!(["work-a"])
    );
    assert!(session.statuses().iter().any(|status| {
        status.contains("selected member's current work")
            && status.contains("Réviser 世界 sans replay")
    }));
    assert!(
        session
            .statuses()
            .iter()
            .any(|status| status.contains("artifact 1/1 not displayed")
                && status.contains("artifacts/report-世界.md")
                && status.contains("protected artifact state"))
    );
    assert!(session.restored());
    assert!(session.cursor_shown());
    Ok(())
}

#[test]
fn native_tui_previews_only_digest_verified_bounded_text_artifacts()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let working_area = directory.path().join("working");
    std::fs::create_dir_all(working_area.join("artifacts"))?;
    std::fs::write(
        working_area.join("artifacts/report-世界.md"),
        "Review summary 世界\ncontrol \u{1b}[31m removed\n",
    )?;
    std::fs::write(
        working_area.join("artifacts/change.diff"),
        "diff --git a/report b/report\n+verified 世界\n",
    )?;
    std::fs::write(
        working_area.join("artifacts/check.json"),
        r#"{"passed":true,"tests":12}"#,
    )?;
    let mut create = request();
    create.working_area = std::fs::canonicalize(working_area)?;
    let backend = ActionBackend {
        inner: FixtureFileBackend::new(directory.path().join("artifact-preview.json")),
        submissions: Arc::new(Mutex::new(0)),
        captured: Arc::new(Mutex::new(Vec::new())),
        reject_stale: false,
    };
    let mut application = SwarmApplication::new(backend);
    let mut execution = FakeExecution {
        registered: None,
        phase: "stopped".to_owned(),
        commands: Arc::new(Mutex::new(Vec::new())),
        policies: Arc::new(Mutex::new(Vec::new())),
    };
    let inspector = WorkspaceArtifactInspector::new(directory.path().join("artifact-state"));
    let mut session = ScriptedSession::new(
        vec![
            Input::New,
            Input::Open,
            Input::Open,
            Input::InspectArtifact,
            Input::InspectArtifact,
            Input::InspectArtifact,
            Input::Quit,
        ],
        140,
        40,
    )?;

    run_with_plans_execution_and_artifacts(
        &mut application,
        &mut session,
        &mut execution,
        &inspector,
        Some(&create),
        Vec::new(),
        Vec::new(),
    )?;
    assert!(
        session.statuses().iter().any(|status| {
            status.contains("verified artifact 1/3") && status.contains("change.diff")
        }) && session.statuses().iter().any(|status| {
            status.contains("verified artifact 2/3") && status.contains("check.json")
        }) && session.statuses().iter().any(|status| {
            status.contains("verified artifact 3/3") && status.contains("report-世界.md")
        }),
        "statuses: {:?}",
        session.statuses()
    );
    assert!(
        session
            .frames()
            .iter()
            .any(|frame| frame.contains("VERIFIED ARTIFACT PREVIEW")
                && frame.contains("diff --git a/report b/report")
                && frame.contains("verified 世 界")),
        "last frame: {:?}",
        session.frames().last()
    );
    assert!(session.frames().iter().any(|frame| {
        frame.contains("VERIFIED ARTIFACT PREVIEW")
            && frame.contains("passed")
            && frame.contains("tests")
    }));
    assert!(
        session
            .frames()
            .iter()
            .any(|frame| frame.contains("Review summary 世 界") && frame.contains("control �[31m"))
    );
    assert!(
        session
            .frames()
            .iter()
            .all(|frame| !frame.contains('\u{1b}'))
    );
    assert!(session.restored());
    Ok(())
}

#[test]
fn stale_authored_action_is_reported_once_without_replay() -> Result<(), Box<dyn std::error::Error>>
{
    let directory = tempfile::tempdir()?;
    let submissions = Arc::new(Mutex::new(0));
    let captured = Arc::new(Mutex::new(Vec::new()));
    let backend = ActionBackend {
        inner: FixtureFileBackend::new(directory.path().join("stale.json")),
        submissions: Arc::clone(&submissions),
        captured: Arc::clone(&captured),
        reject_stale: true,
    };
    let mut application = SwarmApplication::new(backend);
    let mut session = ScriptedSession::new(
        vec![
            Input::New,
            Input::Open,
            Input::Open,
            Input::BeginDirection,
            Input::Paste {
                text: "Use the current evidence".to_owned(),
            },
            Input::Open,
            Input::Open,
            Input::Quit,
        ],
        100,
        30,
    )?;

    let receipt = run_with_actions(&mut application, &mut session, Some(&request()), Vec::new())?;
    assert_eq!(receipt.actions_submitted, 1);
    assert_eq!(*submissions.lock().map_err(|_| "poisoned submissions")?, 1);
    assert_eq!(captured.lock().map_err(|_| "poisoned actions")?.len(), 1);
    assert!(session.statuses().iter().any(|status| {
        status.contains("rejected: stale_room_head") && status.contains("current head 1")
    }));
    assert!(session.restored());
    Ok(())
}

#[test]
fn human_authors_distinct_whole_swarm_suggestion() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let captured = Arc::new(Mutex::new(Vec::new()));
    let backend = ActionBackend {
        inner: FixtureFileBackend::new(directory.path().join("suggestion.json")),
        submissions: Arc::new(Mutex::new(0)),
        captured: Arc::clone(&captured),
        reject_stale: false,
    };
    let mut application = SwarmApplication::new(backend);
    let mut session = ScriptedSession::new(
        vec![
            Input::New,
            Input::Open,
            Input::Open,
            Input::BeginSuggestion,
            Input::Paste {
                text: "Prefer the shorter résumé".to_owned(),
            },
            Input::Open,
            Input::Open,
            Input::Quit,
        ],
        100,
        30,
    )?;

    let receipt = run_with_actions(&mut application, &mut session, Some(&request()), Vec::new())?;
    assert_eq!(receipt.actions_submitted, 1);
    let actions = captured.lock().map_err(|_| "poisoned actions")?;
    assert_eq!(actions[0].action_type, "submit_suggestion");
    assert_eq!(actions[0].payload["summary"], "Prefer the shorter résumé");
    assert_eq!(actions[0].payload["target_scope"], "goal");
    assert_eq!(actions[0].payload["target_work_ids"], serde_json::json!([]));
    assert!(session.restored());
    Ok(())
}

#[test]
fn human_authors_selected_member_configuration_with_exact_revision_and_confirmation()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let submissions = Arc::new(Mutex::new(0));
    let captured = Arc::new(Mutex::new(Vec::new()));
    let backend = ActionBackend {
        inner: FixtureFileBackend::new(directory.path().join("configuration.json")),
        submissions: Arc::clone(&submissions),
        captured: Arc::clone(&captured),
        reject_stale: false,
    };
    let mut application = SwarmApplication::new(backend);
    let mut session = ScriptedSession::new(
        vec![
            Input::New,
            Input::Open,
            Input::Open,
            Input::BeginConfiguration,
            Input::Paste {
                text: "claude | claude-sonnet-latest | high | ack".to_owned(),
            },
            Input::Open,
            Input::Home,
            Input::BeginConfiguration,
            Input::Paste {
                text: "codex | pinned-model-v2 | - | unack".to_owned(),
            },
            Input::Open,
            Input::Open,
            Input::Quit,
        ],
        100,
        30,
    )?;

    let receipt = run_with_actions(&mut application, &mut session, Some(&request()), Vec::new())?;
    assert_eq!(receipt.actions_submitted, 1);
    assert_eq!(*submissions.lock().map_err(|_| "poisoned submissions")?, 1);
    let actions = captured.lock().map_err(|_| "poisoned actions")?;
    assert_eq!(actions.len(), 1);
    let action = &actions[0];
    assert_eq!(action.action_type, "update_member_configuration");
    assert_eq!(action.offer_id, "configuration-offer");
    assert_eq!(action.based_on_room_seq, 0);
    assert_eq!(action.payload["member_key"], "worker");
    assert_eq!(action.payload["expected_configuration_revision"], 1);
    assert_eq!(action.payload["provider"], "codex");
    assert_eq!(action.payload["requested_model"], "pinned-model-v2");
    assert!(action.payload.get("requested_effort").is_none());
    assert_eq!(action.payload["moving_alias_acknowledged"], false);
    assert!(session.statuses().iter().any(|status| {
        status.contains("AUTHORING CONFIG worker r1→r2")
            && status.contains("provider | model | effort-or-- | ack-or-unack")
    }));
    assert!(session.statuses().iter().any(|status| {
        status.contains("STAGED CONFIG worker r1→r2")
            && status.contains("effort none")
            && status.contains("alias unack")
    }));
    assert!(
        session
            .statuses()
            .iter()
            .any(|status| status == "staged mutation cancelled")
    );
    assert!(session.restored());
    Ok(())
}

#[test]
fn tui_controls_attached_execution_without_changing_room_authority()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let mut application = SwarmApplication::new(FixtureFileBackend::new(
        directory.path().join("execution.json"),
    ));
    let commands = Arc::new(Mutex::new(Vec::new()));
    let mut execution = FakeExecution {
        registered: None,
        phase: "stopped".to_owned(),
        commands: Arc::clone(&commands),
        policies: Arc::new(Mutex::new(Vec::new())),
    };
    let mut session = ScriptedSession::new(
        vec![
            Input::New,
            Input::Open,
            Input::Open,
            Input::Resume,
            Input::Pause,
            Input::Stop,
            Input::Quit,
        ],
        100,
        30,
    )?;

    let receipt = run_with_actions_and_execution(
        &mut application,
        &mut session,
        &mut execution,
        Some(&request()),
        Vec::new(),
    )?;

    assert_eq!(receipt.execution_commands, 3);
    assert_eq!(
        *commands.lock().map_err(|_| "poisoned commands")?,
        vec!["running", "paused", "stopped"]
    );
    assert!(execution.registered.is_some());
    assert!(session.restored());
    Ok(())
}

#[test]
fn tui_stages_and_confirms_explicit_capacity_priority_and_budget_policies()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let mut application = SwarmApplication::new(FixtureFileBackend::new(
        directory.path().join("policies.json"),
    ));
    let commands = Arc::new(Mutex::new(Vec::new()));
    let mut execution = FakeExecution {
        registered: None,
        phase: "stopped".to_owned(),
        commands: Arc::clone(&commands),
        policies: Arc::new(Mutex::new(Vec::new())),
    };
    let mut session = ScriptedSession::new(
        vec![
            Input::New,
            Input::Open,
            Input::Open,
            Input::StageNextPolicy,
            Input::Open,
            Input::StageNextPolicy,
            Input::Open,
            Input::StageNextPolicy,
            Input::Open,
            Input::Quit,
        ],
        100,
        30,
    )?;
    let policies = vec![
        ExecutionPolicy::SetProviderCap {
            provider: "controlled".to_owned(),
            limit: 2,
        },
        ExecutionPolicy::SetPriority { priority: 3 },
        ExecutionPolicy::SetBudget {
            invocation_limit: Some(4),
            active_time_limit_ms: Some(60_000),
        },
    ];

    let receipt = run_with_plans_and_execution(
        &mut application,
        &mut session,
        &mut execution,
        Some(&request()),
        Vec::new(),
        policies,
    )?;

    assert_eq!(receipt.policies_applied, 3);
    assert_eq!(receipt.execution_commands, 3);
    assert_eq!(
        *commands.lock().map_err(|_| "poisoned commands")?,
        vec!["policy", "policy", "policy"]
    );
    assert!(session.restored());
    Ok(())
}

#[test]
fn human_authors_capacity_priority_and_optional_budget_without_a_policy_file()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let mut application = SwarmApplication::new(FixtureFileBackend::new(
        directory.path().join("authored-policies.json"),
    ));
    let commands = Arc::new(Mutex::new(Vec::new()));
    let policies = Arc::new(Mutex::new(Vec::new()));
    let mut execution = FakeExecution {
        registered: None,
        phase: "stopped".to_owned(),
        commands: Arc::clone(&commands),
        policies: Arc::clone(&policies),
    };
    let mut session = ScriptedSession::new(
        vec![
            Input::New,
            Input::Open,
            Input::Open,
            Input::BeginExecutionPolicy,
            Input::Paste {
                text: "controlled 0".to_owned(),
            },
            Input::Open,
            Input::Home,
            Input::BeginExecutionPolicy,
            Input::Paste {
                text: "controlled 2".to_owned(),
            },
            Input::Open,
            Input::Open,
            Input::BeginExecutionPolicy,
            Input::ToggleTarget,
            Input::Paste {
                text: "4".to_owned(),
            },
            Input::Open,
            Input::Open,
            Input::BeginExecutionPolicy,
            Input::ToggleTarget,
            Input::ToggleTarget,
            Input::Paste {
                text: "8 | -".to_owned(),
            },
            Input::Open,
            Input::Open,
            Input::Quit,
        ],
        100,
        30,
    )?;

    let receipt = run_with_plans_and_execution(
        &mut application,
        &mut session,
        &mut execution,
        Some(&request()),
        Vec::new(),
        Vec::new(),
    )?;
    assert_eq!(receipt.policies_applied, 3);
    assert_eq!(receipt.execution_commands, 3);
    assert_eq!(
        *policies.lock().map_err(|_| "poisoned policies")?,
        vec![
            ExecutionPolicy::SetProviderCap {
                provider: "controlled".to_owned(),
                limit: 2,
            },
            ExecutionPolicy::SetPriority { priority: 4 },
            ExecutionPolicy::SetBudget {
                invocation_limit: Some(8),
                active_time_limit_ms: None,
            },
        ]
    );
    assert!(session.statuses().iter().any(|status| {
        status.contains("AUTHORING PROVIDER CAP") && status.contains("provider limit")
    }));
    assert!(session.statuses().iter().any(|status| {
        status.contains("AUTHORING SWARM PRIORITY") && status.contains("priority (1..16)")
    }));
    assert!(session.statuses().iter().any(|status| {
        status.contains("AUTHORING SWARM BUDGET")
            && status.contains("invocation-limit-or-- active-ms-or--")
    }));
    assert!(
        session
            .statuses()
            .iter()
            .any(|status| status == "staged mutation cancelled")
    );
    assert!(session.restored());
    Ok(())
}
