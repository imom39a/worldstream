//! Public Room setup adapter over the existing immutable creation and provisioning engines.
//! No progress or authority is stored here; the public operation identifies the
//! retained creation review, and provisioning derives its intent from that review.

use std::sync::Arc;

use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use worldstream_core::CanonicalJsonV1;
use worldstream_hosted_contract::{
    HostedGenesisAccessModeV1, HostedGenesisMembershipPurposeV1, HostedGenesisMembershipV1,
    HostedGenesisPrincipalKindV1,
};
use worldstream_protocol::{
    AccessMode, PackReference, PrincipalKind, ROOM_ARCHIVE_RESPONSE_SCHEMA_V1,
    RoomArchiveResponseV1, RoomHead,
};

use crate::{
    activity_packs::DaemonActivityPackSource,
    agent_profiles::{
        AgentHostContractV1, AgentProfileErrorV1, AgentProfileRevisionViewV1,
        AgentProfileSecretAvailabilityV1, AgentProfileStoreV1,
    },
    room_creation::{
        RoomCreationErrorV1, RoomCreationStateV1, RoomCreationSupervisorV1, next_ulid,
    },
    room_drafts::{
        AgentAssignmentModeV1, RoomDraftSeatReadinessV1, RoomDraftSeatV1, RoomDraftStepV1,
        RoomDraftV1,
    },
    room_launch::RoomLaunchAssessmentV1,
    room_setup_spec::{
        RoomSetupError, RoomSetupIssueCode, RoomSetupSpecificationV1, SetupAssignmentV1,
        parse_setup_specification, resolve_setup_specification,
    },
    runner_templates::{RunnerCompatibilityRuleV1, RunnerTemplateRegistryV1},
    secrets::SecretReferenceV1,
    task_setup::{TaskSetupErrorV1, TaskSetupStageV1, TaskSetupStateV1, TaskSetupSupervisorV1},
};

/// A new attempt; operation identity is supplied in the route before any call.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoomSetupCreateRequestV1 {
    pub specification: RoomSetupSpecificationV1,
    #[serde(default)]
    pub acknowledge_start: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RoomSetupOperationStageV1 {
    Creation,
    Provisioning,
    Member,
    Runner,
    Complete,
}

/// Public progress, deliberately separate from Room state and from private intents.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoomSetupOperationStatusV1 {
    pub version: String,
    pub operation: String,
    pub room_id: Option<String>,
    pub complete: bool,
    pub stage: RoomSetupOperationStageV1,
    pub active_stage: Option<TaskSetupStageV1>,
    pub next_action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assessment: Option<RoomLaunchAssessmentV1>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoomSetupOperationListV1 {
    pub version: String,
    pub operations: Vec<RoomSetupOperationStatusV1>,
}

/// Private sequence-zero evidence assembled only from the retained creation
/// intent and its atomically committed response.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RoomSetupGenesisEvidenceV1 {
    pub pack: PackReference,
    pub room_head: RoomHead,
    pub memberships: Vec<HostedGenesisMembershipV1>,
}

/// Private retained binding needed to authenticate one Pack-neutral result pull.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RoomSetupResultIndexerBindingV1 {
    pub room_id: String,
    pub member_id: String,
    pub principal_id: String,
    pub pack: PackReference,
    pub secret_reference: SecretReferenceV1,
}

/// Private retained binding for the dedicated anonymous Public Projection
/// relay. The bearer remains referenced, never copied into hosted contracts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RoomSetupPublicRelayBindingV1 {
    pub room_id: String,
    pub member_id: String,
    pub principal_id: String,
    pub pack: PackReference,
    pub secret_reference: SecretReferenceV1,
}

/// Shared metadata-only compatibility result for validation and creation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RoomSetupDependencyError {
    Missing,
    Incompatible,
    Unavailable,
}

