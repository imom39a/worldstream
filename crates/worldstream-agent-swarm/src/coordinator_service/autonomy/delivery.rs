//! Policy-bound delivery operations selected by retained LLM decisions.
//! Models choose operations; this module owns exact evidence, Human transport,
//! process control and writeback ordering. It contains no task decomposition.

use super::*;
use crate::{
    ArtifactPath, ArtifactRef, ArtifactWorkspace, AuthoritativeArtifactRef, CodeChangeService,
    ContentDigest, ExactSwarmAction, PackArtifactRef, PackCandidateRef, PackVersionRef,
    PrepareCodeChangeRequest, RecordCodeReviewRequest, RecordedCodeCheck, RunCodeCheckRequest,
    SealCodeChangeRequest, SealedCodeChange, StageCodeWriteBackRequest,
    artifacts::read_stable_regular_file,
    checks::{ReviewVerdict, ReviewerKind},
};

/// Explicit local authorization for one artifact delivered to one resource.
/// Its presence opts in; omitted policies retain contribution-only behavior.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryPolicy {
    pub target: ArtifactPath,
    pub resource_id: String,
    pub expected_resource_version: u64,
    pub maximum_candidate_versions: u64,
    pub process_guard: PathBuf,
    pub checks: Vec<DeliveryCheckPolicy>,
}

/// A trusted checker command. Models cannot replace it or supply its argv.
/// A standalone `{candidate_root}` argument receives the isolated directory;
/// `{candidate_root_json}` inside an argument receives a JSON-quoted path.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryCheckPolicy {
    pub check_id: String,
    pub program: PathBuf,
    pub arguments: Vec<String>,
    pub timeout_seconds: u64,
}

