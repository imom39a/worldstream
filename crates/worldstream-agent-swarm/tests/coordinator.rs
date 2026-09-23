#![cfg(feature = "managed-local-runtime")]

use std::{
    collections::BTreeMap,
    error::Error,
    fs,
    io::{BufRead as _, BufReader, Write as _},
    net::{SocketAddr, TcpListener, TcpStream},
    panic::{self, AssertUnwindSafe},
    path::Path,
    sync::{Arc, Mutex, PoisonError},
    thread,
    time::Duration,
};

use serde_json::{Value, json};
use worldstream_agent_swarm::{
    AcceptanceCriterion, ArtifactWorkspace, BackendError, CoordinatorError, CoordinatorEvent,
    CoordinatorIntentState, CoordinatorIntentView, CoordinatorService, ExactSwarmAction,
    MemberConfiguration, NonSubmissionReason, ProviderConfigurationState, SubmissionDisposition,
    SwarmActionOffer, SwarmActionReceipt, SwarmActor, SwarmApplication, SwarmBackend,
    SwarmCoordinator, SwarmId, SwarmObservation, SwarmSummary, SwarmView, ValidatedCreateSwarm,
    WorkerActionPlan, WorkerSemanticTarget,
    execution::{
        ControlCommand, ControlResult, ExecutionControlClient, ExecutionDaemon, InvocationKind,
        ProviderError, ProviderKind, ProviderRegistry, ResourcePolicy, RunBudget, SessionSelection,
        decode_action_proposal,
        provider::{ProbeOutput, ProviderProbe},
    },
    planning::{PLANNING_DECISION_SCHEMA, PlanningDecisionKind},
};

const GUARD: &str = env!("CARGO_BIN_EXE_worldstream-agent-swarm-process-guard");
const CONTROLLED: &str = env!("CARGO_BIN_EXE_worldstream-agent-swarm-controlled-worker");
type ProxyThread = thread::JoinHandle<Result<(), String>>;
type ObservationMutation = fn(&mut SwarmObservation);
type DaemonThread =
    thread::JoinHandle<Result<ExecutionDaemon, worldstream_agent_swarm::execution::DaemonError>>;

#[derive(Clone)]
struct AuthoritativeBackend {
    state: Arc<Mutex<AuthorityState>>,
}

#[expect(
    clippy::struct_excessive_bools,
    reason = "independent transport and authority failure switches in the integration fixture"
)]
struct AuthorityState {
    view: SwarmView,
    member_id: String,
    room_seq: u64,
    accepted: Vec<ExactSwarmAction>,
    submit_attempts: usize,
    uncertain_once: bool,
    panic_after_accept_once: bool,
    fail_next_observe: bool,
    fail_schema_lookup: bool,
    offered_action_type: String,
    offered_schema_digest: String,
    offered_schema: Value,
}

#[derive(Clone)]
struct LateContributionBackend {
    state: Arc<Mutex<LateContributionState>>,
}

struct LateContributionState {
    view: SwarmView,
    member_id: String,
    room_seq: u64,
    completed: bool,
    results: Vec<Value>,
    late_contributions: Vec<Value>,
    accepted: Vec<ExactSwarmAction>,
}

#[derive(Clone)]
struct DirectionBackend {
    state: Arc<Mutex<DirectionState>>,
}

struct DirectionState {
    view: SwarmView,
    room_seq: u64,
    direction_issued: bool,
    unaffected_completed: bool,
    accepted: Vec<ExactSwarmAction>,
    observation_mutation: Option<ObservationMutation>,
}

impl LateContributionBackend {
    fn new(working_area: &Path) -> Result<Self, Box<dyn Error>> {
        Ok(Self {
            state: Arc::new(Mutex::new(LateContributionState {
                view: SwarmView {
                    swarm_id: SwarmId::new("swarm-coordinator".to_owned())?,
                    room_id: "room-late-contribution".to_owned(),
                    goal: "Retain delayed output without changing the accepted Result".to_owned(),
                    constraints: vec!["Late output remains advisory".to_owned()],
                    acceptance_criteria: vec![AcceptanceCriterion {
                        text: "The accepted Result remains unchanged".to_owned(),
                    }],
                    working_area: working_area.to_path_buf(),
                    roster: vec![MemberConfiguration {
                        member_key: "worker-a".to_owned(),
                        label: "Worker A".to_owned(),
                        provider: "controlled".to_owned(),
                        requested_model: "fixture-v1".to_owned(),
                        requested_effort: Some("medium".to_owned()),
                        configuration_revision: 41,
                        moving_alias_acknowledged: false,
                        configuration_state: ProviderConfigurationState::FixtureUnavailable,
                    }],
                    progress_review_interval_seconds: 300,
                    correction_failure_limit: 3,
                    source_label: "late-output coordinator integration".to_owned(),
                },
                member_id: "01ARZ3NDEKTSV4RRFFQ69G5SA2".to_owned(),
                room_seq: 7,
                completed: false,
                results: Vec::new(),
                late_contributions: Vec::new(),
                accepted: Vec::new(),
            })),
        })
    }

    fn complete_with_result(&self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        assert!(!state.completed);
        state.completed = true;
        state.results = vec![json!({
            "result_id":"accepted-result",
            "version":1,
            "candidate":{"candidate_id":"accepted-candidate","version":1}
        })];
        state.room_seq = state.room_seq.saturating_add(1);
    }

    fn results(&self) -> Vec<Value> {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .results
            .clone()
    }

    fn late_contributions(&self) -> Vec<Value> {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .late_contributions
            .clone()
    }

    fn accepted(&self) -> Vec<ExactSwarmAction> {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .accepted
            .clone()
    }
}

impl DirectionBackend {
    fn new(working_area: &Path) -> Result<Self, Box<dyn Error>> {
        let member = |key: &str, label: &str| MemberConfiguration {
            member_key: key.to_owned(),
            label: label.to_owned(),
            provider: "controlled".to_owned(),
            requested_model: "fixture-v1".to_owned(),
            requested_effort: Some("medium".to_owned()),
            configuration_revision: 1,
            moving_alias_acknowledged: false,
            configuration_state: ProviderConfigurationState::FixtureUnavailable,
        };
        Ok(Self {
            state: Arc::new(Mutex::new(DirectionState {
                view: SwarmView {
                    swarm_id: SwarmId::new("swarm-coordinator".to_owned())?,
                    room_id: "room-targeted-direction".to_owned(),
                    goal: "Cancel only work affected by a targeted Direction".to_owned(),
                    constraints: vec!["Unrelated work must continue".to_owned()],
                    acceptance_criteria: vec![AcceptanceCriterion {
                        text: "Only the affected native invocation stops".to_owned(),
                    }],
                    working_area: working_area.to_path_buf(),
                    roster: vec![
                        member("worker-a", "Worker A"),
                        member("worker-b", "Worker B"),
                    ],
                    progress_review_interval_seconds: 300,
                    correction_failure_limit: 3,
                    source_label: "targeted Direction coordinator integration".to_owned(),
                },
                room_seq: 7,
                direction_issued: false,
                unaffected_completed: false,
                accepted: Vec::new(),
                observation_mutation: None,
            })),
        })
    }

    fn issue_targeted_direction(&self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        assert!(!state.direction_issued);
        state.direction_issued = true;
        state.room_seq = state.room_seq.saturating_add(1);
    }

    fn accepted(&self) -> Vec<ExactSwarmAction> {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .accepted
            .clone()
    }
}

impl AuthoritativeBackend {
    fn with_uncertain_reply(
        working_area: &Path,
        uncertain_once: bool,
    ) -> Result<Self, Box<dyn Error>> {
        Ok(Self {
            state: Arc::new(Mutex::new(AuthorityState {
                view: SwarmView {
                    swarm_id: SwarmId::new("swarm-coordinator".to_owned())?,
                    room_id: "room-coordinator".to_owned(),
                    goal: "Run one controlled exact worker turn".to_owned(),
                    constraints: vec!["Stay in the authorized root".to_owned()],
                    acceptance_criteria: vec![AcceptanceCriterion {
                        text: "The exact Action is accepted".to_owned(),
                    }],
                    working_area: working_area.to_path_buf(),
                    roster: vec![MemberConfiguration {
                        member_key: "worker-a".to_owned(),
                        label: "Worker A".to_owned(),
                        provider: "controlled".to_owned(),
                        requested_model: "fixture-v1".to_owned(),
                        requested_effort: Some("medium".to_owned()),
                        configuration_revision: 41,
                        moving_alias_acknowledged: false,
                        configuration_state: ProviderConfigurationState::FixtureUnavailable,
                    }],
                    progress_review_interval_seconds: 300,
                    correction_failure_limit: 3,
                    source_label: "authoritative test Room".to_owned(),
                },
                member_id: "01ARZ3NDEKTSV4RRFFQ69G5SA2".to_owned(),
                room_seq: 7,
                accepted: Vec::new(),
                submit_attempts: 0,
                uncertain_once,
                panic_after_accept_once: false,
                fail_next_observe: false,
                fail_schema_lookup: false,
                offered_action_type: "submit_contribution".to_owned(),
                offered_schema_digest: format!("blake3:{}", "b".repeat(64)),
                offered_schema: json!({"type":"object"}),
            })),
        })
    }

    fn accepted(&self) -> Vec<ExactSwarmAction> {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .accepted
            .clone()
    }

    fn offer_candidate_schema(&self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        "submit_candidate".clone_into(&mut state.offered_action_type);
        state.offered_schema = json!({
            "type":"object",
            "properties":{"contribution_refs":{"type":"array","minItems":1}},
            "required":["contribution_refs"]
        });
        state.offered_schema_digest = format!(
            "blake3:{}",
            blake3::hash(state.offered_schema.to_string().as_bytes()).to_hex()
        );
    }