/// Checks an exact Profile assignment without resolving credential bytes.
///
/// # Errors
/// Rejects unavailable dependencies or a mismatched immutable execution contract.
pub fn validate_profile_assignment(
    assignment: &SetupAssignmentV1,
    view: &AgentProfileRevisionViewV1,
) -> Result<(), RoomSetupDependencyError> {
    let expected = assignment
        .agent_profile
        .as_ref()
        .ok_or(RoomSetupDependencyError::Missing)?;
    if expected.profile_id != view.profile_id || expected.revision != view.revision {
        return Err(RoomSetupDependencyError::Incompatible);
    }
    if view
        .secret_settings
        .iter()
        .any(|setting| setting.availability == AgentProfileSecretAvailabilityV1::Unavailable)
    {
        return Err(RoomSetupDependencyError::Unavailable);
    }
    if view
        .secret_settings
        .iter()
        .any(|setting| setting.availability != AgentProfileSecretAvailabilityV1::Configured)
    {
        return Err(RoomSetupDependencyError::Missing);
    }
    match (&assignment.mode, &view.host_contract) {
        (_, AgentHostContractV1::GenericMcp) => Ok(()),
        (
            AgentAssignmentModeV1::Managed,
            AgentHostContractV1::ManagedReference {
                runner_template, ..
            }
            | AgentHostContractV1::ManagedHouseOpenrouter {
                runner_template, ..
            },
        ) if assignment.runner_template.as_ref() == Some(runner_template) => Ok(()),
        _ => Err(RoomSetupDependencyError::Incompatible),
    }
}

/// Exact Pack identity and explanatory version are both required by a Template rule.
#[must_use]
pub fn runner_supports_pack(rules: &[RunnerCompatibilityRuleV1], pack: &PackReference) -> bool {
    rules.iter().any(|rule| {
        rule.activity_pack_id == pack.id && rule.exact_revisions.contains(&pack.version)
    })
}

#[derive(Debug, Error)]
pub enum RoomSetupOperationErrorV1 {
    #[error(transparent)]
    Specification(#[from] RoomSetupError),
    #[error("Room setup specification is invalid")]
    Invalid,
    #[error("Activity starts at Genesis; acknowledge this before creating the Room")]
    AcknowledgementRequired,
    #[error("Operation already exists; resume its retained intent")]
    Conflict,
    #[error("Retained Room setup operation was not found")]
    NotFound,
    #[error("Room setup dependencies or retained state are unavailable")]
    Unavailable,
}

impl From<RoomCreationErrorV1> for RoomSetupOperationErrorV1 {
    fn from(error: RoomCreationErrorV1) -> Self {
        match error {
            RoomCreationErrorV1::NotFound => Self::NotFound,
            RoomCreationErrorV1::InvalidDraft => Self::Invalid,
            RoomCreationErrorV1::Unavailable => Self::Unavailable,
        }
    }
}

#[derive(Clone)]
pub struct RoomSetupOperationsV1 {
    creation: RoomCreationSupervisorV1,
    setup: TaskSetupSupervisorV1,
    catalog: Arc<dyn DaemonActivityPackSource>,
    profiles: AgentProfileStoreV1,
    runners: RunnerTemplateRegistryV1,
}

impl RoomSetupOperationsV1 {
    #[must_use]
    pub fn new(
        creation: RoomCreationSupervisorV1,
        setup: TaskSetupSupervisorV1,
        catalog: impl DaemonActivityPackSource,
        profiles: AgentProfileStoreV1,
        runners: RunnerTemplateRegistryV1,
    ) -> Self {
        Self {
            creation,
            setup,
            catalog: Arc::new(catalog),
            profiles,
            runners,
        }
    }

