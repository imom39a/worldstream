//! Controlled planner/Room fixtures around real artifact, checker and writeback
//! implementations. The managed live test separately exercises the real Pack.
use super::*;
use crate::{
    ArtifactPath, ContentDigest, ExactSwarmAction, SwarmActionOffer, execution::RunBudget,
};

const CHECKER: &str =
    "coordinator_service::autonomy::tests::delivery_tests::delivery_checker_fixture";

#[test]
fn delivery_checker_fixture() -> Result<(), Box<dyn Error>> {
    let file = PathBuf::from("answer.txt");
    if file.exists() {
        if fs::read_to_string(&file)? == "slow checker" {
            std::thread::sleep(Duration::from_secs(30));
        }
        assert_eq!(fs::read_to_string(file)?, "reviewed answer");
    }
    Ok(())
}

pub(super) fn submit_fixture_action(
    driver: &Driver,
    action: &ExactSwarmAction,
) -> Result<SwarmActionReceipt, CoordinatorError> {
    let mut state = driver.0.borrow_mut();
    assert_eq!(action.actor, SwarmActor::HumanCoordinator);
    assert_eq!(action.based_on_room_seq, state.observation.room_seq);
    let observation = &mut state.observation;
    let mut row = action.payload.clone();
    match action.action_type.as_str() {
        "record_check" => {
            let previous = observation.activity["checks"]
                .as_array()
                .ok_or(CoordinatorError::InvalidPlan)?
                .iter()
                .filter(|c| c["check_id"] == row["check_id"])
                .filter_map(|c| c["revision"].as_u64())
                .max()
                .unwrap_or(0);
            row["revision"] = json!(previous + 1);
            row["criteria_revision"] = row["expected_criteria_revision"].clone();
            row["criterion"] = observation.activity["acceptance_criteria"][0].clone();
            observation.activity["checks"]
                .as_array_mut()
                .ok_or(CoordinatorError::InvalidPlan)?
                .push(row);
        }
        "request_writeback" | "record_writeback_outcome" => {
            observation.activity["writebacks"]
                .as_array_mut()
                .ok_or(CoordinatorError::InvalidPlan)?
                .push(row);
        }
        "accept_result" => {
            observation.activity["phase"] = json!("completed");
            observation.activity["results"]
                .as_array_mut()
                .ok_or(CoordinatorError::InvalidPlan)?
                .push(row);
        }
        _ => return Err(CoordinatorError::InvalidPlan),
    }
    observation.activity["test_actions"]
        .as_array_mut()
        .ok_or(CoordinatorError::InvalidPlan)?
        .push(serde_json::to_value(action).map_err(|_| CoordinatorError::InvalidPlan)?);
    observation.room_seq += 1;
    observation.authoritative_state_hash = format!("blake3:fixture-{}", observation.room_seq);
    if observation.activity["test_lose_reply"].as_bool() == Some(true) {
        return Err(CoordinatorError::Application(
            crate::ApplicationError::Backend(crate::BackendError::ActionUncertain),
        ));
    }
    Ok(SwarmActionReceipt::Accepted {
        action_id: action.action_id.clone(),
        room_seq: observation.room_seq,
        duplicate: false,
    })
}

#[derive(Clone)]
struct Running(Rc<RefCell<ExecutionSnapshot>>);
impl ExecutionStatusSource for Running {
    fn status(&self) -> Result<ExecutionSnapshot, DaemonError> {
        Ok(self.0.borrow().clone())
    }
}

struct DeliveryFixture {
    directory: tempfile::TempDir,
    service: CoordinatorService,
    driver: Driver,
    execution: Running,
    work_id: String,
    candidate_id: String,
}

fn setup() -> Result<DeliveryFixture, Box<dyn Error>> {
    setup_with_timeout(15)
}

