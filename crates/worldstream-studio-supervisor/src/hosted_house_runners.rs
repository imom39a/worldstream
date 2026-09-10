//! Retained Host-side capacity and launch coordination for reviewed House Agents.
//!
//! This module is deliberately outside `worldstreamd`. It authenticates one
//! pre-Genesis capacity receipt, binds it to the immutable platform Assignment,
//! and starts the existing two-process managed host for the exact Room seat.

use std::{
    collections::BTreeMap,
    fs,
    io::Write as _,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use ring::hmac;
use serde::{Deserialize, Serialize};
use worldstream_core::CanonicalJsonV1;
use worldstream_hosted_contract::{
    HostedHouseRunnerAssignmentV1, HostedHouseRunnerReservationOutcomeV1,
    HostedHouseRunnerReservationReceiptV1, HostedHouseRunnerReservationRequestV1,
    HostedHouseRunnerRetirementDispositionV1, HostedHouseRunnerRetirementReceiptV1,
    HostedHouseRunnerRetirementRequestV1, HostedLaunchRequestV1, HouseAgentRevision,
    ListingRevision, MAX_REVIEWED_HOSTED_CATALOG_REVISIONS,
    validate_hosted_house_runner_reservation_receipt,
    validate_hosted_house_runner_reservation_request,
};
use worldstream_runtime::{
    create_owner_only_file, prepare_data_directory, validate_owner_only_file,
};
use zeroize::Zeroizing;

use crate::{
    agent_profiles::{AgentHostContractV1, AgentProfileErrorV1, AgentProfileStoreV1},
    assignment_mcp::{
        AssignedMembershipLaunchSourceV1, AssignedMembershipSourceErrorV1,
        AssignmentMcpLaunchRegistryV1,
    },
    house_model::HouseProviderCredentialV1,
    managed_agent_host::{
        ManagedAgentHostErrorV1, ManagedAgentHostLaunchPlanV1, ManagedAgentHostOperationsV1,
        ManagedAgentHostPreparedLaunchV1, ManagedAgentHostStartSourceV1, ManagedAgentHostStateV1,
    },
    runner_templates::{RunnerSupervisorV1, RunnerTemplateRegistryV1},
    secrets::{FileSecretVaultV1, SecretKindV1, SecretVaultErrorV1},
    task_setup::{TaskSetupErrorV1, TaskSetupStateV1, TaskSetupSupervisorV1},
};

const RESERVATION_SCHEMA_V1: &str = "worldstream/retained-house-runner-reservation/v1";
const LAUNCH_BINDING_SCHEMA_V1: &str = "worldstream/retained-house-runner-launch-binding/v1";
const RUNTIME_BINDING_SCHEMA_V1: &str = "worldstream/retained-house-runner-runtime-binding/v1";
const RECEIPT_SCHEMA_V1: &str = "worldstream/house-runner-reservation-receipt/v1";
const RECEIPT_KEY_FILE: &str = "receipt-authentication.key";
const RECEIPT_KEY_DOMAIN: &[u8] = b"worldstream/house-runner-receipt-key/v1";
const RECEIPT_TAG_DOMAIN: &str = "worldstream/house-runner-receipt-tag/v1";
const BINDING_DIGEST_DOMAIN: &str = "worldstream/house-runner-binding/v1";
const MAX_RECORD_BYTES: usize = 256 * 1024;
const MAX_RESERVATIONS: usize = 256;
const MAX_HOUSE_RUNNERS: usize = 4;
const MAX_HOUSE_RUNNERS_PER_LAUNCH: usize = 2;
const RETIREMENT_SCHEMA_V1: &str = "worldstream/house-runner-retirement/v1";
const RETIREMENT_RECEIPT_SCHEMA_V1: &str = "worldstream/house-runner-retirement-receipt/v1";
const RETIREMENT_TAG_DOMAIN: &str = "worldstream/house-runner-retirement-tag/v1";
const RETIREMENT_MARKER_FILE: &str = "retired.json";
const RETIREMENT_INTENT_MARKER_FILE: &str = "retiring.json";
const OPERATOR_RETIREMENT_MARKER_FILE: &str = "operator-retired.json";
const LAUNCH_CLOSURE_FENCE_SCHEMA_V1: &str =
    "worldstream/retained-house-runner-launch-closure-fence/v1";
const LAUNCH_CLOSURE_FENCE_TAG_DOMAIN: &str =
    "worldstream/house-runner-launch-closure-fence-tag/v1";

/// Closed Host-side reservation and launch failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostedHouseRunnerErrorV1 {
    Invalid,
    Conflict,
    NotFound,
    Unavailable,
}

/// Exact readiness gate evaluated before the Lobby launch operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostedHouseRunnerGateV1 {
    Ready,
    RetryableFailure,
    TerminalFailure,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct RetainedReservationV1 {
    schema: String,
    request: HostedHouseRunnerReservationRequestV1,
    receipt: HostedHouseRunnerReservationReceiptV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct RetainedLaunchBindingV1 {
    schema: String,
    reservation_operation_id: String,
    launch_request_id: String,
    room_setup_operation_id: String,
    house_agent_assignment_id: String,
    runner_unit_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct RetainedRuntimeBindingV1 {
    schema: String,
    reservation_operation_id: String,
    room_setup_operation_id: String,
    house_agent_assignment_id: String,
    runner_unit_id: String,
    room_id: String,
    local_assignment_id: String,
    member_id: String,
    principal_id: String,
    role: String,
    runner_id: String,
    instance_id: String,
    house_agent_revision_digest: String,
    profile_id: String,
    profile_revision: String,
    runner_template_id: String,
    runner_template_revision: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct RetainedHouseRunnerRetirementV1 {
    schema: String,
    authority: String,
    receipt: HostedHouseRunnerRetirementReceiptV1,
    allowance_reset: bool,
}

/// A Host-authenticated, durable pre-stop fence. It intentionally does not
/// release capacity: only the later signed receipt does that.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct RetainedHouseRunnerRetirementIntentV1 {
    schema: String,
    authority: String,
    request: HostedHouseRunnerRetirementRequestV1,
    authentication_tag: String,
    allowance_reset: bool,
}

/// Host-local monotonic fence shared by reservation, binding, start, and
/// retirement paths for one creator-closed launch lineage.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct RetainedHouseRunnerLaunchClosureFenceV1 {
    schema: String,
    host_installation_id: String,
    launch_request_id: String,
    closure_intent_digest: String,
    authentication_tag: String,
}

#[derive(Serialize)]
struct ReceiptWitnessV1<'a> {
    domain: &'static str,
    schema: &'a str,
    host_installation_id: &'a str,
    reservation_operation_id: &'a str,
    launch_request_id: &'a str,
    listing_revision_digest: &'a str,
    seat_id: &'a str,
    house_agent_revision_digest: &'a str,
    outcome: HostedHouseRunnerReservationOutcomeV1,
    runner_unit_id: &'a Option<String>,
    failure_code: &'a Option<String>,
    binding_digest: &'a str,
}

#[derive(Serialize)]
struct RetirementReceiptWitnessV1<'a> {
    domain: &'static str,
    schema: &'a str,
    host_installation_id: &'a str,
    reservation_operation_id: &'a str,
    launch_request_id: &'a str,
    house_agent_assignment_id: &'a Option<String>,
    runner_unit_id: &'a str,
    disposition: HostedHouseRunnerRetirementDispositionV1,
    platform_evidence_digest: &'a str,
    stop_witness: &'a str,
}

#[derive(Serialize)]
struct RetirementIntentWitnessV1<'a> {
    domain: &'static str,
    schema: &'a str,
    authority: &'a str,
    request: &'a HostedHouseRunnerRetirementRequestV1,
    allowance_reset: bool,
}

#[derive(Serialize)]
struct LaunchClosureFenceWitnessV1<'a> {
    domain: &'static str,
    schema: &'a str,
    host_installation_id: &'a str,
    launch_request_id: &'a str,
    closure_intent_digest: &'a str,
}

enum DependencyCheckErrorV1 {
    Terminal(&'static str),
    Unavailable,
}

enum StartErrorV1 {
    Terminal,
    Unavailable,
}

trait HouseRunnerDependencySourceV1: Send + Sync {
    fn binding_digest(
        &self,
        request: &HostedHouseRunnerReservationRequestV1,
        listing: &ListingRevision,
        revision: &HouseAgentRevision,
    ) -> Result<String, DependencyCheckErrorV1>;

    fn start(
        &self,
        launch: &HostedLaunchRequestV1,
        assignment: &HostedHouseRunnerAssignmentV1,
        room_id: &str,
        listing: &ListingRevision,
        revision: &HouseAgentRevision,
    ) -> Result<(), StartErrorV1>;

    /// Stops the exact locally bound pair. `None` is permitted only when the
    /// coordinator has proved that no retained runtime binding could have
    /// started a child for the exact reservation.
    fn retire(
        &self,
        runtime_binding: Option<&RetainedRuntimeBindingV1>,
    ) -> Result<String, HostedHouseRunnerErrorV1>;
}

trait HouseAssignmentLaunchIssuerV1: Send + Sync {
    fn issue(&self, assignment_id: &str) -> Result<String, AssignedMembershipSourceErrorV1>;
}

impl<S> HouseAssignmentLaunchIssuerV1 for AssignmentMcpLaunchRegistryV1<S>
where
    S: AssignedMembershipLaunchSourceV1,
{
    fn issue(&self, assignment_id: &str) -> Result<String, AssignedMembershipSourceErrorV1> {
        AssignmentMcpLaunchRegistryV1::issue(self, assignment_id)
    }
}

#[derive(Clone)]
struct LiveHouseRunnerDependencySourceV1 {
    runtime_bindings: Arc<PathBuf>,
    unit_root: Arc<PathBuf>,
    state_dir: Arc<PathBuf>,
    helper_executable: Arc<PathBuf>,
    profiles: AgentProfileStoreV1,
    templates: RunnerTemplateRegistryV1,
    runners: RunnerSupervisorV1,
    launches: Arc<dyn HouseAssignmentLaunchIssuerV1>,
    vault: FileSecretVaultV1,
    task_setup: TaskSetupSupervisorV1,
    managed_hosts: ManagedAgentHostOperationsV1,
    development_provider_address: Option<SocketAddr>,
    mutation: Arc<Mutex<()>>,
}

impl LiveHouseRunnerDependencySourceV1 {
    #[allow(clippy::too_many_arguments)]
    fn open<S>(
        root: &Path,
        state_dir: &Path,
        helper_executable: &Path,
        profiles: AgentProfileStoreV1,
        templates: RunnerTemplateRegistryV1,
        runners: RunnerSupervisorV1,
        launches: AssignmentMcpLaunchRegistryV1<S>,
        vault: FileSecretVaultV1,
        task_setup: TaskSetupSupervisorV1,
        managed_hosts: ManagedAgentHostOperationsV1,
        development_provider_address: Option<SocketAddr>,
    ) -> Result<Self, HostedHouseRunnerErrorV1>
    where
        S: AssignedMembershipLaunchSourceV1,
    {
        if !state_dir.is_absolute() || !helper_executable.is_absolute() {
            return Err(HostedHouseRunnerErrorV1::Invalid);
        }
        let source = Self {
            runtime_bindings: Arc::new(
                prepare_data_directory(&root.join("runtime-bindings"))
                    .map_err(|_| HostedHouseRunnerErrorV1::Unavailable)?,
            ),
            unit_root: Arc::new(
                prepare_data_directory(&root.join("units"))
                    .map_err(|_| HostedHouseRunnerErrorV1::Unavailable)?,
            ),
            state_dir: Arc::new(state_dir.to_owned()),
            helper_executable: Arc::new(helper_executable.to_owned()),
            profiles,
            templates,
            runners,
            launches: Arc::new(launches),
            vault,
            task_setup,
            managed_hosts,
            development_provider_address,
            mutation: Arc::new(Mutex::new(())),
        };
        source.validate_runtime_bindings()?;
        Ok(source)
    }

    fn exact_profile_and_secret(
        &self,
        revision: &HouseAgentRevision,
    ) -> Result<crate::agent_profiles::AgentProfileRevisionV1, DependencyCheckErrorV1> {
        let (profile_id, profile_revision) = revision.agent_profile();
        match self.profiles.revision(profile_id, profile_revision) {
            Ok(_) => {}
            Err(AgentProfileErrorV1::NotFound) => {
                return Err(DependencyCheckErrorV1::Terminal(
                    "house_profile_unavailable",
                ));
            }
            Err(_) => return Err(DependencyCheckErrorV1::Unavailable),
        }
        let profile = self
            .profiles
            .retained_revision(profile_id, profile_revision)
            .map_err(|_| DependencyCheckErrorV1::Unavailable)?;
        let (host_contract_revision, runner_template) = match &profile.host_contract {
            AgentHostContractV1::ManagedHouseOpenrouter {
                host_contract_revision,
                runner_template,
            } => (host_contract_revision, runner_template),
            AgentHostContractV1::GenericMcp | AgentHostContractV1::ManagedReference { .. } => {
                return Err(DependencyCheckErrorV1::Terminal(
                    "house_profile_incompatible",
                ));
            }
        };
        if host_contract_revision != "1"
            || (
                runner_template.template_id.as_str(),
                runner_template.revision.as_str(),
            ) != revision.runner_template()
        {
            return Err(DependencyCheckErrorV1::Terminal(
                "house_profile_incompatible",
            ));
        }
        let mut settings = profile.secret_settings.iter().filter(|setting| {
            setting.kind == SecretKindV1::ModelProvider && setting.key == "MODEL_PROVIDER_TOKEN"
        });
        let setting = settings
            .next()
            .filter(|_| settings.next().is_none())
            .ok_or(DependencyCheckErrorV1::Terminal(
                "house_credential_unavailable",
            ))?;
        let credential = self
            .vault
            .resolve(SecretKindV1::ModelProvider, &setting.reference)
            .map_err(|error| match error {
                SecretVaultErrorV1::Missing => {
                    DependencyCheckErrorV1::Terminal("house_credential_unavailable")
                }
                SecretVaultErrorV1::InvalidMaterial
                | SecretVaultErrorV1::InvalidReference
                | SecretVaultErrorV1::Unavailable => DependencyCheckErrorV1::Unavailable,
            })?;
        HouseProviderCredentialV1::new(Zeroizing::new(credential.as_bytes().to_vec()))
            .map_err(|_| DependencyCheckErrorV1::Terminal("house_credential_unavailable"))?;
        Ok(profile)
    }

    fn runtime_binding(&self, requested: &RetainedRuntimeBindingV1) -> Result<(), StartErrorV1> {
        let _guard = self.mutation.lock().unwrap_or_else(PoisonError::into_inner);
        let path = self
            .runtime_bindings
            .join(format!("{}.json", requested.reservation_operation_id));
        match read_record::<RetainedRuntimeBindingV1>(&path) {
            Ok(existing) if existing == *requested => Ok(()),
            Ok(_) => Err(StartErrorV1::Terminal),
            Err(RecordReadErrorV1::NotFound) => {
                persist_new(&path, requested).map_err(|_| StartErrorV1::Unavailable)
            }
            Err(RecordReadErrorV1::Unavailable) => Err(StartErrorV1::Unavailable),
        }
    }