    /// Resolves and retains one attempt before delegating any daemon mutation.
    ///
    /// # Errors
    /// Fails before mutation for invalid intent, missing acknowledgement or identity
    /// conflict. Publication/transport uncertainty never authorizes a fresh attempt.
    pub fn create(
        &self,
        operation: &str,
        request: &RoomSetupCreateRequestV1,
    ) -> Result<RoomSetupOperationStatusV1, RoomSetupOperationErrorV1> {
        validate_operation_reference(operation)?;
        match self.creation.status(operation) {
            Ok(_) => return Err(RoomSetupOperationErrorV1::Conflict),
            Err(RoomCreationErrorV1::NotFound) => {}
            Err(error) => return Err(error.into()),
        }
        let bytes = serde_json::to_vec(&request.specification)
            .map_err(|_| RoomSetupOperationErrorV1::Invalid)?;
        let parsed = parse_setup_specification(&bytes)?;
        let catalog = self
            .catalog
            .revision(&parsed.pack.digest)
            .map_err(|_| RoomSetupOperationErrorV1::Unavailable)?;
        let resolved = resolve_setup_specification(&bytes, &catalog)?;
        self.validate_dependencies(&resolved)?;
        if catalog.revision.lobby_compatibility.is_none()
            && catalog.revision.activity_start_compatibility.is_none()
            && !request.acknowledge_start
        {
            return Err(RoomSetupOperationErrorV1::AcknowledgementRequired);
        }
        let spectators = resolved.spectators.clone();
        let draft = resolved_draft(operation, resolved)?;
        if spectators.is_empty() {
            self.creation.prepare_reviewed(&draft)?;
        } else {
            self.creation
                .prepare_reviewed_with_spectators(&draft, &spectators)?;
        }
        self.resume(operation)
    }

    /// Resumes only the retained immutable creation/provisioning records.
    ///
    /// # Errors
    /// Missing/corrupt intent is never recreated. Failed calls retain their public
    /// operation reference and any safely readable partial progress.
    pub fn resume(
        &self,
        operation: &str,
    ) -> Result<RoomSetupOperationStatusV1, RoomSetupOperationErrorV1> {
        validate_operation_reference(operation)?;
        self.creation.status(operation)?;
        if self.creation.reconcile(operation).is_err() {
            return self.status(operation);
        }
        if self.creation.status(operation)?.state == RoomCreationStateV1::Succeeded {
            match self.setup.status(operation) {
                Ok(_) => {
                    // Also closes a fresh marker if the original setup route
                    // already prepared this operation. Retry never creates intent.
                    self.creation.begin_setup_preparation(operation)?;
                    let _ = self.setup.retry(operation);
                }
                Err(TaskSetupErrorV1::NotFound) => {
                    if !self.creation.begin_setup_preparation(operation)? {
                        return self.status(operation);
                    }
                    let _ = self.setup.start(operation);
                }
                Err(_) => return Err(RoomSetupOperationErrorV1::Unavailable),
            }
        }
        self.status(operation)
    }

    /// Reads setup progress without issuing daemon mutations.
    ///
    /// # Errors
    /// Unsafe/malformed retained records fail closed.
    pub fn status(
        &self,
        operation: &str,
    ) -> Result<RoomSetupOperationStatusV1, RoomSetupOperationErrorV1> {
        validate_operation_reference(operation)?;
        let creation = self.creation.status(operation)?;
        let mut status = RoomSetupOperationStatusV1 {
            version: "room_setup_operation.v1".to_owned(),
            operation: operation.to_owned(),
            room_id: creation.room_id,
            complete: false,
            stage: RoomSetupOperationStageV1::Creation,
            active_stage: None,
            next_action: "resume".to_owned(),
            assessment: None,
        };
        if creation
            .attention
            .is_some_and(|attention| !attention.retryable)
        {
            "inspect_operation".clone_into(&mut status.next_action);
        }
        if creation.state != RoomCreationStateV1::Succeeded {
            return Ok(status);
        }
        status.stage = RoomSetupOperationStageV1::Provisioning;
        match self.setup.status(operation) {
            Ok(setup) => {
                status.assessment = Some((&setup).into());
                status.complete = setup.state == TaskSetupStateV1::Ready;
                status.stage = if status.complete {
                    RoomSetupOperationStageV1::Complete
                } else {
                    match &setup.active_stage {
                        Some(TaskSetupStageV1::MemberCapability { .. }) => {
                            RoomSetupOperationStageV1::Member
                        }
                        Some(TaskSetupStageV1::RunnerCapability { .. }) => {
                            RoomSetupOperationStageV1::Runner
                        }
                        None => RoomSetupOperationStageV1::Provisioning,
                    }
                };
                status.active_stage = setup.active_stage;
                if status.complete {
                    "inspect_room".clone_into(&mut status.next_action);
                } else if setup
                    .attention
                    .is_some_and(|attention| !attention.retryable)
                {
                    "inspect_operation".clone_into(&mut status.next_action);
                }
            }
            Err(TaskSetupErrorV1::NotFound) => {
                if creation.setup_preparation_started != Some(false) {
                    "restore_setup_record".clone_into(&mut status.next_action);
                }
            }
            Err(_) => return Err(RoomSetupOperationErrorV1::Unavailable),
        }
        Ok(status)
    }