#[allow(clippy::too_many_lines)]
fn setup_with_timeout(timeout_seconds: u64) -> Result<DeliveryFixture, Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let work = directory.path().join("work");
    fs::create_dir(&work)?;
    let work = work.canonicalize()?;
    fs::write(work.join("answer.txt"), b"baseline")?;
    let mut observation = observation()?;
    observation.swarm.working_area = work.clone();
    let mut judge = observation.swarm.roster[0].clone();
    judge.member_key = "judge".to_owned();
    judge.label = "judge".to_owned();
    observation.swarm.roster.push(judge);
    observation.activity["roster"] = json!(observation.swarm.roster);
    observation.activity["criteria_revision"] = json!(1);
    observation.activity["acceptance_criteria"] = json!(["Independent evidence"]);
    observation.activity["resources"] = json!([{"resource_id":"target","version":1,"local_path":work.join("answer.txt"),"digest":ContentDigest::of(b"baseline").to_string()}]);
    for name in [
        "contributions",
        "candidates",
        "checks",
        "reviews",
        "findings",
        "writebacks",
        "results",
        "test_actions",
    ] {
        observation.activity[name] = json!([]);
    }
    observation.action_offers = [
        "record_check",
        "request_writeback",
        "record_writeback_outcome",
        "accept_result",
    ]
    .into_iter()
    .map(|name| SwarmActionOffer {
        offer_id: format!("offer-{name}"),
        action_type: name.to_owned(),
        payload_schema_digest: "sha256:fixture".to_owned(),
    })
    .collect();
    let swarm_id = observation.swarm.swarm_id.clone();
    let execution = Running(Rc::new(RefCell::new(ExecutionSnapshot {
        revision: 1,
        provider_caps: BTreeMap::from([(ProviderKind::Controlled, 2)]),
        swarms: vec![SwarmExecutionSnapshot {
            swarm_id: swarm_id.as_str().to_owned(),
            desired: DesiredExecution::Running,
            phase: ExecutionPhase::Running,
            execution_epoch: 1,
            priority: 1,
            budget: RunBudget::default(),
            invocations_started: 0,
            active_time_ms: 0,
            queued: Vec::new(),
            active: Vec::new(),
            unknown_effects: Vec::new(),
        }],
    })));
    let driver = Driver(Rc::new(RefCell::new(DriverState {
        observation,
        intents: BTreeMap::new(),
        fail_preflight: false,
    })));
    let mut service = CoordinatorService::open_with_ports(
        directory.path(),
        &swarm_id,
        Box::new(driver.clone()),
        Box::new(execution.clone()),
    )?;
    let executable = std::env::current_exe()?.canonicalize()?;
    let guard = executable
        .parent()
        .and_then(Path::parent)
        .ok_or("no debug root")?
        .join(format!(
            "worldstream-agent-swarm-process-guard{}",
            std::env::consts::EXE_SUFFIX
        ));
    service.enable_autonomy(AutonomyPolicy {
        max_work_items: 3,
        invocation_limit: 64,
        resource_policy: ResourcePolicy::ReadOnly,
        allowed_tools: Vec::new(),
        delivery: Some(DeliveryPolicy {
            target: ArtifactPath::new("answer.txt")?,
            resource_id: "target".to_owned(),
            expected_resource_version: 1,
            maximum_candidate_versions: 3,
            process_guard: guard,
            checks: vec![DeliveryCheckPolicy {
                check_id: "criterion-1".to_owned(),
                program: executable,
                arguments: vec![
                    "--exact".to_owned(),
                    CHECKER.to_owned(),
                    "--nocapture".to_owned(),
                ],
                timeout_seconds,
            }],
        }),
    })?;
    let plan = reason(&mut service, &driver)?;
    let option = instruction(&plan)?
        .options
        .into_iter()
        .find(|o| {
            o.member_key == "worker-a"
                && matches!(o.semantic_target, WorkerSemanticTarget::WorkProposal { .. })
        })
        .ok_or("no proposal")?;
    let WorkerSemanticTarget::WorkProposal { work_id, .. } = &option.semantic_target else {
        unreachable!()
    };
    let work_id = work_id.clone();
    settle_decision(
        &mut service,
        &driver,
        &plan,
        PlanningDecisionKind::Dispatch {
            steps: vec![select(&option)],
            reason: "Fixture proposes integration work.".to_owned(),
        },
    )?;
    let observed = driver.0.borrow().observation.clone();
    service.advance_autonomy(&observed)?;
    let worker = service
        .state
        .plans
        .values()
        .find(|p| {
            matches!(
                p.semantic_target,
                Some(WorkerSemanticTarget::WorkProposal { .. })
            )
        })
        .ok_or("worker missing")?
        .clone();
    let accepted = intent_for(
        &worker,
        SubmissionDisposition::Received(SwarmActionReceipt::Accepted {
            action_id: "fixture".to_owned(),
            room_seq: 2,
            duplicate: false,
        }),
    );
    driver
        .0
        .borrow_mut()
        .intents
        .insert(worker.invocation_id.clone(), accepted.clone());
    service.record_intents([accepted])?;
    let namespace = service
        .state
        .autonomy
        .as_ref()
        .ok_or("autonomy")?
        .namespace
        .clone();
    let candidate_id = format!("result-{namespace}");
    {
        let mut state = driver.0.borrow_mut();
        state.observation.activity["work_items"] = json!([{"work_id":work_id,"execution_epoch":1,"revision":2,"kind":"integration","status":"claimed","owner_member_id":"member-worker-a","active_attempt_id":"attempt","dependency_ids":[]}]);
        state.observation.activity["work_attempts"] = json!([{"attempt_id":"attempt","revision":1,"status":"active","work_id":work_id,"owner_member_id":"member-worker-a","execution_epoch":1}]);
        state.observation.room_seq += 1;
        fs::write(work.join("contribution.txt"), b"upstream evidence")?;
        state.observation.activity["contributions"] = json!([{"contribution_id":"source","version":1,"work_id":work_id,"author_member_id":"member-worker-a","artifact":{"artifact_id":"input","media_type":"text/plain","local_path":work.join("contribution.txt"),"digest":ContentDigest::of(b"upstream evidence").to_string()}}]);
    }
    Ok(DeliveryFixture {
        directory,
        service,
        driver,
        execution,
        work_id,
        candidate_id,
    })
}