    fn validate_runtime_bindings(&self) -> Result<(), HostedHouseRunnerErrorV1> {
        let mut count = 0_usize;
        for entry in fs::read_dir(self.runtime_bindings.as_ref())
            .map_err(|_| HostedHouseRunnerErrorV1::Unavailable)?
        {
            let path = entry
                .map_err(|_| HostedHouseRunnerErrorV1::Unavailable)?
                .path();
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .ok_or(HostedHouseRunnerErrorV1::Unavailable)?;
            if name.starts_with('.')
                && Path::new(name)
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("tmp"))
            {
                continue;
            }
            let operation = name
                .strip_suffix(".json")
                .filter(|value| uuid_reference(value))
                .ok_or(HostedHouseRunnerErrorV1::Unavailable)?;
            let binding = read_record::<RetainedRuntimeBindingV1>(&path)
                .map_err(|_| HostedHouseRunnerErrorV1::Unavailable)?;
            if binding.reservation_operation_id != operation || !valid_runtime_binding(&binding) {
                return Err(HostedHouseRunnerErrorV1::Unavailable);
            }
            count = count.saturating_add(1);
            if count > MAX_RESERVATIONS {
                return Err(HostedHouseRunnerErrorV1::Unavailable);
            }
        }
        Ok(())
    }
}

impl HouseRunnerDependencySourceV1 for LiveHouseRunnerDependencySourceV1 {
    fn binding_digest(
        &self,
        request: &HostedHouseRunnerReservationRequestV1,
        listing: &ListingRevision,
        revision: &HouseAgentRevision,
    ) -> Result<String, DependencyCheckErrorV1> {
        let profile = self.exact_profile_and_secret(revision)?;
        let (template_id, template_revision) = revision.runner_template();
        let manifest = self
            .templates
            .approved_manifest(template_id, template_revision)
            .filter(|manifest| {
                manifest.compatibility.iter().any(|rule| {
                    rule.activity_pack_id == listing.pack().id
                        && rule.exact_revisions.contains(&listing.pack().version)
                })
            })
            .ok_or(DependencyCheckErrorV1::Terminal(
                "house_runner_template_unavailable",
            ))?;
        canonical_blake3(&serde_json::json!({
            "domain": BINDING_DIGEST_DOMAIN,
            "request": request,
            "house_agent_revision_digest": revision.digest(),
            "house_agent_revision": serde_json::from_slice::<serde_json::Value>(revision.canonical_bytes())
                .map_err(|_| DependencyCheckErrorV1::Unavailable)?,
            "agent_profile": profile,
            "runner_template": manifest,
        }))
        .map_err(|_| DependencyCheckErrorV1::Unavailable)
    }

    #[allow(
        clippy::too_many_lines,
        reason = "exact post-Genesis binding checks remain ordered before the single process start"
    )]
    fn start(
        &self,
        launch: &HostedLaunchRequestV1,
        assignment: &HostedHouseRunnerAssignmentV1,
        room_id: &str,
        listing: &ListingRevision,
        revision: &HouseAgentRevision,
    ) -> Result<(), StartErrorV1> {
        let receipt = &assignment.reservation_receipt;
        let runner_unit_id = receipt
            .runner_unit_id
            .as_deref()
            .ok_or(StartErrorV1::Terminal)?;
        let local = self
            .profiles
            .assignment_for_room_seat(room_id, &receipt.seat_id)
            .map_err(|error| match error {
                AgentProfileErrorV1::NotFound | AgentProfileErrorV1::Unavailable => {
                    StartErrorV1::Unavailable
                }
                _ => StartErrorV1::Terminal,
            })?;
        if (
            local.profile.profile_id.as_str(),
            local.profile.revision.as_str(),
        ) != revision.agent_profile()
        {
            return Err(StartErrorV1::Terminal);
        }
        let setup = self
            .task_setup
            .status(&local.draft_id)
            .map_err(map_setup_start_error)?;
        if setup.state != TaskSetupStateV1::Ready || setup.room_id != room_id {
            return Err(StartErrorV1::Unavailable);
        }
        let metadata = self
            .task_setup
            .managed_reference_metadata(&local)
            .map_err(map_setup_start_error)?;
        let (template_id, template_revision) = revision.runner_template();
        if metadata.template_id != template_id
            || metadata.template_revision != template_revision
            || metadata.pack.id != listing.pack().id
            || metadata.pack.version != listing.pack().version
            || metadata.pack.digest != listing.pack().digest
        {
            return Err(StartErrorV1::Terminal);
        }
        let profile = self
            .exact_profile_and_secret(revision)
            .map_err(|error| match error {
                DependencyCheckErrorV1::Terminal(_) => StartErrorV1::Terminal,
                DependencyCheckErrorV1::Unavailable => StartErrorV1::Unavailable,
            })?;
        let AgentHostContractV1::ManagedHouseOpenrouter {
            runner_template, ..
        } = &profile.host_contract
        else {
            return Err(StartErrorV1::Terminal);
        };
        let binding = RetainedRuntimeBindingV1 {
            schema: RUNTIME_BINDING_SCHEMA_V1.to_owned(),
            reservation_operation_id: receipt.reservation_operation_id.clone(),
            room_setup_operation_id: launch.room_setup_operation_id.clone(),
            house_agent_assignment_id: assignment.house_agent_assignment_id.clone(),
            runner_unit_id: runner_unit_id.to_owned(),
            room_id: room_id.to_owned(),
            local_assignment_id: local.assignment_id.clone(),
            member_id: local.membership.member_id.clone(),
            principal_id: local.membership.principal_id.clone(),
            role: local.membership.role.clone(),
            runner_id: metadata.runner_id.clone(),
            instance_id: metadata.instance_id.clone(),
            house_agent_revision_digest: revision.digest().to_owned(),
            profile_id: local.profile.profile_id.clone(),
            profile_revision: local.profile.revision.clone(),
            runner_template_id: runner_template.template_id.clone(),
            runner_template_revision: runner_template.revision.clone(),
        };
        self.runtime_binding(&binding)?;
        let prepared = self.prepare_retained(&binding, revision)?;
        let status = self
            .managed_hosts
            .start_house(&local.assignment_id, &prepared)
            .map_err(map_managed_start_error)?;
        if status.state != ManagedAgentHostStateV1::Running || !status.ready {
            return Err(StartErrorV1::Unavailable);
        }
        Ok(())
    }

    fn retire(
        &self,
        runtime_binding: Option<&RetainedRuntimeBindingV1>,
    ) -> Result<String, HostedHouseRunnerErrorV1> {
        let Some(binding) = runtime_binding else {
            // A managed House child can only be started after its durable
            // runtime binding is retained. Its absence is therefore the
            // narrow proof that a pre-Genesis unit has no child to stop.
            return canonical_blake3(&serde_json::json!({
                "domain": "worldstream/house-runner-stop-witness/v1",
                "kind": "no_runtime_binding",
            }));
        };
        let status = self
            .managed_hosts
            .stop(&binding.local_assignment_id)
            .map_err(|_| HostedHouseRunnerErrorV1::Unavailable)?;
        if status.state != ManagedAgentHostStateV1::Stopped || status.ready {
            return Err(HostedHouseRunnerErrorV1::Unavailable);
        }
        canonical_blake3(&serde_json::json!({
            "domain": "worldstream/house-runner-stop-witness/v1",
            "kind": "exact_managed_pair_stopped",
            "reservation_operation_id": binding.reservation_operation_id,
            "room_setup_operation_id": binding.room_setup_operation_id,
            "house_agent_assignment_id": binding.house_agent_assignment_id,
            "runner_unit_id": binding.runner_unit_id,
            "local_assignment_id": binding.local_assignment_id,
        }))
    }
}

impl LiveHouseRunnerDependencySourceV1 {
    // Used for both initial startup and lifecycle restoration. No reservation,
    // Assignment, or allowance is minted here; every identity is revalidated.
    #[allow(clippy::too_many_lines)]
    fn prepare_retained(
        &self,
        binding: &RetainedRuntimeBindingV1,
        revision: &HouseAgentRevision,
    ) -> Result<ManagedAgentHostPreparedLaunchV1, StartErrorV1> {
        let local = self
            .profiles
            .assignment(&binding.local_assignment_id)
            .map_err(|_| StartErrorV1::Unavailable)?;
        let metadata = self
            .task_setup
            .managed_reference_metadata(&local)
            .map_err(map_setup_start_error)?;
        if !valid_runtime_binding(binding)
            || binding.house_agent_revision_digest != revision.digest()
            || (
                binding.profile_id.as_str(),
                binding.profile_revision.as_str(),
            ) != revision.agent_profile()
            || (
                binding.runner_template_id.as_str(),
                binding.runner_template_revision.as_str(),
            ) != revision.runner_template()
            || local.draft_id != binding.room_setup_operation_id
            || local.membership.room_id != binding.room_id
            || local.membership.member_id != binding.member_id
            || local.membership.principal_id != binding.principal_id
            || local.membership.role != binding.role
            || local.profile.profile_id != binding.profile_id
            || local.profile.revision != binding.profile_revision
            || metadata.runner_id != binding.runner_id
            || metadata.instance_id != binding.instance_id
            || metadata.template_id != binding.runner_template_id
            || metadata.template_revision != binding.runner_template_revision
        {
            return Err(StartErrorV1::Terminal);
        }
        let profile = self
            .exact_profile_and_secret(revision)
            .map_err(|error| match error {
                DependencyCheckErrorV1::Terminal(_) => StartErrorV1::Terminal,
                DependencyCheckErrorV1::Unavailable => StartErrorV1::Unavailable,
            })?;
        let AgentHostContractV1::ManagedHouseOpenrouter {
            host_contract_revision,
            ..
        } = &profile.host_contract
        else {
            return Err(StartErrorV1::Terminal);
        };
        let secret = profile
            .secret_settings
            .iter()
            .find(|setting| {
                setting.kind == SecretKindV1::ModelProvider && setting.key == "MODEL_PROVIDER_TOKEN"
            })
            .ok_or(StartErrorV1::Terminal)?;
        let runner_unit_id = &binding.runner_unit_id;
        let executable = self
            .runners
            .managed_reference_executable(
                &metadata.instance_id,
                &metadata.template_id,
                &metadata.template_revision,
            )
            .ok_or(StartErrorV1::Unavailable)?;
        let launch_reference = self
            .launches
            .issue(&local.assignment_id)
            .map_err(|_| StartErrorV1::Unavailable)?;
        let credential = self
            .vault
            .resolve(SecretKindV1::ModelProvider, &secret.reference)
            .map_err(|_| StartErrorV1::Unavailable)?;
        let working_directory = prepare_data_directory(&self.unit_root.join(runner_unit_id))
            .map_err(|_| StartErrorV1::Unavailable)?;
        let plan = if let Some(provider_address) = self.development_provider_address {
            ManagedAgentHostLaunchPlanV1::new_house_development(
                &self.helper_executable,
                &self.state_dir,
                &launch_reference,
                &executable,
                &working_directory,
                runner_unit_id,
                revision,
                provider_address,
            )
        } else {
            ManagedAgentHostLaunchPlanV1::new_house(
                &self.helper_executable,
                &self.state_dir,
                &launch_reference,
                &executable,
                &working_directory,
                runner_unit_id,
                revision,
            )
        }
        .map_err(map_managed_start_error)?;
        ManagedAgentHostPreparedLaunchV1::new_house(
            &local.assignment_id,
            host_contract_revision,
            secret.reference.as_str(),
            plan,
            Zeroizing::new(credential.as_bytes().to_vec()),
        )
        .map_err(map_managed_start_error)
    }
}

struct RetainedHouseStartSourceV1 {
    source: LiveHouseRunnerDependencySourceV1,
    revisions: Arc<BTreeMap<String, HouseAgentRevision>>,
}

impl ManagedAgentHostStartSourceV1 for RetainedHouseStartSourceV1 {
    fn prepare(
        &self,
        assignment_id: &str,
    ) -> Result<ManagedAgentHostPreparedLaunchV1, ManagedAgentHostErrorV1> {
        self.source
            .validate_runtime_bindings()
            .map_err(|_| ManagedAgentHostErrorV1::Unavailable)?;
        let mut selected = None;
        for entry in fs::read_dir(self.source.runtime_bindings.as_ref())
            .map_err(|_| ManagedAgentHostErrorV1::Unavailable)?
        {
            let path = entry
                .map_err(|_| ManagedAgentHostErrorV1::Unavailable)?
                .path();
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            let binding: RetainedRuntimeBindingV1 =
                read_record(&path).map_err(|_| ManagedAgentHostErrorV1::Unavailable)?;
            if binding.local_assignment_id == assignment_id {
                if selected.is_some() {
                    return Err(ManagedAgentHostErrorV1::Corrupt);
                }
                selected = Some(binding);
            }
        }
        let binding = selected.ok_or(ManagedAgentHostErrorV1::InvalidInput)?;
        let revision = self
            .revisions
            .get(&binding.house_agent_revision_digest)
            .ok_or(ManagedAgentHostErrorV1::Unavailable)?;
        self.source
            .prepare_retained(&binding, revision)
            .map_err(|error| match error {
                StartErrorV1::Terminal => ManagedAgentHostErrorV1::Corrupt,
                StartErrorV1::Unavailable => ManagedAgentHostErrorV1::Unavailable,
            })
    }
}

fn map_setup_start_error(error: TaskSetupErrorV1) -> StartErrorV1 {
    match error {
        TaskSetupErrorV1::Unavailable | TaskSetupErrorV1::NotReady => StartErrorV1::Unavailable,
        TaskSetupErrorV1::NotFound | TaskSetupErrorV1::InvalidCreation => StartErrorV1::Terminal,
    }
}

fn map_managed_start_error(error: ManagedAgentHostErrorV1) -> StartErrorV1 {
    match error {
        ManagedAgentHostErrorV1::AtCapacity
        | ManagedAgentHostErrorV1::Unavailable
        | ManagedAgentHostErrorV1::Ambiguous => StartErrorV1::Unavailable,
        ManagedAgentHostErrorV1::InvalidInput
        | ManagedAgentHostErrorV1::ImmutableProfileConflict
        | ManagedAgentHostErrorV1::Corrupt => StartErrorV1::Terminal,
    }
}

/// Durable reservation coordinator shared by the service routes and launch gate.
#[derive(Clone)]
pub struct HostedHouseRunnerOperationsV1 {
    reservations: Arc<PathBuf>,
    units: Arc<PathBuf>,
    launch_bindings: Arc<PathBuf>,
    launch_closure_fences: Arc<PathBuf>,
    host_installation_id: Arc<str>,
    listings: Arc<BTreeMap<String, ListingRevision>>,
    revisions: Arc<BTreeMap<String, HouseAgentRevision>>,
    authenticator: Arc<hmac::Key>,
    source: Arc<dyn HouseRunnerDependencySourceV1>,
    // Keeps the registered weak restoration adapter alive for this coordinator.
    house_start_source: Option<Arc<dyn ManagedAgentHostStartSourceV1>>,
    mutation: Arc<Mutex<()>>,
}