    fn submit_attempts(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .submit_attempts
    }

    fn fail_schema_lookup(&self) {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .fail_schema_lookup = true;
    }

    fn fail_next_observe(&self) {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .fail_next_observe = true;
    }

    fn panic_after_next_accept(&self) {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .panic_after_accept_once = true;
    }

    fn select_configuration(&self, model: &str, effort: Option<&str>) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        model.clone_into(&mut state.view.roster[0].requested_model);
        state.view.roster[0].requested_effort = effort.map(str::to_owned);
        state.view.roster[0].configuration_revision = state.view.roster[0]
            .configuration_revision
            .saturating_add(1);
        state.view.roster[0].moving_alias_acknowledged = false;
        state.room_seq = state.room_seq.saturating_add(1);
    }
}

impl SwarmBackend for AuthoritativeBackend {
    fn action_payload_schema(
        &self,
        swarm_id: &SwarmId,
        offer: &SwarmActionOffer,
    ) -> Result<Value, BackendError> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.fail_schema_lookup {
            return Err(BackendError::StorageUnavailable);
        }
        if *swarm_id != state.view.swarm_id
            || offer.action_type != state.offered_action_type
            || offer.payload_schema_digest != state.offered_schema_digest
        {
            return Err(BackendError::InvalidData);
        }
        Ok(state.offered_schema.clone())
    }
    fn create(&mut self, _request: ValidatedCreateSwarm) -> Result<SwarmView, BackendError> {
        Err(BackendError::Unsupported)
    }

    fn list(&self) -> Result<Vec<SwarmSummary>, BackendError> {
        Ok(vec![
            self.state
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .view
                .summary(),
        ])
    }

    fn open(&self, swarm_id: &SwarmId) -> Result<SwarmView, BackendError> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        (state.view.swarm_id == *swarm_id)
            .then(|| state.view.clone())
            .ok_or(BackendError::NotFound)
    }

    fn observe(
        &self,
        swarm_id: &SwarmId,
        actor: &SwarmActor,
    ) -> Result<SwarmObservation, BackendError> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.view.swarm_id != *swarm_id
            || actor
                != &(SwarmActor::Worker {
                    member_key: "worker-a".to_owned(),
                })
        {
            return Err(BackendError::AuthorityUnavailable);
        }
        if state.fail_next_observe {
            state.fail_next_observe = false;
            return Err(BackendError::StorageUnavailable);
        }
        let configured = &state.view.roster[0];
        Ok(SwarmObservation {
            swarm: state.view.clone(),
            actor: actor.clone(),
            member_id: state.member_id.clone(),
            room_seq: state.room_seq,
            authoritative_state_hash: format!("blake3:{}", "a".repeat(64)),
            action_offers: vec![SwarmActionOffer {
                offer_id: "offer-submit".to_owned(),
                action_type: state.offered_action_type.clone(),
                payload_schema_digest: state.offered_schema_digest.clone(),
            }],
            activity: json!({
                "setup_revision": 2,
                "roster": [{
                    "member_key": "worker-a",
                    "member_id": state.member_id,
                    "label": "Worker A",
                    "provider": "controlled",
                    "requested_model": configured.requested_model,
                    "requested_effort": configured.requested_effort,
                    "configuration_revision": configured.configuration_revision,
                    "moving_alias_acknowledged": configured.moving_alias_acknowledged,
                    "configuration_state": "fixture_unavailable"
                }],
                "phase": "open"
            }),
        })
    }

    fn submit(
        &mut self,
        swarm_id: &SwarmId,
        request: &ExactSwarmAction,
    ) -> Result<SwarmActionReceipt, BackendError> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.submit_attempts += 1;
        if let Some(accepted) = state
            .accepted
            .iter()
            .find(|accepted| accepted.action_id == request.action_id)
            .cloned()
        {
            if &accepted != request {
                return Err(BackendError::InvalidData);
            }
            if state.uncertain_once {
                state.uncertain_once = false;
                return Err(BackendError::ActionUncertain);
            }
            return Ok(SwarmActionReceipt::Accepted {
                action_id: request.action_id.clone(),
                room_seq: state.room_seq,
                duplicate: true,
            });
        }
        if state.view.swarm_id != *swarm_id || request.based_on_room_seq != state.room_seq {
            return Err(BackendError::StaleObservation);
        }
        state.accepted.push(request.clone());
        state.room_seq = state.room_seq.saturating_add(1);
        if state.panic_after_accept_once {
            state.panic_after_accept_once = false;
            panic::resume_unwind(Box::new(
                "simulated coordinator crash after accepted submission",
            ));
        }
        if state.uncertain_once {
            state.uncertain_once = false;
            return Err(BackendError::ActionUncertain);
        }
        Ok(SwarmActionReceipt::Accepted {
            action_id: request.action_id.clone(),
            room_seq: state.room_seq,
            duplicate: false,
        })
    }
}

impl SwarmBackend for LateContributionBackend {
    fn action_payload_schema(
        &self,
        _swarm_id: &SwarmId,
        _offer: &SwarmActionOffer,
    ) -> Result<Value, BackendError> {
        Ok(json!({"type":"object"}))
    }
    fn create(&mut self, _request: ValidatedCreateSwarm) -> Result<SwarmView, BackendError> {
        Err(BackendError::Unsupported)
    }

    fn list(&self) -> Result<Vec<SwarmSummary>, BackendError> {
        Ok(vec![
            self.state
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .view
                .summary(),
        ])
    }

    fn open(&self, swarm_id: &SwarmId) -> Result<SwarmView, BackendError> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        (state.view.swarm_id == *swarm_id)
            .then(|| state.view.clone())
            .ok_or(BackendError::NotFound)
    }

    fn observe(
        &self,
        swarm_id: &SwarmId,
        actor: &SwarmActor,
    ) -> Result<SwarmObservation, BackendError> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.view.swarm_id != *swarm_id
            || actor
                != &(SwarmActor::Worker {
                    member_key: "worker-a".to_owned(),
                })
        {
            return Err(BackendError::AuthorityUnavailable);
        }
        let configured = &state.view.roster[0];
        let (phase, work_revision, work_status, attempt_revision, attempt_status, offer) =
            if state.completed {
                (
                    "completed",
                    3,
                    "superseded",
                    2,
                    "interruption_requested",
                    SwarmActionOffer {
                        offer_id: "offer-late".to_owned(),
                        action_type: "record_late_contribution".to_owned(),
                        payload_schema_digest: format!("blake3:{}", "c".repeat(64)),
                    },
                )
            } else {
                (
                    "open",
                    2,
                    "claimed",
                    1,
                    "active",
                    SwarmActionOffer {
                        offer_id: "offer-submit".to_owned(),
                        action_type: "submit_contribution".to_owned(),
                        payload_schema_digest: format!("blake3:{}", "b".repeat(64)),
                    },
                )
            };
        Ok(SwarmObservation {
            swarm: state.view.clone(),
            actor: actor.clone(),
            member_id: state.member_id.clone(),
            room_seq: state.room_seq,
            authoritative_state_hash: format!("blake3:{:064x}", state.room_seq),
            action_offers: vec![offer],
            activity: json!({
                "setup_revision":2,
                "phase":phase,
                "execution_epoch":3,
                "goal_revision":4,
                "direction_revision":2,
                "roster":[{
                    "member_key":"worker-a",
                    "member_id":state.member_id,
                    "label":"Worker A",
                    "provider":"controlled",
                    "requested_model":configured.requested_model,
                    "requested_effort":configured.requested_effort,
                    "configuration_revision":configured.configuration_revision,
                    "moving_alias_acknowledged":configured.moving_alias_acknowledged,
                    "configuration_state":"fixture_unavailable"
                }],
                "work_items":[{
                    "work_id":"work-1",
                    "revision":work_revision,
                    "execution_epoch":3,
                    "kind":"goal",
                    "status":work_status,
                    "owner_member_id":state.member_id,
                    "active_attempt_id":"attempt-work-1"
                }],
                "work_attempts":[{
                    "attempt_id":"attempt-work-1",
                    "revision":attempt_revision,
                    "work_id":"work-1",
                    "execution_epoch":3,
                    "owner_member_id":state.member_id,
                    "status":attempt_status
                }],
                "results":state.results,
                "suggestions":[],
                "late_contributions":state.late_contributions
            }),
        })
    }

    fn submit(
        &mut self,
        swarm_id: &SwarmId,
        request: &ExactSwarmAction,
    ) -> Result<SwarmActionReceipt, BackendError> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(accepted) = state
            .accepted
            .iter()
            .find(|accepted| accepted.action_id == request.action_id)
        {
            return if accepted == request {
                Ok(SwarmActionReceipt::Accepted {
                    action_id: request.action_id.clone(),
                    room_seq: state.room_seq,
                    duplicate: true,
                })
            } else {
                Err(BackendError::InvalidData)
            };
        }
        if state.view.swarm_id != *swarm_id
            || request.based_on_room_seq != state.room_seq
            || !state.completed
            || request.action_type != "record_late_contribution"
            || request.offer_id != "offer-late"
            || request.payload.get("work_id").and_then(Value::as_str) != Some("work-1")
            || request.payload.get("attempt_id").and_then(Value::as_str) != Some("attempt-work-1")
            || request
                .payload
                .get("expected_work_revision")
                .and_then(Value::as_u64)
                != Some(3)
            || request.payload.get("late_id").and_then(Value::as_str) != Some("late-1")
        {
            return Err(BackendError::StaleObservation);
        }
        let result_before = state.results.clone();
        state.late_contributions.push(request.payload.clone());
        if state.results != result_before {
            return Err(BackendError::InvalidData);
        }
        state.accepted.push(request.clone());
        state.room_seq = state.room_seq.saturating_add(1);
        Ok(SwarmActionReceipt::Accepted {
            action_id: request.action_id.clone(),
            room_seq: state.room_seq,
            duplicate: false,
        })
    }
}