impl DeliveryFixture {
    fn candidate(&mut self, version: u64, text: &str) -> Result<(), Box<dyn Error>> {
        let mut d = self.driver.0.borrow_mut();
        let work = &d.observation.swarm.working_area;
        let path = work.join(format!("candidate-{version}.txt"));
        fs::write(&path, text)?;
        d.observation.activity["candidates"].as_array_mut().ok_or("candidates")?.push(json!({
            "candidate_id":self.candidate_id,"version":version,"work_id":self.work_id,"work_revision":2,"author_member_id":"member-worker-a",
            "execution_epoch":1,"criteria_revision":1,"direction_revision":0,"resource_basis":[{"resource_id":"target","version":1}],
            "contribution_refs":[{"contribution_id":"source","version":1}],
            "artifact":{"artifact_id":format!("candidate-{version}"),"digest":ContentDigest::of(text.as_bytes()).to_string(),"local_path":path,"media_type":"text/plain"}
        }));
        d.observation.room_seq += 1;
        Ok(())
    }

    fn operate(&mut self, kind: &str) -> Result<(), Box<dyn Error>> {
        let plan = reason(&mut self.service, &self.driver)?;
        let options = serde_json::to_value(instruction(&plan)?.delivery_options)?;
        let option = options
            .as_array()
            .ok_or("options")?
            .iter()
            .find(|v| v["operation"]["kind"].as_str() == Some(kind))
            .ok_or("operation missing")?;
        settle_decision(
            &mut self.service,
            &self.driver,
            &plan,
            PlanningDecisionKind::Operate {
                target_id: option["target_id"].as_str().ok_or("target")?.to_owned(),
                reason: "Controlled fixture chooses the operation.".to_owned(),
            },
        )?;
        let observed = self.driver.0.borrow().observation.clone();
        self.service.advance_autonomy(&observed)?;
        if kind == "check"
            && self.driver.0.borrow().observation.activity["test_lose_reply"].as_bool()
                != Some(true)
            && let Some(view) = &self.service.view().autonomy
            && view.phase == "needs_attention"
        {
            return Err(view.reason.clone().into());
        }
        Ok(())
    }