impl HostedHouseRunnerOperationsV1 {
    /// Opens the production reservation store and exact local Runner boundary.
    ///
    /// # Errors
    /// Rejects unsafe storage, invalid reviewed artifacts, or unavailable local dependencies.
    #[allow(clippy::too_many_arguments)]
    pub fn open_production<S>(
        root: &Path,
        state_dir: &Path,
        helper_executable: &Path,
        host_installation_id: &str,
        listings: Vec<ListingRevision>,
        revisions: Vec<HouseAgentRevision>,
        profiles: AgentProfileStoreV1,
        templates: RunnerTemplateRegistryV1,
        runners: RunnerSupervisorV1,
        launches: AssignmentMcpLaunchRegistryV1<S>,
        vault: FileSecretVaultV1,
        task_setup: TaskSetupSupervisorV1,
        managed_hosts: ManagedAgentHostOperationsV1,
    ) -> Result<Self, HostedHouseRunnerErrorV1>
    where
        S: AssignedMembershipLaunchSourceV1,
    {
        let root =
            prepare_data_directory(root).map_err(|_| HostedHouseRunnerErrorV1::Unavailable)?;
        let source = LiveHouseRunnerDependencySourceV1::open(
            &root,
            state_dir,
            helper_executable,
            profiles,
            templates,
            runners,
            launches,
            vault,
            task_setup,
            managed_hosts,
            None,
        )?;
        Self::open_live(&root, host_installation_id, listings, revisions, source)
    }

    /// Opens the same retained House boundary with one explicit loopback-only
    /// provider substitute for local acceptance testing.
    ///
    /// # Errors
    /// Rejects non-loopback provider addresses and the same invalid or
    /// unavailable dependencies as [`Self::open_production`].
    #[allow(clippy::too_many_arguments)]
    pub fn open_development_loopback<S>(
        root: &Path,
        state_dir: &Path,
        helper_executable: &Path,
        host_installation_id: &str,
        listings: Vec<ListingRevision>,
        revisions: Vec<HouseAgentRevision>,
        profiles: AgentProfileStoreV1,
        templates: RunnerTemplateRegistryV1,
        runners: RunnerSupervisorV1,
        launches: AssignmentMcpLaunchRegistryV1<S>,
        vault: FileSecretVaultV1,
        task_setup: TaskSetupSupervisorV1,
        managed_hosts: ManagedAgentHostOperationsV1,
        provider_address: SocketAddr,
    ) -> Result<Self, HostedHouseRunnerErrorV1>
    where
        S: AssignedMembershipLaunchSourceV1,
    {
        if !provider_address.ip().is_loopback() || provider_address.port() == 0 {
            return Err(HostedHouseRunnerErrorV1::Invalid);
        }
        let root =
            prepare_data_directory(root).map_err(|_| HostedHouseRunnerErrorV1::Unavailable)?;
        let source = LiveHouseRunnerDependencySourceV1::open(
            &root,
            state_dir,
            helper_executable,
            profiles,
            templates,
            runners,
            launches,
            vault,
            task_setup,
            managed_hosts,
            Some(provider_address),
        )?;
        Self::open_live(&root, host_installation_id, listings, revisions, source)
    }

    fn open_live(
        root: &Path,
        host_installation_id: &str,
        listings: Vec<ListingRevision>,
        revisions: Vec<HouseAgentRevision>,
        source: LiveHouseRunnerDependencySourceV1,
    ) -> Result<Self, HostedHouseRunnerErrorV1> {
        let mut operations = Self::open_with(
            root,
            host_installation_id,
            listings,
            revisions,
            source.clone(),
        )?;
        let managed_hosts = source.managed_hosts.clone();
        let restore: Arc<dyn ManagedAgentHostStartSourceV1> =
            Arc::new(RetainedHouseStartSourceV1 {
                source,
                revisions: Arc::clone(&operations.revisions),
            });
        managed_hosts
            .register_house_start_source(&restore)
            .map_err(|_| HostedHouseRunnerErrorV1::Unavailable)?;
        operations.house_start_source = Some(restore);
        Ok(operations)
    }

    fn open_with(
        root: &Path,
        host_installation_id: &str,
        listings: Vec<ListingRevision>,
        revisions: Vec<HouseAgentRevision>,
        source: impl HouseRunnerDependencySourceV1 + 'static,
    ) -> Result<Self, HostedHouseRunnerErrorV1> {
        if !safe_public_reference(host_installation_id, 128)
            || listings.is_empty()
            || listings.len() > MAX_REVIEWED_HOSTED_CATALOG_REVISIONS
            || revisions.len() > MAX_REVIEWED_HOSTED_CATALOG_REVISIONS
        {
            return Err(HostedHouseRunnerErrorV1::Invalid);
        }
        let listings = unique_by_digest(listings)?;
        let revisions = unique_by_digest(revisions)?;
        let receipt_key_root = prepare_data_directory(&root.join("receipt-authentication"))
            .map_err(|_| HostedHouseRunnerErrorV1::Unavailable)?;
        let receipt_key = load_or_create_receipt_key(&receipt_key_root)?;
        let root_key = hmac::Key::new(hmac::HMAC_SHA256, receipt_key.as_slice());
        let derived = hmac::sign(&root_key, RECEIPT_KEY_DOMAIN);
        let operations = Self {
            units: Arc::new(
                prepare_data_directory(&root.join("units"))
                    .map_err(|_| HostedHouseRunnerErrorV1::Unavailable)?,
            ),
            reservations: Arc::new(
                prepare_data_directory(&root.join("reservations"))
                    .map_err(|_| HostedHouseRunnerErrorV1::Unavailable)?,
            ),
            launch_bindings: Arc::new(
                prepare_data_directory(&root.join("launch-bindings"))
                    .map_err(|_| HostedHouseRunnerErrorV1::Unavailable)?,
            ),
            launch_closure_fences: Arc::new(
                prepare_data_directory(&root.join("launch-closure-fences"))
                    .map_err(|_| HostedHouseRunnerErrorV1::Unavailable)?,
            ),
            host_installation_id: Arc::from(host_installation_id),
            listings: Arc::new(listings),
            revisions: Arc::new(revisions),
            authenticator: Arc::new(hmac::Key::new(hmac::HMAC_SHA256, derived.as_ref())),
            source: Arc::new(source),
            house_start_source: None,
            mutation: Arc::new(Mutex::new(())),
        };
        operations.validate_retained()?;
        Ok(operations)
    }