    /// Resolves only enough of the retained creation operation to close one
    /// hosted launch. It never starts task provisioning or launches a Lobby.
    /// A definitive pre-Genesis rejection returns `None`; ambiguous creation
    /// remains unavailable so the exact operation can be retried safely.
    pub(crate) fn close_hosted_launch(
        &self,
        operation: &str,
        idempotency_key: &str,
    ) -> Result<Option<RoomArchiveResponseV1>, RoomSetupOperationErrorV1> {
        validate_operation_reference(operation)?;
        let mut creation = match self.creation.status(operation) {
            Ok(creation) => creation,
            Err(crate::room_creation::RoomCreationErrorV1::NotFound) => return Ok(None),
            Err(crate::room_creation::RoomCreationErrorV1::InvalidDraft) => {
                return Err(RoomSetupOperationErrorV1::Invalid);
            }
            Err(crate::room_creation::RoomCreationErrorV1::Unavailable) => {
                return Err(RoomSetupOperationErrorV1::Unavailable);
            }
        };
        if creation.state != RoomCreationStateV1::Succeeded
            && !(creation.state == RoomCreationStateV1::NeedsAttention
                && creation
                    .attention
                    .as_ref()
                    .is_some_and(|attention| !attention.retryable))
        {
            creation = self.creation.reconcile(operation)?;
        }
        if creation.state != RoomCreationStateV1::Succeeded {
            return if creation.state == RoomCreationStateV1::NeedsAttention
                && creation
                    .attention
                    .as_ref()
                    .is_some_and(|attention| !attention.retryable)
            {
                Ok(None)
            } else {
                Err(RoomSetupOperationErrorV1::Unavailable)
            };
        }
        let room_id = creation
            .room_id
            .as_deref()
            .ok_or(RoomSetupOperationErrorV1::Unavailable)?;
        let response = self
            .creation
            .archive_room(room_id, idempotency_key)
            .map_err(|error| match error {
                crate::room_creation::RoomCreationAttemptErrorV1::Rejected => {
                    RoomSetupOperationErrorV1::Invalid
                }
                crate::room_creation::RoomCreationAttemptErrorV1::Ambiguous
                | crate::room_creation::RoomCreationAttemptErrorV1::OperatorFixRequired => {
                    RoomSetupOperationErrorV1::Unavailable
                }
            })?;
        if response.schema != ROOM_ARCHIVE_RESPONSE_SCHEMA_V1
            || response.room_id != room_id
            || response.room_head.room_id != room_id
        {
            return Err(RoomSetupOperationErrorV1::Unavailable);
        }
        Ok(Some(response))
    }