    fn review(&self, version: u64, member: &str, verdict: &str) -> Result<(), Box<dyn Error>> {
        let mut d = self.driver.0.borrow_mut();
        d.observation.activity["reviews"].as_array_mut().ok_or("reviews")?.push(json!({
            "review_id":format!("review-{version}-{member}"),"revision":1,"candidate":{"candidate_id":self.candidate_id,"version":version},
            "reviewer_member_id":format!("member-{member}"),"verdict":verdict,"criteria_revision":1,"resource_basis":[{"resource_id":"target","version":1}],"finding_ids":[]
        }));
        d.observation.room_seq += 1;
        Ok(())
    }

    fn can_deliver(&self) -> Result<bool, Box<dyn Error>> {
        let options = serde_json::to_value(
            self.service
                .delivery_options(&self.driver.0.borrow().observation)?,
        )?;
        Ok(options
            .as_array()
            .ok_or("options")?
            .iter()
            .any(|v| v["operation"]["kind"] == "deliver"))
    }

    fn reopen(self) -> Result<Self, Box<dyn Error>> {
        let Self {
            directory,
            service,
            driver,
            execution,
            work_id,
            candidate_id,
        } = self;
        let id = service.state.swarm_id.clone();
        drop(service);
        let service = CoordinatorService::open_with_ports(
            directory.path(),
            &id,
            Box::new(driver.clone()),
            Box::new(execution.clone()),
        )?;
        Ok(Self {
            directory,
            service,
            driver,
            execution,
            work_id,
            candidate_id,
        })
    }
}