impl SwarmBackend for DirectionBackend {
    fn action_payload_schema(
        &self,
        _swarm_id: &SwarmId,
        _offer: &SwarmActionOffer,
    ) -> Result<Value, BackendError> {
        Ok(json!({"type":"object"}))
    }
    fn create(&mut self, _request: ValidatedCreateSwarm) -> Result<SwarmView, BackendError> {
        Err(BackendError::Unsupported)
    }

    fn list(&self) -> Result<Vec<SwarmSummary>, BackendError> {
        Ok(vec![
            self.state
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .view
                .summary(),
        ])
    }

    fn open(&self, swarm_id: &SwarmId) -> Result<SwarmView, BackendError> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        (state.view.swarm_id == *swarm_id)
            .then(|| state.view.clone())
            .ok_or(BackendError::NotFound)
    }

    // The fixture models both sides of one targeted Direction in a single
    // authoritative observation so the native cancellation proof stays exact.
    #[allow(clippy::too_many_lines)]
    fn observe(
        &self,
        swarm_id: &SwarmId,
        actor: &SwarmActor,
    ) -> Result<SwarmObservation, BackendError> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.view.swarm_id != *swarm_id {
            return Err(BackendError::AuthorityUnavailable);
        }
        let member_id = match actor {
            SwarmActor::HumanCoordinator => "human-1",
            SwarmActor::Worker { member_key } if member_key == "worker-a" => "member-1",
            SwarmActor::Worker { member_key } if member_key == "worker-b" => "member-2",
            SwarmActor::Worker { .. } => return Err(BackendError::AuthorityUnavailable),
        };
        let action_offers = match actor {
            SwarmActor::Worker { member_key }
                if (!state.direction_issued && member_key == "worker-a")
                    || (!state.unaffected_completed && member_key == "worker-b") =>
            {
                vec![SwarmActionOffer {
                    offer_id: format!("offer-submit-{member_key}"),
                    action_type: "submit_contribution".to_owned(),
                    payload_schema_digest: format!("blake3:{}", "b".repeat(64)),
                }]
            }
            SwarmActor::HumanCoordinator | SwarmActor::Worker { .. } => Vec::new(),
        };
        let affected_work = if state.direction_issued {
            json!({
                "work_id":"work-affected",
                "revision":3,
                "execution_epoch":1,
                "kind":"goal",
                "status":"blocked",
                "blocker_ids":[],
                "owner_member_id":null,
                "active_attempt_id":null
            })
        } else {
            json!({
                "work_id":"work-affected",
                "revision":2,
                "execution_epoch":1,
                "kind":"goal",
                "status":"claimed",
                "blocker_ids":[],
                "owner_member_id":"member-1",
                "active_attempt_id":"attempt-affected"
            })
        };
        let unaffected_work = if state.unaffected_completed {
            json!({
                "work_id":"work-unaffected",
                "revision":3,
                "execution_epoch":1,
                "kind":"goal",
                "status":"completed",
                "blocker_ids":[],
                "owner_member_id":"member-2",
                "active_attempt_id":"attempt-unaffected"
            })
        } else {
            json!({
                "work_id":"work-unaffected",
                "revision":2,
                "execution_epoch":1,
                "kind":"goal",
                "status":"claimed",
                "blocker_ids":[],
                "owner_member_id":"member-2",
                "active_attempt_id":"attempt-unaffected"
            })
        };
        let affected_attempt = json!({
            "attempt_id":"attempt-affected",
            "revision":if state.direction_issued { 2 } else { 1 },
            "work_id":"work-affected",
            "execution_epoch":1,
            "owner_member_id":"member-1",
            "status":if state.direction_issued { "interruption_requested" } else { "active" }
        });
        let unaffected_attempt = json!({
            "attempt_id":"attempt-unaffected",
            "revision":if state.unaffected_completed { 2 } else { 1 },
            "work_id":"work-unaffected",
            "execution_epoch":1,
            "owner_member_id":"member-2",
            "status":if state.unaffected_completed { "completed" } else { "active" }
        });
        let mut observation = SwarmObservation {
            swarm: state.view.clone(),
            actor: actor.clone(),
            member_id: member_id.to_owned(),
            room_seq: state.room_seq,
            authoritative_state_hash: format!("blake3:{:064x}", state.room_seq),
            action_offers,
            activity: json!({
                "setup_revision":2,
                "phase":"open",
                "execution_epoch":1,
                "goal_revision":1,
                "direction_revision":i32::from(state.direction_issued),
                "roster":[{
                    "member_key":"worker-a",
                    "member_id":"member-1",
                    "provider":"controlled",
                    "requested_model":"fixture-v1",
                    "requested_effort":"medium",
                    "configuration_revision":1,
                    "moving_alias_acknowledged":false
                }, {
                    "member_key":"worker-b",
                    "member_id":"member-2",
                    "provider":"controlled",
                    "requested_model":"fixture-v1",
                    "requested_effort":"medium",
                    "configuration_revision":1,
                    "moving_alias_acknowledged":false
                }],
                "outstanding_progress_reviews":[],
                "blockers":[],
                "problems":[],
                "resource_conflicts":[],
                "work_items":[affected_work, unaffected_work],
                "work_attempts":[affected_attempt, unaffected_attempt]
            }),
        };
        if let Some(mutate) = state.observation_mutation {
            mutate(&mut observation);
        }
        Ok(observation)
    }

    fn submit(
        &mut self,
        swarm_id: &SwarmId,
        request: &ExactSwarmAction,
    ) -> Result<SwarmActionReceipt, BackendError> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(accepted) = state
            .accepted
            .iter()
            .find(|accepted| accepted.action_id == request.action_id)
        {
            return if accepted == request {
                Ok(SwarmActionReceipt::Accepted {
                    action_id: request.action_id.clone(),
                    room_seq: state.room_seq,
                    duplicate: true,
                })
            } else {
                Err(BackendError::InvalidData)
            };
        }
        if state.view.swarm_id != *swarm_id
            || request.based_on_room_seq != state.room_seq
            || !state.direction_issued
            || state.unaffected_completed
            || request.actor
                != (SwarmActor::Worker {
                    member_key: "worker-b".to_owned(),
                })
            || request.action_type != "submit_contribution"
            || request.offer_id != "offer-submit-worker-b"
            || request.payload.get("work_id").and_then(Value::as_str) != Some("work-unaffected")
            || request
                .payload
                .get("expected_work_revision")
                .and_then(Value::as_u64)
                != Some(2)
            || request
                .payload
                .get("expected_attempt_revision")
                .and_then(Value::as_u64)
                != Some(1)
        {
            return Err(BackendError::StaleObservation);
        }
        state.accepted.push(request.clone());
        state.unaffected_completed = true;
        state.room_seq = state.room_seq.saturating_add(1);
        Ok(SwarmActionReceipt::Accepted {
            action_id: request.action_id.clone(),
            room_seq: state.room_seq,
            duplicate: false,
        })
    }
}

#[test]
fn native_planning_decisions_are_durable_and_never_submit_room_actions()
-> Result<(), Box<dyn Error>> {
    for scenario in [
        "retained",
        "changed_head",
        "changed_configuration",
        "invalid_output",
    ] {
        assert_native_planning_decision(scenario)?;
    }
    Ok(())
}

fn assert_native_planning_decision(scenario: &str) -> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let work = temporary.path().join("work");
    let execution_root = temporary.path().join("execution");
    fs::create_dir(&work)?;
    let (control, server) = start_daemon_with_cap(&execution_root, 64, 1)?;
    let backend = DirectionBackend::new(&work)?;
    let artifacts = ArtifactWorkspace::open(&work, &temporary.path().join("artifacts"))?;
    let capabilities = controlled_capabilities()?;
    let mut coordinator = SwarmCoordinator::open(
        SwarmApplication::new(backend.clone()),
        ExecutionControlClient::open(&execution_root, Duration::from_secs(10))?,
        capabilities.clone(),
        artifacts.clone(),
    )?;
    let mut plan = controlled_work_attempt_plan(
        "planning-1",
        "worker-a",
        "work-affected",
        "attempt-affected",
        1,
    )?;
    let observed = backend.observe(
        &plan.swarm_id,
        &SwarmActor::Worker {
            member_key: "worker-a".to_owned(),
        },
    )?;
    plan.resource_policy = ResourcePolicy::ReadOnly;
    plan.allowed_action_types.clear();
    plan.semantic_target = Some(WorkerSemanticTarget::Planning {
        execution_epoch: 1,
        goal_revision: 1,
        direction_revision: 0,
        room_seq: observed.room_seq,
        authoritative_state_hash: observed.authoritative_state_hash,
    });
    plan.instruction = if scenario == "invalid_output" {
        "Not a planning decision".to_owned()
    } else {
        json!({"schema": PLANNING_DECISION_SCHEMA, "decision": {"kind": "wait", "reason": "Await the independent contribution."}}).to_string()
    };
    coordinator.stage(plan)?;
    assert!(
        coordinator
            .dispatch()?
            .iter()
            .any(|event| matches!(event, CoordinatorEvent::Launched { .. }))
    );
    if scenario == "changed_head" {
        backend.issue_targeted_direction();
    } else if scenario == "changed_configuration" {
        let mut state = backend.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.view.roster[0].configuration_revision = 2;
        state.observation_mutation = Some(|observation| {
            observation.activity["roster"][0]["configuration_revision"] = json!(2);
        });
    }
    thread::sleep(Duration::from_secs(1));
    coordinator.harvest()?;
    let retained = coordinator.intent("planning-1")?;
    assert_planning_disposition(&retained, scenario)?;
    assert!(backend.accepted().is_empty());
    drop(coordinator);
    let reopened = SwarmCoordinator::open(
        SwarmApplication::new(backend.clone()),
        ExecutionControlClient::open(&execution_root, Duration::from_secs(10))?,
        capabilities,
        artifacts,
    )?;
    assert_eq!(reopened.intent("planning-1")?, retained);
    drop(reopened);
    drop(control);
    drain_daemon(&execution_root, server)?;
    Ok(())
}