    /// Reads exact secret-free Genesis correspondence without issuing effects.
    ///
    /// The Room creation receipt is the authority for sequence zero. Later
    /// task-setup or Lobby progress cannot alter these identities.
    pub(crate) fn genesis_evidence(
        &self,
        operation: &str,
    ) -> Result<RoomSetupGenesisEvidenceV1, RoomSetupOperationErrorV1> {
        validate_operation_reference(operation)?;
        let creation = self.creation.status(operation)?;
        if creation.state != RoomCreationStateV1::Succeeded {
            return Err(RoomSetupOperationErrorV1::Unavailable);
        }
        let response = creation
            .response
            .as_ref()
            .ok_or(RoomSetupOperationErrorV1::Unavailable)?;
        if response.room_head.room_seq != 0
            || response.room_id != response.room_head.room_id
            || response.member_ids.len() != creation.request.members.len()
        {
            return Err(RoomSetupOperationErrorV1::Unavailable);
        }

        let mut memberships = Vec::with_capacity(response.member_ids.len());
        for (member, membership_id) in creation.request.members.iter().zip(&response.member_ids) {
            let principal_kind = match member.principal_kind {
                PrincipalKind::Human => HostedGenesisPrincipalKindV1::Human,
                PrincipalKind::Agent => HostedGenesisPrincipalKindV1::Agent,
            };
            let evidence = match member.access_mode {
                AccessMode::Participant => {
                    let seat = creation
                        .review
                        .seats
                        .iter()
                        .find(|seat| seat.principal_id.as_deref() == Some(&member.principal_id))
                        .filter(|seat| member.role.as_deref() == Some(seat.role.as_str()))
                        .ok_or(RoomSetupOperationErrorV1::Unavailable)?;
                    HostedGenesisMembershipV1 {
                        access_mode: HostedGenesisAccessModeV1::Participant,
                        purpose: HostedGenesisMembershipPurposeV1::Participant,
                        seat_id: Some(seat.seat_id.clone()),
                        role: Some(seat.role.clone()),
                        principal_kind,
                        principal_id: member.principal_id.clone(),
                        membership_id: membership_id.clone(),
                        scopes: vec![],
                    }
                }
                AccessMode::Spectator => {
                    let receipt = creation
                        .spectator_credentials
                        .iter()
                        .find(|receipt| {
                            receipt.principal_id == member.principal_id
                                && receipt.member_id == *membership_id
                                && receipt.room_id == response.room_id
                        })
                        .ok_or(RoomSetupOperationErrorV1::Unavailable)?;
                    let purpose = match receipt.purpose {
                        crate::room_setup_spec::SetupSpectatorPurposeV2::Creator => {
                            HostedGenesisMembershipPurposeV1::CreatorSpectator
                        }
                        crate::room_setup_spec::SetupSpectatorPurposeV2::ResultIndexer => {
                            HostedGenesisMembershipPurposeV1::ResultIndexer
                        }
                        crate::room_setup_spec::SetupSpectatorPurposeV2::PublicRelay => {
                            HostedGenesisMembershipPurposeV1::PublicProjectionRelay
                        }
                    };
                    HostedGenesisMembershipV1 {
                        access_mode: HostedGenesisAccessModeV1::Spectator,
                        purpose,
                        seat_id: None,
                        role: None,
                        principal_kind,
                        principal_id: member.principal_id.clone(),
                        membership_id: membership_id.clone(),
                        scopes: receipt.scopes.clone(),
                    }
                }
                AccessMode::Operator => return Err(RoomSetupOperationErrorV1::Unavailable),
            };
            memberships.push(evidence);
        }

        Ok(RoomSetupGenesisEvidenceV1 {
            pack: creation.request.pack,
            room_head: response.room_head.clone(),
            memberships,
        })
    }