#[test]
fn autonomous_delivery_revises_after_failed_checks_and_preserves_review_findings()
-> Result<(), Box<dyn Error>> {
    let mut f = setup()?;
    f.candidate(1, "wrong answer")?;
    f.operate("check")?;
    assert!(!f.can_deliver()?);
    assert_eq!(
        f.driver.0.borrow().observation.activity["checks"][0]["status"],
        "failed"
    );
    f.candidate(2, "reviewed answer")?;
    f.operate("check")?;
    f.review(2, "judge", "changes_requested")?;
    f.driver.0.borrow_mut().observation.activity["findings"] = json!([{"finding_id":"gap","revision":1,"candidate":{"candidate_id":f.candidate_id,"version":2},"review_id":"review-2-judge","severity":"blocking","status":"unresolved","summary":"Review requires correction."}]);
    assert!(!f.can_deliver()?);
    f.candidate(3, "reviewed answer")?;
    f.operate("check")?;
    f.review(3, "worker-a", "passed")?;
    assert!(!f.can_deliver()?);
    f.review(3, "judge", "passed")?;
    assert!(
        !f.can_deliver()?,
        "passing later review must not clear old findings"
    );
    f.driver.0.borrow_mut().observation.activity["findings"][0]["status"] = json!("resolved");
    assert!(f.can_deliver()?);
    f = f.reopen()?;
    assert!(f.can_deliver()?);
    f.operate("deliver")?;
    let d = f.driver.0.borrow();
    assert_eq!(
        fs::read_to_string(d.observation.swarm.working_area.join("answer.txt"))?,
        "reviewed answer"
    );
    assert_eq!(d.observation.activity["phase"], "completed");
    let actions = d.observation.activity["test_actions"]
        .as_array()
        .ok_or("actions")?;
    let tail = actions
        .iter()
        .rev()
        .take(3)
        .map(|a| a["action_type"].as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    assert_eq!(
        tail,
        [
            "accept_result",
            "record_writeback_outcome",
            "request_writeback"
        ]
    );
    assert_eq!(
        actions.last().ok_or("last")?["payload"]["check_refs"],
        json!([{"id":"criterion-1","revision":3}])
    );
    Ok(())
}

#[test]
fn autonomous_delivery_preserves_changed_target_and_refuses_policy_rebinding()
-> Result<(), Box<dyn Error>> {
    let mut f = setup()?;
    f.candidate(1, "reviewed answer")?;
    f.operate("check")?;
    f.review(1, "judge", "passed")?;
    let mut changed = f
        .service
        .state
        .autonomy
        .as_ref()
        .ok_or("policy")?
        .policy
        .clone();
    changed.delivery.as_mut().ok_or("delivery")?.target = ArtifactPath::new("other.txt")?;
    assert!(matches!(
        f.service.enable_autonomy(changed),
        Err(CoordinatorServiceError::Conflict)
    ));
    let target = f
        .driver
        .0
        .borrow()
        .observation
        .swarm
        .working_area
        .join("answer.txt");
    fs::write(&target, "newer user bytes")?;
    f.operate("deliver")?;
    assert_eq!(fs::read_to_string(target)?, "newer user bytes");
    assert_eq!(
        f.service.view().autonomy.as_ref().ok_or("view")?.phase,
        "needs_attention"
    );
    assert!(
        f.driver.0.borrow().observation.activity["writebacks"]
            .as_array()
            .ok_or("writes")?
            .is_empty()
    );
    Ok(())
}

#[test]
fn autonomous_delivery_lost_action_reply_is_not_retried_after_reopen() -> Result<(), Box<dyn Error>>
{
    let mut f = setup()?;
    f.candidate(1, "reviewed answer")?;
    f.driver.0.borrow_mut().observation.activity["test_lose_reply"] = json!(true);
    f.operate("check")?;
    assert!(
        f.service.has_blocking_ambiguity(),
        "{:?}",
        f.service.view().autonomy
    );
    f = f.reopen()?;
    let before = f.driver.0.borrow().observation.activity["test_actions"].clone();
    let observed = f.driver.0.borrow().observation.clone();
    f.service.advance_autonomy(&observed)?;
    assert_eq!(
        f.driver.0.borrow().observation.activity["test_actions"],
        before
    );
    assert!(f.service.has_blocking_ambiguity());
    let journal = serde_json::to_value(&f.service.state)?;
    let operations = journal["autonomy"]["delivery"]["operations"]
        .as_object()
        .ok_or("operations")?;
    let action = &operations.values().next().ok_or("operation")?["actions"][0];
    assert!(action["receipt"].is_null());
    assert!(action["action"]["action_id"].is_string());
    Ok(())
}

#[test]
fn autonomous_delivery_goal_change_and_stale_decision_cannot_apply_effects()
-> Result<(), Box<dyn Error>> {
    let mut f = setup()?;
    f.candidate(1, "reviewed answer")?;
    let plan = reason(&mut f.service, &f.driver)?;
    let option = instruction(&plan)?.delivery_options[0].target_id.clone();
    settle_decision(
        &mut f.service,
        &f.driver,
        &plan,
        PlanningDecisionKind::Operate {
            target_id: option,
            reason: "Check current Candidate.".to_owned(),
        },
    )?;
    f.driver.0.borrow_mut().observation.activity["direction_revision"] = json!(1);
    let observed = f.driver.0.borrow().observation.clone();
    f.service.advance_autonomy(&observed)?;
    assert_eq!(
        f.service.view().autonomy.as_ref().ok_or("view")?.phase,
        "needs_attention"
    );
    assert!(
        f.driver.0.borrow().observation.activity["test_actions"]
            .as_array()
            .ok_or("actions")?
            .is_empty()
    );
    Ok(())
}

#[test]
fn autonomous_delivery_unknown_operation_returns_feedback_without_effects()
-> Result<(), Box<dyn Error>> {
    let mut f = setup()?;
    f.candidate(1, "reviewed answer")?;
    let plan = reason(&mut f.service, &f.driver)?;
    settle_decision(
        &mut f.service,
        &f.driver,
        &plan,
        PlanningDecisionKind::Operate {
            target_id: "made-up-command".to_owned(),
            reason: "Try an unoffered operation.".to_owned(),
        },
    )?;
    let observed = f.driver.0.borrow().observation.clone();
    f.service.advance_autonomy(&observed)?;
    let state = f.service.state.autonomy.as_ref().ok_or("autonomy")?;
    assert!(
        state.rejected_decisions[&plan.invocation_id]
            .feedback
            .contains("exact current target_id")
    );
    assert!(
        f.driver.0.borrow().observation.activity["test_actions"]
            .as_array()
            .ok_or("actions")?
            .is_empty()
    );
    assert!(!f.service.has_blocking_ambiguity());
    Ok(())
}

#[test]
fn autonomous_delivery_checker_deadline_contains_process_and_publishes_no_pass()
-> Result<(), Box<dyn Error>> {
    let mut f = setup_with_timeout(1)?;
    f.candidate(1, "slow checker")?;
    let began = std::time::Instant::now();
    let error = f.operate("check").err().ok_or("checker must time out")?;
    assert!(error.to_string().contains("interrupted"), "{error}");
    assert!(began.elapsed() < Duration::from_secs(15));
    assert!(
        f.driver.0.borrow().observation.activity["checks"]
            .as_array()
            .ok_or("checks")?
            .is_empty()
    );
    assert!(!f.can_deliver()?);
    Ok(())
}

#[test]
fn autonomous_delivery_stop_prevents_selected_operation() -> Result<(), Box<dyn Error>> {
    let mut f = setup()?;
    f.candidate(1, "reviewed answer")?;
    f.execution.0.borrow_mut().swarms[0].desired = DesiredExecution::Stopped;
    let error = f
        .operate("check")
        .err()
        .ok_or("stopped execution must not check")?;
    assert!(error.to_string().contains("stopped"), "{error}");
    assert!(
        f.driver.0.borrow().observation.activity["checks"]
            .as_array()
            .ok_or("checks")?
            .is_empty()
    );
    Ok(())
}

#[test]
fn autonomous_delivery_judge_selection_discards_a_retained_resume_session()
-> Result<(), Box<dyn Error>> {
    let mut f = setup()?;
    f.candidate(1, "reviewed answer")?;
    f.operate("check")?;
    let planning = reason(&mut f.service, &f.driver)?;
    let option = instruction(&planning)?
        .options
        .into_iter()
        .find(|option| {
            option.member_key == "judge"
                && matches!(
                    option.semantic_target,
                    WorkerSemanticTarget::CandidateReview { .. }
                )
        })
        .ok_or("independent judge option missing")?;
    // Model a previous explicit session selection on this roster member. The
    // selected review must preserve the fresh-session policy of its option.
    let mut previous = planning.clone();
    previous.invocation_id = "000-previous-judge-plan".to_owned();
    previous.member_key = "judge".to_owned();
    previous.session = SessionSelection::Resume {
        session_id: "earlier-conversation".to_owned(),
    };
    f.service
        .state
        .plans
        .insert(previous.invocation_id.clone(), previous);
    let observed = f.driver.0.borrow().observation.clone();
    assert!(matches!(
        member_execution_profiles(&observed, &f.service.state.plans)?["judge"].session,
        SessionSelection::Resume { .. }
    ));
    let selected = f
        .service
        .selected_plans(&observed, &planning, &[select(&option)])?;
    assert_eq!(selected.len(), 1);
    assert_eq!(
        selected[0].session,
        SessionSelection::Fresh { requested_id: None }
    );
    Ok(())
}