impl DeliveryPolicy {
    pub(super) fn validate(&self) -> Result<(), CoordinatorServiceError> {
        if !valid_identifier(&self.resource_id)
            || self.expected_resource_version == 0
            || !(1..=8).contains(&self.maximum_candidate_versions)
            || !self.process_guard.is_absolute()
            || self.checks.is_empty()
            || self.checks.len() > 64
            || self.checks.iter().enumerate().any(|(index, check)| {
                check.check_id != format!("criterion-{}", index + 1)
                    || !check.program.is_absolute()
                    || !(1..=120).contains(&check.timeout_seconds)
                    || check.arguments.len() > 256
                    || check
                        .arguments
                        .iter()
                        .any(|arg| arg.contains('\0') || arg.len() > 4096)
            })
        {
            return Err(CoordinatorServiceError::InvalidState);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DeliveryState {
    goal_revision: u64,
    direction_revision: u64,
    criteria_revision: u64,
    criteria: Vec<String>,
    working_area: PathBuf,
    original: ArtifactRef,
    executables: BTreeMap<PathBuf, ContentDigest>,
    versions: BTreeMap<u64, VersionEvidence>,
    operations: BTreeMap<String, OperationRecord>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct VersionEvidence {
    sealed: SealedCodeChange,
    checks: BTreeMap<String, RecordedCodeCheck>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct OperationRecord {
    target: DeliveryOption,
    status: String,
    detail: String,
    actions: Vec<RetainedAction>,
    writeback: Option<crate::code_change::WriteBackStatus>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct RetainedAction {
    action: ExactSwarmAction,
    receipt: Option<SwarmActionReceipt>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DeliveryOption {
    pub target_id: String,
    candidate: PackCandidateRef,
    artifact_digest: String,
    operation: DeliveryOperation,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum DeliveryOperation {
    Check { check_id: String },
    Deliver,
}

type DeliveryResult<T> = Result<T, String>;

fn closed(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn entities<'a>(
    observation: &'a SwarmObservation,
    name: &str,
) -> Result<&'a [Value], CoordinatorServiceError> {
    bounded_array(observation.activity.get(name), 256)
}

fn reference(candidate: &Value) -> DeliveryResult<PackCandidateRef> {
    Ok(PackCandidateRef {
        candidate_id: required_identifier(candidate, "candidate_id")
            .map_err(closed)?
            .to_owned(),
        version: required_revision(candidate, "version").map_err(closed)?,
    })
}

fn artifact(candidate: &Value) -> DeliveryResult<AuthoritativeArtifactRef> {
    serde_json::from_value(candidate["artifact"].clone()).map_err(closed)
}

fn same_candidate(value: &Value, candidate: &PackCandidateRef) -> bool {
    value.get("candidate_id").and_then(Value::as_str) == Some(candidate.candidate_id.as_str())
        && value.get("version").and_then(Value::as_u64) == Some(candidate.version)
}

fn criteria(observation: &SwarmObservation) -> Result<Vec<String>, CoordinatorServiceError> {
    entities(observation, "acceptance_criteria")?
        .iter()
        .map(|value| {
            value
                .as_str()
                .filter(|text| !text.is_empty())
                .map(str::to_owned)
                .ok_or(CoordinatorServiceError::InvalidObservation)
        })
        .collect()
}

impl DeliveryState {
    pub(super) fn validate(&self, autonomy: &AutonomyState) -> Result<(), CoordinatorServiceError> {
        let policy = autonomy
            .policy
            .delivery
            .as_ref()
            .ok_or(CoordinatorServiceError::InvalidState)?;
        let expected_executables = policy
            .checks
            .iter()
            .map(|c| &c.program)
            .chain([&policy.process_guard])
            .collect::<BTreeSet<_>>();
        if self.goal_revision == 0
            || self.criteria_revision == 0
            || !self.working_area.is_absolute()
            || self.criteria.len() != policy.checks.len()
            || self
                .criteria
                .iter()
                .any(|c| c.trim().is_empty() || c.len() > 4096)
            || self.executables.keys().collect::<BTreeSet<_>>() != expected_executables
            || self.versions.iter().any(|(version, evidence)| {
                *version == 0
                    || *version > policy.maximum_candidate_versions
                    || evidence.checks.iter().any(|(id, check)| {
                        !policy.checks.iter().any(|c| &c.check_id == id)
                            || check.action_payload.check_id != *id
                            || check.action_payload.candidate.version != *version
                            || check.action_payload.candidate.candidate_id
                                != format!("result-{}", autonomy.namespace)
                            || check.action_payload.expected_criteria_revision
                                != self.criteria_revision
                    })
            })
            || self.operations.len() > autonomy.policy.invocation_limit
            || self.operations.iter().any(|(id, op)| {
                id.strip_suffix("-operation")
                    .is_none_or(|planning| !autonomy.consumed_decisions.contains(planning))
                    || !matches!(op.status.as_str(), "started" | "completed" | "failed")
                    || op.target.candidate.candidate_id != format!("result-{}", autonomy.namespace)
                    || op.target.candidate.version == 0
                    || op.target.candidate.version > policy.maximum_candidate_versions
                    || op.actions.len() > 3
                    || op.actions.iter().any(|a| {
                        a.action.actor != SwarmActor::HumanCoordinator
                            || !matches!(
                                a.action.action_type.as_str(),
                                "record_check"
                                    | "request_writeback"
                                    | "record_writeback_outcome"
                                    | "accept_result"
                            )
                    })
            })
        {
            return Err(CoordinatorServiceError::InvalidState);
        }
        Ok(())
    }

    pub(super) fn bind(
        policy: &DeliveryPolicy,
        observation: &SwarmObservation,
        root: &Path,
    ) -> Result<Self, CoordinatorServiceError> {
        policy.validate()?;
        let artifacts = ArtifactWorkspace::open(
            &observation.swarm.working_area,
            &root.join("delivery-artifacts"),
        )
        .map_err(|_| CoordinatorServiceError::UnsafeStorage)?;
        let resource = entities(observation, "resources")?
            .iter()
            .filter(|r| r["resource_id"].as_str() == Some(&policy.resource_id))
            .max_by_key(|r| r["version"].as_u64())
            .ok_or(CoordinatorServiceError::InvalidObservation)?;
        let path = artifacts.authorized_root().join(policy.target.as_str());
        if resource["version"].as_u64() != Some(policy.expected_resource_version)
            || resource["local_path"].as_str() != path.to_str()
        {
            return Err(CoordinatorServiceError::InvalidObservation);
        }
        let original = artifacts
            .capture_authoritative(
                &policy.target,
                &AuthoritativeArtifactRef {
                    artifact_id: policy.resource_id.clone(),
                    digest: resource["digest"]
                        .as_str()
                        .ok_or(CoordinatorServiceError::InvalidObservation)?
                        .to_owned(),
                    local_path: path.to_string_lossy().into_owned(),
                    media_type: "application/octet-stream".to_owned(),
                },
            )
            .map_err(|_| CoordinatorServiceError::InvalidObservation)?;
        let criteria = criteria(observation)?;
        if criteria.len() != policy.checks.len() {
            return Err(CoordinatorServiceError::InvalidState);
        }
        let executables = policy
            .checks
            .iter()
            .map(|c| &c.program)
            .chain([&policy.process_guard])
            .map(|path| {
                read_stable_regular_file(path)
                    .map(|bytes| (path.clone(), ContentDigest::of(&bytes)))
            })
            .collect::<Result<_, _>>()
            .map_err(|_| CoordinatorServiceError::UnsafeStorage)?;
        Ok(Self {
            goal_revision: required_revision(&observation.activity, "goal_revision")?,
            direction_revision: observation.activity["direction_revision"]
                .as_u64()
                .ok_or(CoordinatorServiceError::InvalidObservation)?,
            criteria_revision: required_revision(&observation.activity, "criteria_revision")?,
            criteria,
            working_area: artifacts.authorized_root().to_owned(),
            original,
            executables,
            versions: BTreeMap::new(),
            operations: BTreeMap::new(),
        })
    }

    fn matches(&self, policy: &DeliveryPolicy, observation: &SwarmObservation) -> bool {
        observation.activity["phase"].as_str() == Some("open")
            && observation.swarm.working_area == self.working_area
            && observation.activity["goal_revision"].as_u64() == Some(self.goal_revision)
            && observation.activity["direction_revision"].as_u64() == Some(self.direction_revision)
            && observation.activity["criteria_revision"].as_u64() == Some(self.criteria_revision)
            && criteria(observation).is_ok_and(|c| c == self.criteria)
            && entities(observation, "resources").is_ok_and(|resources| {
                resources
                    .iter()
                    .filter(|r| r["resource_id"].as_str() == Some(&policy.resource_id))
                    .max_by_key(|r| r["version"].as_u64())
                    .is_some_and(|r| {
                        r["version"].as_u64() == Some(policy.expected_resource_version)
                    })
            })
    }
}

impl CoordinatorService {
    pub(super) fn push_delivery_candidate_option(
        &self,
        options: &mut Vec<PlanningOption>,
        profile: &MemberExecutionProfile,
        observation: &SwarmObservation,
        target: WorkerSemanticTarget,
    ) -> Result<(), CoordinatorServiceError> {
        let state = self
            .state
            .autonomy
            .as_ref()
            .ok_or(CoordinatorServiceError::InvalidState)?;
        let Some(policy) = &state.policy.delivery else {
            return Ok(());
        };
        let candidate_id = format!("result-{}", state.namespace);
        let version = entities(observation, "candidates")?
            .iter()
            .filter(|c| c["candidate_id"].as_str() == Some(&candidate_id))
            .filter_map(|c| c["version"].as_u64())
            .max()
            .unwrap_or(0)
            + 1;
        if version > policy.maximum_candidate_versions {
            return Ok(());
        }
        let contributions = entities(observation, "contributions")?
            .iter()
            .filter(|c| {
                c["execution_epoch"]
                    .as_u64()
                    .is_none_or(|e| e == state.execution_epoch)
            })
            .map(|c| json!({"contribution_id":c["contribution_id"],"version":c["version"]}))
            .collect::<Vec<_>>();
        if contributions.is_empty() {
            return Ok(());
        }
        self.push_option(options,profile,target,vec!["submit_candidate".to_owned(),"report_work_blocker".to_owned()],state.policy.resource_policy,json!({
            "task":"integrate", "candidate_id":candidate_id,"next_version":version,"result_path":policy.target,
            "contribution_refs":contributions,"resource_basis":[{"resource_id":policy.resource_id,"version":policy.expected_resource_version}],
            "check_feedback":self.delivery_feedback()?,
            "instructions":"Check this Work Item's stated prerequisites against observed evidence before integration, even if dependency_ids is empty; report a blocker when required evidence is missing. Author the complete deliverable by integrating relevant digest-verified Contributions and addressing actual check/review feedback. Submit only submit_candidate with candidate_id exactly as supplied, work_id and expected_work_revision from the target, contribution_refs naming Contributions actually incorporated, and the supplied resource_basis. Never fabricate a Contribution reference to satisfy a schema minimum. Declare artifact {artifact_id: unique ID for this version, local_path: portable hint, media_type: appropriate type, inline_text: complete source/text}. Do not include expected_attempt_revision, summary or completes_work in a Candidate payload. Do not claim checks passed. If blocked, report_work_blocker with work_id, expected_work_revision, blocker_id, evidence_refs and summary. The host supplies no implementation."
        }), observation.room_seq)
    }

    pub(super) fn push_delivery_review_options(
        &self,
        options: &mut Vec<PlanningOption>,
        profiles: &BTreeMap<String, MemberExecutionProfile>,
        observation: &SwarmObservation,
    ) -> Result<(), CoordinatorServiceError> {
        let Ok((policy, delivery)) = self.delivery_state() else {
            return Ok(());
        };
        let state = self
            .state
            .autonomy
            .as_ref()
            .ok_or(CoordinatorServiceError::InvalidState)?;
        let Some(candidate) = self.delivery_candidate(observation) else {
            return Ok(());
        };
        let r = reference(candidate).map_err(|_| CoordinatorServiceError::InvalidObservation)?;
        if !delivery.versions.get(&r.version).is_some_and(|v| {
            policy
                .checks
                .iter()
                .all(|c| v.checks.contains_key(&c.check_id))
        }) {
            return Ok(());
        }
        let authors = Self::candidate_authors(observation, candidate)
            .map_err(|_| CoordinatorServiceError::InvalidObservation)?;
        let members = self.authenticated_worker_member_keys(observation)?;
        for profile in profiles.values() {
            let member = members
                .iter()
                .find_map(|(id, key)| (key == &profile.member_key).then_some(id))
                .ok_or(CoordinatorServiceError::InvalidObservation)?;
            let mut profile = profile.clone();
            profile.session = SessionSelection::Fresh { requested_id: None };
            let review_id = format!(
                "review-{}-v{}-{}",
                state.namespace, r.version, profile.member_key
            );
            if !authors.contains(member)
                && !entities(observation, "reviews")?
                    .iter()
                    .any(|v| v["review_id"].as_str() == Some(&review_id))
            {
                let target = WorkerSemanticTarget::CandidateReview {
                    execution_epoch: state.execution_epoch,
                    candidate_id: r.candidate_id.clone(),
                    candidate_version: r.version,
                    criteria_revision: delivery.criteria_revision,
                    review_id: review_id.clone(),
                };
                self.push_option(options,&profile,target,vec!["record_review".to_owned()],ResourcePolicy::ReadOnly,json!({
                    "task":"judge", "candidate":r,"review_id":review_id,"expected_criteria_revision":delivery.criteria_revision,
                    "resource_basis":candidate["resource_basis"],"check_feedback":self.delivery_feedback()?,
                    "instructions":"Independently judge the exact Candidate source, goal, Contributions and actual check evidence in this fresh conversation. Passing tests are not a reason to rubber-stamp. Return record_review with candidate, expected_criteria_revision, resource_basis, review_id exactly supplied; verdict passed/changes_requested/disputed; findings [{finding_id: unique ID, severity: blocking/advisory, summary: concrete finding}]. changes_requested requires a finding. Do not attach an artifact. You authored none of this Candidate's Contributions."
                }),observation.room_seq)?;
            }
            for finding in entities(observation, "findings")?.iter().filter(|f| {
                f["status"].as_str() != Some("resolved")
                    && f["candidate"]["candidate_id"].as_str() == Some(&r.candidate_id)
            }) {
                let target = WorkerSemanticTarget::FindingResolution {
                    execution_epoch: state.execution_epoch,
                    finding_id: required_identifier(finding, "finding_id")?.to_owned(),
                    finding_revision: required_revision(finding, "revision")?,
                    candidate_id: r.candidate_id.clone(),
                    candidate_version: required_revision(&finding["candidate"], "version")?,
                    review_id: required_identifier(finding, "review_id")?.to_owned(),
                };
                self.push_option(options,&profile,target,vec!["resolve_review_finding".to_owned()],ResourcePolicy::ReadOnly,json!({
                    "task":"resolve_finding","finding":finding,"latest_candidate":r,"latest_digest":candidate["artifact"]["digest"],"check_feedback":self.delivery_feedback()?,
                    "instructions":"Independently assess whether the earlier finding is resolved by the latest exact Candidate and verified checks. Submit resolve_review_finding with finding_id, expected_finding_revision, resolution resolved or disputed, and evidence_refs containing actual candidate/check identities. Only resolve when the evidence supports it; another passing review alone is not evidence the finding was fixed. Do not attach an artifact."
                }),observation.room_seq)?;
            }
        }
        Ok(())
    }

    fn delivery_state(&self) -> DeliveryResult<(&DeliveryPolicy, &DeliveryState)> {
        let state = self
            .state
            .autonomy
            .as_ref()
            .ok_or("Autonomy is disabled.")?;
        Ok((
            state
                .policy
                .delivery
                .as_ref()
                .ok_or("Delivery is not authorized.")?,
            state
                .delivery
                .as_ref()
                .ok_or("Delivery binding is missing.")?,
        ))
    }

    pub(super) fn delivery_scope_current(&self, observation: &SwarmObservation) -> bool {
        self.state.autonomy.as_ref().is_none_or(|state| {
            state.policy.delivery.as_ref().is_none_or(|policy| {
                state
                    .delivery
                    .as_ref()
                    .is_some_and(|d| d.matches(policy, observation))
            })
        })
    }

    pub(super) fn delivery_interrupted(&self) -> bool {
        self.delivery_state()
            .is_ok_and(|(_, d)| d.operations.values().any(|op| op.status == "started"))
    }

    pub(crate) fn delivery_has_uncertain_effects(&self) -> bool {
        self.delivery_state().is_ok_and(|(_, d)| {
            d.operations.values().any(|op| {
                op.status == "started"
                    || op.actions.iter().any(|a| a.receipt.is_none())
                    || op.writeback.as_ref().is_some_and(|w| {
                        w.disposition
                            == crate::code_change::WriteBackDisposition::ReconciliationRequired
                    })
            })
        })
    }

    fn delivery_candidate<'a>(&self, observation: &'a SwarmObservation) -> Option<&'a Value> {
        let state = self.state.autonomy.as_ref()?;
        let (policy, delivery) = self.delivery_state().ok()?;
        let id = format!("result-{}", state.namespace);
        entities(observation, "candidates").ok()?.iter()
            .filter(|c| c["candidate_id"].as_str() == Some(&id)
                && c["execution_epoch"].as_u64() == Some(state.execution_epoch)
                && c["criteria_revision"].as_u64() == Some(delivery.criteria_revision)
                && c["direction_revision"].as_u64() == Some(delivery.direction_revision)
                && c["version"].as_u64().is_some_and(|v| v <= policy.maximum_candidate_versions)
                && c["resource_basis"] == json!([{"resource_id":policy.resource_id,"version":policy.expected_resource_version}])
                && c["work_id"].as_str().is_some_and(|id| state.proposed_work_ids.contains(id))
                && entities(observation, "work_items").is_ok_and(|works| works.iter().any(|w|
                    w["work_id"] == c["work_id"] && w["kind"].as_str() == Some("integration"))))
            .max_by_key(|c| c["version"].as_u64())
    }

    pub(super) fn delivery_options(
        &self,
        observation: &SwarmObservation,
    ) -> Result<Vec<DeliveryOption>, CoordinatorServiceError> {
        let Ok((policy, state)) = self.delivery_state() else {
            return Ok(Vec::new());
        };
        if !state.matches(policy, observation) {
            return Ok(Vec::new());
        }
        let Some(candidate) = self.delivery_candidate(observation) else {
            return Ok(Vec::new());
        };
        let candidate_ref =
            reference(candidate).map_err(|_| CoordinatorServiceError::InvalidObservation)?;
        let digest = artifact(candidate)
            .map_err(|_| CoordinatorServiceError::InvalidObservation)?
            .digest;
        let mut operations = policy
            .checks
            .iter()
            .filter(|check| {
                !state
                    .versions
                    .get(&candidate_ref.version)
                    .is_some_and(|v| v.checks.contains_key(&check.check_id))
            })
            .map(|check| DeliveryOperation::Check {
                check_id: check.check_id.clone(),
            })
            .collect::<Vec<_>>();
        if self.delivery_ready(observation, candidate).is_ok() {
            operations.push(DeliveryOperation::Deliver);
        }
        operations
            .into_iter()
            .map(|operation| {
                let bytes = serde_json::to_vec(&json!([candidate_ref, digest, operation]))
                    .map_err(|_| CoordinatorServiceError::InvalidState)?;
                Ok(DeliveryOption {
                    target_id: format!("delivery-{}", blake3::hash(&bytes).to_hex()),
                    candidate: candidate_ref.clone(),
                    artifact_digest: digest.clone(),
                    operation,
                })
            })
            .collect()
    }

    /// Bounded, verified check output and operation failures are model context.
    pub(super) fn delivery_feedback(&self) -> Result<Value, CoordinatorServiceError> {
        let Ok((_, state)) = self.delivery_state() else {
            return Ok(Value::Null);
        };
        let mut checks = Vec::new();
        for (version, evidence) in &state.versions {
            let service = self
                .delivery_workspace(*version)
                .map_err(|_| CoordinatorServiceError::UnsafeStorage)?;
            for check in evidence.checks.values() {
                let read = |reference: &ArtifactRef| -> Result<String, CoordinatorServiceError> {
                    let bytes = service
                        .workspace()
                        .artifacts()
                        .read(reference)
                        .map_err(|_| CoordinatorServiceError::UnsafeStorage)?;
                    Ok(String::from_utf8_lossy(&bytes[..bytes.len().min(16_384)]).into_owned())
                };
                checks.push(json!({"candidate_version":version,"check_id":check.action_payload.check_id,
                    "status":check.action_payload.status,"stdout":read(&check.evidence.stdout)?,"stderr":read(&check.evidence.stderr)?}));
            }
        }
        Ok(
            json!({"checks":checks,"operations":state.operations.iter().map(|(id, op)|
            json!({"operation_id":id,"target":op.target,"status":op.status,"detail":op.detail})).collect::<Vec<_>>()}),
        )
    }

    fn delivery_workspace(&self, version: u64) -> DeliveryResult<CodeChangeService> {
        let (_, state) = self.delivery_state()?;
        CodeChangeService::open(
            &state.working_area,
            &self.journal.root.join(format!("delivery-v{version}")),
        )
        .map_err(closed)
    }

    fn candidate_authors(
        observation: &SwarmObservation,
        candidate: &Value,
    ) -> DeliveryResult<BTreeSet<String>> {
        let mut authors = BTreeSet::from([required_identifier(candidate, "author_member_id")
            .map_err(closed)?
            .to_owned()]);
        let refs = candidate["contribution_refs"]
            .as_array()
            .filter(|refs| !refs.is_empty())
            .ok_or("Candidate has no Contributions.")?;
        for r in refs {
            let contribution = entities(observation, "contributions")
                .map_err(closed)?
                .iter()
                .find(|c| {
                    c["contribution_id"] == r["contribution_id"] && c["version"] == r["version"]
                })
                .ok_or("Candidate references an absent Contribution.")?;
            authors.insert(
                required_identifier(contribution, "author_member_id")
                    .map_err(closed)?
                    .to_owned(),
            );
        }
        Ok(authors)
    }

    fn delivery_ready(
        &self,
        observation: &SwarmObservation,
        candidate: &Value,
    ) -> DeliveryResult<Vec<Value>> {
        let (policy, state) = self.delivery_state()?;
        if !state.matches(policy, observation) {
            return Err("Delivery scope changed.".to_owned());
        }
        let r = reference(candidate)?;
        if self
            .delivery_candidate(observation)
            .is_none_or(|latest| latest != candidate)
        {
            return Err("Candidate was superseded before delivery.".to_owned());
        }
        let evidence = state
            .versions
            .get(&r.version)
            .ok_or("Candidate has not been checked.")?;
        for (index, check) in policy.checks.iter().enumerate() {
            let local = evidence
                .checks
                .get(&check.check_id)
                .ok_or("Required check is missing.")?;
            let recorded = entities(observation, "checks")
                .map_err(closed)?
                .iter()
                .filter(|c| {
                    same_candidate(&c["candidate"], &r)
                        && c["check_id"].as_str() == Some(&check.check_id)
                })
                .collect::<Vec<_>>();
            if local.action_payload.status != crate::CheckStatus::Passed
                || recorded.len() != 1
                || !recorded.first().is_some_and(|c| {
                    c["status"].as_str() == Some("passed")
                        && c["criteria_revision"].as_u64() == Some(state.criteria_revision)
                        && c["criterion"].as_str() == Some(&state.criteria[index])
                        && c["resource_basis"] == candidate["resource_basis"]
                        && c["evidence_refs"] == json!(local.action_payload.evidence_refs)
                })
            {
                return Err("Exact passing Room check is missing.".to_owned());
            }
        }
        if entities(observation, "findings")
            .map_err(closed)?
            .iter()
            .any(|f| {
                f["severity"].as_str() == Some("blocking")
                    && f["status"].as_str() != Some("resolved")
            })
        {
            return Err("Unresolved blocking review finding.".to_owned());
        }
        if entities(observation, "resource_conflicts")
            .map_err(closed)?
            .iter()
            .any(|c| c["status"].as_str() == Some("unresolved"))
            || entities(observation, "blockers")
                .map_err(closed)?
                .iter()
                .any(|c| c["status"].as_str() == Some("unresolved"))
        {
            return Err("Unresolved blocker or resource conflict.".to_owned());
        }
        let authors = Self::candidate_authors(observation, candidate)?;
        let reviews = entities(observation, "reviews")
            .map_err(closed)?
            .iter()
            .filter(|v| same_candidate(&v["candidate"], &r))
            .cloned()
            .collect::<Vec<_>>();
        if reviews
            .iter()
            .any(|v| v["verdict"].as_str() != Some("passed"))
        {
            return Err("A non-passing exact review requires a new Candidate revision.".to_owned());
        }
        let independent = reviews
            .into_iter()
            .filter(|v| {
                v["reviewer_member_id"]
                    .as_str()
                    .is_some_and(|id| !authors.contains(id))
                    && v["criteria_revision"].as_u64() == Some(state.criteria_revision)
                    && v["resource_basis"] == candidate["resource_basis"]
            })
            .collect::<Vec<_>>();
        if independent.is_empty() {
            return Err("Independent passing review is missing.".to_owned());
        }
        Ok(independent)
    }

    pub(super) fn consume_delivery_option(
        &mut self,
        observation: &SwarmObservation,
        planning: &WorkerActionPlan,
        decision: crate::planning::PlanningDecision,
        target_id: &str,
    ) -> Result<(), CoordinatorServiceError> {
        let instruction: PlanningInstruction = serde_json::from_str(&planning.instruction)
            .map_err(|_| CoordinatorServiceError::InvalidState)?;
        let selected = instruction
            .delivery_options
            .iter()
            .find(|o| o.target_id == target_id)
            .cloned();
        let current = self.delivery_options(observation)?;
        let Some(selected) = selected.filter(|s| current.contains(s)) else {
            return self.retain_rejected_decision(planning, decision, "Select an exact current target_id from delivery_options; no commands or payloads are accepted.");
        };
        let operation_id = format!("{}-operation", planning.invocation_id);
        self.mutate(|state| {
            if let Some(autonomy) = &mut state.autonomy {
                autonomy
                    .consumed_decisions
                    .insert(planning.invocation_id.clone());
                if let Some(delivery) = &mut autonomy.delivery {
                    delivery.operations.insert(
                        operation_id.clone(),
                        OperationRecord {
                            target: selected.clone(),
                            status: "started".to_owned(),
                            detail: String::new(),
                            actions: Vec::new(),
                            writeback: None,
                        },
                    );
                }
            }
        })?;
        let outcome = self.execute_delivery_option(&operation_id, &selected);
        self.mutate(|state| {
            if let Some(op) = state
                .autonomy
                .as_mut()
                .and_then(|a| a.delivery.as_mut())
                .and_then(|d| d.operations.get_mut(&operation_id))
            {
                if outcome.is_ok() {
                    "completed"
                } else {
                    "failed"
                }
                .clone_into(&mut op.status);
                op.detail = outcome
                    .as_ref()
                    .err()
                    .cloned()
                    .unwrap_or_else(|| "Verified operation completed.".to_owned());
            }
        })?;
        match outcome {
            Ok(()) => self.autonomy_status(
                if matches!(selected.operation, DeliveryOperation::Deliver) {
                    "completed"
                } else {
                    "reasoning"
                },
                "The model-selected delivery operation completed with retained evidence.",
            ),
            Err(error) => self.halt_autonomy(&format!("Delivery requires attention: {error}")),
        }
    }

    fn execute_delivery_option(
        &mut self,
        operation_id: &str,
        selected: &DeliveryOption,
    ) -> DeliveryResult<()> {
        let observation = self.authoritative_observation().map_err(closed)?;
        self.require_delivery_execution(&observation, false)?;
        let candidate = self
            .delivery_candidate(&observation)
            .filter(|c| {
                reference(c).is_ok_and(|r| r == selected.candidate)
                    && c["artifact"]["digest"].as_str() == Some(&selected.artifact_digest)
            })
            .ok_or("Selected Candidate is no longer current.")?
            .clone();
        match &selected.operation {
            DeliveryOperation::Check { check_id } => {
                self.execute_delivery_check(operation_id, &observation, &candidate, check_id)
            }
            DeliveryOperation::Deliver => {
                self.execute_delivery_writeback(operation_id, &observation, &candidate)
            }
        }
    }

    fn require_delivery_execution(
        &self,
        observation: &SwarmObservation,
        allow_pause: bool,
    ) -> DeliveryResult<()> {
        let (policy, state) = self.delivery_state()?;
        if !state.matches(policy, observation)
            || self.state.autonomy.as_ref().is_none_or(|a| {
                observation.activity["execution_epoch"].as_u64() != Some(a.execution_epoch)
            })
        {
            return Err("Confirmed delivery scope changed.".to_owned());
        }
        let snapshot = self.execution.status().map_err(closed)?;
        let swarm = target_snapshot(&snapshot, &self.state.swarm_id).map_err(closed)?;
        if self
            .state
            .autonomy
            .as_ref()
            .is_none_or(|a| swarm.execution_epoch != a.execution_epoch)
        {
            return Err("Execution epoch changed.".to_owned());
        }
        if execution_gate(swarm) != Gate::Run
            && !(allow_pause
                && swarm.desired == DesiredExecution::Paused
                && !matches!(
                    swarm.phase,
                    ExecutionPhase::RecoveryRequired | ExecutionPhase::BlockedUnknown
                )
                && swarm.unknown_effects.is_empty())
        {
            return Err("Execution is stopped, paused, or requires recovery.".to_owned());
        }
        Ok(())
    }

    fn verify_delivery_files(&self) -> DeliveryResult<()> {
        let (policy, state) = self.delivery_state()?;
        for (path, expected) in &state.executables {
            if ContentDigest::of(&read_stable_regular_file(path).map_err(closed)?) != *expected {
                return Err("An authorized checker or process guard changed.".to_owned());
            }
        }
        let workspace = ArtifactWorkspace::open(
            &state.working_area,
            &self.journal.root.join("delivery-artifacts"),
        )
        .map_err(closed)?;
        if workspace.capture(&policy.target).map_err(closed)? != state.original {
            return Err(
                "Delivery target changed since authorization; newer bytes were preserved."
                    .to_owned(),
            );
        }
        Ok(())
    }

    fn seal_delivery_candidate(
        &mut self,
        observation: &SwarmObservation,
        candidate: &Value,
    ) -> DeliveryResult<SealedCodeChange> {
        let r = reference(candidate)?;
        if let Some(evidence) = self.delivery_state()?.1.versions.get(&r.version) {
            return Ok(evidence.sealed.clone());
        }
        self.verify_delivery_files()?;
        let (policy, state) = self.delivery_state()?;
        let policy = policy.clone();
        let criteria = state.criteria.clone();
        let criteria_revision = state.criteria_revision;
        let service = self.delivery_workspace(r.version)?;
        let selected = artifact(candidate)?;
        let (path, captured) = service
            .workspace()
            .artifacts()
            .capture_authoritative_bounded(&observation.activity, &selected, 64 * 1024)
            .map_err(closed)?;
        let source = service
            .workspace()
            .artifacts()
            .read(&captured)
            .map_err(closed)?;
        // Each version already has its own durable workspace. The local and
        // Pack candidate IDs must remain identical for evidence validation.
        let local_id = r.candidate_id.clone();
        let draft = service
            .prepare(&PrepareCodeChangeRequest {
                candidate_id: local_id.clone(),
                targets: vec![policy.target.clone()],
            })
            .map_err(closed)?;
        fs::write(draft.editable_root.join(policy.target.as_str()), source).map_err(closed)?;
        let sealed = service
            .seal(&SealCodeChangeRequest {
                acceptance_criteria: criteria,
                authors: Self::candidate_authors(observation, candidate)?
                    .into_iter()
                    .collect(),
                candidate_id: local_id,
                criteria_revision,
                inputs: Vec::new(),
                pack_candidate: r.clone(),
                pack_candidate_artifact: Some(PackArtifactRef {
                    artifact_id: selected.artifact_id,
                    digest: selected.digest,
                    local_path: selected.local_path,
                    media_type: selected.media_type,
                }),
                pack_candidate_path: Some(path),
                resource_basis: vec![PackVersionRef {
                    resource_id: policy.resource_id,
                    version: policy.expected_resource_version,
                }],
            })
            .map_err(closed)?;
        self.mutate(|state| {
            if let Some(d) = state.autonomy.as_mut().and_then(|a| a.delivery.as_mut()) {
                d.versions.insert(
                    r.version,
                    VersionEvidence {
                        sealed: sealed.clone(),
                        checks: BTreeMap::new(),
                    },
                );
            }
        })
        .map_err(closed)?;
        Ok(sealed)
    }

    fn execute_delivery_check(
        &mut self,
        operation_id: &str,
        observation: &SwarmObservation,
        candidate: &Value,
        check_id: &str,
    ) -> DeliveryResult<()> {
        self.verify_delivery_files()?;
        let sealed = self.seal_delivery_candidate(observation, candidate)?;
        let (policy, _) = self.delivery_state()?;
        let check = policy
            .checks
            .iter()
            .find(|c| c.check_id == check_id)
            .ok_or("Unknown authorized check.")?
            .clone();
        let guard = policy.process_guard.clone();
        let version = sealed.pack_candidate.version;
        let service = self.delivery_workspace(version)?;
        let draft = service
            .prepare(&PrepareCodeChangeRequest {
                candidate_id: sealed.candidate_id.clone(),
                targets: vec![policy.target.clone()],
            })
            .map_err(closed)?;
        let root = draft
            .editable_root
            .to_str()
            .ok_or("Checker directory is not UTF-8.")?;
        let quoted = serde_json::to_string(root).map_err(closed)?;
        let arguments = check
            .arguments
            .iter()
            .map(|arg| {
                if arg == "{candidate_root}" {
                    root.to_owned()
                } else {
                    arg.replace("{candidate_root_json}", &quoted)
                }
            })
            .collect();
        let namespace = &self
            .state
            .autonomy
            .as_ref()
            .ok_or("Autonomy absent.")?
            .namespace;
        let check_revision = entities(observation, "checks")
            .map_err(closed)?
            .iter()
            .filter(|c| c["check_id"].as_str() == Some(check_id))
            .filter_map(|c| c["revision"].as_u64())
            .max()
            .unwrap_or(0)
            + 1;
        let request = RunCodeCheckRequest {
            arguments,
            candidate_id: sealed.candidate_id,
            check_id: check_id.to_owned(),
            evidence_path: ArtifactPath::new(format!(
                ".swarm-check-{namespace}-v{version}-{check_id}.json"
            ))
            .map_err(closed)?,
            environment: BTreeMap::new(),
            pack_check_revision: check_revision,
            program: check.program,
            revision_digest: sealed.revision_digest,
        };
        let started = std::time::Instant::now();
        let checked = service
            .run_check_while(&request, &guard, &mut || {
                started.elapsed() < Duration::from_secs(check.timeout_seconds)
                    && self
                        .authoritative_observation()
                        .is_ok_and(|o| self.require_delivery_execution(&o, true).is_ok())
            })
            .map_err(closed)?;
        self.mutate(|state| {
            if let Some(v) = state
                .autonomy
                .as_mut()
                .and_then(|a| a.delivery.as_mut())
                .and_then(|d| d.versions.get_mut(&version))
            {
                v.checks.insert(check_id.to_owned(), checked.clone());
            }
        })
        .map_err(closed)?;
        self.submit_delivery_action(
            operation_id,
            "record_check",
            serde_json::to_value(&checked.action_payload).map_err(closed)?,
        )
    }

    fn submit_delivery_action(
        &mut self,
        operation_id: &str,
        action_type: &str,
        payload: Value,
    ) -> DeliveryResult<()> {
        let observation = self.authoritative_observation().map_err(closed)?;
        self.require_delivery_execution(&observation, true)?;
        let offers = observation
            .action_offers
            .iter()
            .filter(|o| o.action_type == action_type)
            .collect::<Vec<_>>();
        if offers.len() != 1 {
            return Err(format!("No unique current {action_type} offer."));
        }
        let action_id = crate::coordinator::next_action_id().map_err(closed)?;
        let action = ExactSwarmAction {
            actor: SwarmActor::HumanCoordinator,
            action_id,
            based_on_room_seq: observation.room_seq,
            offer_id: offers[0].offer_id.clone(),
            action_type: action_type.to_owned(),
            payload_schema_digest: offers[0].payload_schema_digest.clone(),
            payload,
        };
        self.mutate(|state| {
            if let Some(op) = state
                .autonomy
                .as_mut()
                .and_then(|a| a.delivery.as_mut())
                .and_then(|d| d.operations.get_mut(operation_id))
            {
                op.actions.push(RetainedAction {
                    action: action.clone(),
                    receipt: None,
                });
            }
        })
        .map_err(closed)?;
        let receipt = self
            .driver
            .submit_delivery_action(&self.state.swarm_id, &action)
            .map_err(|e| {
                format!(
                    "Exact Action {} requires reconciliation: {e}",
                    action.action_id
                )
            })?;
        self.mutate(|state| {
            if let Some(action) = state
                .autonomy
                .as_mut()
                .and_then(|a| a.delivery.as_mut())
                .and_then(|d| d.operations.get_mut(operation_id))
                .and_then(|op| op.actions.last_mut())
            {
                action.receipt = Some(receipt.clone());
            }
        })
        .map_err(closed)?;
        match receipt {
            SwarmActionReceipt::Accepted { .. } => Ok(()),
            SwarmActionReceipt::Rejected { code, .. } => {
                Err(format!("Delivery Action rejected: {code}"))
            }
        }
    }

    fn execute_delivery_writeback(
        &mut self,
        operation_id: &str,
        observation: &SwarmObservation,
        candidate: &Value,
    ) -> DeliveryResult<()> {
        let reviews = self.delivery_ready(observation, candidate)?;
        self.verify_delivery_files()?;
        let r = reference(candidate)?;
        let (policy, state) = self.delivery_state()?;
        let policy = policy.clone();
        let sealed = state
            .versions
            .get(&r.version)
            .ok_or("Missing checked revision.")?
            .sealed
            .clone();
        let service = self.delivery_workspace(r.version)?;
        for review in &reviews {
            let recorded = service
                .record_review(&RecordCodeReviewRequest {
                    candidate_id: sealed.candidate_id.clone(),
                    findings: Vec::new(),
                    pack_review_revision: required_revision(review, "revision").map_err(closed)?,
                    review_id: required_identifier(review, "review_id")
                        .map_err(closed)?
                        .to_owned(),
                    reviewer_id: required_identifier(review, "reviewer_member_id")
                        .map_err(closed)?
                        .to_owned(),
                    reviewer_kind: ReviewerKind::Agent,
                    revision_digest: sealed.revision_digest,
                    verdict: ReviewVerdict::Pass,
                })
                .map_err(closed)?;
            if recorded.gate == crate::ReviewGate::Accepted {
                break;
            }
        }
        let current = self.authoritative_observation().map_err(closed)?;
        self.require_delivery_execution(&current, false)?;
        self.delivery_ready(&current, candidate)?;
        self.submit_delivery_action(operation_id,"request_writeback",json!({"candidate":r,"resource_id":policy.resource_id,"expected_resource_version":policy.expected_resource_version,"operation_id":operation_id}))?;
        // No new decision is needed for the mechanics of this selected exact
        // operation; authority and execution scope are checked before effects.
        let current = self.authoritative_observation().map_err(closed)?;
        self.require_delivery_execution(&current, true)?;
        self.delivery_ready(&current, candidate)?;
        service
            .stage_write_back(&StageCodeWriteBackRequest {
                candidate_id: sealed.candidate_id,
                operation_id: operation_id.to_owned(),
                revision_digest: sealed.revision_digest,
            })
            .map_err(closed)?;
        let result = service.reconcile_write_back(operation_id).map_err(closed)?;
        self.mutate(|state| {
            if let Some(op) = state
                .autonomy
                .as_mut()
                .and_then(|a| a.delivery.as_mut())
                .and_then(|d| d.operations.get_mut(operation_id))
            {
                op.writeback = Some(result.clone());
            }
        })
        .map_err(closed)?;
        let status = match result.disposition {
            crate::code_change::WriteBackDisposition::Applied => "applied",
            crate::code_change::WriteBackDisposition::BlockedConflict => "conflicted",
            crate::code_change::WriteBackDisposition::ReconciliationRequired => "uncertain",
        };
        let mut outcome = json!({"operation_id":operation_id,"status":status,"expected_writeback_revision":1,"evidence_refs":[format!("code-change:{}",sealed.revision_digest)]});
        if status == "applied" {
            outcome["resulting_version"] = json!(policy.expected_resource_version + 1);
        }
        self.submit_delivery_action(operation_id, "record_writeback_outcome", outcome)?;
        if status != "applied" {
            return Err("Writeback did not apply; no Result was accepted.".to_owned());
        }
        let current = self.authoritative_observation().map_err(closed)?;
        self.delivery_ready(&current, candidate)?;
        let checks = policy
            .checks
            .iter()
            .map(|required| {
                let check = entities(&current, "checks")
                    .map_err(closed)?
                    .iter()
                    .find(|c| {
                        same_candidate(&c["candidate"], &r)
                            && c["check_id"].as_str() == Some(&required.check_id)
                    })
                    .ok_or("Missing exact Room check.")?;
                Ok(json!({"id":required.check_id,"revision":check["revision"]}))
            })
            .collect::<DeliveryResult<Vec<_>>>()?;
        let refs = reviews
            .iter()
            .map(|v| json!({"id":v["review_id"],"revision":v["revision"]}))
            .collect::<Vec<_>>();
        self.submit_delivery_action(operation_id,"accept_result",json!({"candidate":r,"check_refs":checks,"review_refs":refs,
            "expected_direction_revision":current.activity["direction_revision"],"result_id":r.candidate_id}))
    }
}