    /// Reserves one exact Host-local unit or returns the identical retained receipt.
    ///
    /// # Errors
    /// Returns a closed invalid, conflict, or availability failure.
    #[allow(
        clippy::too_many_lines,
        reason = "reservation validation, capacity admission, authentication, and publication are one ordered operation"
    )]
    pub fn reserve(
        &self,
        request: &HostedHouseRunnerReservationRequestV1,
    ) -> Result<HostedHouseRunnerReservationReceiptV1, HostedHouseRunnerErrorV1> {
        let listing = self
            .listings
            .get(&request.listing_revision_digest)
            .ok_or(HostedHouseRunnerErrorV1::Invalid)?;
        validate_hosted_house_runner_reservation_request(
            request,
            &self.host_installation_id,
            listing,
            &self.revisions.values().cloned().collect::<Vec<_>>(),
        )
        .map_err(|_| HostedHouseRunnerErrorV1::Invalid)?;
        let revision = self
            .revisions
            .get(&request.house_agent_revision_digest)
            .ok_or(HostedHouseRunnerErrorV1::Invalid)?;
        let _guard = self.lock();
        if self
            .load_launch_closure_fence_unlocked(&request.launch_request_id)?
            .is_some()
        {
            return Err(HostedHouseRunnerErrorV1::Conflict);
        }
        match self.load_reservation(&request.reservation_operation_id) {
            Ok(existing) if existing.request == *request => {
                self.verify_receipt(&existing.receipt)?;
                return Ok(existing.receipt);
            }
            Ok(_) => return Err(HostedHouseRunnerErrorV1::Conflict),
            Err(HostedHouseRunnerErrorV1::NotFound) => {}
            Err(error) => return Err(error),
        }
        let retained = self.reservations_unlocked()?;
        let successful = retained
            .iter()
            .filter(|item| item.receipt.outcome == HostedHouseRunnerReservationOutcomeV1::Succeeded)
            .collect::<Vec<_>>();
        // An offline operator retirement fences process startup before the
        // platform releases its matching slot. Keep immutable receipts and
        // per-launch uniqueness; only global concurrent capacity is reusable.
        let active_count = successful.iter().try_fold(0_usize, |count, item| {
            self.operator_retired(&item.receipt)
                .map(|retired| count + usize::from(!retired))
        })?;
        let same_launch = successful
            .iter()
            .filter(|item| item.request.launch_request_id == request.launch_request_id)
            .collect::<Vec<_>>();
        let conflict = same_launch.iter().any(|item| {
            item.request.seat_id == request.seat_id
                || item.request.house_agent_revision_digest == request.house_agent_revision_digest
        });
        let dependency = self.source.binding_digest(request, listing, revision);
        let (outcome, runner_unit_id, failure_code, binding_digest) = match dependency {
            Err(DependencyCheckErrorV1::Unavailable) => {
                return Err(HostedHouseRunnerErrorV1::Unavailable);
            }
            Err(DependencyCheckErrorV1::Terminal(code)) => (
                HostedHouseRunnerReservationOutcomeV1::TerminalFailed,
                None,
                Some(code.to_owned()),
                failed_binding_digest(request, code)?,
            ),
            Ok(_digest)
                if active_count >= MAX_HOUSE_RUNNERS
                    || same_launch.len() >= MAX_HOUSE_RUNNERS_PER_LAUNCH =>
            {
                (
                    HostedHouseRunnerReservationOutcomeV1::TerminalFailed,
                    None,
                    Some("house_runner_capacity_exhausted".to_owned()),
                    failed_binding_digest(request, "house_runner_capacity_exhausted")?,
                )
            }
            Ok(_digest) if conflict => (
                HostedHouseRunnerReservationOutcomeV1::TerminalFailed,
                None,
                Some("house_runner_assignment_conflict".to_owned()),
                failed_binding_digest(request, "house_runner_assignment_conflict")?,
            ),
            Ok(digest) => (
                HostedHouseRunnerReservationOutcomeV1::Succeeded,
                Some(stable_runner_unit_id(
                    &self.host_installation_id,
                    &request.reservation_operation_id,
                )),
                None,
                digest,
            ),
        };
        let mut receipt = HostedHouseRunnerReservationReceiptV1 {
            schema: RECEIPT_SCHEMA_V1.to_owned(),
            host_installation_id: self.host_installation_id.to_string(),
            reservation_operation_id: request.reservation_operation_id.clone(),
            launch_request_id: request.launch_request_id.clone(),
            listing_revision_digest: request.listing_revision_digest.clone(),
            seat_id: request.seat_id.clone(),
            house_agent_revision_digest: request.house_agent_revision_digest.clone(),
            outcome,
            runner_unit_id,
            failure_code,
            binding_digest,
            authentication_tag: String::new(),
        };
        receipt.authentication_tag = self.sign_receipt(&receipt)?;
        let record = RetainedReservationV1 {
            schema: RESERVATION_SCHEMA_V1.to_owned(),
            request: request.clone(),
            receipt: receipt.clone(),
        };
        persist_new(
            &self
                .reservations
                .join(format!("{}.json", request.reservation_operation_id)),
            &record,
        )?;
        Ok(receipt)
    }

    /// Stops, fences, and durably records one exact retained House Runner unit.
    ///
    /// The hosted Controller route authenticates the caller. This operation
    /// then independently binds that request to the retained reservation and
    /// (when Genesis occurred) to the exact assignment/runtime binding. It
    /// never changes an Assignment, reservation receipt, or allowance ledger.
    /// A retry returns the original signed retirement receipt.
    ///
    /// # Errors
    /// Returns a closed failure when platform evidence is malformed, the
    /// target is changed or ambiguous, a known child cannot be stopped, or the
    /// durable fence cannot be established.
    pub fn retire(
        &self,
        request: &HostedHouseRunnerRetirementRequestV1,
    ) -> Result<HostedHouseRunnerRetirementReceiptV1, HostedHouseRunnerErrorV1> {
        validate_retirement_request(request, &self.host_installation_id)?;
        let _guard = self.lock();
        let reservation = self.load_reservation(&request.reservation_operation_id)?;
        self.verify_receipt(&reservation.receipt)?;
        if reservation.receipt.outcome != HostedHouseRunnerReservationOutcomeV1::Succeeded
            || reservation.receipt.launch_request_id != request.launch_request_id
        {
            return Err(HostedHouseRunnerErrorV1::Invalid);
        }
        let runner_unit_id = reservation
            .receipt
            .runner_unit_id
            .as_deref()
            .ok_or(HostedHouseRunnerErrorV1::Unavailable)?;
        let unit_directory = self.units.join(runner_unit_id);

        if let Some(receipt) = self.automatic_retirement(&unit_directory)? {
            if receipt_matches_retirement_request(&receipt, request, runner_unit_id) {
                self.verify_retirement_receipt(&receipt)?;
                return Ok(receipt);
            }
            return Err(HostedHouseRunnerErrorV1::Conflict);
        }
        // A retained legacy operator marker is already a fence. It cannot be
        // converted into an automatic receipt because it lacks the platform
        // evidence correspondence required by this path.
        if self.operator_retired(&reservation.receipt)? {
            return Err(HostedHouseRunnerErrorV1::Conflict);
        }

        let launch_binding =
            self.load_launch_binding_unlocked(&reservation.receipt.reservation_operation_id)?;
        let runtime_binding =
            self.load_runtime_binding_unlocked(&reservation.receipt.reservation_operation_id)?;
        validate_retirement_target(
            request,
            &reservation.receipt,
            launch_binding.as_ref(),
            runtime_binding.as_ref(),
        )?;

        // Install the Host-owned fence before draining a live unit. A crash
        // after this point can leave capacity occupied, but never lets a late
        // launch retry spawn a replacement child; the same evidence-bound
        // request resumes the stop/finalize sequence below.
        let directory = prepare_data_directory(&unit_directory)
            .map_err(|_| HostedHouseRunnerErrorV1::Unavailable)?;
        self.install_or_match_retirement_intent(&directory, request)?;
        let stop_witness = self.source.retire(runtime_binding.as_ref())?;
        if !valid_evidence_digest(&stop_witness) {
            return Err(HostedHouseRunnerErrorV1::Unavailable);
        }
        let mut receipt = HostedHouseRunnerRetirementReceiptV1 {
            schema: RETIREMENT_RECEIPT_SCHEMA_V1.to_owned(),
            host_installation_id: self.host_installation_id.to_string(),
            reservation_operation_id: request.reservation_operation_id.clone(),
            launch_request_id: request.launch_request_id.clone(),
            house_agent_assignment_id: request.house_agent_assignment_id.clone(),
            runner_unit_id: runner_unit_id.to_owned(),
            disposition: request.disposition,
            platform_evidence_digest: request.platform_evidence_digest.clone(),
            stop_witness,
            authentication_tag: String::new(),
        };
        receipt.authentication_tag = self.sign_retirement_receipt(&receipt)?;
        let marker = RetainedHouseRunnerRetirementV1 {
            schema: RETIREMENT_SCHEMA_V1.to_owned(),
            authority: "host_reconciliation_after_exact_stop".to_owned(),
            receipt: receipt.clone(),
            allowance_reset: false,
        };
        let marker_path = directory.join(RETIREMENT_MARKER_FILE);
        match persist_new(&marker_path, &marker) {
            Ok(()) => Ok(receipt),
            Err(HostedHouseRunnerErrorV1::Conflict) => {
                let Some(existing) = self.automatic_retirement(&directory)? else {
                    return Err(HostedHouseRunnerErrorV1::Unavailable);
                };
                if existing == receipt {
                    self.verify_retirement_receipt(&existing)?;
                    Ok(existing)
                } else {
                    Err(HostedHouseRunnerErrorV1::Conflict)
                }
            }
            Err(error) => Err(error),
        }
    }

    /// Stops or pre-fences every successful House reservation belonging to a
    /// creator-closed launch. The supplied digest names the durable closure
    /// intent installed before this operation begins.
    ///
    /// # Errors
    /// Fails closed if any retained reservation or child cannot be matched,
    /// stopped, and durably fenced.
    pub fn close_launch(
        &self,
        launch_request_id: &str,
        closure_intent_digest: &str,
    ) -> Result<(), HostedHouseRunnerErrorV1> {
        if !uuid_reference(launch_request_id) || !valid_evidence_digest(closure_intent_digest) {
            return Err(HostedHouseRunnerErrorV1::Invalid);
        }
        let requests = {
            let _guard = self.lock();
            self.install_or_match_launch_closure_fence_unlocked(
                launch_request_id,
                closure_intent_digest,
            )?;
            let mut requests = Vec::new();
            for reservation in self.reservations_unlocked()? {
                if reservation.receipt.launch_request_id != launch_request_id
                    || reservation.receipt.outcome
                        != HostedHouseRunnerReservationOutcomeV1::Succeeded
                {
                    continue;
                }
                // A terminal or earlier abandonment path may already have
                // installed exact stop evidence. It is already safe for the
                // launch closure to rely on that stronger fence; attempting a
                // second disposition would only manufacture a false conflict.
                if self.operator_retired(&reservation.receipt)? {
                    continue;
                }
                let runner_unit_id = reservation
                    .receipt
                    .runner_unit_id
                    .as_deref()
                    .ok_or(HostedHouseRunnerErrorV1::Unavailable)?;
                if let Some(intent) = self.retirement_intent(&self.units.join(runner_unit_id))? {
                    // A failed earlier stop already installed the stronger
                    // spawn fence. Resume its exact request so closure cannot
                    // deadlock by trying to replace immutable evidence with a
                    // new disposition.
                    requests.push(intent.request);
                    continue;
                }
                let launch_binding = self
                    .load_launch_binding_unlocked(&reservation.receipt.reservation_operation_id)?;
                requests.push(HostedHouseRunnerRetirementRequestV1 {
                    schema: "worldstream/house-runner-retirement-request/v1".to_owned(),
                    host_installation_id: self.host_installation_id.to_string(),
                    reservation_operation_id: reservation.receipt.reservation_operation_id,
                    launch_request_id: launch_request_id.to_owned(),
                    house_agent_assignment_id: launch_binding
                        .map(|binding| binding.house_agent_assignment_id),
                    disposition: HostedHouseRunnerRetirementDispositionV1::CreatorClosed,
                    platform_evidence_digest: closure_intent_digest.to_owned(),
                });
            }
            requests
        };
        for request in requests {
            self.retire(&request)?;
        }
        Ok(())
    }

    fn operator_retired(
        &self,
        receipt: &HostedHouseRunnerReservationReceiptV1,
    ) -> Result<bool, HostedHouseRunnerErrorV1> {
        let unit = receipt
            .runner_unit_id
            .as_ref()
            .ok_or(HostedHouseRunnerErrorV1::Unavailable)?;
        let directory = self.units.join(unit);
        if let Some(retirement) = self.automatic_retirement(&directory)? {
            // A valid Host signature alone is not sufficient to release a
            // slot. The retained fence must attest to this exact succeeded
            // reservation and its deterministic unit; otherwise a copied
            // fence could free unrelated capacity.
            if retirement.reservation_operation_id != receipt.reservation_operation_id
                || retirement.launch_request_id != receipt.launch_request_id
                || retirement.runner_unit_id != *unit
            {
                return Err(HostedHouseRunnerErrorV1::Unavailable);
            }
            return Ok(true);
        }
        // An incomplete retirement is a durable spawn fence, but not proof
        // of a completed stop and therefore never frees capacity.
        if self.retirement_intent(&directory)?.is_some() {
            return Ok(false);
        }
        let evidence: serde_json::Value =
            match read_record(&directory.join(OPERATOR_RETIREMENT_MARKER_FILE)) {
                Ok(value) => value,
                Err(RecordReadErrorV1::NotFound) => return Ok(false),
                Err(RecordReadErrorV1::Unavailable) => {
                    return Err(HostedHouseRunnerErrorV1::Unavailable);
                }
            };
        worldstream_runtime::validate_data_directory(&directory)
            .map_err(|_| HostedHouseRunnerErrorV1::Unavailable)?;
        if evidence["schema"] != "worldstream/operator-house-retirement/v1"
            || evidence["authority"] != "operator_observation_not_runtime_attestation"
            || evidence["installation_id"] != self.host_installation_id.as_ref()
            || evidence["runner_unit_id"] != *unit
            || evidence["reservation_operation_id"] != receipt.reservation_operation_id
            || evidence["launch_request_id"] != receipt.launch_request_id
            || evidence["allowance_reset"] != false
        {
            return Err(HostedHouseRunnerErrorV1::Unavailable);
        }
        Ok(true)
    }

    fn automatic_retirement(
        &self,
        directory: &Path,
    ) -> Result<Option<HostedHouseRunnerRetirementReceiptV1>, HostedHouseRunnerErrorV1> {
        let marker: RetainedHouseRunnerRetirementV1 =
            match read_record(&directory.join(RETIREMENT_MARKER_FILE)) {
                Ok(value) => value,
                Err(RecordReadErrorV1::NotFound) => return Ok(None),
                Err(RecordReadErrorV1::Unavailable) => {
                    return Err(HostedHouseRunnerErrorV1::Unavailable);
                }
            };
        worldstream_runtime::validate_data_directory(directory)
            .map_err(|_| HostedHouseRunnerErrorV1::Unavailable)?;
        if marker.schema != RETIREMENT_SCHEMA_V1
            || marker.authority != "host_reconciliation_after_exact_stop"
            || marker.allowance_reset
        {
            return Err(HostedHouseRunnerErrorV1::Unavailable);
        }
        self.verify_retirement_receipt(&marker.receipt)?;
        Ok(Some(marker.receipt))
    }

    fn install_or_match_retirement_intent(
        &self,
        directory: &Path,
        request: &HostedHouseRunnerRetirementRequestV1,
    ) -> Result<(), HostedHouseRunnerErrorV1> {
        let mut intent = RetainedHouseRunnerRetirementIntentV1 {
            schema: RETIREMENT_SCHEMA_V1.to_owned(),
            authority: "host_reconciliation_before_exact_stop".to_owned(),
            request: request.clone(),
            authentication_tag: String::new(),
            allowance_reset: false,
        };
        intent.authentication_tag = self.sign_retirement_intent(&intent)?;
        let path = directory.join(RETIREMENT_INTENT_MARKER_FILE);
        match persist_new(&path, &intent) {
            Ok(()) => Ok(()),
            Err(HostedHouseRunnerErrorV1::Conflict) => {
                let existing = self
                    .retirement_intent(directory)?
                    .ok_or(HostedHouseRunnerErrorV1::Unavailable)?;
                if existing.request == *request {
                    Ok(())
                } else {
                    Err(HostedHouseRunnerErrorV1::Conflict)
                }
            }
            Err(error) => Err(error),
        }
    }

    fn retirement_intent(
        &self,
        directory: &Path,
    ) -> Result<Option<RetainedHouseRunnerRetirementIntentV1>, HostedHouseRunnerErrorV1> {
        let intent: RetainedHouseRunnerRetirementIntentV1 =
            match read_record(&directory.join(RETIREMENT_INTENT_MARKER_FILE)) {
                Ok(value) => value,
                Err(RecordReadErrorV1::NotFound) => return Ok(None),
                Err(RecordReadErrorV1::Unavailable) => {
                    return Err(HostedHouseRunnerErrorV1::Unavailable);
                }
            };
        worldstream_runtime::validate_data_directory(directory)
            .map_err(|_| HostedHouseRunnerErrorV1::Unavailable)?;
        if intent.schema != RETIREMENT_SCHEMA_V1
            || intent.authority != "host_reconciliation_before_exact_stop"
            || intent.allowance_reset
        {
            return Err(HostedHouseRunnerErrorV1::Unavailable);
        }
        self.verify_retirement_intent(&intent)?;
        Ok(Some(intent))
    }

    fn load_launch_binding_unlocked(
        &self,
        reservation_operation_id: &str,
    ) -> Result<Option<RetainedLaunchBindingV1>, HostedHouseRunnerErrorV1> {
        match read_record(
            &self
                .launch_bindings
                .join(format!("{reservation_operation_id}.json")),
        ) {
            Ok(binding) if valid_launch_binding(&binding) => Ok(Some(binding)),
            Ok(_) => Err(HostedHouseRunnerErrorV1::Unavailable),
            Err(RecordReadErrorV1::NotFound) => Ok(None),
            Err(RecordReadErrorV1::Unavailable) => Err(HostedHouseRunnerErrorV1::Unavailable),
        }
    }

    fn load_runtime_binding_unlocked(
        &self,
        reservation_operation_id: &str,
    ) -> Result<Option<RetainedRuntimeBindingV1>, HostedHouseRunnerErrorV1> {
        let root = self
            .units
            .parent()
            .ok_or(HostedHouseRunnerErrorV1::Unavailable)?
            .join("runtime-bindings");
        match read_record(&root.join(format!("{reservation_operation_id}.json"))) {
            Ok(binding) if valid_runtime_binding(&binding) => Ok(Some(binding)),
            Ok(_) => Err(HostedHouseRunnerErrorV1::Unavailable),
            Err(RecordReadErrorV1::NotFound) => Ok(None),
            Err(RecordReadErrorV1::Unavailable) => Err(HostedHouseRunnerErrorV1::Unavailable),
        }
    }

    /// Reads one exact reservation identity without re-running dependency checks.
    ///
    /// # Errors
    /// Returns a closed invalid, conflict, not-found, or availability failure.
    pub fn read(
        &self,
        request: &HostedHouseRunnerReservationRequestV1,
    ) -> Result<HostedHouseRunnerReservationReceiptV1, HostedHouseRunnerErrorV1> {
        let listing = self
            .listings
            .get(&request.listing_revision_digest)
            .ok_or(HostedHouseRunnerErrorV1::Invalid)?;
        validate_hosted_house_runner_reservation_request(
            request,
            &self.host_installation_id,
            listing,
            &self.revisions.values().cloned().collect::<Vec<_>>(),
        )
        .map_err(|_| HostedHouseRunnerErrorV1::Invalid)?;
        let _guard = self.lock();
        let record = self.load_reservation(&request.reservation_operation_id)?;
        if record.request != *request {
            return Err(HostedHouseRunnerErrorV1::Conflict);
        }
        self.verify_receipt(&record.receipt)?;
        Ok(record.receipt)
    }

    /// Authenticates and durably binds every successful receipt before Room mutation.
    ///
    /// # Errors
    /// Returns a closed failure for invalid authentication, changed identity, or unavailable storage.
    pub fn bind_launch(
        &self,
        launch: &HostedLaunchRequestV1,
    ) -> Result<(), HostedHouseRunnerErrorV1> {
        if launch.house_runner_assignments.len() > MAX_HOUSE_RUNNERS_PER_LAUNCH {
            return Err(HostedHouseRunnerErrorV1::Invalid);
        }
        let _guard = self.lock();
        if self
            .load_launch_closure_fence_unlocked(
                &launch.capacity_authorization.reservation_reference,
            )?
            .is_some()
        {
            return Err(HostedHouseRunnerErrorV1::Conflict);
        }
        let existing = self.launch_bindings_unlocked()?;
        for assignment in &launch.house_runner_assignments {
            let receipt = &assignment.reservation_receipt;
            self.verify_receipt(receipt)?;
            let reservation = self.load_reservation(&receipt.reservation_operation_id)?;
            if reservation.receipt != *receipt
                || receipt.outcome != HostedHouseRunnerReservationOutcomeV1::Succeeded
            {
                return Err(HostedHouseRunnerErrorV1::Invalid);
            }
            let requested = RetainedLaunchBindingV1 {
                schema: LAUNCH_BINDING_SCHEMA_V1.to_owned(),
                reservation_operation_id: receipt.reservation_operation_id.clone(),
                launch_request_id: receipt.launch_request_id.clone(),
                room_setup_operation_id: launch.room_setup_operation_id.clone(),
                house_agent_assignment_id: assignment.house_agent_assignment_id.clone(),
                runner_unit_id: receipt
                    .runner_unit_id
                    .clone()
                    .ok_or(HostedHouseRunnerErrorV1::Invalid)?,
            };
            if existing.iter().any(|binding| {
                (binding.house_agent_assignment_id == requested.house_agent_assignment_id
                    || binding.runner_unit_id == requested.runner_unit_id
                    || binding.reservation_operation_id == requested.reservation_operation_id)
                    && binding != &requested
            }) {
                return Err(HostedHouseRunnerErrorV1::Conflict);
            }
            let path = self
                .launch_bindings
                .join(format!("{}.json", requested.reservation_operation_id));
            match read_record::<RetainedLaunchBindingV1>(&path) {
                Ok(binding) if binding == requested => {}
                Ok(_) => return Err(HostedHouseRunnerErrorV1::Conflict),
                Err(RecordReadErrorV1::NotFound) => persist_new(&path, &requested)?,
                Err(RecordReadErrorV1::Unavailable) => {
                    return Err(HostedHouseRunnerErrorV1::Unavailable);
                }
            }
        }
        Ok(())
    }

    /// Starts or reconciles the same exact units and reports the Lobby gate.
    #[must_use]
    pub fn start_launch(
        &self,
        launch: &HostedLaunchRequestV1,
        room_id: &str,
    ) -> HostedHouseRunnerGateV1 {
        if launch.house_runner_assignments.is_empty() {
            return HostedHouseRunnerGateV1::Ready;
        }
        let Some(listing) = self.listings.get(&launch.listing_revision_digest) else {
            return HostedHouseRunnerGateV1::TerminalFailure;
        };
        let _guard = self.lock();
        match self.load_launch_closure_fence_unlocked(
            &launch.capacity_authorization.reservation_reference,
        ) {
            Ok(Some(_)) => return HostedHouseRunnerGateV1::TerminalFailure,
            Ok(None) => {}
            Err(_) => return HostedHouseRunnerGateV1::RetryableFailure,
        }
        for assignment in &launch.house_runner_assignments {
            let receipt = &assignment.reservation_receipt;
            let Some(revision) = self.revisions.get(&receipt.house_agent_revision_digest) else {
                return HostedHouseRunnerGateV1::TerminalFailure;
            };
            match self
                .source
                .start(launch, assignment, room_id, listing, revision)
            {
                Ok(()) => {}
                Err(StartErrorV1::Terminal) => {
                    return HostedHouseRunnerGateV1::TerminalFailure;
                }
                Err(StartErrorV1::Unavailable) => {
                    return HostedHouseRunnerGateV1::RetryableFailure;
                }
            }
        }
        HostedHouseRunnerGateV1::Ready
    }

    fn validate_retained(&self) -> Result<(), HostedHouseRunnerErrorV1> {
        let _guard = self.lock();
        for reservation in self.reservations_unlocked()? {
            let Some(listing) = self
                .listings
                .get(&reservation.request.listing_revision_digest)
            else {
                return Err(HostedHouseRunnerErrorV1::Unavailable);
            };
            if reservation.schema != RESERVATION_SCHEMA_V1
                || validate_hosted_house_runner_reservation_request(
                    &reservation.request,
                    &self.host_installation_id,
                    listing,
                    &self.revisions.values().cloned().collect::<Vec<_>>(),
                )
                .is_err()
                || !receipt_matches_request(&reservation.receipt, &reservation.request)
            {
                return Err(HostedHouseRunnerErrorV1::Unavailable);
            }
            self.verify_receipt(&reservation.receipt)?;
        }
        let mut assignment_ids = std::collections::BTreeSet::new();
        let mut runner_units = std::collections::BTreeSet::new();
        for binding in self.launch_bindings_unlocked()? {
            let reservation = self
                .load_reservation(&binding.reservation_operation_id)
                .map_err(|_| HostedHouseRunnerErrorV1::Unavailable)?;
            if !valid_launch_binding(&binding)
                || reservation.receipt.outcome != HostedHouseRunnerReservationOutcomeV1::Succeeded
                || reservation.receipt.launch_request_id != binding.launch_request_id
                || reservation.receipt.runner_unit_id.as_deref()
                    != Some(binding.runner_unit_id.as_str())
                || !assignment_ids.insert(binding.house_agent_assignment_id.clone())
                || !runner_units.insert(binding.runner_unit_id.clone())
            {
                return Err(HostedHouseRunnerErrorV1::Unavailable);
            }
        }
        for fence in self.launch_closure_fences_unlocked()? {
            self.verify_launch_closure_fence(&fence)?;
        }
        Ok(())
    }

    fn reservations_unlocked(
        &self,
    ) -> Result<Vec<RetainedReservationV1>, HostedHouseRunnerErrorV1> {
        read_records(&self.reservations, MAX_RESERVATIONS)
    }

    fn launch_bindings_unlocked(
        &self,
    ) -> Result<Vec<RetainedLaunchBindingV1>, HostedHouseRunnerErrorV1> {
        read_records(&self.launch_bindings, MAX_RESERVATIONS)
    }

    fn launch_closure_fences_unlocked(
        &self,
    ) -> Result<Vec<RetainedHouseRunnerLaunchClosureFenceV1>, HostedHouseRunnerErrorV1> {
        read_records(&self.launch_closure_fences, MAX_RESERVATIONS)
    }

    fn load_launch_closure_fence_unlocked(
        &self,
        launch_request_id: &str,
    ) -> Result<Option<RetainedHouseRunnerLaunchClosureFenceV1>, HostedHouseRunnerErrorV1> {
        if !uuid_reference(launch_request_id) {
            return Err(HostedHouseRunnerErrorV1::Invalid);
        }
        match read_record(
            &self
                .launch_closure_fences
                .join(format!("{launch_request_id}.json")),
        ) {
            Ok(fence) => {
                self.verify_launch_closure_fence(&fence)?;
                Ok(Some(fence))
            }
            Err(RecordReadErrorV1::NotFound) => Ok(None),
            Err(RecordReadErrorV1::Unavailable) => Err(HostedHouseRunnerErrorV1::Unavailable),
        }
    }

    fn install_or_match_launch_closure_fence_unlocked(
        &self,
        launch_request_id: &str,
        closure_intent_digest: &str,
    ) -> Result<(), HostedHouseRunnerErrorV1> {
        let mut fence = RetainedHouseRunnerLaunchClosureFenceV1 {
            schema: LAUNCH_CLOSURE_FENCE_SCHEMA_V1.to_owned(),
            host_installation_id: self.host_installation_id.to_string(),
            launch_request_id: launch_request_id.to_owned(),
            closure_intent_digest: closure_intent_digest.to_owned(),
            authentication_tag: String::new(),
        };
        fence.authentication_tag = self.sign_launch_closure_fence(&fence)?;
        let path = self
            .launch_closure_fences
            .join(format!("{launch_request_id}.json"));
        match persist_new(&path, &fence) {
            Ok(()) => Ok(()),
            Err(HostedHouseRunnerErrorV1::Conflict) => {
                let existing = self
                    .load_launch_closure_fence_unlocked(launch_request_id)?
                    .ok_or(HostedHouseRunnerErrorV1::Unavailable)?;
                if existing == fence {
                    Ok(())
                } else {
                    Err(HostedHouseRunnerErrorV1::Conflict)
                }
            }
            Err(error) => Err(error),
        }
    }

    fn load_reservation(
        &self,
        operation: &str,
    ) -> Result<RetainedReservationV1, HostedHouseRunnerErrorV1> {
        if !uuid_reference(operation) {
            return Err(HostedHouseRunnerErrorV1::Invalid);
        }
        match read_record(&self.reservations.join(format!("{operation}.json"))) {
            Ok(record) => Ok(record),
            Err(RecordReadErrorV1::NotFound) => Err(HostedHouseRunnerErrorV1::NotFound),
            Err(RecordReadErrorV1::Unavailable) => Err(HostedHouseRunnerErrorV1::Unavailable),
        }
    }

    fn sign_receipt(
        &self,
        receipt: &HostedHouseRunnerReservationReceiptV1,
    ) -> Result<String, HostedHouseRunnerErrorV1> {
        let bytes = canonical_bytes(&ReceiptWitnessV1 {
            domain: RECEIPT_TAG_DOMAIN,
            schema: &receipt.schema,
            host_installation_id: &receipt.host_installation_id,
            reservation_operation_id: &receipt.reservation_operation_id,
            launch_request_id: &receipt.launch_request_id,
            listing_revision_digest: &receipt.listing_revision_digest,
            seat_id: &receipt.seat_id,
            house_agent_revision_digest: &receipt.house_agent_revision_digest,
            outcome: receipt.outcome,
            runner_unit_id: &receipt.runner_unit_id,
            failure_code: &receipt.failure_code,
            binding_digest: &receipt.binding_digest,
        })?;
        Ok(hex(hmac::sign(&self.authenticator, &bytes).as_ref()))
    }

    fn verify_receipt(
        &self,
        receipt: &HostedHouseRunnerReservationReceiptV1,
    ) -> Result<(), HostedHouseRunnerErrorV1> {
        validate_hosted_house_runner_reservation_receipt(receipt)
            .map_err(|_| HostedHouseRunnerErrorV1::Invalid)?;
        if receipt.host_installation_id != self.host_installation_id.as_ref() {
            return Err(HostedHouseRunnerErrorV1::Invalid);
        }
        hmac::verify(
            &self.authenticator,
            &canonical_bytes(&ReceiptWitnessV1 {
                domain: RECEIPT_TAG_DOMAIN,
                schema: &receipt.schema,
                host_installation_id: &receipt.host_installation_id,
                reservation_operation_id: &receipt.reservation_operation_id,
                launch_request_id: &receipt.launch_request_id,
                listing_revision_digest: &receipt.listing_revision_digest,
                seat_id: &receipt.seat_id,
                house_agent_revision_digest: &receipt.house_agent_revision_digest,
                outcome: receipt.outcome,
                runner_unit_id: &receipt.runner_unit_id,
                failure_code: &receipt.failure_code,
                binding_digest: &receipt.binding_digest,
            })?,
            &decode_hex(&receipt.authentication_tag)?,
        )
        .map_err(|_| HostedHouseRunnerErrorV1::Invalid)?;
        Ok(())
    }

    fn sign_retirement_receipt(
        &self,
        receipt: &HostedHouseRunnerRetirementReceiptV1,
    ) -> Result<String, HostedHouseRunnerErrorV1> {
        let bytes = canonical_bytes(&RetirementReceiptWitnessV1 {
            domain: RETIREMENT_TAG_DOMAIN,
            schema: &receipt.schema,
            host_installation_id: &receipt.host_installation_id,
            reservation_operation_id: &receipt.reservation_operation_id,
            launch_request_id: &receipt.launch_request_id,
            house_agent_assignment_id: &receipt.house_agent_assignment_id,
            runner_unit_id: &receipt.runner_unit_id,
            disposition: receipt.disposition,
            platform_evidence_digest: &receipt.platform_evidence_digest,
            stop_witness: &receipt.stop_witness,
        })?;
        Ok(hex(hmac::sign(&self.authenticator, &bytes).as_ref()))
    }

    fn verify_retirement_receipt(
        &self,
        receipt: &HostedHouseRunnerRetirementReceiptV1,
    ) -> Result<(), HostedHouseRunnerErrorV1> {
        if receipt.schema != RETIREMENT_RECEIPT_SCHEMA_V1
            || receipt.host_installation_id != self.host_installation_id.as_ref()
            || !uuid_reference(&receipt.reservation_operation_id)
            || !uuid_reference(&receipt.launch_request_id)
            || !receipt
                .house_agent_assignment_id
                .as_deref()
                .is_none_or(uuid_reference)
            || !safe_public_reference(&receipt.runner_unit_id, 128)
            || !valid_evidence_digest(&receipt.platform_evidence_digest)
            || !valid_evidence_digest(&receipt.stop_witness)
        {
            return Err(HostedHouseRunnerErrorV1::Invalid);
        }
        hmac::verify(
            &self.authenticator,
            &canonical_bytes(&RetirementReceiptWitnessV1 {
                domain: RETIREMENT_TAG_DOMAIN,
                schema: &receipt.schema,
                host_installation_id: &receipt.host_installation_id,
                reservation_operation_id: &receipt.reservation_operation_id,
                launch_request_id: &receipt.launch_request_id,
                house_agent_assignment_id: &receipt.house_agent_assignment_id,
                runner_unit_id: &receipt.runner_unit_id,
                disposition: receipt.disposition,
                platform_evidence_digest: &receipt.platform_evidence_digest,
                stop_witness: &receipt.stop_witness,
            })?,
            &decode_hex(&receipt.authentication_tag)?,
        )
        .map_err(|_| HostedHouseRunnerErrorV1::Invalid)?;
        Ok(())
    }

    fn sign_retirement_intent(
        &self,
        intent: &RetainedHouseRunnerRetirementIntentV1,
    ) -> Result<String, HostedHouseRunnerErrorV1> {
        let bytes = canonical_bytes(&RetirementIntentWitnessV1 {
            domain: RETIREMENT_TAG_DOMAIN,
            schema: &intent.schema,
            authority: &intent.authority,
            request: &intent.request,
            allowance_reset: intent.allowance_reset,
        })?;
        Ok(hex(hmac::sign(&self.authenticator, &bytes).as_ref()))
    }

    fn verify_retirement_intent(
        &self,
        intent: &RetainedHouseRunnerRetirementIntentV1,
    ) -> Result<(), HostedHouseRunnerErrorV1> {
        validate_retirement_request(&intent.request, &self.host_installation_id)?;
        hmac::verify(
            &self.authenticator,
            &canonical_bytes(&RetirementIntentWitnessV1 {
                domain: RETIREMENT_TAG_DOMAIN,
                schema: &intent.schema,
                authority: &intent.authority,
                request: &intent.request,
                allowance_reset: intent.allowance_reset,
            })?,
            &decode_hex(&intent.authentication_tag)?,
        )
        .map_err(|_| HostedHouseRunnerErrorV1::Invalid)
    }

    fn sign_launch_closure_fence(
        &self,
        fence: &RetainedHouseRunnerLaunchClosureFenceV1,
    ) -> Result<String, HostedHouseRunnerErrorV1> {
        let bytes = canonical_bytes(&LaunchClosureFenceWitnessV1 {
            domain: LAUNCH_CLOSURE_FENCE_TAG_DOMAIN,
            schema: &fence.schema,
            host_installation_id: &fence.host_installation_id,
            launch_request_id: &fence.launch_request_id,
            closure_intent_digest: &fence.closure_intent_digest,
        })?;
        Ok(hex(hmac::sign(&self.authenticator, &bytes).as_ref()))
    }

    fn verify_launch_closure_fence(
        &self,
        fence: &RetainedHouseRunnerLaunchClosureFenceV1,
    ) -> Result<(), HostedHouseRunnerErrorV1> {
        if fence.schema != LAUNCH_CLOSURE_FENCE_SCHEMA_V1
            || fence.host_installation_id != self.host_installation_id.as_ref()
            || !uuid_reference(&fence.launch_request_id)
            || !valid_evidence_digest(&fence.closure_intent_digest)
        {
            return Err(HostedHouseRunnerErrorV1::Invalid);
        }
        hmac::verify(
            &self.authenticator,
            &canonical_bytes(&LaunchClosureFenceWitnessV1 {
                domain: LAUNCH_CLOSURE_FENCE_TAG_DOMAIN,
                schema: &fence.schema,
                host_installation_id: &fence.host_installation_id,
                launch_request_id: &fence.launch_request_id,
                closure_intent_digest: &fence.closure_intent_digest,
            })?,
            &decode_hex(&fence.authentication_tag)?,
        )
        .map_err(|_| HostedHouseRunnerErrorV1::Invalid)
    }

    fn lock(&self) -> MutexGuard<'_, ()> {
        self.mutation.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn unique_by_digest<T>(items: Vec<T>) -> Result<BTreeMap<String, T>, HostedHouseRunnerErrorV1>