    /// Resolves only the dedicated result-indexer Membership and retained bearer reference.
    ///
    /// The bearer bytes remain in the protected vault and are resolved lazily by
    /// the fixed Host transport after any managed-process proof.
    pub(crate) fn result_indexer_binding(
        &self,
        operation: &str,
    ) -> Result<RoomSetupResultIndexerBindingV1, RoomSetupOperationErrorV1> {
        validate_operation_reference(operation)?;
        let creation = self.creation.status(operation)?;
        if creation.state != RoomCreationStateV1::Succeeded {
            return Err(RoomSetupOperationErrorV1::Unavailable);
        }
        let response = creation
            .response
            .as_ref()
            .ok_or(RoomSetupOperationErrorV1::Unavailable)?;
        let spectator = creation
            .spectators
            .iter()
            .find(|spectator| {
                spectator.purpose == crate::room_setup_spec::SetupSpectatorPurposeV2::ResultIndexer
            })
            .ok_or(RoomSetupOperationErrorV1::Unavailable)?;
        let receipt = creation
            .spectator_credentials
            .iter()
            .find(|receipt| {
                receipt.purpose == crate::room_setup_spec::SetupSpectatorPurposeV2::ResultIndexer
                    && receipt.room_id == response.room_id
                    && receipt.principal_id == spectator.principal_id
                    && receipt.capability_id == spectator.capability_id
            })
            .filter(|receipt| {
                receipt.scopes == ["room:attach", "room:observe_public", "room:replay"]
            })
            .ok_or(RoomSetupOperationErrorV1::Unavailable)?;
        Ok(RoomSetupResultIndexerBindingV1 {
            room_id: response.room_id.clone(),
            member_id: receipt.member_id.clone(),
            principal_id: receipt.principal_id.clone(),
            pack: creation.request.pack,
            secret_reference: spectator.secret_reference.clone(),
        })
    }

    /// Resolves only the dedicated public-relay Spectator Membership and its
    /// retained bearer reference. It deliberately excludes replay authority.
    pub(crate) fn public_relay_binding(
        &self,
        operation: &str,
    ) -> Result<RoomSetupPublicRelayBindingV1, RoomSetupOperationErrorV1> {
        validate_operation_reference(operation)?;
        let creation = self.creation.status(operation)?;
        if creation.state != RoomCreationStateV1::Succeeded {
            return Err(RoomSetupOperationErrorV1::Unavailable);
        }
        let response = creation
            .response
            .as_ref()
            .ok_or(RoomSetupOperationErrorV1::Unavailable)?;
        let spectator = creation
            .spectators
            .iter()
            .find(|spectator| {
                spectator.purpose == crate::room_setup_spec::SetupSpectatorPurposeV2::PublicRelay
            })
            .ok_or(RoomSetupOperationErrorV1::Unavailable)?;
        let receipt = creation
            .spectator_credentials
            .iter()
            .find(|receipt| {
                receipt.purpose == crate::room_setup_spec::SetupSpectatorPurposeV2::PublicRelay
                    && receipt.room_id == response.room_id
                    && receipt.principal_id == spectator.principal_id
                    && receipt.capability_id == spectator.capability_id
            })
            .filter(|receipt| receipt.scopes == ["room:attach", "room:observe_public"])
            .ok_or(RoomSetupOperationErrorV1::Unavailable)?;
        Ok(RoomSetupPublicRelayBindingV1 {
            room_id: response.room_id.clone(),
            member_id: receipt.member_id.clone(),
            principal_id: receipt.principal_id.clone(),
            pack: creation.request.pack,
            secret_reference: spectator.secret_reference.clone(),
        })
    }

    /// Lists unfinished operations, including creation-only partial attempts.
    ///
    /// # Errors
    /// Rejects unreadable retained inventory rather than silently omitting it.
    pub fn unfinished(&self) -> Result<RoomSetupOperationListV1, RoomSetupOperationErrorV1> {
        let mut operations = Vec::new();
        for creation in self.creation.statuses()? {
            let status = self.status(&creation.draft_id)?;
            if !status.complete {
                operations.push(status);
            }
        }
        Ok(RoomSetupOperationListV1 {
            version: "room_setup_operations.v1".to_owned(),
            operations,
        })
    }