fn assert_planning_disposition(
    retained: &CoordinatorIntentView,
    scenario: &str,
) -> Result<(), Box<dyn Error>> {
    assert!(retained.proposal.is_none());
    assert!(retained.action.is_none());
    let evidence = retained
        .evidence
        .as_ref()
        .ok_or("planning evidence missing")?;
    assert!(evidence.configuration.is_some());
    assert!(evidence.output.is_some());
    if scenario == "retained" {
        assert_eq!(retained.state, CoordinatorIntentState::Settled);
        assert!(
            matches!(&retained.submission, SubmissionDisposition::PlanningDecision(decision)
            if matches!(decision.decision, PlanningDecisionKind::Wait { .. }))
        );
    } else {
        let reason = if scenario == "invalid_output" {
            NonSubmissionReason::ProviderOutputInvalid
        } else {
            assert_eq!(retained.state, CoordinatorIntentState::NeedsReevaluation);
            NonSubmissionReason::AuthorityChanged
        };
        assert_eq!(
            retained.submission,
            SubmissionDisposition::NotSubmitted(reason)
        );
    }
    Ok(())
}

#[test]
fn daemon_owned_turn_survives_coordinator_detach_and_submits_exact_action()
-> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let work = temporary.path().join("work");
    let artifacts_root = temporary.path().join("artifacts");
    let execution_root = temporary.path().join("execution");
    std::fs::create_dir(&work)?;

    let (control, server) = start_daemon(&execution_root, 18)?;

    let capabilities = controlled_capabilities()?;
    let backend = AuthoritativeBackend::with_uncertain_reply(&work, true)?;
    let (proposed_payload, plan) = controlled_contribution_plan()?;

    let artifacts = ArtifactWorkspace::open(&work, &artifacts_root)?;
    let mut coordinator = SwarmCoordinator::open(
        SwarmApplication::new(backend.clone()),
        ExecutionControlClient::open(&execution_root, Duration::from_secs(10))?,
        capabilities.clone(),
        artifacts.clone(),
    )?;
    let staged = coordinator.stage(plan.clone())?;
    assert_initial_configuration(&staged);
    let (publication, proxy) = drop_third_control_response(&execution_root)?;
    let dispatched = coordinator.dispatch()?;
    assert!(dispatched.iter().any(|event| matches!(
        event,
        CoordinatorEvent::LaunchOutcomeUncertain { invocation_id }
            if invocation_id == "invocation-1"
    )));
    proxy.join().map_err(|_| "control proxy panicked")??;
    fs::write(
        execution_root.join("execution-control.v1.json"),
        publication,
    )?;

    // Disconnect every coordinator/client handle while the daemon retains the
    // owned process and an uncertain Launch receipt. Reopening performs no
    // Resume command and retries the byte-identical prepared request.
    drop(coordinator);
    drop(control);

    let mut reopened = SwarmCoordinator::open(
        SwarmApplication::new(backend.clone()),
        ExecutionControlClient::open(&execution_root, Duration::from_secs(10))?,
        capabilities.clone(),
        artifacts.clone(),
    )?;
    assert!(matches!(
        reopened.retry_uncertain_launch("invocation-1")?,
        CoordinatorEvent::Launched { .. }
    ));
    thread::sleep(Duration::from_secs(1));
    backend.fail_next_observe();
    assert!(reopened.harvest().is_err());
    let retained = reopened.intent("invocation-1")?;
    assert_eq!(retained.state, CoordinatorIntentState::ProposalRetained);
    assert!(retained.proposal.is_some());
    assert!(retained.action.is_none());

    // A verified provider proposal is journaled before artifact capture or
    // Action materialization. If fresh authority is temporarily unavailable,
    // reopening resumes from that retained proposal while the daemon keeps
    // the completion unacknowledged.
    drop(reopened);
    let mut reopened = SwarmCoordinator::open(
        SwarmApplication::new(backend.clone()),
        ExecutionControlClient::open(&execution_root, Duration::from_secs(10))?,
        capabilities.clone(),
        artifacts.clone(),
    )?;
    backend.panic_after_next_accept();
    assert!(panic::catch_unwind(AssertUnwindSafe(|| reopened.harvest())).is_err());
    let fenced = reopened.intent("invocation-1")?;
    assert_eq!(fenced.state, CoordinatorIntentState::AwaitingSubmission);
    assert!(fenced.action.is_some());

    // The backend accepted the Action, advanced Head, and then the coordinator
    // crashed before journaling the receipt. Recovery must submit the exact
    // fenced Action without rebinding it to the newer Head. A second lost reply
    // remains explicitly retryable with those same bytes.
    drop(reopened);
    let mut reopened = SwarmCoordinator::open(
        SwarmApplication::new(backend.clone()),
        ExecutionControlClient::open(&execution_root, Duration::from_secs(10))?,
        capabilities,
        artifacts.clone(),
    )?;
    let events = reopened.harvest()?;
    assert!(events.iter().any(|event| matches!(
        event,
        CoordinatorEvent::SubmissionUncertain { invocation_id }
            if invocation_id == "invocation-1"
    )));
    assert!(matches!(
        reopened.retry_uncertain("invocation-1")?,
        CoordinatorEvent::Accepted {
            duplicate: true,
            ..
        }
    ));
    let completed = reopened.intent("invocation-1")?;
    assert_eq!(completed.state, CoordinatorIntentState::Settled);
    let evidence = completed.evidence.ok_or("evidence missing")?;
    let stdout = artifacts.read(&evidence.stdout)?;
    assert!(String::from_utf8(stdout)?.contains("\"type\":\"result\""));

    let accepted = backend.accepted();
    assert_eq!(accepted.len(), 1);
    assert_accepted_contribution(&accepted[0], proposed_payload, &work)?;

    let daemon = server.join().map_err(|_| "daemon thread panicked")??;
    drop(daemon);
    Ok(())
}

#[test]
fn delayed_native_output_becomes_exact_late_contribution_without_changing_result()
-> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let work = temporary.path().join("work");
    let artifacts_root = temporary.path().join("artifacts");
    let execution_root = temporary.path().join("execution");
    fs::create_dir(&work)?;

    let (control, server) = start_daemon(&execution_root, 11)?;
    let backend = LateContributionBackend::new(&work)?;
    let artifacts = ArtifactWorkspace::open(&work, &artifacts_root)?;
    let mut coordinator = SwarmCoordinator::open(
        SwarmApplication::new(backend.clone()),
        ExecutionControlClient::open(&execution_root, Duration::from_secs(10))?,
        controlled_capabilities()?,
        artifacts,
    )?;
    coordinator.stage(controlled_late_contribution_plan()?)?;
    assert!(coordinator.dispatch()?.iter().any(|event| matches!(
        event,
        CoordinatorEvent::Launched { invocation_id }
            if invocation_id == "invocation-late-1"
    )));

    // The controlled process remains live for a deterministic delay while an
    // accepted Result completes the Room and advances the exact Work/Attempt
    // revisions once. Only that transition unlocks the late Action offer.
    backend.complete_with_result();
    let accepted_result = backend.results();
    thread::sleep(Duration::from_secs(1));
    assert!(coordinator.harvest()?.iter().any(|event| matches!(
        event,
        CoordinatorEvent::Accepted { invocation_id, duplicate: false }
            if invocation_id == "invocation-late-1"
    )));

    assert_eq!(backend.results(), accepted_result);
    assert_eq!(backend.late_contributions().len(), 1);
    let accepted = backend.accepted();
    assert_eq!(accepted.len(), 1);
    assert_eq!(accepted[0].action_type, "record_late_contribution");
    assert_eq!(accepted[0].based_on_room_seq, 8);
    assert_eq!(accepted[0].payload["work_id"], json!("work-1"));
    assert_eq!(accepted[0].payload["attempt_id"], json!("attempt-work-1"));
    assert_eq!(accepted[0].payload["expected_work_revision"], json!(3));
    assert_eq!(accepted[0].payload["late_id"], json!("late-1"));
    assert!(accepted[0].payload.get("artifact").is_some());

    drop(coordinator);
    drop(control);
    let daemon = server.join().map_err(|_| "daemon thread panicked")??;
    drop(daemon);
    Ok(())
}

#[test]
fn targeted_direction_cancels_only_affected_native_invocation_and_unrelated_finishes()
-> Result<(), Box<dyn Error>> {
    assert_targeted_direction_isolated(false)
}

#[test]
fn direction_after_native_completion_harvests_evidence_and_rejects_stale_output()
-> Result<(), Box<dyn Error>> {
    assert_targeted_direction_isolated(true)
}