where
    T: DigestIdentifiedV1,
{
    let mut indexed = BTreeMap::new();
    for item in items {
        if indexed.insert(item.digest().to_owned(), item).is_some() {
            return Err(HostedHouseRunnerErrorV1::Invalid);
        }
    }
    Ok(indexed)
}

trait DigestIdentifiedV1 {
    fn digest(&self) -> &str;
}

impl DigestIdentifiedV1 for ListingRevision {
    fn digest(&self) -> &str {
        ListingRevision::digest(self)
    }
}

impl DigestIdentifiedV1 for HouseAgentRevision {
    fn digest(&self) -> &str {
        HouseAgentRevision::digest(self)
    }
}

fn failed_binding_digest(
    request: &HostedHouseRunnerReservationRequestV1,
    code: &str,
) -> Result<String, HostedHouseRunnerErrorV1> {
    canonical_blake3(&serde_json::json!({
        "domain": BINDING_DIGEST_DOMAIN,
        "failure_code": code,
        "request": request,
    }))
}

fn stable_runner_unit_id(host: &str, operation: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"worldstream/house-runner-unit/v1\0");
    hasher.update(host.as_bytes());
    hasher.update(b"\0");
    hasher.update(operation.as_bytes());
    format!("house-{}", &hasher.finalize().to_hex()[..32])
}

fn receipt_matches_request(
    receipt: &HostedHouseRunnerReservationReceiptV1,
    request: &HostedHouseRunnerReservationRequestV1,
) -> bool {
    receipt.host_installation_id == request.host_installation_id
        && receipt.reservation_operation_id == request.reservation_operation_id
        && receipt.launch_request_id == request.launch_request_id
        && receipt.listing_revision_digest == request.listing_revision_digest
        && receipt.seat_id == request.seat_id
        && receipt.house_agent_revision_digest == request.house_agent_revision_digest
}

fn valid_launch_binding(binding: &RetainedLaunchBindingV1) -> bool {
    binding.schema == LAUNCH_BINDING_SCHEMA_V1
        && uuid_reference(&binding.reservation_operation_id)
        && uuid_reference(&binding.launch_request_id)
        && safe_public_reference(&binding.room_setup_operation_id, 128)
        && uuid_reference(&binding.house_agent_assignment_id)
        && safe_public_reference(&binding.runner_unit_id, 128)
}