    fn validate_dependencies(
        &self,
        specification: &RoomSetupSpecificationV1,
    ) -> Result<(), RoomSetupOperationErrorV1> {
        let templates = self.runners.templates();
        for (index, seat) in specification.seats.iter().enumerate() {
            let Some(assignment) = &seat.assignment else {
                continue;
            };
            if let Some(profile) = &assignment.agent_profile {
                let path = format!("/seats/{index}/assignment/agent_profile");
                let view = self
                    .profiles
                    .revision(&profile.profile_id, &profile.revision)
                    .map_err(|error| {
                        if error == AgentProfileErrorV1::Unavailable {
                            RoomSetupOperationErrorV1::Unavailable
                        } else {
                            dependency_issue(&path, RoomSetupIssueCode::DependencyMissing)
                        }
                    })?;
                validate_profile_assignment(assignment, &view).map_err(|error| {
                    if error == RoomSetupDependencyError::Unavailable {
                        RoomSetupOperationErrorV1::Unavailable
                    } else {
                        dependency_issue(
                            &path,
                            match error {
                                RoomSetupDependencyError::Missing => {
                                    RoomSetupIssueCode::DependencyMissing
                                }
                                _ => RoomSetupIssueCode::DependencyIncompatible,
                            },
                        )
                    }
                })?;
            }
            if let Some(reference) = &assignment.runner_template {
                let path = format!("/seats/{index}/assignment/runner_template");
                let template = templates
                    .iter()
                    .find(|template| {
                        template.template_id == reference.template_id
                            && template.revision == reference.revision
                    })
                    .ok_or_else(|| {
                        dependency_issue(&path, RoomSetupIssueCode::DependencyMissing)
                    })?;
                if !runner_supports_pack(&template.compatibility, &specification.pack) {
                    return Err(dependency_issue(
                        &path,
                        RoomSetupIssueCode::DependencyIncompatible,
                    ));
                }
            }
        }
        Ok(())
    }
}

fn dependency_issue(path: &str, code: RoomSetupIssueCode) -> RoomSetupOperationErrorV1 {
    RoomSetupError::Specification {
        path: path.to_owned(),
        code,
    }
    .into()
}

fn resolved_draft(
    operation: &str,
    specification: RoomSetupSpecificationV1,
) -> Result<RoomDraftV1, RoomSetupOperationErrorV1> {
    let mut seats = Vec::with_capacity(specification.seats.len());
    let mut readiness = Vec::with_capacity(specification.seats.len());
    for seat in specification.seats {
        validate_operation_reference(&seat.label)?;
        readiness.push(RoomDraftSeatReadinessV1 {
            seat_id: seat.label.clone(),
            role: seat.role.clone(),
            required: seat.required,
        });
        seats.push(RoomDraftSeatV1 {
            seat_id: seat.label,
            role: seat.role,
            required: seat.required,
            display_name: seat.display_name,
            principal_id: seat.principal.as_ref().map(|_| next_ulid()).transpose()?,
            principal_kind: seat.principal.map(|principal| principal.kind),
            agent_assignment: seat.assignment.as_ref().map(|assignment| assignment.mode),
            agent_profile: seat
                .assignment
                .as_ref()
                .and_then(|assignment| assignment.agent_profile.clone()),
            runner_template: seat
                .assignment
                .and_then(|assignment| assignment.runner_template),
        });
    }
    Ok(RoomDraftV1 {
        schema: "worldstream/studio-room-draft/v1".to_owned(),
        draft_id: operation.to_owned(),
        pack: Some(specification.pack),
        configuration: specification.configuration,
        seats,
        readiness,
        operator_view: specification.operator_view,
        last_valid_step: Some(RoomDraftStepV1::Review),
    })
}

fn validate_operation_reference(value: &str) -> Result<(), RoomSetupOperationErrorV1> {
    if value.is_empty()
        || value.len() > 64
        || !value.as_bytes()[0].is_ascii_alphanumeric()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(RoomSetupOperationErrorV1::Invalid);
    }
    Ok(())
}