// Keep the end-to-end native lifecycle in one test: splitting it would weaken
// the assertion that cancellation and unrelated completion share one daemon.
#[allow(clippy::too_many_lines)]
fn assert_targeted_direction_isolated(wait_for_completion: bool) -> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let work = temporary.path().join("work");
    let artifacts_root = temporary.path().join("artifacts");
    let execution_root = temporary.path().join("execution");
    let service_root = temporary.path().join("service");
    fs::create_dir(&work)?;

    let (control, server) = start_daemon_with_cap(&execution_root, 128, 2)?;
    let backend = DirectionBackend::new(&work)?;
    let artifacts = ArtifactWorkspace::open(&work, &artifacts_root)?;
    let coordinator = SwarmCoordinator::open(
        SwarmApplication::new(backend.clone()),
        ExecutionControlClient::open(&execution_root, Duration::from_secs(10))?,
        controlled_capabilities()?,
        artifacts,
    )?;
    let swarm_id = SwarmId::new("swarm-coordinator".to_owned())?;
    let mut service = CoordinatorService::open(
        &service_root,
        &swarm_id,
        coordinator,
        ExecutionControlClient::open(&execution_root, Duration::from_secs(10))?,
    )?;
    let affected = controlled_work_attempt_plan(
        "invocation-affected",
        "worker-a",
        "work-affected",
        "attempt-affected",
        1,
    )?;
    let unaffected = controlled_work_attempt_plan(
        "invocation-unaffected",
        "worker-b",
        "work-unaffected",
        "attempt-unaffected",
        2,
    )?;
    service.stage(affected.clone())?;
    service.stage(unaffected.clone())?;
    let launched = service.dispatch_once()?;
    assert_eq!(
        launched
            .iter()
            .filter(|event| matches!(event, CoordinatorEvent::Launched { .. }))
            .count(),
        2
    );

    if wait_for_completion {
        wait_for_retained_completion(&control, &affected.invocation_id)?;
    }
    // Completion can race cancellation. Both paths must fence the affected
    // output while preserving the independent Work Attempt.
    backend.issue_targeted_direction();
    let cancelled = service.dispatch_once()?;
    assert!(cancelled.iter().any(|event| matches!(event,
        CoordinatorEvent::NotSubmitted { invocation_id, reason: NonSubmissionReason::ProcessFailed }
            | CoordinatorEvent::NeedsReevaluation { invocation_id }
            if invocation_id == &affected.invocation_id)));
    let ControlResult::Status(snapshot) =
        control.request(ControlCommand::Status { swarm_id: None })?
    else {
        return Err("daemon status response missing".into());
    };
    let target = snapshot
        .swarms
        .iter()
        .find(|snapshot| snapshot.swarm_id == swarm_id.as_str())
        .ok_or("target Swarm status missing")?;
    assert!(
        target
            .active
            .iter()
            .all(|active| active.ticket.invocation_id == unaffected.invocation_id)
    );

    thread::sleep(Duration::from_secs(1));
    let finished = service.harvest_once()?;
    assert!(cancelled.iter().chain(&finished).any(|event| matches!(
        event,
        CoordinatorEvent::Accepted { invocation_id, duplicate: false }
            if invocation_id == &unaffected.invocation_id
    )));
    let affected_intent = service.intent(&affected.invocation_id)?;
    assert!(matches!(
        affected_intent.submission,
        SubmissionDisposition::NotSubmitted(
            NonSubmissionReason::ProcessFailed | NonSubmissionReason::AuthorityChanged
        )
    ));
    assert!(affected_intent.action.is_none());
    if wait_for_completion {
        assert_eq!(
            affected_intent.state,
            CoordinatorIntentState::NeedsReevaluation
        );
        assert_eq!(
            affected_intent.submission,
            SubmissionDisposition::NotSubmitted(NonSubmissionReason::AuthorityChanged)
        );
        assert!(
            affected_intent
                .evidence
                .as_ref()
                .is_some_and(|evidence| evidence.output.is_some())
        );
        assert!(affected_intent.proposal.is_some());
    }
    let unaffected_intent = service.intent(&unaffected.invocation_id)?;
    assert_eq!(unaffected_intent.state, CoordinatorIntentState::Settled);
    assert!(matches!(
        unaffected_intent.submission,
        SubmissionDisposition::Received(SwarmActionReceipt::Accepted {
            duplicate: false,
            ..
        })
    ));
    let accepted = backend.accepted();
    assert_eq!(accepted.len(), 1);
    assert_eq!(
        accepted[0].actor,
        SwarmActor::Worker {
            member_key: "worker-b".to_owned(),
        }
    );
    assert_eq!(accepted[0].payload["work_id"], json!("work-unaffected"));

    drop(service);
    drop(control);
    drain_daemon(&execution_root, server)?;
    Ok(())
}

fn wait_for_retained_completion(
    control: &ExecutionControlClient,
    invocation_id: &str,
) -> Result<(), Box<dyn Error>> {
    for _ in 0..64 {
        match control.request(ControlCommand::Collect {
            invocation_id: invocation_id.to_owned(),
        }) {
            Ok(ControlResult::Completion(result)) => {
                assert_eq!(result.invocation_id, invocation_id);
                assert!(result.exit.is_some());
                return Ok(());
            }
            Err(worldstream_agent_swarm::execution::DaemonError::Control(
                worldstream_agent_swarm::execution::ControlFault::NotFound,
            )) => {
                thread::sleep(Duration::from_millis(20));
            }
            _ => return Err("unexpected completion response".into()),
        }
    }
    Err("native completion did not become available".into())
}

#[test]
fn queued_work_attempt_launches_after_unrelated_room_head_advances() -> Result<(), Box<dyn Error>> {
    assert_queued_work_attempt_launches(|view| {
        view.activity["resource_conflicts"] = json!([{
            "status":"unresolved", "affected_work_ids":["work-affected"]
        }]);
    })
}

#[test]
fn queued_work_attempt_launches_past_unrelated_multi_target_direction() -> Result<(), Box<dyn Error>>
{
    assert_queued_work_attempt_launches(|view| set_multi_target_direction(view, "unresolved"))?;
    assert_queued_work_attempt_launches(|view| set_multi_target_direction(view, "resolved"))?;
    Ok(())
}

fn set_multi_target_direction(view: &mut SwarmObservation, status: &str) {
    let blocker_id = "direction:multiple-targets";
    let resolved = status == "resolved";
    view.activity["blockers"] = json!([{
        "blocker_id":blocker_id, "revision":if resolved { 2 } else { 1 }, "execution_epoch":1,
        "status":status, "scope":"work", "work_id":null, "summary":"Revalidate both branches.",
        "reported_by_member_id":"human-1", "evidence_refs":[]
    }]);
    view.activity["work_items"][0]["blocker_ids"] = json!([blocker_id]);
    if resolved {
        view.activity["work_items"][0]["status"] = json!("open");
        view.activity["work_items"][0]["revision"] = json!(4);
    }
    if let Some(work_items) = view.activity["work_items"].as_array_mut() {
        work_items.push(json!({
            "work_id":"work-also-affected", "revision":if resolved { 4 } else { 3 }, "execution_epoch":1,
            "kind":"goal", "status":if resolved { "open" } else { "blocked" }, "blocker_ids":[blocker_id],
            "owner_member_id":null, "active_attempt_id":null
        }));
    }
}

fn assert_queued_work_attempt_launches(mutate: ObservationMutation) -> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let work = temporary.path().join("work");
    let execution_root = temporary.path().join("execution");
    fs::create_dir(&work)?;
    let (control, server) = start_daemon_with_cap(&execution_root, 64, 0)?;
    let backend = DirectionBackend::new(&work)?;
    let mut coordinator = SwarmCoordinator::open(
        SwarmApplication::new(backend.clone()),
        ExecutionControlClient::open(&execution_root, Duration::from_secs(10))?,
        controlled_capabilities()?,
        ArtifactWorkspace::open(&work, &temporary.path().join("artifacts"))?,
    )?;
    let plan = controlled_work_attempt_plan(
        "invocation-queued",
        "worker-b",
        "work-unaffected",
        "attempt-unaffected",
        1,
    )?;
    coordinator.stage(plan.clone())?;
    assert!(coordinator.dispatch()?.is_empty());

    // Another branch changes while this exact owned Work Attempt waits for capacity.
    backend.issue_targeted_direction();
    backend
        .state
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .observation_mutation = Some(mutate);
    control.request(ControlCommand::SetProviderCap {
        provider: ProviderKind::Controlled,
        limit: 1,
    })?;
    assert_eq!(
        coordinator.dispatch()?,
        vec![CoordinatorEvent::Launched {
            invocation_id: plan.invocation_id.clone(),
        }]
    );
    thread::sleep(Duration::from_secs(1));
    assert!(coordinator.harvest()?.iter().any(|event| matches!(
        event,
        CoordinatorEvent::Accepted { invocation_id, duplicate: false }
            if invocation_id == &plan.invocation_id
    )));
    let accepted = backend.accepted();
    assert_eq!(accepted.len(), 1);
    assert_eq!(accepted[0].based_on_room_seq, 8);
    assert_eq!(accepted[0].payload["expected_work_revision"], json!(2));
    assert_eq!(accepted[0].payload["expected_attempt_revision"], json!(1));

    drop(coordinator);
    drop(control);
    drain_daemon(&execution_root, server)?;
    Ok(())
}