fn validate_retirement_request(
    request: &HostedHouseRunnerRetirementRequestV1,
    host_installation_id: &str,
) -> Result<(), HostedHouseRunnerErrorV1> {
    if request.schema != "worldstream/house-runner-retirement-request/v1"
        || request.host_installation_id != host_installation_id
        || !uuid_reference(&request.reservation_operation_id)
        || !uuid_reference(&request.launch_request_id)
        || !request
            .house_agent_assignment_id
            .as_deref()
            .is_none_or(uuid_reference)
        || !valid_evidence_digest(&request.platform_evidence_digest)
    {
        return Err(HostedHouseRunnerErrorV1::Invalid);
    }
    Ok(())
}

fn validate_retirement_target(
    request: &HostedHouseRunnerRetirementRequestV1,
    reservation: &HostedHouseRunnerReservationReceiptV1,
    launch_binding: Option<&RetainedLaunchBindingV1>,
    runtime_binding: Option<&RetainedRuntimeBindingV1>,
) -> Result<(), HostedHouseRunnerErrorV1> {
    let Some(runner_unit_id) = reservation.runner_unit_id.as_deref() else {
        return Err(HostedHouseRunnerErrorV1::Invalid);
    };
    match launch_binding {
        Some(binding)
            if binding.reservation_operation_id == request.reservation_operation_id
                && binding.launch_request_id == request.launch_request_id
                && binding.runner_unit_id == runner_unit_id
                && request.house_agent_assignment_id.as_deref()
                    == Some(binding.house_agent_assignment_id.as_str()) => {}
        Some(_) => return Err(HostedHouseRunnerErrorV1::Conflict),
        None if matches!(
            request.disposition,
            HostedHouseRunnerRetirementDispositionV1::FailedPreGenesis
                | HostedHouseRunnerRetirementDispositionV1::CreatorClosed
        ) && request.house_agent_assignment_id.is_none() => {}
        None => return Err(HostedHouseRunnerErrorV1::Conflict),
    }
    match runtime_binding {
        Some(binding)
            if binding.reservation_operation_id == request.reservation_operation_id
                && binding.runner_unit_id == runner_unit_id
                && request.house_agent_assignment_id.as_deref()
                    == Some(binding.house_agent_assignment_id.as_str())
                && request.disposition
                    != HostedHouseRunnerRetirementDispositionV1::FailedPreGenesis =>
        {
            if let Some(launch) = launch_binding
                && binding.room_setup_operation_id != launch.room_setup_operation_id
            {
                return Err(HostedHouseRunnerErrorV1::Conflict);
            }
        }
        Some(_) => return Err(HostedHouseRunnerErrorV1::Conflict),
        None if request.disposition == HostedHouseRunnerRetirementDispositionV1::RunTerminal => {
            return Err(HostedHouseRunnerErrorV1::Unavailable);
        }
        None => {}
    }
    Ok(())
}

fn receipt_matches_retirement_request(
    receipt: &HostedHouseRunnerRetirementReceiptV1,
    request: &HostedHouseRunnerRetirementRequestV1,
    runner_unit_id: &str,
) -> bool {
    receipt.schema == RETIREMENT_RECEIPT_SCHEMA_V1
        && receipt.host_installation_id == request.host_installation_id
        && receipt.reservation_operation_id == request.reservation_operation_id
        && receipt.launch_request_id == request.launch_request_id
        && receipt.house_agent_assignment_id == request.house_agent_assignment_id
        && receipt.runner_unit_id == runner_unit_id
        && receipt.disposition == request.disposition
        && receipt.platform_evidence_digest == request.platform_evidence_digest
}

fn valid_runtime_binding(binding: &RetainedRuntimeBindingV1) -> bool {
    binding.schema == RUNTIME_BINDING_SCHEMA_V1
        && uuid_reference(&binding.reservation_operation_id)
        && safe_public_reference(&binding.room_setup_operation_id, 128)
        && uuid_reference(&binding.house_agent_assignment_id)
        && safe_public_reference(&binding.runner_unit_id, 128)
        && safe_public_reference(&binding.room_id, 128)
        && safe_public_reference(&binding.local_assignment_id, 128)
        && safe_public_reference(&binding.member_id, 128)
        && safe_public_reference(&binding.principal_id, 128)
        && safe_public_reference(&binding.role, 128)
        && safe_public_reference(&binding.runner_id, 128)
        && safe_public_reference(&binding.instance_id, 128)
        && valid_blake3_digest(&binding.house_agent_revision_digest)
        && safe_public_reference(&binding.profile_id, 128)
        && safe_public_reference(&binding.profile_revision, 128)
        && safe_public_reference(&binding.runner_template_id, 128)
        && safe_public_reference(&binding.runner_template_revision, 128)
}

fn valid_blake3_digest(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("blake3:")
        && value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_evidence_digest(value: &str) -> bool {
    value.len() == 71
        && (value.starts_with("blake3:") || value.starts_with("sha256:"))
        && value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn canonical_blake3(value: &serde_json::Value) -> Result<String, HostedHouseRunnerErrorV1> {
    let raw = serde_json::to_vec(value).map_err(|_| HostedHouseRunnerErrorV1::Unavailable)?;
    let canonical = CanonicalJsonV1::parse(&raw)
        .and_then(|value| value.to_bytes())
        .map_err(|_| HostedHouseRunnerErrorV1::Unavailable)?;
    Ok(format!("blake3:{}", blake3::hash(&canonical).to_hex()))
}

fn canonical_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, HostedHouseRunnerErrorV1> {
    let raw = serde_json::to_vec(value).map_err(|_| HostedHouseRunnerErrorV1::Unavailable)?;
    CanonicalJsonV1::parse(&raw)
        .and_then(|value| value.to_bytes())
        .map_err(|_| HostedHouseRunnerErrorV1::Unavailable)
}

fn load_or_create_receipt_key(root: &Path) -> Result<Zeroizing<Vec<u8>>, HostedHouseRunnerErrorV1> {
    let target = root.join(RECEIPT_KEY_FILE);
    match load_receipt_key(&target) {
        Ok(key) => return Ok(key),
        Err(RecordReadErrorV1::Unavailable) => {
            return Err(HostedHouseRunnerErrorV1::Unavailable);
        }
        Err(RecordReadErrorV1::NotFound) => {}
    }
    let mut key = Zeroizing::new(vec![0_u8; 32]);
    getrandom::fill(key.as_mut()).map_err(|_| HostedHouseRunnerErrorV1::Unavailable)?;
    let nonce = getrandom::u64().map_err(|_| HostedHouseRunnerErrorV1::Unavailable)?;
    let temporary = root.join(format!(".{RECEIPT_KEY_FILE}.{nonce:016x}.tmp"));
    let mut file =
        create_owner_only_file(&temporary).map_err(|_| HostedHouseRunnerErrorV1::Unavailable)?;
    if file
        .write_all(key.as_slice())
        .and_then(|()| file.sync_all())
        .is_err()
    {
        let _ = fs::remove_file(&temporary);
        return Err(HostedHouseRunnerErrorV1::Unavailable);
    }
    drop(file);
    match fs::hard_link(&temporary, &target) {
        Ok(()) => {
            let synchronized = fs::File::open(root).and_then(|directory| directory.sync_all());
            let _ = fs::remove_file(&temporary);
            synchronized.map_err(|_| HostedHouseRunnerErrorV1::Unavailable)?;
            Ok(key)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let _ = fs::remove_file(&temporary);
            load_receipt_key(&target).map_err(|_| HostedHouseRunnerErrorV1::Unavailable)
        }
        Err(_) => {
            let _ = fs::remove_file(&temporary);
            Err(HostedHouseRunnerErrorV1::Unavailable)
        }
    }
}

fn load_receipt_key(path: &Path) -> Result<Zeroizing<Vec<u8>>, RecordReadErrorV1> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(RecordReadErrorV1::NotFound);
        }
        Err(_) => return Err(RecordReadErrorV1::Unavailable),
        Ok(_) => {}
    }
    validate_owner_only_file(path).map_err(|_| RecordReadErrorV1::Unavailable)?;
    let bytes = Zeroizing::new(fs::read(path).map_err(|_| RecordReadErrorV1::Unavailable)?);
    if bytes.len() != 32 {
        return Err(RecordReadErrorV1::Unavailable);
    }
    Ok(bytes)
}

fn hex(bytes: &[u8]) -> String {
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        value.push(char::from(b"0123456789abcdef"[usize::from(byte >> 4)]));
        value.push(char::from(b"0123456789abcdef"[usize::from(byte & 0x0f)]));
    }
    value
}

fn decode_hex(value: &str) -> Result<Vec<u8>, HostedHouseRunnerErrorV1> {
    if !value.len().is_multiple_of(2) {
        return Err(HostedHouseRunnerErrorV1::Invalid);
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let text = std::str::from_utf8(pair).map_err(|_| HostedHouseRunnerErrorV1::Invalid)?;
            u8::from_str_radix(text, 16).map_err(|_| HostedHouseRunnerErrorV1::Invalid)
        })
        .collect()
}

enum RecordReadErrorV1 {
    NotFound,
    Unavailable,
}

fn read_record<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, RecordReadErrorV1> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(RecordReadErrorV1::NotFound);
        }
        Err(_) => return Err(RecordReadErrorV1::Unavailable),
        Ok(_) => {}
    }
    validate_owner_only_file(path).map_err(|_| RecordReadErrorV1::Unavailable)?;
    let bytes = fs::read(path).map_err(|_| RecordReadErrorV1::Unavailable)?;
    if bytes.is_empty() || bytes.len() > MAX_RECORD_BYTES {
        return Err(RecordReadErrorV1::Unavailable);
    }
    serde_json::from_slice(&bytes).map_err(|_| RecordReadErrorV1::Unavailable)
}

fn read_records<T: for<'de> Deserialize<'de>>(
    root: &Path,
    maximum: usize,
) -> Result<Vec<T>, HostedHouseRunnerErrorV1> {
    let mut records = Vec::new();
    for entry in fs::read_dir(root).map_err(|_| HostedHouseRunnerErrorV1::Unavailable)? {
        let path = entry
            .map_err(|_| HostedHouseRunnerErrorV1::Unavailable)?
            .path();
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or(HostedHouseRunnerErrorV1::Unavailable)?;
        if name.starts_with('.')
            && Path::new(name)
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("tmp"))
        {
            continue;
        }
        if !Path::new(name)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
            || records.len() >= maximum
        {
            return Err(HostedHouseRunnerErrorV1::Unavailable);
        }
        records.push(read_record(&path).map_err(|_| HostedHouseRunnerErrorV1::Unavailable)?);
    }
    Ok(records)
}

fn persist_new<T: Serialize>(path: &Path, value: &T) -> Result<(), HostedHouseRunnerErrorV1> {
    let bytes =
        serde_json::to_vec_pretty(value).map_err(|_| HostedHouseRunnerErrorV1::Unavailable)?;
    if bytes.is_empty() || bytes.len() > MAX_RECORD_BYTES {
        return Err(HostedHouseRunnerErrorV1::Unavailable);
    }
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or(HostedHouseRunnerErrorV1::Unavailable)?;
    let nonce = getrandom::u64().map_err(|_| HostedHouseRunnerErrorV1::Unavailable)?;
    let temporary = path.with_file_name(format!(".{file_name}.{nonce:016x}.tmp"));
    let mut file =
        create_owner_only_file(&temporary).map_err(|_| HostedHouseRunnerErrorV1::Unavailable)?;
    if file
        .write_all(&bytes)
        .and_then(|()| file.sync_all())
        .is_err()
    {
        let _ = fs::remove_file(&temporary);
        return Err(HostedHouseRunnerErrorV1::Unavailable);
    }
    drop(file);
    let result = fs::hard_link(&temporary, path)
        .and_then(|()| {
            fs::File::open(
                path.parent()
                    .ok_or_else(|| std::io::Error::other("missing parent"))?,
            )?
            .sync_all()
        })
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                HostedHouseRunnerErrorV1::Conflict
            } else {
                HostedHouseRunnerErrorV1::Unavailable
            }
        });
    let _ = fs::remove_file(temporary);
    result
}