pub fn room_setup_operations_router(operations: RoomSetupOperationsV1) -> Router {
    Router::new()
        .route("/api/v1/room-setup-operations", get(list_operations))
        .route(
            "/api/v1/room-setup-operations/{operation}",
            get(operation_status).post(create_operation),
        )
        .route(
            "/api/v1/room-setup-operations/{operation}/resume",
            post(resume_operation),
        )
        .layer(DefaultBodyLimit::max(1_048_576))
        .with_state(operations)
}

async fn create_operation(
    State(operations): State<RoomSetupOperationsV1>,
    Path(operation): Path<String>,
    body: Bytes,
) -> Result<Response, RoomSetupOperationErrorV1> {
    CanonicalJsonV1::parse(&body).map_err(|_| RoomSetupOperationErrorV1::Invalid)?;
    let request = serde_json::from_slice(&body).map_err(|_| RoomSetupOperationErrorV1::Invalid)?;
    let status = tokio::task::spawn_blocking(move || operations.create(&operation, &request))
        .await
        .map_err(|_| RoomSetupOperationErrorV1::Unavailable)??;
    Ok(progress_response(status))
}

async fn resume_operation(
    State(operations): State<RoomSetupOperationsV1>,
    Path(operation): Path<String>,
    body: Bytes,
) -> Result<Response, RoomSetupOperationErrorV1> {
    if !body.is_empty() {
        return Err(RoomSetupOperationErrorV1::Invalid);
    }
    let status = tokio::task::spawn_blocking(move || operations.resume(&operation))
        .await
        .map_err(|_| RoomSetupOperationErrorV1::Unavailable)??;
    Ok(progress_response(status))
}

async fn operation_status(
    State(operations): State<RoomSetupOperationsV1>,
    Path(operation): Path<String>,
) -> Result<Json<RoomSetupOperationStatusV1>, RoomSetupOperationErrorV1> {
    tokio::task::spawn_blocking(move || operations.status(&operation))
        .await
        .map_err(|_| RoomSetupOperationErrorV1::Unavailable)?
        .map(Json)
}

async fn list_operations(
    State(operations): State<RoomSetupOperationsV1>,
) -> Result<Json<RoomSetupOperationListV1>, RoomSetupOperationErrorV1> {
    tokio::task::spawn_blocking(move || operations.unfinished())
        .await
        .map_err(|_| RoomSetupOperationErrorV1::Unavailable)?
        .map(Json)
}

fn progress_response(status: RoomSetupOperationStatusV1) -> Response {
    (
        if status.complete {
            StatusCode::OK
        } else {
            StatusCode::ACCEPTED
        },
        Json(status),
    )
        .into_response()
}

impl IntoResponse for RoomSetupOperationErrorV1 {
    fn into_response(self) -> Response {
        if let Self::Specification(error) = self {
            return (StatusCode::BAD_REQUEST, Json(error)).into_response();
        }
        let (status, code) = match self {
            Self::Specification(_) | Self::Invalid => {
                (StatusCode::BAD_REQUEST, "room_setup_invalid")
            }
            Self::AcknowledgementRequired => {
                (StatusCode::CONFLICT, "room_setup_acknowledgement_required")
            }
            Self::Conflict => (StatusCode::CONFLICT, "room_setup_operation_exists"),
            Self::NotFound => (StatusCode::NOT_FOUND, "room_setup_operation_not_found"),
            Self::Unavailable => (StatusCode::SERVICE_UNAVAILABLE, "room_setup_unavailable"),
        };
        (
            status,
            Json(serde_json::json!({"error":{"code":code,"message":self.to_string()}})),
        )
            .into_response()
    }
}