fn work_attempt_authority_mutations() -> [(&'static str, ObservationMutation); 18] {
    [
        ("work-revision", |view| {
            view.activity["work_items"][1]["revision"] = json!(3);
        }),
        ("attempt-revision", |view| {
            view.activity["work_attempts"][1]["revision"] = json!(2);
        }),
        ("ownership", |view| {
            view.activity["work_items"][1]["owner_member_id"] = json!("member-1");
        }),
        ("configuration", |view| {
            view.swarm.roster[1].configuration_revision = 2;
            view.activity["roster"][1]["configuration_revision"] = json!(2);
        }),
        ("model", |view| {
            "fixture-v2".clone_into(&mut view.swarm.roster[1].requested_model);
            view.activity["roster"][1]["requested_model"] = json!("fixture-v2");
        }),
        ("effort", |view| {
            view.swarm.roster[1].requested_effort = Some("high".to_owned());
            view.activity["roster"][1]["requested_effort"] = json!("high");
        }),
        ("phase", |view| {
            view.activity["phase"] = json!("completed");
        }),
        ("execution-epoch", |view| {
            view.activity["execution_epoch"] = json!(2);
        }),
        ("offer-schema", |view| {
            view.action_offers[0].payload_schema_digest = format!("blake3:{}", "c".repeat(64));
        }),
        ("resource-conflict", |view| {
            view.activity["resource_conflicts"] = json!([{
                "status":"unresolved", "affected_work_ids":["work-unaffected"]
            }]);
        }),
        ("goal-blocker", |view| {
            view.activity["blockers"] = json!([{
                "status":"unresolved", "scope":"goal"
            }]);
        }),
        ("escalated-problem", |view| {
            view.activity["problems"] = json!([{
                "status":"escalated", "scope":"work", "affected_work_ids":["work-unaffected"]
            }]);
        }),
        ("missing-blockers", |view| {
            if let Some(activity) = view.activity.as_object_mut() {
                activity.remove("blockers");
            }
        }),
        ("malformed-conflict-work-ids", |view| {
            view.activity["resource_conflicts"] = json!([{
                "status":"unresolved", "affected_work_ids":"work-unaffected"
            }]);
        }),
        ("missing-blocker-status", |view| {
            view.activity["blockers"] = json!([{"scope":"goal"}]);
        }),
        ("unknown-problem-status", |view| {
            view.activity["problems"] = json!([{
                "status":"unexpected", "scope":"work", "affected_work_ids":["work-unaffected"]
            }]);
        }),
        ("non-object-entry", |view| {
            view.activity["blockers"] = json!(["unresolved"]);
        }),
        ("multi-target-direction-affects-owner", |view| {
            set_multi_target_direction(view, "unresolved");
            view.activity["work_items"][1]["blocker_ids"] = json!(["direction:multiple-targets"]);
        }),
    ]
}

#[test]
fn queued_work_attempt_rejects_changed_semantic_authority_before_launch()
-> Result<(), Box<dyn Error>> {
    for (name, mutate) in work_attempt_authority_mutations() {
        let temporary = tempfile::tempdir()?;
        let work = temporary.path().join("work");
        let execution_root = temporary.path().join("execution");
        fs::create_dir(&work)?;
        let (control, server) = start_daemon_with_cap(&execution_root, 64, 0)?;
        let backend = DirectionBackend::new(&work)?;
        let mut coordinator = SwarmCoordinator::open(
            SwarmApplication::new(backend.clone()),
            ExecutionControlClient::open(&execution_root, Duration::from_secs(10))?,
            controlled_capabilities()?,
            ArtifactWorkspace::open(&work, &temporary.path().join("artifacts"))?,
        )?;
        let plan = controlled_work_attempt_plan(
            "invocation-queued",
            "worker-b",
            "work-unaffected",
            "attempt-unaffected",
            1,
        )?;
        coordinator.stage(plan.clone())?;
        backend.issue_targeted_direction();
        backend
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .observation_mutation = Some(mutate);
        control.request(ControlCommand::SetProviderCap {
            provider: ProviderKind::Controlled,
            limit: 1,
        })?;
        assert_eq!(
            coordinator.dispatch()?,
            vec![CoordinatorEvent::NeedsReevaluation {
                invocation_id: plan.invocation_id.clone(),
            }],
            "{name}"
        );
        let intent = coordinator.intent(&plan.invocation_id)?;
        assert_eq!(
            intent.state,
            CoordinatorIntentState::NeedsReevaluation,
            "{name}"
        );
        assert_eq!(
            intent.submission,
            SubmissionDisposition::NotSubmitted(NonSubmissionReason::AuthorityChanged),
            "{name}"
        );
        assert!(backend.accepted().is_empty(), "{name}");
        drop(coordinator);
        drop(control);
        drain_daemon(&execution_root, server)?;
    }
    Ok(())
}

fn drain_daemon(execution_root: &Path, server: DaemonThread) -> Result<(), Box<dyn Error>> {
    let drain = ExecutionControlClient::open(execution_root, Duration::from_millis(100))?;
    for _ in 0..256 {
        if drain
            .request(ControlCommand::Status { swarm_id: None })
            .is_err()
        {
            break;
        }
    }
    drop(drain);
    let daemon = server.join().map_err(|_| "daemon thread panicked")??;
    drop(daemon);
    Ok(())
}

#[test]
fn next_invocation_revision_binds_model_effort_and_session() -> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let work = temporary.path().join("work");
    let artifacts_root = temporary.path().join("artifacts");
    let execution_root = temporary.path().join("execution");
    fs::create_dir(&work)?;
    let (control, server) = start_daemon(&execution_root, 8)?;
    let capabilities = controlled_capabilities()?;
    let backend = AuthoritativeBackend::with_uncertain_reply(&work, false)?;
    let artifacts = ArtifactWorkspace::open(&work, &artifacts_root)?;
    let mut coordinator = SwarmCoordinator::open(
        SwarmApplication::new(backend.clone()),
        ExecutionControlClient::open(&execution_root, Duration::from_secs(10))?,
        capabilities.clone(),
        artifacts.clone(),
    )?;

    let (_, first) = controlled_contribution_plan()?;
    assert_initial_configuration(&coordinator.stage(first.clone())?);

    let mut conflicting = first.clone();
    conflicting.invocation_id = "invocation-conflicting-selection".to_owned();
    conflicting.due_sequence = 2;
    conflicting.model = "fixture-v2".to_owned();
    conflicting.effort = Some("high".to_owned());
    conflicting.session = SessionSelection::Fresh {
        requested_id: Some("session-v2".to_owned()),
    };
    assert!(matches!(
        coordinator.stage(conflicting.clone()),
        Err(CoordinatorError::Conflict)
    ));

    let mut unsafe_resume = conflicting.clone();
    unsafe_resume.invocation_id = "invocation-unsafe-resume".to_owned();
    unsafe_resume.configuration_revision = 42;
    unsafe_resume.session = SessionSelection::Resume {
        session_id: "session-v1".to_owned(),
    };
    assert!(matches!(
        coordinator.stage(unsafe_resume),
        Err(CoordinatorError::Conflict)
    ));

    let mut next = conflicting;
    next.invocation_id = "invocation-2".to_owned();
    next.configuration_revision = 42;
    backend.select_configuration("fixture-v2", Some("high"));
    let next_view = coordinator.stage(next.clone())?;
    assert_eq!(next_view.configuration_revision, 42);
    assert_eq!(next_view.requested_model, "fixture-v2");
    assert_eq!(next_view.requested_effort.as_deref(), Some("high"));
    assert_eq!(next_view.session, next.session);

    assert!(coordinator.dispatch()?.iter().any(|event| matches!(
        event,
        CoordinatorEvent::NotSubmitted {
            invocation_id,
            reason: worldstream_agent_swarm::NonSubmissionReason::ConfigurationSuperseded,
        } if invocation_id == "invocation-1"
    )));

    let mut reused_revision = next;
    reused_revision.invocation_id = "invocation-conflicting-session".to_owned();
    reused_revision.due_sequence = 3;
    reused_revision.session = SessionSelection::Fresh {
        requested_id: Some("different-session".to_owned()),
    };
    assert!(matches!(
        coordinator.stage(reused_revision),
        Err(CoordinatorError::Conflict)
    ));

    let mut stale = first;
    stale.invocation_id = "invocation-stale-configuration".to_owned();
    stale.due_sequence = 4;
    assert!(matches!(
        coordinator.stage(stale),
        Err(CoordinatorError::Conflict)
    ));

    drop(coordinator);
    drop(control);
    let reopened = SwarmCoordinator::open(
        SwarmApplication::new(backend),
        ExecutionControlClient::open(&execution_root, Duration::from_secs(10))?,
        capabilities,
        artifacts,
    )?;
    assert_eq!(reopened.intent("invocation-1")?.configuration_revision, 41);
    let reopened_next = reopened.intent("invocation-2")?;
    assert_eq!(reopened_next.configuration_revision, 42);
    assert_eq!(reopened_next.requested_model, "fixture-v2");
    assert_eq!(reopened_next.session, next_view.session);
    assert!(matches!(
        reopened.intent("invocation-conflicting-session"),
        Err(CoordinatorError::NotFound)
    ));
    assert!(matches!(
        reopened.intent("invocation-stale-configuration"),
        Err(CoordinatorError::NotFound)
    ));
    drop(reopened);
    let daemon = server.join().map_err(|_| "daemon thread panicked")??;
    drop(daemon);
    Ok(())
}

#[test]
fn running_invocation_keeps_its_launch_bound_selection_after_human_update()
-> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let work = temporary.path().join("work");
    let artifacts_root = temporary.path().join("artifacts");
    let execution_root = temporary.path().join("execution");
    fs::create_dir(&work)?;
    let (control, server) = start_daemon(&execution_root, 11)?;
    let backend = AuthoritativeBackend::with_uncertain_reply(&work, false)?;
    let artifacts = ArtifactWorkspace::open(&work, &artifacts_root)?;
    let mut coordinator = SwarmCoordinator::open(
        SwarmApplication::new(backend.clone()),
        ExecutionControlClient::open(&execution_root, Duration::from_secs(10))?,
        controlled_capabilities()?,
        artifacts,
    )?;
    let (_, plan) = controlled_contribution_plan()?;
    coordinator.stage(plan)?;
    assert!(coordinator.dispatch()?.iter().any(|event| matches!(
        event,
        CoordinatorEvent::Launched { invocation_id } if invocation_id == "invocation-1"
    )));

    // The authoritative Human selection advances while the exact old process
    // is already owned by the daemon. It must not alter that process, but the
    // completed proposal is rebound to the fresh Head and offer.
    backend.select_configuration("fixture-v2", Some("high"));
    thread::sleep(Duration::from_secs(1));
    assert!(coordinator.harvest()?.iter().any(|event| matches!(
        event,
        CoordinatorEvent::Accepted { invocation_id, .. } if invocation_id == "invocation-1"
    )));
    let settled = coordinator.intent("invocation-1")?;
    assert_eq!(settled.configuration_revision, 41);
    assert_eq!(settled.requested_model, "fixture-v1");
    let accepted = backend.accepted();
    assert_eq!(accepted.len(), 1);
    assert_eq!(accepted[0].based_on_room_seq, 8);

    drop(coordinator);
    drop(control);
    let daemon = server.join().map_err(|_| "daemon thread panicked")??;
    drop(daemon);
    Ok(())
}