fn safe_public_reference(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn uuid_reference(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    const LISTING: &[u8] = include_bytes!("../../../config/hosted/listings/agent-heist-0.3.0.json");
    const PLANNER: &[u8] =
        include_bytes!("../../../config/hosted/house-agents/cooperative-planner-1.json");
    const AUDITOR: &[u8] =
        include_bytes!("../../../config/hosted/house-agents/skeptical-auditor-1.json");

    #[derive(Clone)]
    struct FakeSource {
        dependency: Result<String, &'static str>,
        starts: Arc<Mutex<Vec<String>>>,
        retirements: Arc<Mutex<Vec<Option<String>>>>,
        retirement_failure: bool,
    }

    impl FakeSource {
        fn ready() -> Self {
            Self {
                dependency: Ok(format!("blake3:{}", "a".repeat(64))),
                starts: Arc::new(Mutex::new(Vec::new())),
                retirements: Arc::new(Mutex::new(Vec::new())),
                retirement_failure: false,
            }
        }
    }

    impl HouseRunnerDependencySourceV1 for FakeSource {
        fn binding_digest(
            &self,
            _request: &HostedHouseRunnerReservationRequestV1,
            _listing: &ListingRevision,
            _revision: &HouseAgentRevision,
        ) -> Result<String, DependencyCheckErrorV1> {
            self.dependency
                .clone()
                .map_err(DependencyCheckErrorV1::Terminal)
        }

        fn start(
            &self,
            _launch: &HostedLaunchRequestV1,
            assignment: &HostedHouseRunnerAssignmentV1,
            _room_id: &str,
            _listing: &ListingRevision,
            _revision: &HouseAgentRevision,
        ) -> Result<(), StartErrorV1> {
            self.starts
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(assignment.house_agent_assignment_id.clone());
            Ok(())
        }

        fn retire(
            &self,
            runtime_binding: Option<&RetainedRuntimeBindingV1>,
        ) -> Result<String, HostedHouseRunnerErrorV1> {
            if self.retirement_failure {
                return Err(HostedHouseRunnerErrorV1::Unavailable);
            }
            self.retirements
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(runtime_binding.map(|binding| binding.local_assignment_id.clone()));
            canonical_blake3(&serde_json::json!({
                "domain": "worldstream/house-runner-stop-witness/v1",
                "binding": runtime_binding.map(|binding| &binding.reservation_operation_id),
            }))
        }
    }

    fn artifacts() -> (ListingRevision, Vec<HouseAgentRevision>) {
        let canonical = |bytes: &[u8]| {
            CanonicalJsonV1::parse(bytes)
                .and_then(|value| value.to_bytes())
                .unwrap_or_else(|error| unreachable!("canonical fixture: {error}"))
        };
        (
            ListingRevision::from_canonical_bytes(&canonical(LISTING))
                .unwrap_or_else(|error| unreachable!("listing fixture: {error}")),
            vec![
                HouseAgentRevision::from_canonical_bytes(&canonical(PLANNER))
                    .unwrap_or_else(|error| unreachable!("planner fixture: {error}")),
                HouseAgentRevision::from_canonical_bytes(&canonical(AUDITOR))
                    .unwrap_or_else(|error| unreachable!("auditor fixture: {error}")),
            ],
        )
    }

    #[test]
    fn receipt_authentication_key_is_owner_only_and_stable() {
        let directory = tempdir().unwrap_or_else(|error| unreachable!("tempdir: {error}"));
        let root = prepare_data_directory(&directory.path().join("receipt-authentication"))
            .unwrap_or_else(|error| unreachable!("prepared key root: {error}"));
        let first = load_or_create_receipt_key(&root)
            .unwrap_or_else(|error| unreachable!("first receipt key: {error:?}"));
        let second = load_or_create_receipt_key(&root)
            .unwrap_or_else(|error| unreachable!("second receipt key: {error:?}"));
        assert_eq!(first.as_slice(), second.as_slice());
        assert_eq!(first.len(), 32);
    }

    #[test]
    fn full_reviewed_house_catalog_opens_within_the_shared_catalog_bound() {
        let directory = tempdir().unwrap_or_else(|error| unreachable!("tempdir: {error}"));
        let (listings, revisions) = crate::hosted_artifacts::reviewed_hosted_artifacts()
            .unwrap_or_else(|error| unreachable!("reviewed catalog: {error}"));
        assert_eq!(
            revisions.len(),
            33,
            "update this regression when the catalog grows"
        );
        assert!(listings.len() <= MAX_REVIEWED_HOSTED_CATALOG_REVISIONS);

        let operations = HostedHouseRunnerOperationsV1::open_with(
            directory.path(),
            "hosted-test",
            listings.clone(),
            revisions.clone(),
            FakeSource::ready(),
        )
        .unwrap_or_else(|error| unreachable!("full reviewed catalog: {error:?}"));
        assert_eq!(operations.revisions.len(), revisions.len());
        let (listing, seat, revision) = listings
            .iter()
            .find_map(|listing| {
                ["navigator", "insider", "broker"]
                    .into_iter()
                    .find_map(|seat| {
                        revisions
                            .iter()
                            .find(|revision| {
                                listing
                                    .verify_house_agent_for_seat(seat, revision.digest())
                                    .is_ok()
                            })
                            .map(|revision| (listing, seat, revision))
                    })
            })
            .unwrap_or_else(|| unreachable!("reviewed catalog contains a House-enabled seat"));
        let request = request(1, 1, seat, revision, listing);
        operations
            .reserve(&request)
            .unwrap_or_else(|error| unreachable!("full catalog reservation: {error:?}"));

        let mut over_limit = revisions;
        let padding = MAX_REVIEWED_HOSTED_CATALOG_REVISIONS - over_limit.len() + 1;
        let retained = over_limit.clone();
        over_limit.extend(retained.into_iter().cycle().take(padding));
        assert!(matches!(
            HostedHouseRunnerOperationsV1::open_with(
                directory.path(),
                "hosted-test-over-limit",
                listings,
                over_limit,
                FakeSource::ready(),
            ),
            Err(HostedHouseRunnerErrorV1::Invalid)
        ));
    }

    fn make_operations(
        root: &Path,
        source: FakeSource,
    ) -> (
        HostedHouseRunnerOperationsV1,
        ListingRevision,
        Vec<HouseAgentRevision>,
    ) {
        let (listing, revisions) = artifacts();
        let operations = HostedHouseRunnerOperationsV1::open_with(
            root,
            "hosted-test",
            vec![listing.clone()],
            revisions.clone(),
            source,
        )
        .unwrap_or_else(|error| unreachable!("operations: {error:?}"));
        (operations, listing, revisions)
    }

    fn request(
        operation_tail: u8,
        launch_tail: u8,
        seat: &str,
        revision: &HouseAgentRevision,
        listing: &ListingRevision,
    ) -> HostedHouseRunnerReservationRequestV1 {
        HostedHouseRunnerReservationRequestV1 {
            schema: "worldstream/house-runner-reservation-request/v1".to_owned(),
            host_installation_id: "hosted-test".to_owned(),
            reservation_operation_id: format!("00000000-0000-4000-8000-{operation_tail:012x}"),
            launch_request_id: format!("10000000-0000-4000-8000-{launch_tail:012x}"),
            listing_revision_digest: listing.digest().to_owned(),
            seat_id: seat.to_owned(),
            house_agent_revision_digest: revision.digest().to_owned(),
        }
    }

    fn launch(
        listing: &ListingRevision,
        assignments: Vec<HostedHouseRunnerAssignmentV1>,
    ) -> HostedLaunchRequestV1 {
        HostedLaunchRequestV1 {
            schema: "worldstream/hosted-launch-request/v1".to_owned(),
            listing_revision_digest: listing.digest().to_owned(),
            launch_request_digest: format!("blake3:{}", "b".repeat(64)),
            launch_input_digest: format!("sha256:{}", "c".repeat(64)),
            frozen_roster_digest: format!("sha256:{}", "d".repeat(64)),
            room_setup_specification_digest: format!("blake3:{}", "e".repeat(64)),
            room_setup_operation_id: "hosted-house-launch-01".to_owned(),
            capacity_authorization: worldstream_hosted_contract::HostedCapacityAuthorizationV1 {
                schema: "worldstream/platform-capacity-authorization/v1".to_owned(),
                host_installation_id: "hosted-test".to_owned(),
                reservation_reference: "10000000-0000-4000-8000-000000000001".to_owned(),
            },
            house_runner_assignments: assignments,
            frozen_launch_request: serde_json::json!({}),
            frozen_roster: serde_json::json!({}),
            frozen_room_setup_specification: serde_json::json!({}),
        }
    }

    fn retirement_request(
        receipt: &HostedHouseRunnerReservationReceiptV1,
        assignment_id: Option<&str>,
        disposition: HostedHouseRunnerRetirementDispositionV1,
    ) -> HostedHouseRunnerRetirementRequestV1 {
        HostedHouseRunnerRetirementRequestV1 {
            schema: "worldstream/house-runner-retirement-request/v1".to_owned(),
            host_installation_id: receipt.host_installation_id.clone(),
            reservation_operation_id: receipt.reservation_operation_id.clone(),
            launch_request_id: receipt.launch_request_id.clone(),
            house_agent_assignment_id: assignment_id.map(str::to_owned),
            disposition,
            platform_evidence_digest: format!("sha256:{}", "7".repeat(64)),
        }
    }

    fn runtime_binding(
        receipt: &HostedHouseRunnerReservationReceiptV1,
        assignment_id: &str,
    ) -> RetainedRuntimeBindingV1 {
        RetainedRuntimeBindingV1 {
            schema: RUNTIME_BINDING_SCHEMA_V1.to_owned(),
            reservation_operation_id: receipt.reservation_operation_id.clone(),
            room_setup_operation_id: "hosted-house-launch-01".to_owned(),
            house_agent_assignment_id: assignment_id.to_owned(),
            runner_unit_id: receipt.runner_unit_id.clone().unwrap(),
            room_id: "room-01".to_owned(),
            local_assignment_id: "local-assignment-01".to_owned(),
            member_id: "member-01".to_owned(),
            principal_id: "principal-01".to_owned(),
            role: "navigator".to_owned(),
            runner_id: "runner-01".to_owned(),
            instance_id: "instance-01".to_owned(),
            house_agent_revision_digest: receipt.house_agent_revision_digest.clone(),
            profile_id: "profile-01".to_owned(),
            profile_revision: "1".to_owned(),
            runner_template_id: "template-01".to_owned(),
            runner_template_revision: "1".to_owned(),
        }
    }

    #[test]
    fn exact_retry_and_restart_return_the_authenticated_receipt() {
        let directory = tempdir().unwrap_or_else(|error| unreachable!("tempdir: {error}"));
        let source = FakeSource::ready();
        let (operations, listing, revisions) = make_operations(directory.path(), source.clone());
        let request = request(1, 1, "navigator", &revisions[0], &listing);
        let first = operations
            .reserve(&request)
            .unwrap_or_else(|error| unreachable!("reserve: {error:?}"));
        assert_eq!(
            operations
                .reserve(&request)
                .unwrap_or_else(|error| unreachable!("retry: {error:?}")),
            first
        );
        drop(operations);
        let reopened = HostedHouseRunnerOperationsV1::open_with(
            directory.path(),
            "hosted-test",
            vec![listing],
            revisions,
            source,
        )
        .unwrap_or_else(|error| unreachable!("reopen: {error:?}"));
        assert_eq!(
            reopened
                .read(&request)
                .unwrap_or_else(|error| unreachable!("read: {error:?}")),
            first
        );
    }

    #[test]
    fn changed_operation_or_tampered_receipt_fails_closed() {
        let directory = tempdir().unwrap_or_else(|error| unreachable!("tempdir: {error}"));
        let (operations, listing, revisions) =
            make_operations(directory.path(), FakeSource::ready());
        let request = request(1, 1, "navigator", &revisions[0], &listing);
        let receipt = operations
            .reserve(&request)
            .unwrap_or_else(|error| unreachable!("reserve: {error:?}"));
        let mut changed = request.clone();
        changed.seat_id = "insider".to_owned();
        assert_eq!(
            operations.reserve(&changed),
            Err(HostedHouseRunnerErrorV1::Conflict)
        );

        let mut tampered = receipt;
        tampered.seat_id = "insider".to_owned();
        let launch = launch(
            &listing,
            vec![HostedHouseRunnerAssignmentV1 {
                house_agent_assignment_id: "20000000-0000-4000-8000-000000000001".to_owned(),
                reservation_receipt: tampered,
            }],
        );
        assert_eq!(
            operations.bind_launch(&launch),
            Err(HostedHouseRunnerErrorV1::Invalid)
        );
    }

    #[test]
    fn global_and_per_launch_capacity_fail_terminally_without_a_new_unit() {
        let directory = tempdir().unwrap_or_else(|error| unreachable!("tempdir: {error}"));
        let (operations, listing, revisions) =
            make_operations(directory.path(), FakeSource::ready());
        for index in 1..=4 {
            let receipt = operations
                .reserve(&request(index, index, "navigator", &revisions[0], &listing))
                .unwrap_or_else(|error| unreachable!("reserve {index}: {error:?}"));
            assert_eq!(
                receipt.outcome,
                HostedHouseRunnerReservationOutcomeV1::Succeeded
            );
        }
        let global = operations
            .reserve(&request(5, 5, "navigator", &revisions[0], &listing))
            .unwrap_or_else(|error| unreachable!("global cap: {error:?}"));
        assert_eq!(
            global.outcome,
            HostedHouseRunnerReservationOutcomeV1::TerminalFailed
        );
        assert_eq!(
            global.failure_code.as_deref(),
            Some("house_runner_capacity_exhausted")
        );

        let another = tempdir().unwrap_or_else(|error| unreachable!("tempdir: {error}"));
        let (operations, listing, revisions) = make_operations(another.path(), FakeSource::ready());
        assert!(
            operations
                .reserve(&request(11, 9, "navigator", &revisions[0], &listing))
                .is_ok()
        );
        assert!(
            operations
                .reserve(&request(12, 9, "insider", &revisions[1], &listing))
                .is_ok()
        );
        let per_launch = operations
            .reserve(&request(13, 9, "broker", &revisions[0], &listing))
            .unwrap_or_else(|error| unreachable!("per-launch cap: {error:?}"));
        assert_eq!(
            per_launch.failure_code.as_deref(),
            Some("house_runner_capacity_exhausted")
        );
    }

    #[test]
    fn retired_units_release_global_capacity_without_rewriting_reservations() {
        let directory = tempdir().unwrap();
        let (operations, listing, revisions) =
            make_operations(directory.path(), FakeSource::ready());
        let first_request = request(1, 1, "navigator", &revisions[0], &listing);
        let first = operations.reserve(&first_request).unwrap();
        for index in 2..=4 {
            assert_eq!(
                operations
                    .reserve(&request(index, index, "navigator", &revisions[0], &listing))
                    .unwrap()
                    .outcome,
                HostedHouseRunnerReservationOutcomeV1::Succeeded
            );
        }
        let unit = first.runner_unit_id.as_ref().unwrap();
        let root =
            worldstream_runtime::prepare_data_directory(&directory.path().join("units").join(unit))
                .unwrap();
        let evidence = serde_json::json!({
            "schema":"worldstream/operator-house-retirement/v1",
            "authority":"operator_observation_not_runtime_attestation",
            "installation_id": first.host_installation_id,
            "runner_unit_id":unit,
            "reservation_operation_id":first.reservation_operation_id,
            "launch_request_id":first.launch_request_id,
            "allowance_reset":false,
        });
        super::persist_new(&root.join("operator-retired.json"), &evidence).unwrap();
        let mut wrong = evidence.clone();
        wrong["runner_unit_id"] = serde_json::json!("another-unit");
        std::fs::write(
            root.join("operator-retired.json"),
            serde_json::to_vec(&wrong).unwrap(),
        )
        .unwrap();
        assert_eq!(
            operations.reserve(&request(5, 5, "navigator", &revisions[0], &listing)),
            Err(HostedHouseRunnerErrorV1::Unavailable)
        );
        std::fs::write(
            root.join("operator-retired.json"),
            serde_json::to_vec(&evidence).unwrap(),
        )
        .unwrap();
        assert_eq!(
            operations
                .reserve(&request(5, 5, "navigator", &revisions[0], &listing))
                .unwrap()
                .outcome,
            HostedHouseRunnerReservationOutcomeV1::Succeeded
        );
        assert_eq!(operations.reserve(&first_request).unwrap(), first);
        let (reopened, _, _) = make_operations(directory.path(), FakeSource::ready());
        assert_eq!(reopened.reserve(&first_request).unwrap(), first);
        assert_eq!(
            operations
                .reserve(&request(6, 6, "navigator", &revisions[0], &listing))
                .unwrap()
                .failure_code
                .as_deref(),
            Some("house_runner_capacity_exhausted")
        );
    }

    #[test]
    fn automatic_retirement_fences_exact_units_then_reuses_only_global_capacity() {
        let directory = tempdir().unwrap();
        let source = FakeSource::ready();
        let retirements = Arc::clone(&source.retirements);
        let (operations, listing, revisions) = make_operations(directory.path(), source);
        let first_request = request(1, 1, "navigator", &revisions[0], &listing);
        let first = operations.reserve(&first_request).unwrap();
        for index in 2..=4 {
            assert_eq!(
                operations
                    .reserve(&request(index, index, "navigator", &revisions[0], &listing))
                    .unwrap()
                    .outcome,
                HostedHouseRunnerReservationOutcomeV1::Succeeded
            );
        }
        assert_eq!(
            operations
                .reserve(&request(5, 5, "navigator", &revisions[0], &listing))
                .unwrap()
                .failure_code
                .as_deref(),
            Some("house_runner_capacity_exhausted")
        );

        let retired = operations
            .retire(&retirement_request(
                &first,
                None,
                HostedHouseRunnerRetirementDispositionV1::FailedPreGenesis,
            ))
            .unwrap();
        assert_eq!(
            retired.runner_unit_id,
            *first.runner_unit_id.as_ref().unwrap()
        );
        assert_eq!(
            retirements.lock().unwrap().as_slice(),
            &[None],
            "the no-Genesis branch proves no local child was started"
        );
        assert_eq!(
            operations
                .retire(&retirement_request(
                    &first,
                    None,
                    HostedHouseRunnerRetirementDispositionV1::FailedPreGenesis,
                ))
                .unwrap(),
            retired,
            "a duplicate reconciliation returns the original signed evidence"
        );
        assert_eq!(
            operations
                .reserve(&request(6, 6, "navigator", &revisions[0], &listing))
                .unwrap()
                .outcome,
            HostedHouseRunnerReservationOutcomeV1::Succeeded,
            "only the global concurrent slot is reusable"
        );
        assert_eq!(operations.read(&first_request).unwrap(), first);
        let marker = directory
            .path()
            .join("units")
            .join(first.runner_unit_id.as_ref().unwrap())
            .join(RETIREMENT_MARKER_FILE);
        assert!(marker.exists());
    }

    #[test]
    fn terminal_retirement_requires_the_exact_runtime_binding_and_stops_that_child() {
        let directory = tempdir().unwrap();
        let source = FakeSource::ready();
        let retirements = Arc::clone(&source.retirements);
        let (operations, listing, revisions) = make_operations(directory.path(), source);
        let request = request(1, 1, "navigator", &revisions[0], &listing);
        let reservation = operations.reserve(&request).unwrap();
        let assignment = "20000000-0000-4000-8000-000000000001";
        operations
            .bind_launch(&launch(
                &listing,
                vec![HostedHouseRunnerAssignmentV1 {
                    house_agent_assignment_id: assignment.to_owned(),
                    reservation_receipt: reservation.clone(),
                }],
            ))
            .unwrap();
        let bindings = prepare_data_directory(&directory.path().join("runtime-bindings")).unwrap();
        persist_new(
            &bindings.join(format!("{}.json", reservation.reservation_operation_id)),
            &runtime_binding(&reservation, assignment),
        )
        .unwrap();

        operations
            .retire(&retirement_request(
                &reservation,
                Some(assignment),
                HostedHouseRunnerRetirementDispositionV1::RunTerminal,
            ))
            .unwrap();
        assert_eq!(
            retirements.lock().unwrap().as_slice(),
            &[Some("local-assignment-01".to_owned())]
        );

        let wrong = operations.retire(&retirement_request(
            &reservation,
            Some("20000000-0000-4000-8000-000000000002"),
            HostedHouseRunnerRetirementDispositionV1::RunTerminal,
        ));
        assert_eq!(wrong, Err(HostedHouseRunnerErrorV1::Conflict));
    }

    #[test]
    fn terminal_retirement_race_and_restart_reuse_one_slot_without_resetting_identity_or_allowance()
    {
        use crate::house_model::{
            DeterministicHouseProviderFaultV1, DeterministicHouseProviderPortV1,
            FileHouseAllowanceLedgerV1, HouseAllowancePeriodV1, HouseInvocationIdentityV1,
            HouseModelExecutorV1, HouseProposedActionV1, HouseSpendLimitsV1,
        };

        let directory = tempdir().unwrap();
        let source = FakeSource::ready();
        let (operations, listing, revisions) = make_operations(directory.path(), source.clone());
        let first_request = request(1, 1, "navigator", &revisions[0], &listing);
        let reservation = operations.reserve(&first_request).unwrap();
        let assignment = "20000000-0000-4000-8000-000000000001";
        let launch = launch(
            &listing,
            vec![HostedHouseRunnerAssignmentV1 {
                house_agent_assignment_id: assignment.to_owned(),
                reservation_receipt: reservation.clone(),
            }],
        );
        operations.bind_launch(&launch).unwrap();
        let bindings = prepare_data_directory(&directory.path().join("runtime-bindings")).unwrap();
        let binding_path = bindings.join(format!("{}.json", reservation.reservation_operation_id));
        persist_new(&binding_path, &runtime_binding(&reservation, assignment)).unwrap();
        let binding_bytes = fs::read(&binding_path).unwrap();

        // Consume real durable allowance before terminal evidence arrives. No
        // network call or provider credential is needed by this deterministic port.
        let period = HouseAllowancePeriodV1::from_unix_seconds(1_783_036_800).unwrap();
        let ledger_root = directory.path().join("allowance-ledger");
        let ledger = FileHouseAllowanceLedgerV1::open_at(
            &ledger_root,
            HouseSpendLimitsV1::hobby_preview(),
            period,
        )
        .unwrap();
        let executor = HouseModelExecutorV1::new(
            DeterministicHouseProviderPortV1::new(
                HouseProposedActionV1 {
                    offer_id: "4:0:wait".to_owned(),
                    payload: serde_json::json!({}),
                },
                DeterministicHouseProviderFaultV1::Timeout,
            ),
            ledger.clone(),
        );
        let identity = HouseInvocationIdentityV1::new(
            "local-assignment-01",
            "activation-01",
            1,
            &format!("blake3:{}", "a".repeat(64)),
        )
        .unwrap();
        assert!(
            executor
                .execute(
                    &HouseProviderCredentialV1::new(Zeroizing::new(
                        b"synthetic-test-credential-not-a-secret".to_vec(),
                    ))
                    .unwrap(),
                    &revisions[0],
                    &identity,
                    &serde_json::json!({"phase": "planning"}),
                    &serde_json::json!({
                        "schema": "worldstream/assignment-action-offer-list/v1",
                        "precondition": {"room_seq": 4, "head_hash": format!("blake3:{}", "c".repeat(64))},
                        "offers": [{"action_type": "wait", "offer_id": "4:0:wait", "payload_schema": {
                            "schema_id": "agent-heist/wait/v1",
                            "schema_digest": format!("blake3:{}", "b".repeat(64)),
                            "schema": {"type": "object", "additionalProperties": false, "properties": {}}
                        }}]
                    }),
                    period,
                )
                .is_err()
        );
        let usage = ledger.usage("local-assignment-01").unwrap();
        assert_eq!(usage.attempts, 1);
        assert!(usage.consumed_input_units > 0);
        assert!(usage.consumed_output_units > 0);
        let allowance_bytes = fs::read(ledger_root.join("allowances.json")).unwrap();

        for index in 2..=4 {
            assert_eq!(
                operations
                    .reserve(&request(index, index, "navigator", &revisions[0], &listing))
                    .unwrap()
                    .outcome,
                HostedHouseRunnerReservationOutcomeV1::Succeeded
            );
        }
        let retirement = retirement_request(
            &reservation,
            Some(assignment),
            HostedHouseRunnerRetirementDispositionV1::RunTerminal,
        );
        let barrier = std::sync::Barrier::new(2);
        let receipts = std::thread::scope(|scope| {
            let reconcile = || {
                barrier.wait();
                operations.retire(&retirement).unwrap()
            };
            let first = scope.spawn(reconcile);
            let duplicate = scope.spawn(reconcile);
            (first.join().unwrap(), duplicate.join().unwrap())
        });
        assert_eq!(receipts.0, receipts.1);
        drop(operations);

        let (reopened, _, _) = make_operations(directory.path(), source.clone());
        assert_eq!(reopened.retire(&retirement).unwrap(), receipts.0);
        assert_eq!(
            source.retirements.lock().unwrap().as_slice(),
            &[Some("local-assignment-01".to_owned())]
        );
        assert_eq!(reopened.reserve(&first_request).unwrap(), reservation);
        assert_eq!(fs::read(&binding_path).unwrap(), binding_bytes);
        assert_eq!(
            fs::read(ledger_root.join("allowances.json")).unwrap(),
            allowance_bytes
        );
        assert_eq!(ledger.usage("local-assignment-01").unwrap(), usage);
        let next = reopened
            .reserve(&request(5, 5, "navigator", &revisions[0], &listing))
            .unwrap();
        assert_eq!(
            next.outcome,
            HostedHouseRunnerReservationOutcomeV1::Succeeded
        );
        assert_ne!(next.runner_unit_id, reservation.runner_unit_id);
        assert_eq!(
            reopened
                .reserve(&request(6, 6, "navigator", &revisions[0], &listing))
                .unwrap()
                .failure_code
                .as_deref(),
            Some("house_runner_capacity_exhausted"),
            "duplicate and restarted reconciliation release exactly one slot"
        );
        assert_eq!(reopened.bind_launch(&launch), Ok(()));
        assert!(reopened.operator_retired(&reservation).unwrap());
        assert!(source.starts.lock().unwrap().is_empty());
    }

    #[test]
    fn pre_start_abandonment_retires_the_frozen_assignment_without_inventing_an_outcome() {
        let directory = tempdir().unwrap();
        let source = FakeSource::ready();
        let retirements = Arc::clone(&source.retirements);
        let (operations, listing, revisions) = make_operations(directory.path(), source);
        let request = request(1, 1, "navigator", &revisions[0], &listing);
        let reservation = operations.reserve(&request).unwrap();
        let assignment = "20000000-0000-4000-8000-000000000001";
        operations
            .bind_launch(&launch(
                &listing,
                vec![HostedHouseRunnerAssignmentV1 {
                    house_agent_assignment_id: assignment.to_owned(),
                    reservation_receipt: reservation.clone(),
                }],
            ))
            .unwrap();

        let receipt = operations
            .retire(&retirement_request(
                &reservation,
                Some(assignment),
                HostedHouseRunnerRetirementDispositionV1::PreStartAbandoned,
            ))
            .unwrap();
        assert_eq!(
            receipt.disposition,
            HostedHouseRunnerRetirementDispositionV1::PreStartAbandoned
        );
        assert_eq!(retirements.lock().unwrap().as_slice(), &[None]);
    }

    #[test]
    fn unavailable_stop_never_installs_a_capacity_releasing_fence() {
        let directory = tempdir().unwrap();
        let mut source = FakeSource::ready();
        source.retirement_failure = true;
        let (operations, listing, revisions) = make_operations(directory.path(), source);
        let reservation = operations
            .reserve(&request(1, 1, "navigator", &revisions[0], &listing))
            .unwrap();
        assert_eq!(
            operations.retire(&retirement_request(
                &reservation,
                None,
                HostedHouseRunnerRetirementDispositionV1::FailedPreGenesis,
            )),
            Err(HostedHouseRunnerErrorV1::Unavailable)
        );
        assert!(
            !directory
                .path()
                .join("units")
                .join(reservation.runner_unit_id.as_ref().unwrap())
                .join(RETIREMENT_MARKER_FILE)
                .exists()
        );
        assert!(
            directory
                .path()
                .join("units")
                .join(reservation.runner_unit_id.as_ref().unwrap())
                .join(RETIREMENT_INTENT_MARKER_FILE)
                .exists(),
            "a failed stop remains durably fenced and keeps capacity occupied"
        );
    }

    #[test]
    fn creator_close_resumes_an_incomplete_retirement_intent_after_restart() {
        let directory = tempdir().unwrap_or_else(|error| unreachable!("tempdir: {error}"));
        let mut failing_source = FakeSource::ready();
        failing_source.retirement_failure = true;
        let (operations, listing, revisions) = make_operations(directory.path(), failing_source);
        let request = request(1, 1, "navigator", &revisions[0], &listing);
        let reservation = operations
            .reserve(&request)
            .unwrap_or_else(|error| unreachable!("reserve: {error:?}"));
        assert_eq!(
            operations.retire(&retirement_request(
                &reservation,
                None,
                HostedHouseRunnerRetirementDispositionV1::FailedPreGenesis,
            )),
            Err(HostedHouseRunnerErrorV1::Unavailable)
        );
        drop(operations);

        let recovered_source = FakeSource::ready();
        let retirements = recovered_source.retirements.clone();
        let reopened = HostedHouseRunnerOperationsV1::open_with(
            directory.path(),
            "hosted-test",
            vec![listing],
            revisions,
            recovered_source,
        )
        .unwrap_or_else(|error| unreachable!("reopen: {error:?}"));
        let closure_digest = format!("blake3:{}", "7".repeat(64));
        reopened
            .close_launch(&request.launch_request_id, &closure_digest)
            .unwrap_or_else(|error| unreachable!("close resumes stop: {error:?}"));
        reopened
            .close_launch(&request.launch_request_id, &closure_digest)
            .unwrap_or_else(|error| unreachable!("close retry: {error:?}"));
        assert_eq!(
            retirements
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .as_slice(),
            &[None]
        );
    }

    #[test]
    fn creator_close_fences_late_reservation_binding_and_start_across_restart() {
        let directory = tempdir().unwrap_or_else(|error| unreachable!("tempdir: {error}"));
        let source = FakeSource::ready();
        let retirements = source.retirements.clone();
        let (operations, listing, revisions) = make_operations(directory.path(), source.clone());
        let request = request(1, 1, "navigator", &revisions[0], &listing);
        let receipt = operations
            .reserve(&request)
            .unwrap_or_else(|error| unreachable!("reserve: {error:?}"));
        let launch = launch(
            &listing,
            vec![HostedHouseRunnerAssignmentV1 {
                house_agent_assignment_id: "20000000-0000-4000-8000-000000000001".to_owned(),
                reservation_receipt: receipt,
            }],
        );
        let closure_digest = format!("blake3:{}", "7".repeat(64));

        operations
            .close_launch(&request.launch_request_id, &closure_digest)
            .unwrap_or_else(|error| unreachable!("close: {error:?}"));
        assert_eq!(
            retirements
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .as_slice(),
            &[None]
        );
        assert_eq!(
            operations.reserve(&request),
            Err(HostedHouseRunnerErrorV1::Conflict)
        );
        assert_eq!(
            operations.bind_launch(&launch),
            Err(HostedHouseRunnerErrorV1::Conflict)
        );
        assert_eq!(
            operations.start_launch(&launch, "01JY0000000000000000000000"),
            HostedHouseRunnerGateV1::TerminalFailure
        );
        operations
            .close_launch(&request.launch_request_id, &closure_digest)
            .unwrap_or_else(|error| unreachable!("close retry: {error:?}"));
        assert_eq!(
            retirements
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .len(),
            1
        );

        drop(operations);
        let reopened = HostedHouseRunnerOperationsV1::open_with(
            directory.path(),
            "hosted-test",
            vec![listing],
            revisions,
            source,
        )
        .unwrap_or_else(|error| unreachable!("reopen: {error:?}"));
        assert_eq!(
            reopened.reserve(&request),
            Err(HostedHouseRunnerErrorV1::Conflict)
        );
        assert_eq!(
            reopened.close_launch(
                &request.launch_request_id,
                &format!("blake3:{}", "8".repeat(64)),
            ),
            Err(HostedHouseRunnerErrorV1::Conflict)
        );
    }

    #[test]
    fn exact_bound_assignment_is_started_once_per_gate_attempt() {
        let directory = tempdir().unwrap_or_else(|error| unreachable!("tempdir: {error}"));
        let source = FakeSource::ready();
        let starts = source.starts.clone();
        let (operations, listing, revisions) = make_operations(directory.path(), source);
        let request = request(1, 1, "navigator", &revisions[0], &listing);
        let receipt = operations
            .reserve(&request)
            .unwrap_or_else(|error| unreachable!("reserve: {error:?}"));
        let launch = launch(
            &listing,
            vec![HostedHouseRunnerAssignmentV1 {
                house_agent_assignment_id: "20000000-0000-4000-8000-000000000001".to_owned(),
                reservation_receipt: receipt,
            }],
        );
        operations
            .bind_launch(&launch)
            .unwrap_or_else(|error| unreachable!("bind: {error:?}"));
        assert_eq!(
            operations.start_launch(&launch, "01JY0000000000000000000000"),
            HostedHouseRunnerGateV1::Ready
        );
        assert_eq!(
            starts
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .as_slice(),
            ["20000000-0000-4000-8000-000000000001"]
        );
    }
}