fn controlled_capabilities() -> Result<
    BTreeMap<ProviderKind, worldstream_agent_swarm::execution::ProviderCapabilities>,
    Box<dyn Error>,
> {
    struct ControlledMetadataProbe;

    impl ProviderProbe for ControlledMetadataProbe {
        fn run(
            &self,
            _executable: &Path,
            arguments: &[&str],
        ) -> Result<ProbeOutput, ProviderError> {
            let stdout = match arguments {
                ["--version"] => "worldstream-controlled-worker 1",
                ["--help"] => {
                    "worldstream-controlled-worker invoke --invocation-id ID --model MODEL --effort EFFORT --new-session [ID] --resume ID --behavior BEHAVIOR"
                }
                _ => return Err(ProviderError::Unsupported),
            };
            Ok(ProbeOutput {
                success: true,
                stdout: stdout.to_owned(),
                stderr: String::new(),
            })
        }
    }

    let provider = ProviderRegistry::new().inspect(
        ProviderKind::Controlled,
        &ControlledMetadataProbe,
        Path::new(CONTROLLED),
    );
    Ok(BTreeMap::from([(
        ProviderKind::Controlled,
        provider
            .capabilities
            .ok_or("controlled capability missing")?,
    )]))
}

fn assert_initial_configuration(staged: &CoordinatorIntentView) {
    assert_eq!(staged.state, CoordinatorIntentState::Queued);
    assert_eq!(staged.configuration_revision, 41);
    assert_eq!(staged.requested_model, "fixture-v1");
    assert_eq!(staged.requested_effort.as_deref(), Some("medium"));
    assert_eq!(
        staged.session,
        SessionSelection::Fresh { requested_id: None }
    );
    assert!(staged.proposal.is_none());
    assert!(staged.action.is_none());
}

fn start_daemon(
    execution_root: &Path,
    request_count: usize,
) -> Result<(ExecutionControlClient, DaemonThread), Box<dyn Error>> {
    start_daemon_with_cap(execution_root, request_count, 1)
}

fn start_daemon_with_cap(
    execution_root: &Path,
    request_count: usize,
    provider_cap: u16,
) -> Result<(ExecutionControlClient, DaemonThread), Box<dyn Error>> {
    let mut daemon = ExecutionDaemon::bind_with_guard(execution_root, Path::new(GUARD))?;
    let server = thread::spawn(move || {
        for _ in 0..request_count {
            daemon.serve_once()?;
        }
        Ok(daemon)
    });
    let control = ExecutionControlClient::open(execution_root, Duration::from_secs(10))?;
    control.request(ControlCommand::RegisterSwarm {
        swarm_id: "swarm-coordinator".to_owned(),
        priority: 1,
        budget: RunBudget::default(),
        roster_provider_counts: BTreeMap::new(),
    })?;
    control.request(ControlCommand::SetProviderCap {
        provider: ProviderKind::Controlled,
        limit: provider_cap,
    })?;
    control.request(ControlCommand::Resume {
        swarm_id: "swarm-coordinator".to_owned(),
    })?;
    Ok((control, server))
}

fn controlled_contribution_plan() -> Result<(Value, WorkerActionPlan), Box<dyn Error>> {
    let proposed_payload = json!({
        "completes_work": true,
        "contribution_id": "contribution-1",
        "expected_attempt_revision": 1,
        "expected_work_revision": 1,
        "resource_basis": [],
        "source_refs": ["controlled-source"],
        "summary": "Worker-authored controlled contribution.",
        "work_id": "work-1"
    });
    let proposal = json!({
        "schema": "worldstream/agent-swarm-action-proposal@1",
        "action_type": "submit_contribution",
        "payload": proposed_payload,
        "artifact": {
            "artifact_id": "artifact-1",
            "local_path": "contributions/worker-a.txt",
            "media_type": "text/plain"
        }
    });
    Ok((
        proposed_payload,
        WorkerActionPlan {
            invocation_id: "invocation-1".to_owned(),
            swarm_id: SwarmId::new("swarm-coordinator".to_owned())?,
            member_key: "worker-a".to_owned(),
            allowed_action_types: vec!["submit_contribution".to_owned()],
            provider: ProviderKind::Controlled,
            configuration_revision: 41,
            model: "fixture-v1".to_owned(),
            effort: Some("medium".to_owned()),
            moving_alias_acknowledged: false,
            resource_policy: ResourcePolicy::WorkspaceWrite,
            allowed_tools: Vec::new(),
            session: SessionSelection::Fresh { requested_id: None },
            kind: InvocationKind::Work,
            due_sequence: 1,
            semantic_target: Some(WorkerSemanticTarget::ControlledFixtureAdmin {
                fixture_id: "coordinator-integration".to_owned(),
            }),
            instruction: serde_json::to_string(&proposal)?,
        },
    ))
}

fn controlled_late_contribution_plan() -> Result<WorkerActionPlan, Box<dyn Error>> {
    let proposal = json!({
        "schema":"worldstream/agent-swarm-action-proposal@1",
        "action_type":"record_late_contribution",
        "payload":{
            "attempt_id":"attempt-work-1",
            "expected_work_revision":3,
            "late_id":"late-1",
            "summary":"The exact delayed output arrived after Result acceptance.",
            "work_id":"work-1"
        },
        "artifact":{
            "artifact_id":"artifact-late-1",
            "local_path":"late/worker-a.txt",
            "media_type":"text/plain"
        }
    });
    Ok(WorkerActionPlan {
        invocation_id: "invocation-late-1".to_owned(),
        swarm_id: SwarmId::new("swarm-coordinator".to_owned())?,
        member_key: "worker-a".to_owned(),
        allowed_action_types: vec!["record_late_contribution".to_owned()],
        provider: ProviderKind::Controlled,
        configuration_revision: 41,
        model: "fixture-v1".to_owned(),
        effort: Some("medium".to_owned()),
        moving_alias_acknowledged: false,
        resource_policy: ResourcePolicy::WorkspaceWrite,
        allowed_tools: Vec::new(),
        session: SessionSelection::Fresh { requested_id: None },
        kind: InvocationKind::Work,
        due_sequence: 1,
        semantic_target: Some(WorkerSemanticTarget::LateContribution {
            execution_epoch: 3,
            work_id: "work-1".to_owned(),
            work_revision: 2,
            attempt_id: "attempt-work-1".to_owned(),
            attempt_revision: 1,
            late_id: "late-1".to_owned(),
        }),
        instruction: serde_json::to_string(&proposal)?,
    })
}

fn controlled_work_attempt_plan(
    invocation_id: &str,
    member_key: &str,
    work_id: &str,
    attempt_id: &str,
    due_sequence: u64,
) -> Result<WorkerActionPlan, Box<dyn Error>> {
    let proposal = json!({
        "schema":"worldstream/agent-swarm-action-proposal@1",
        "action_type":"submit_contribution",
        "payload":{
            "completes_work":true,
            "contribution_id":format!("contribution-{work_id}"),
            "expected_attempt_revision":1,
            "expected_work_revision":2,
            "resource_basis":[],
            "source_refs":["controlled-source"],
            "summary":format!("Complete {work_id}."),
            "work_id":work_id
        },
        "artifact":{
            "artifact_id":format!("artifact-{work_id}"),
            "local_path":format!("direction/{member_key}.txt"),
            "media_type":"text/plain"
        }
    });
    Ok(WorkerActionPlan {
        invocation_id: invocation_id.to_owned(),
        swarm_id: SwarmId::new("swarm-coordinator".to_owned())?,
        member_key: member_key.to_owned(),
        allowed_action_types: vec!["submit_contribution".to_owned()],
        provider: ProviderKind::Controlled,
        configuration_revision: 1,
        model: "fixture-v1".to_owned(),
        effort: Some("medium".to_owned()),
        moving_alias_acknowledged: false,
        resource_policy: ResourcePolicy::WorkspaceWrite,
        allowed_tools: Vec::new(),
        session: SessionSelection::Fresh { requested_id: None },
        kind: InvocationKind::Work,
        due_sequence,
        semantic_target: Some(WorkerSemanticTarget::WorkAttempt {
            execution_epoch: 1,
            work_id: work_id.to_owned(),
            work_revision: 2,
            attempt_id: attempt_id.to_owned(),
            attempt_revision: 1,
        }),
        instruction: serde_json::to_string(&proposal)?,
    })
}

fn assert_accepted_contribution(
    accepted: &ExactSwarmAction,
    mut expected_payload: Value,
    work: &Path,
) -> Result<(), Box<dyn Error>> {
    assert!(
        accepted
            .action_id
            .parse::<worldstream_protocol::UlidString>()
            .is_ok()
    );
    assert_eq!(accepted.offer_id, "offer-submit");
    assert_eq!(accepted.based_on_room_seq, 7);
    assert_eq!(accepted.action_type, "submit_contribution");
    expected_payload["artifact"] = json!({
        "artifact_id": "artifact-1",
        "digest": format!(
            "blake3:{}",
            blake3::hash(b"controlled contribution from invocation-1\n").to_hex()
        ),
        "local_path": fs::canonicalize(work)?.join("contributions/worker-a.txt"),
        "media_type": "text/plain"
    });
    assert_eq!(accepted.payload, expected_payload);
    assert_eq!(
        fs::read_to_string(work.join("contributions/worker-a.txt"))?,
        "controlled contribution from invocation-1\n"
    );
    Ok(())
}

#[test]
fn action_proposal_decoder_rejects_ambiguous_or_unbounded_shapes() {
    let valid = json!({
        "schema": "worldstream/agent-swarm-action-proposal@1",
        "action_type": "propose_work_item",
        "payload": {"summary": "bounded"}
    });
    assert!(decode_action_proposal(&valid.to_string()).is_ok());

    let mut unknown_field = valid.clone();
    unknown_field["commentary"] = json!("not part of the contract");
    assert!(decode_action_proposal(&unknown_field.to_string()).is_err());
    assert!(decode_action_proposal(&format!("```json\n{valid}\n```")).is_err());

    let mut non_object_payload = valid.clone();
    non_object_payload["payload"] = json!(["ambiguous"]);
    assert!(decode_action_proposal(&non_object_payload.to_string()).is_err());

    let mut bad_digest = valid;
    bad_digest["artifact"] = json!({
        "artifact_id": "artifact-1",
        "local_path": "contributions/worker-a.txt",
        "media_type": "text/plain",
        "expected_digest": "blake3:not-a-digest"
    });
    assert!(decode_action_proposal(&bad_digest.to_string()).is_err());

    bad_digest["artifact"]["expected_digest"] = Value::Null;
    bad_digest["artifact"]["inline_text"] = json!("model-authored source");
    assert!(decode_action_proposal(&bad_digest.to_string()).is_ok());
    bad_digest["artifact"]["inline_text"] = json!("x".repeat(65537));
    assert!(decode_action_proposal(&bad_digest.to_string()).is_err());
}

#[test]
fn inline_contribution_captures_model_bytes_at_host_selected_path() -> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let work = temporary.path().join("work");
    fs::create_dir(&work)?;
    let execution_root = temporary.path().join("execution");
    let (control, server) = start_daemon(&execution_root, 80)?;
    let backend = AuthoritativeBackend::with_uncertain_reply(&work, false)?;
    let artifacts = ArtifactWorkspace::open(&work, &temporary.path().join("artifacts"))?;
    let mut coordinator = SwarmCoordinator::open(
        SwarmApplication::new(backend.clone()),
        ExecutionControlClient::open(&execution_root, Duration::from_secs(10))?,
        controlled_capabilities()?,
        artifacts,
    )?;
    let (_, mut plan) = controlled_contribution_plan()?;
    let mut proposal: Value = serde_json::from_str(&plan.instruction)?;
    let generated = "print('model-authored source')\n";
    proposal["artifact"]["inline_text"] = json!(generated);
    plan.instruction = serde_json::to_string(&proposal)?;
    coordinator.stage(plan)?;
    coordinator.dispatch()?;
    thread::sleep(Duration::from_secs(1));
    coordinator.harvest()?;
    let accepted = backend.accepted();
    assert_eq!(accepted.len(), 1);
    let path = accepted[0].payload["artifact"]["local_path"]
        .as_str()
        .ok_or("missing generated artifact")?;
    assert_eq!(fs::read_to_string(path)?, generated);
    assert_eq!(
        Path::new(path).file_name().and_then(|name| name.to_str()),
        Some(
            format!(
                ".swarm-generated-{}.txt",
                blake3::hash(generated.as_bytes()).to_hex()
            )
            .as_str()
        )
    );
    // The fixture writes different bytes to the model's path hint. The host
    // must publish and attribute the returned inline bytes independently.
    assert_ne!(
        fs::read_to_string(work.join("contributions/worker-a.txt"))?,
        generated
    );
    drop(coordinator);
    drop(control);
    drain_daemon(&execution_root, server)?;
    Ok(())
}

#[test]
fn empty_candidate_contribution_refs_never_reach_backend() -> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let work = temporary.path().join("work");
    fs::create_dir(&work)?;
    let execution_root = temporary.path().join("execution");
    let (control, server) = start_daemon(&execution_root, 80)?;
    let backend = AuthoritativeBackend::with_uncertain_reply(&work, false)?;
    backend.offer_candidate_schema();
    let artifacts = ArtifactWorkspace::open(&work, &temporary.path().join("artifacts"))?;
    let mut coordinator = SwarmCoordinator::open(
        SwarmApplication::new(backend.clone()),
        ExecutionControlClient::open(&execution_root, Duration::from_secs(10))?,
        controlled_capabilities()?,
        artifacts,
    )?;
    let (_, mut plan) = controlled_contribution_plan()?;
    plan.allowed_action_types = vec!["submit_candidate".to_owned()];
    let mut proposal: Value = serde_json::from_str(&plan.instruction)?;
    proposal["action_type"] = json!("submit_candidate");
    proposal["payload"] = json!({
        "candidate_id":"result-fixture",
        "contribution_refs":[],
        "expected_work_revision":1,
        "resource_basis":[],
        "work_id":"work-1"
    });
    plan.instruction = serde_json::to_string(&proposal)?;
    coordinator.stage(plan)?;
    coordinator.dispatch()?;
    thread::sleep(Duration::from_secs(1));
    let events = coordinator.harvest()?;
    assert!(events.iter().any(|event| matches!(
        event,
        CoordinatorEvent::NotSubmitted {
            reason: NonSubmissionReason::ProviderOutputInvalid,
            ..
        }
    )));
    assert!(
        backend.accepted().is_empty(),
        "invalid Action reached backend"
    );
    assert_eq!(backend.submit_attempts(), 0);
    let intent = coordinator.intent("invocation-1")?;
    assert!(intent.action.is_none());
    let feedback = intent
        .validation_feedback
        .ok_or("missing schema feedback")?;
    assert!(feedback.contains("$.contribution_refs: array is shorter than minItems"));
    assert!(feedback.contains("No Action was submitted"));
    drop(coordinator);
    let mut reopened = SwarmCoordinator::open(
        SwarmApplication::new(backend.clone()),
        ExecutionControlClient::open(&execution_root, Duration::from_secs(10))?,
        controlled_capabilities()?,
        ArtifactWorkspace::open(&work, &temporary.path().join("artifacts"))?,
    )?;
    assert!(
        reopened
            .intent("invocation-1")?
            .validation_feedback
            .is_some()
    );
    assert!(reopened.harvest()?.is_empty());
    assert!(
        backend.accepted().is_empty(),
        "reopen retried invalid Action"
    );
    assert_eq!(backend.submit_attempts(), 0);
    drop(reopened);
    drop(control);
    drain_daemon(&execution_root, server)?;
    Ok(())
}

#[test]
fn schema_lookup_unavailable_fails_closed_without_model_blame() -> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let work = temporary.path().join("work");
    fs::create_dir(&work)?;
    let execution_root = temporary.path().join("execution");
    let (control, server) = start_daemon(&execution_root, 80)?;
    let backend = AuthoritativeBackend::with_uncertain_reply(&work, false)?;
    backend.offer_candidate_schema();
    backend.fail_schema_lookup();
    let artifacts = ArtifactWorkspace::open(&work, &temporary.path().join("artifacts"))?;
    let mut coordinator = SwarmCoordinator::open(
        SwarmApplication::new(backend.clone()),
        ExecutionControlClient::open(&execution_root, Duration::from_secs(10))?,
        controlled_capabilities()?,
        artifacts,
    )?;
    let (_, mut plan) = controlled_contribution_plan()?;
    plan.allowed_action_types = vec!["submit_candidate".to_owned()];
    let mut proposal: Value = serde_json::from_str(&plan.instruction)?;
    proposal["action_type"] = json!("submit_candidate");
    proposal["payload"] = json!({
        "candidate_id":"result-fixture",
        "contribution_refs":[{"contribution_id":"source-1","version":1}],
        "expected_work_revision":1,
        "resource_basis":[],
        "work_id":"work-1"
    });
    plan.instruction = serde_json::to_string(&proposal)?;
    coordinator.stage(plan)?;
    coordinator.dispatch()?;
    thread::sleep(Duration::from_secs(1));
    assert!(coordinator.harvest().is_err());
    let intent = coordinator.intent("invocation-1")?;
    assert_eq!(intent.state, CoordinatorIntentState::ProposalRetained);
    assert!(intent.validation_feedback.is_none());
    assert_eq!(backend.submit_attempts(), 0);
    drop(coordinator);
    drop(control);
    drain_daemon(&execution_root, server)?;
    Ok(())
}

fn drop_third_control_response(
    execution_root: &Path,
) -> Result<(Vec<u8>, ProxyThread), Box<dyn Error>> {
    let publication_path = execution_root.join("execution-control.v1.json");
    let original = fs::read(&publication_path)?;
    let mut record: serde_json::Value = serde_json::from_slice(&original)?;
    let upstream: SocketAddr = record
        .get("endpoint")
        .and_then(serde_json::Value::as_str)
        .ok_or("daemon endpoint missing")?
        .parse()?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    record["endpoint"] = json!(listener.local_addr()?.to_string());
    fs::write(&publication_path, serde_json::to_vec_pretty(&record)?)?;

    let proxy = thread::spawn(move || -> Result<(), String> {
        for request_index in 0..3 {
            let (mut downstream, _) = listener.accept().map_err(|error| error.to_string())?;
            let mut upstream_stream =
                TcpStream::connect(upstream).map_err(|error| error.to_string())?;
            let mut request = Vec::new();
            BufReader::new(downstream.try_clone().map_err(|error| error.to_string())?)
                .read_until(b'\n', &mut request)
                .map_err(|error| error.to_string())?;
            upstream_stream
                .write_all(&request)
                .map_err(|error| error.to_string())?;
            let mut response = Vec::new();
            BufReader::new(upstream_stream)
                .read_until(b'\n', &mut response)
                .map_err(|error| error.to_string())?;
            if request_index < 2 {
                downstream
                    .write_all(&response)
                    .map_err(|error| error.to_string())?;
            }
        }
        Ok(())
    });
    Ok((original, proxy))
}
