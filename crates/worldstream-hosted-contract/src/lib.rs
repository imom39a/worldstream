//! Bounded immutable contracts shared by the hosted control plane and Room Host.
//!
//! A browser selects one reviewed Listing Revision and supplies only its
//! declared launch inputs. The server resolves exact Pack, Activity Client,
//! roster, Room Setup, and Result Projector identities before any mutation.

mod runtime_v1;

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use thiserror::Error;
use worldstream_activity_client::ActivityClientReleaseV1;
use worldstream_core::CanonicalJsonV1;

const MAX_REVISION_BYTES: usize = 262_144;
const MAX_JSON_NODES: usize = 4_096;
const MAX_JSON_DEPTH: usize = 32;
const MAX_SEATS: usize = 32;
const MAX_RESULT_OUTPUT_BYTES: usize = 16_384;
const PROJECTION_SCHEMA: &str = "agent-heist/projection/v1";
const PROJECTION_SCHEMA_DIGEST: &str =
    "blake3:a617f398abd1469d237ce4d832704459deddf44193267fab8bb0e1311e5faa8f";
const RESULT_SCHEMA: &str = "worldstream/result-summary/v1";
const RESULT_SCHEMA_DIGEST: &str =
    "blake3:816295fc5ac531886f090e59de24901633a60ed68af1510e133cead3d7eb35c6";
const RUNTIME_ID: &str = "worldstream.result-projector.declarative";
const RUNTIME_VERSION: &str = "1.0.0";
const RUNTIME_DIGEST: &str =
    "blake3:7278757ac8f047330292e399ac3c8f4ed49225b05ecb6b997b75500784b0526e";
const RUST_RUNTIME_PATH: &str = "crates/worldstream-hosted-contract/src/runtime_v1.rs";
const TYPESCRIPT_RUNTIME_PATH: &str = "sdk/typescript-hosted-contract/src/runtimeV1.ts";
const RUST_RUNTIME_SOURCE_DIGEST: &str =
    "blake3:29abed9d256ae4978fb30090b4e8f1f1b61bd0e28653b54080c324c04cfb0f1f";
const TYPESCRIPT_RUNTIME_SOURCE_DIGEST: &str =
    "blake3:375f96e77cec51ed81ffb83f22bc9187579dfaf00223917815bc947d07e30701";
const RUST_RUNTIME_SOURCE: &[u8] = include_bytes!("runtime_v1.rs");
const TYPESCRIPT_RUNTIME_SOURCE: &[u8] =
    include_bytes!("../../../sdk/typescript-hosted-contract/src/runtimeV1.ts");

/// Closed failures safe to expose across the hosted control/Room Host boundary.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ContractError {
    #[error("contract input exceeds its byte limit")]
    TooLarge,
    #[error("contract input is not canonical JSON")]
    NonCanonical,
    #[error("contract input does not match its closed schema")]
    InvalidShape,
    #[error("contract schema or implementation is unsupported")]
    Unsupported,
    #[error("contract value exceeds a semantic bound")]
    Unbounded,
    #[error("immutable contract references do not match")]
    ReferenceMismatch,
    #[error("derived contract output exceeds its byte limit")]
    OutputTooLarge,
}

/// Exact Pack identity selected by a reviewed listing.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PackReference {
    pub id: String,
    pub version: String,
    pub digest: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClientSurfaceReference {
    client_id: String,
    release_digest: String,
    client_contract: String,
    surface_id: String,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CatalogVisibility {
    Public,
    Unlisted,
    Private,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CatalogReviewStatus {
    Reviewed,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum LaunchInputKind {
    None,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LaunchInputSchema {
    schema: String,
    accepts: LaunchInputKind,
    defaults: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RoomSetupTemplate {
    configuration: Value,
}

/// Participation mechanisms a reviewed listing permits for one seat.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ParticipationKind {
    AccountHuman,
    AccountExternalAgent,
    HouseAgentFill,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ListingSeat {
    seat_id: String,
    role: String,
    display_name: String,
    required: bool,
    allowed_participation: Vec<ParticipationKind>,
    allowed_house_agent_revisions: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CreatorAccess {
    MustClaimSeat,
    MaySpectate,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PublicViewingPolicy {
    Disabled,
    AnonymousByLink,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
struct SchemaReference {
    schema: String,
    digest: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
struct ProjectorReference {
    id: String,
    version: String,
    digest: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum ResultPublicationPolicy {
    Disabled,
    PublicRecentResults,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum ResultAttribution {
    None,
    ReviewedPseudonymousSeats,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum PublicOutputPolicy {
    None,
    ProjectorSummaryOnly,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SuppressionPolicy {
    UnhealthyInconclusiveOrConflict,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResultPublication {
    policy: ResultPublicationPolicy,
    attribution: ResultAttribution,
    public_output: PublicOutputPolicy,
    suppression: SuppressionPolicy,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ListingResultContract {
    projection: SchemaReference,
    projector: ProjectorReference,
    publication: ResultPublication,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ListingDocument {
    schema: String,
    listing_id: String,
    version: String,
    title: String,
    description: String,
    catalog: CatalogMetadata,
    pack: PackReference,
    client: ClientSurfaceReference,
    launch_input_schema: LaunchInputSchema,
    room_setup: RoomSetupTemplate,
    seats: Vec<ListingSeat>,
    creator_access: CreatorAccess,
    public_viewing_policy: PublicViewingPolicy,
    pre_start_deadline_seconds: u64,
    result: ListingResultContract,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogMetadata {
    visibility: CatalogVisibility,
    review_status: CatalogReviewStatus,
}

/// Validated, byte-identified Activity Listing Revision.
#[derive(Clone, Debug)]
pub struct ListingRevision {
    document: ListingDocument,
    canonical_bytes: Vec<u8>,
    digest: String,
}

impl ListingRevision {
    /// Reads canonical Listing Revision bytes and enforces every v1 bound.
    ///
    /// # Errors
    /// Returns a closed contract error for noncanonical, unsupported, malformed,
    /// or unbounded bytes.
    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, ContractError> {
        let (document, canonical_bytes) = decode_canonical(bytes, MAX_REVISION_BYTES)?;
        validate_listing(&document)?;
        let digest = blake3_digest(&canonical_bytes);
        Ok(Self {
            document,
            canonical_bytes,
            digest,
        })
    }

    #[must_use]
    pub fn digest(&self) -> &str {
        &self.digest
    }

    #[must_use]
    pub fn canonical_bytes(&self) -> &[u8] {
        &self.canonical_bytes
    }

    #[must_use]
    pub fn pack(&self) -> &PackReference {
        &self.document.pack
    }

    /// Verifies the exact catalog-resolved Pack revision.
    ///
    /// # Errors
    /// Returns [`ContractError::ReferenceMismatch`] unless all identity fields match.
    pub fn verify_pack(&self, pack: &PackReference) -> Result<(), ContractError> {
        if &self.document.pack == pack {
            Ok(())
        } else {
            Err(ContractError::ReferenceMismatch)
        }
    }

    /// Verifies the exact Activity Client release and required surface.
    ///
    /// # Errors
    /// Returns [`ContractError::ReferenceMismatch`] unless the release and surface match.
    pub fn verify_client_release(
        &self,
        release: &ActivityClientReleaseV1,
    ) -> Result<(), ContractError> {
        let expected = &self.document.client;
        if expected.client_id == release.client_id
            && expected.release_digest == release.release_digest
            && expected.client_contract == release.client_contract
            && release
                .surfaces
                .iter()
                .any(|surface| surface.surface_id == expected.surface_id)
        {
            Ok(())
        } else {
            Err(ContractError::ReferenceMismatch)
        }
    }

    /// Verifies the exact privacy-reviewed projector and schema identities.
    ///
    /// # Errors
    /// Returns [`ContractError::ReferenceMismatch`] unless all pinned identities match.
    pub fn verify_projector(
        &self,
        projector: &ResultProjectorRevision,
    ) -> Result<(), ContractError> {
        let expected = &self.document.result.projector;
        if expected.id == projector.document.projector_id
            && expected.version == projector.document.version
            && expected.digest == projector.digest
            && self.document.result.projection == projector.document.input.projection
            && self.document.pack == projector.document.input.pack
            && projector.document.input.listing_schema == self.document.schema
        {
            Ok(())
        } else {
            Err(ContractError::ReferenceMismatch)
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HouseAgentBehaviorPolicyV1 {
    policy_id: String,
    revision: String,
    instructions: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum HouseAgentGatewayV1 {
    Openrouter,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum CompletionTokenParameterV1 {
    MaxTokens,
    MaxCompletionTokens,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum ProviderDataCollectionV1 {
    Deny,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HouseAgentRouteV1 {
    gateway: HouseAgentGatewayV1,
    model_slug: String,
    provider_slug: String,
    completion_token_parameter: CompletionTokenParameterV1,
    maximum_prompt_price: String,
    maximum_completion_price: String,
    zero_data_retention: bool,
    data_collection: ProviderDataCollectionV1,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HouseAgentProfileReferenceV1 {
    profile_id: String,
    revision: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HouseRunnerTemplateReferenceV1 {
    template_id: String,
    revision: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AccountingTokenizerReferenceV1 {
    tokenizer_id: String,
    revision: String,
}

/// Fixed operator-funded limits attached to every House Agent Assignment.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HouseAgentExecutionAllowanceV1 {
    pub model_call_attempts: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub input_tokens_per_call: u64,
    pub output_tokens_per_call: u64,
    pub concurrent_calls: u64,
    pub call_timeout_seconds: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HouseAgentRevisionDocumentV1 {
    schema: String,
    house_agent_id: String,
    version: String,
    display_name: String,
    behavior_policy: HouseAgentBehaviorPolicyV1,
    route: HouseAgentRouteV1,
    agent_profile: HouseAgentProfileReferenceV1,
    runner_template: HouseRunnerTemplateReferenceV1,
    tools: Vec<Value>,
    accounting_tokenizer: AccountingTokenizerReferenceV1,
    allowance: HouseAgentExecutionAllowanceV1,
}

/// Validated, immutable House Agent Revision identified by canonical bytes.
#[derive(Clone, Debug)]
pub struct HouseAgentRevision {
    document: HouseAgentRevisionDocumentV1,
    canonical_bytes: Vec<u8>,
    digest: String,
}

impl HouseAgentRevision {
    /// Reads canonical revision bytes and enforces the complete exhibition-only contract.
    ///
    /// # Errors
    /// Returns a closed contract error for noncanonical, malformed, or unbounded bytes.
    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, ContractError> {
        let (document, canonical_bytes) = decode_canonical(bytes, MAX_REVISION_BYTES)?;
        validate_house_agent_revision(&document)?;
        let digest = blake3_digest(&canonical_bytes);
        Ok(Self {
            document,
            canonical_bytes,
            digest,
        })
    }

    #[must_use]
    pub fn digest(&self) -> &str {
        &self.digest
    }

    #[must_use]
    pub fn canonical_bytes(&self) -> &[u8] {
        &self.canonical_bytes
    }

    #[must_use]
    pub fn house_agent_id(&self) -> &str {
        &self.document.house_agent_id
    }

    #[must_use]
    pub fn display_name(&self) -> &str {
        &self.document.display_name
    }

    #[must_use]
    pub fn model_slug(&self) -> &str {
        &self.document.route.model_slug
    }

    #[must_use]
    pub fn provider_slug(&self) -> &str {
        &self.document.route.provider_slug
    }

    #[must_use]
    pub fn agent_profile(&self) -> (&str, &str) {
        (
            &self.document.agent_profile.profile_id,
            &self.document.agent_profile.revision,
        )
    }

    #[must_use]
    pub fn runner_template(&self) -> (&str, &str) {
        (
            &self.document.runner_template.template_id,
            &self.document.runner_template.revision,
        )
    }

    #[must_use]
    pub const fn allowance(&self) -> HouseAgentExecutionAllowanceV1 {
        self.document.allowance
    }
}

fn validate_house_agent_revision(
    document: &HouseAgentRevisionDocumentV1,
) -> Result<(), ContractError> {
    const ALLOWANCE: HouseAgentExecutionAllowanceV1 = HouseAgentExecutionAllowanceV1 {
        model_call_attempts: 10,
        total_input_tokens: 120_000,
        total_output_tokens: 10_000,
        input_tokens_per_call: 12_000,
        output_tokens_per_call: 1_000,
        concurrent_calls: 1,
        call_timeout_seconds: 60,
    };
    if document.schema != "worldstream/house-agent-revision/v1" {
        return Err(ContractError::Unsupported);
    }
    validate_identifier(&document.house_agent_id, 128)?;
    validate_version(&document.version)?;
    validate_text(&document.display_name, 128)?;
    validate_identifier(&document.behavior_policy.policy_id, 128)?;
    validate_version(&document.behavior_policy.revision)?;
    validate_text(&document.behavior_policy.instructions, 4_096)?;
    validate_identifier(&document.route.model_slug, 192)?;
    validate_identifier(&document.route.provider_slug, 192)?;
    validate_decimal_price(&document.route.maximum_prompt_price)?;
    validate_decimal_price(&document.route.maximum_completion_price)?;
    if !document.route.zero_data_retention
        || !matches!(document.route.gateway, HouseAgentGatewayV1::Openrouter)
        || !matches!(
            document.route.data_collection,
            ProviderDataCollectionV1::Deny
        )
        || !matches!(
            document.route.completion_token_parameter,
            CompletionTokenParameterV1::MaxTokens | CompletionTokenParameterV1::MaxCompletionTokens
        )
        || !document.tools.is_empty()
        || document.allowance != ALLOWANCE
    {
        return Err(ContractError::InvalidShape);
    }
    validate_identifier(&document.agent_profile.profile_id, 128)?;
    validate_version(&document.agent_profile.revision)?;
    validate_identifier(&document.runner_template.template_id, 128)?;
    validate_version(&document.runner_template.revision)?;
    validate_identifier(&document.accounting_tokenizer.tokenizer_id, 128)?;
    validate_version(&document.accounting_tokenizer.revision)?;
    Ok(())
}

fn validate_decimal_price(value: &str) -> Result<(), ContractError> {
    let Some(fraction) = value.strip_prefix("0.") else {
        return Err(ContractError::InvalidShape);
    };
    if fraction.is_empty()
        || fraction.len() > 18
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        Err(ContractError::InvalidShape)
    } else {
        Ok(())
    }
}

fn validate_listing(document: &ListingDocument) -> Result<(), ContractError> {
    if document.schema != "worldstream/activity-listing-revision/v1"
        || document.launch_input_schema.schema != "worldstream/launch-input-schema/v1"
    {
        return Err(ContractError::Unsupported);
    }
    let _ = (
        document.catalog.visibility,
        document.catalog.review_status,
        document.launch_input_schema.accepts,
        document.creator_access,
        document.public_viewing_policy,
        document.result.publication.suppression,
    );
    if !document.launch_input_schema.defaults.is_empty() {
        return Err(ContractError::InvalidShape);
    }
    validate_identifier(&document.listing_id, 128)?;
    validate_version(&document.version)?;
    validate_text(&document.title, 128)?;
    validate_text(&document.description, 1_024)?;
    validate_pack(&document.pack)?;
    validate_identifier(&document.client.client_id, 128)?;
    validate_digest(&document.client.release_digest, "sha256")?;
    validate_identifier(&document.client.client_contract, 128)?;
    validate_identifier(&document.client.surface_id, 128)?;
    validate_json(&document.room_setup.configuration)?;
    if !(60..=86_400).contains(&document.pre_start_deadline_seconds) {
        return Err(ContractError::Unbounded);
    }
    validate_schema_reference(&document.result.projection)?;
    validate_projector_reference(&document.result.projector)?;
    let publication = &document.result.publication;
    if !matches!(
        (
            publication.policy,
            publication.attribution,
            publication.public_output
        ),
        (
            ResultPublicationPolicy::Disabled,
            ResultAttribution::None,
            PublicOutputPolicy::None
        ) | (
            ResultPublicationPolicy::PublicRecentResults,
            ResultAttribution::ReviewedPseudonymousSeats,
            PublicOutputPolicy::ProjectorSummaryOnly
        )
    ) {
        return Err(ContractError::InvalidShape);
    }
    if document.seats.is_empty() || document.seats.len() > MAX_SEATS {
        return Err(ContractError::Unbounded);
    }
    let mut seat_ids = BTreeSet::new();
    for seat in &document.seats {
        validate_seat_label(&seat.seat_id)?;
        validate_public_reference(&seat.role, 128)?;
        validate_text(&seat.display_name, 128)?;
        if !seat_ids.insert(seat.seat_id.as_str())
            || seat.allowed_participation.is_empty()
            || seat.allowed_participation.len() > 3
        {
            return Err(ContractError::InvalidShape);
        }
        let kinds = seat
            .allowed_participation
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        if kinds.len() != seat.allowed_participation.len()
            || (kinds.contains(&ParticipationKind::HouseAgentFill)
                == seat.allowed_house_agent_revisions.is_empty())
            || seat.allowed_house_agent_revisions.len() > 32
        {
            return Err(ContractError::InvalidShape);
        }
        let revisions = seat
            .allowed_house_agent_revisions
            .iter()
            .collect::<BTreeSet<_>>();
        if revisions.len() != seat.allowed_house_agent_revisions.len() {
            return Err(ContractError::InvalidShape);
        }
        for digest in &seat.allowed_house_agent_revisions {
            validate_digest(digest, "blake3")?;
        }
        let _ = seat.required;
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LaunchRequest {
    schema: String,
    listing_revision_digest: String,
    inputs: BTreeMap<String, Value>,
    creator: FrozenCreatorElection,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CreatorParticipation {
    Seat,
    Spectator,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FrozenCreatorElection {
    participation: CreatorParticipation,
    principal_reference: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AgentProfileReference {
    profile_id: String,
    revision: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RunnerTemplateReference {
    template_id: String,
    revision: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FrozenRoster {
    schema: String,
    listing_revision_digest: String,
    members: Vec<FrozenMember>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FrozenMember {
    seat_id: String,
    participation: ParticipationKind,
    principal_reference: String,
    display_name: String,
    #[serde(default)]
    house_agent_revision_digest: Option<String>,
    #[serde(default)]
    agent_profile: Option<AgentProfileReference>,
    #[serde(default)]
    runner_template: Option<RunnerTemplateReference>,
}

#[derive(Debug, Serialize)]
struct RoomSetupSpecification {
    schema: &'static str,
    pack: PackReference,
    configuration: Value,
    seats: Vec<SetupSeat>,
    spectators: Vec<SetupSpectator>,
    operator_view: bool,
}

#[derive(Debug, Serialize)]
struct SetupSpectator {
    purpose: &'static str,
    principal: SetupPrincipal,
}

#[derive(Debug, Serialize)]
struct SetupSeat {
    label: String,
    role: String,
    required: bool,
    display_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    principal: Option<SetupPrincipal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    assignment: Option<SetupAssignment>,
}

#[derive(Debug, Serialize)]
struct SetupPrincipal {
    reference: String,
    kind: &'static str,
}

#[derive(Debug, Serialize)]
struct SetupAssignment {
    mode: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent_profile: Option<AgentProfileReference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    runner_template: Option<RunnerTemplateReference>,
}

/// Canonical complete Room Setup Specification derived without I/O.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DerivedRoomSetup(Vec<u8>);

impl DerivedRoomSetup {
    /// Returns the retained canonical `worldstream/room-setup/v2` bytes.
    ///
    /// # Errors
    /// Returns [`ContractError::InvalidShape`] if retained bytes cannot be decoded.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ContractError> {
        canonicalize(
            &serde_json::from_slice::<Value>(&self.0).map_err(|_| ContractError::InvalidShape)?,
        )
    }
}

/// Derives a complete Room Setup from a reviewed listing and frozen roster.
///
/// # Errors
/// Returns a closed contract error for malformed inputs, identity mismatch,
/// invalid occupancy, or an unsupported participation kind.
pub fn derive_room_setup(
    listing: &ListingRevision,
    launch_bytes: &[u8],
    roster_bytes: &[u8],
) -> Result<DerivedRoomSetup, ContractError> {
    derive_room_setup_with_house_agents(listing, launch_bytes, roster_bytes, &[])
}

/// Derives a complete Room Setup while resolving every House assignment to an
/// exact reviewed immutable House Agent Revision.
///
/// # Errors
/// Returns a closed contract error for malformed inputs, identity mismatch,
/// an unreviewed House revision, or a mismatched Profile/Runner reference.
pub fn derive_room_setup_with_house_agents(
    listing: &ListingRevision,
    launch_bytes: &[u8],
    roster_bytes: &[u8],
    house_agents: &[HouseAgentRevision],
) -> Result<DerivedRoomSetup, ContractError> {
    let (launch, _) = decode_canonical::<LaunchRequest>(launch_bytes, 16_384)?;
    let (roster, _) = decode_canonical::<FrozenRoster>(roster_bytes, 65_536)?;
    if launch.schema != "worldstream/launch-request/v2"
        || roster.schema != "worldstream/frozen-roster/v1"
    {
        return Err(ContractError::Unsupported);
    }
    if launch.listing_revision_digest != listing.digest
        || roster.listing_revision_digest != listing.digest
    {
        return Err(ContractError::ReferenceMismatch);
    }
    if !launch.inputs.is_empty() || roster.members.len() > listing.document.seats.len() {
        return Err(ContractError::InvalidShape);
    }
    validate_public_reference(&launch.creator.principal_reference, 128)?;
    let (mut members, mut principal_references) = index_frozen_members(roster.members)?;
    validate_house_assignments(&members, house_agents)?;
    let creator_spectator_reference = resolve_creator_spectator(
        listing.document.creator_access,
        launch.creator,
        &members,
        &principal_references,
    )?;

    let mut seats = Vec::with_capacity(listing.document.seats.len());
    for listed in &listing.document.seats {
        let member = members.remove(&listed.seat_id);
        if listed.required && member.is_none() {
            return Err(ContractError::InvalidShape);
        }
        let (display_name, principal, assignment) = match member {
            None => (listed.display_name.clone(), None, None),
            Some(member) => {
                if !listed.allowed_participation.contains(&member.participation) {
                    return Err(ContractError::Unsupported);
                }
                setup_participation(listed, member)?
            }
        };
        seats.push(SetupSeat {
            label: listed.seat_id.clone(),
            role: listed.role.clone(),
            required: listed.required,
            display_name,
            principal,
            assignment,
        });
    }
    if !members.is_empty() {
        return Err(ContractError::InvalidShape);
    }
    let mut spectators = Vec::with_capacity(3);
    append_setup_spectator(
        &mut spectators,
        &mut principal_references,
        "result_indexer",
        "worldstream:result-indexer",
        "agent",
    )?;
    if let Some(reference) = creator_spectator_reference {
        append_setup_spectator(
            &mut spectators,
            &mut principal_references,
            "creator",
            &reference,
            "human",
        )?;
    }
    if matches!(
        listing.document.public_viewing_policy,
        PublicViewingPolicy::AnonymousByLink
    ) {
        append_setup_spectator(
            &mut spectators,
            &mut principal_references,
            "public_relay",
            "worldstream:public-relay",
            "agent",
        )?;
    }
    let setup = RoomSetupSpecification {
        schema: "worldstream/room-setup/v2",
        pack: listing.document.pack.clone(),
        configuration: listing.document.room_setup.configuration.clone(),
        seats,
        spectators,
        operator_view: false,
    };
    let bytes = canonicalize(&setup)?;
    if bytes.len() > MAX_REVISION_BYTES {
        return Err(ContractError::OutputTooLarge);
    }
    Ok(DerivedRoomSetup(bytes))
}

fn validate_house_assignments(
    members: &BTreeMap<String, FrozenMember>,
    house_agents: &[HouseAgentRevision],
) -> Result<(), ContractError> {
    let mut revisions = BTreeMap::new();
    let mut assigned_revisions = BTreeSet::new();
    for revision in house_agents {
        if revisions.insert(revision.digest(), revision).is_some() {
            return Err(ContractError::InvalidShape);
        }
    }
    for member in members.values() {
        if !matches!(member.participation, ParticipationKind::HouseAgentFill) {
            continue;
        }
        let revision = revisions
            .get(
                member
                    .house_agent_revision_digest
                    .as_deref()
                    .ok_or(ContractError::InvalidShape)?,
            )
            .ok_or(ContractError::ReferenceMismatch)?;
        if !assigned_revisions.insert(revision.digest())
            || member.display_name != revision.display_name()
        {
            return Err(ContractError::ReferenceMismatch);
        }
        let profile = member
            .agent_profile
            .as_ref()
            .ok_or(ContractError::InvalidShape)?;
        let runner = member
            .runner_template
            .as_ref()
            .ok_or(ContractError::InvalidShape)?;
        let expected_profile = revision.agent_profile();
        let expected_runner = revision.runner_template();
        if (profile.profile_id.as_str(), profile.revision.as_str()) != expected_profile
            || (runner.template_id.as_str(), runner.revision.as_str()) != expected_runner
        {
            return Err(ContractError::ReferenceMismatch);
        }
    }
    Ok(())
}

/// Service-authenticated proof that the platform reserved one exact active-Run slot.
/// The reference is the frozen Launch UUID. Fly treats it as an immutable
/// attestation and receives no Supabase key.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HostedCapacityAuthorizationV1 {
    pub schema: String,
    pub host_installation_id: String,
    pub reservation_reference: String,
}

/// Complete frozen input accepted by the narrow Host launch adapter.
/// It contains no credential, provider choice, arbitrary Host path, or raw model content.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HostedLaunchRequestV1 {
    pub schema: String,
    pub listing_revision_digest: String,
    pub launch_request_digest: String,
    pub launch_input_digest: String,
    pub frozen_roster_digest: String,
    pub room_setup_specification_digest: String,
    pub room_setup_operation_id: String,
    pub capacity_authorization: HostedCapacityAuthorizationV1,
    pub frozen_launch_request: Value,
    pub frozen_roster: Value,
    pub frozen_room_setup_specification: Value,
}

/// Identity-only read request for one previously retained hosted launch.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HostedLaunchEvidenceRequestV1 {
    pub schema: String,
    pub listing_revision_digest: String,
    pub launch_request_digest: String,
    pub room_setup_operation_id: String,
}

/// Bounded hosted launch progress. Activity phase and outcome remain `WorldStream` facts.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HostedLaunchStageV1 {
    Bound,
    CreatingRoom,
    Provisioning,
    WaitingForReadiness,
    Launching,
    Launched,
    NeedsAttention,
}

/// Secret-free evidence returned from Fly to the platform BFF.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct HostedLaunchStatusV1 {
    pub schema: String,
    pub listing_revision_digest: String,
    pub launch_request_digest: String,
    pub room_setup_operation_id: String,
    pub room_id: Option<String>,
    pub stage: HostedLaunchStageV1,
    pub room_setup_complete: bool,
    pub lobby_launch_committed: bool,
    pub retryable: bool,
    pub terminal_before_genesis: bool,
}

/// Validates every digest and independently re-derives the supplied Room Setup.
///
/// # Errors
/// Returns a closed contract error when any frozen value, reviewed identity,
/// House assignment, capacity attestation, or derived setup differs.
pub fn validate_hosted_launch_request(
    request: &HostedLaunchRequestV1,
    expected_host_installation_id: &str,
    listing: &ListingRevision,
    house_agents: &[HouseAgentRevision],
) -> Result<DerivedRoomSetup, ContractError> {
    if request.schema != "worldstream/hosted-launch-request/v1"
        || request.capacity_authorization.schema != "worldstream/platform-capacity-authorization/v1"
        || request.listing_revision_digest != listing.digest()
        || request.capacity_authorization.host_installation_id != expected_host_installation_id
        || house_agents.len() > 32
    {
        return Err(ContractError::ReferenceMismatch);
    }
    validate_host_installation_reference(expected_host_installation_id)?;
    validate_hosted_operation_reference(&request.room_setup_operation_id)?;
    validate_uuid_reference(&request.capacity_authorization.reservation_reference)?;
    validate_digest(&request.launch_request_digest, "blake3")?;
    validate_digest(&request.launch_input_digest, "sha256")?;
    validate_digest(&request.frozen_roster_digest, "sha256")?;
    validate_digest(&request.room_setup_specification_digest, "blake3")?;

    let launch = canonicalize(&request.frozen_launch_request)?;
    let launch_document = serde_json::from_slice::<LaunchRequest>(&launch)
        .map_err(|_| ContractError::InvalidShape)?;
    let launch_inputs = canonicalize(&launch_document.inputs)?;
    let roster = canonicalize(&request.frozen_roster)?;
    let roster_document =
        serde_json::from_slice::<FrozenRoster>(&roster).map_err(|_| ContractError::InvalidShape)?;
    validate_hosted_house_principals(
        &roster_document,
        &request.capacity_authorization.reservation_reference,
    )?;
    let supplied_setup = canonicalize(&request.frozen_room_setup_specification)?;
    if launch.len() > 16_384 || roster.len() > 65_536 || supplied_setup.len() > 65_536 {
        return Err(ContractError::TooLarge);
    }
    if blake3_digest(&launch) != request.launch_request_digest
        || sha256_digest(&launch_inputs) != request.launch_input_digest
        || sha256_digest(&roster) != request.frozen_roster_digest
        || blake3_digest(&supplied_setup) != request.room_setup_specification_digest
    {
        return Err(ContractError::ReferenceMismatch);
    }

    let derived = derive_room_setup_with_house_agents(listing, &launch, &roster, house_agents)?;
    if derived.canonical_bytes()? != supplied_setup {
        return Err(ContractError::ReferenceMismatch);
    }
    Ok(derived)
}

fn validate_hosted_house_principals(
    roster: &FrozenRoster,
    launch_reference: &str,
) -> Result<(), ContractError> {
    for member in &roster.members {
        if matches!(member.participation, ParticipationKind::HouseAgentFill)
            && member.principal_reference != format!("house:{launch_reference}:{}", member.seat_id)
        {
            return Err(ContractError::ReferenceMismatch);
        }
    }
    Ok(())
}

/// Validates the immutable identity tuple used for status and reconciliation reads.
///
/// # Errors
/// Returns a closed contract error for malformed or mismatched identities.
pub fn validate_hosted_launch_evidence_request(
    request: &HostedLaunchEvidenceRequestV1,
) -> Result<(), ContractError> {
    if request.schema != "worldstream/hosted-launch-evidence-request/v1" {
        return Err(ContractError::Unsupported);
    }
    validate_digest(&request.listing_revision_digest, "blake3")?;
    validate_digest(&request.launch_request_digest, "blake3")?;
    validate_hosted_operation_reference(&request.room_setup_operation_id)
}

fn validate_hosted_operation_reference(value: &str) -> Result<(), ContractError> {
    if value.is_empty()
        || value.len() > 64
        || !value.as_bytes()[0].is_ascii_lowercase()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(ContractError::InvalidShape);
    }
    Ok(())
}

fn validate_host_installation_reference(value: &str) -> Result<(), ContractError> {
    if value.is_empty()
        || value.len() > 128
        || !value.as_bytes()[0].is_ascii_alphanumeric()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(ContractError::InvalidShape);
    }
    Ok(())
}

fn validate_uuid_reference(value: &str) -> Result<(), ContractError> {
    if value.len() != 36
        || value.as_bytes().get(8) != Some(&b'-')
        || value.as_bytes().get(13) != Some(&b'-')
        || value.as_bytes().get(18) != Some(&b'-')
        || value.as_bytes().get(23) != Some(&b'-')
        || !value.bytes().enumerate().all(|(index, byte)| {
            matches!(index, 8 | 13 | 18 | 23)
                || byte.is_ascii_digit()
                || (b'a'..=b'f').contains(&byte)
        })
    {
        return Err(ContractError::InvalidShape);
    }
    Ok(())
}

fn index_frozen_members(
    frozen: Vec<FrozenMember>,
) -> Result<(BTreeMap<String, FrozenMember>, BTreeSet<String>), ContractError> {
    let mut members = BTreeMap::new();
    let mut principal_references = BTreeSet::new();
    for member in frozen {
        validate_seat_label(&member.seat_id)?;
        validate_public_reference(&member.principal_reference, 128)?;
        validate_text(&member.display_name, 128)?;
        validate_member_references(&member)?;
        if !principal_references.insert(member.principal_reference.clone())
            || members.insert(member.seat_id.clone(), member).is_some()
        {
            return Err(ContractError::InvalidShape);
        }
    }
    Ok((members, principal_references))
}

fn resolve_creator_spectator(
    access: CreatorAccess,
    creator: FrozenCreatorElection,
    members: &BTreeMap<String, FrozenMember>,
    principal_references: &BTreeSet<String>,
) -> Result<Option<String>, ContractError> {
    match creator.participation {
        CreatorParticipation::Seat => members
            .values()
            .any(|member| {
                member.principal_reference == creator.principal_reference
                    && matches!(member.participation, ParticipationKind::AccountHuman)
            })
            .then_some(None)
            .ok_or(ContractError::InvalidShape),
        CreatorParticipation::Spectator => {
            if !matches!(access, CreatorAccess::MaySpectate)
                || creator.principal_reference != "worldstream:creator-spectator"
                || principal_references.contains(&creator.principal_reference)
            {
                return Err(ContractError::InvalidShape);
            }
            Ok(Some(creator.principal_reference))
        }
    }
}

fn append_setup_spectator(
    spectators: &mut Vec<SetupSpectator>,
    principal_references: &mut BTreeSet<String>,
    purpose: &'static str,
    reference: &str,
    kind: &'static str,
) -> Result<(), ContractError> {
    if !principal_references.insert(reference.to_owned()) {
        return Err(ContractError::InvalidShape);
    }
    spectators.push(SetupSpectator {
        purpose,
        principal: SetupPrincipal {
            reference: reference.to_owned(),
            kind,
        },
    });
    Ok(())
}

type SetupParticipation = (String, Option<SetupPrincipal>, Option<SetupAssignment>);

fn setup_participation(
    listed: &ListingSeat,
    member: FrozenMember,
) -> Result<SetupParticipation, ContractError> {
    let principal_kind = match member.participation {
        ParticipationKind::AccountHuman => "human",
        ParticipationKind::AccountExternalAgent | ParticipationKind::HouseAgentFill => "agent",
    };
    let assignment = match member.participation {
        ParticipationKind::AccountHuman => None,
        ParticipationKind::AccountExternalAgent => Some(SetupAssignment {
            mode: "external",
            agent_profile: None,
            runner_template: None,
        }),
        ParticipationKind::HouseAgentFill => {
            let profile = member.agent_profile.ok_or(ContractError::InvalidShape)?;
            let runner = member.runner_template.ok_or(ContractError::InvalidShape)?;
            let house_digest = member
                .house_agent_revision_digest
                .ok_or(ContractError::InvalidShape)?;
            if !listed.allowed_house_agent_revisions.contains(&house_digest) {
                return Err(ContractError::ReferenceMismatch);
            }
            Some(SetupAssignment {
                mode: "managed",
                agent_profile: Some(profile),
                runner_template: Some(runner),
            })
        }
    };
    Ok((
        member.display_name,
        Some(SetupPrincipal {
            reference: member.principal_reference,
            kind: principal_kind,
        }),
        assignment,
    ))
}

fn validate_member_references(member: &FrozenMember) -> Result<(), ContractError> {
    match member.participation {
        ParticipationKind::AccountHuman | ParticipationKind::AccountExternalAgent
            if member.house_agent_revision_digest.is_some()
                || member.agent_profile.is_some()
                || member.runner_template.is_some() =>
        {
            Err(ContractError::InvalidShape)
        }
        ParticipationKind::HouseAgentFill => {
            validate_digest(
                member
                    .house_agent_revision_digest
                    .as_ref()
                    .ok_or(ContractError::InvalidShape)?,
                "blake3",
            )?;
            let profile = member
                .agent_profile
                .as_ref()
                .ok_or(ContractError::InvalidShape)?;
            validate_public_reference(&profile.profile_id, 128)?;
            validate_public_reference(&profile.revision, 128)?;
            let runner = member
                .runner_template
                .as_ref()
                .ok_or(ContractError::InvalidShape)?;
            validate_public_reference(&runner.template_id, 128)?;
            validate_public_reference(&runner.revision, 128)
        }
        ParticipationKind::AccountHuman | ParticipationKind::AccountExternalAgent => Ok(()),
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectorInputContract {
    pack: PackReference,
    listing_schema: String,
    complete_head_schema: String,
    projection: SchemaReference,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TerminalRule {
    field: String,
    equals: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum SummaryField {
    Enum {
        output: String,
        source: String,
        values: Vec<String>,
    },
    NullableIdentifier {
        output: String,
        source: String,
        maximum_bytes: usize,
    },
    Integer {
        output: String,
        source: String,
        minimum: u64,
        maximum: u64,
    },
}

impl SummaryField {
    fn output(&self) -> &str {
        match self {
            Self::Enum { output, .. }
            | Self::NullableIdentifier { output, .. }
            | Self::Integer { output, .. } => output,
        }
    }

    fn source(&self) -> &str {
        match self {
            Self::Enum { source, .. }
            | Self::NullableIdentifier { source, .. }
            | Self::Integer { source, .. } => source,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectorProgram {
    schema: String,
    terminal: TerminalRule,
    outcome_field: String,
    summary_fields: Vec<SummaryField>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectorOutputContract {
    schema: String,
    schema_digest: String,
    canonicalizer: String,
    maximum_bytes: usize,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeReference {
    id: String,
    version: String,
    digest: String,
}

#[derive(Clone, Copy, Debug)]
enum RetainedRuntime {
    DeclarativeV1,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeArtifact {
    schema: String,
    runtime_id: String,
    version: String,
    implementations: Vec<RuntimeImplementation>,
    semantics: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
struct RuntimeImplementation {
    language: String,
    path: String,
    digest: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResultProjectorDocument {
    schema: String,
    projector_id: String,
    version: String,
    runtime: RuntimeReference,
    input: ProjectorInputContract,
    program: ProjectorProgram,
    output: ProjectorOutputContract,
    maximum_input_bytes: usize,
}

/// Validated, byte-identified Result Projector Revision.
#[derive(Clone, Debug)]
pub struct ResultProjectorRevision {
    document: ResultProjectorDocument,
    canonical_bytes: Vec<u8>,
    digest: String,
}

/// A projector whose runtime and schema artifacts match the reviewed revision.
///
/// This handle is the only value accepted by [`project_result`]. It can only be
/// created by resolving canonical, content-addressed artifacts.
#[derive(Clone, Debug)]
pub struct ResolvedResultProjector {
    revision: ResultProjectorRevision,
    runtime: RetainedRuntime,
}

impl ResultProjectorRevision {
    /// Reads canonical Result Projector Revision bytes and enforces every v1 bound.
    ///
    /// # Errors
    /// Returns a closed contract error for noncanonical, unsupported, malformed,
    /// or unbounded bytes.
    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, ContractError> {
        let (document, canonical_bytes) = decode_canonical(bytes, MAX_REVISION_BYTES)?;
        validate_projector(&document)?;
        let digest = blake3_digest(&canonical_bytes);
        Ok(Self {
            document,
            canonical_bytes,
            digest,
        })
    }

    #[must_use]
    pub fn digest(&self) -> &str {
        &self.digest
    }

    #[must_use]
    pub fn canonical_bytes(&self) -> &[u8] {
        &self.canonical_bytes
    }

    #[must_use]
    pub const fn maximum_output_bytes(&self) -> usize {
        self.document.output.maximum_bytes
    }

    /// Resolves the exact runtime and schema artifacts into an executable handle.
    ///
    /// # Errors
    /// Returns a closed contract error for invalid files or an identity mismatch.
    pub fn resolve_artifacts(
        &self,
        runtime_bytes: &[u8],
        projection_schema_bytes: &[u8],
        output_schema_bytes: &[u8],
    ) -> Result<ResolvedResultProjector, ContractError> {
        let runtime = resolve_runtime_artifact(&self.document.runtime, runtime_bytes)?;
        if canonical_document_digest(projection_schema_bytes)?
            != self.document.input.projection.digest
            || canonical_document_digest(output_schema_bytes)? != self.document.output.schema_digest
        {
            return Err(ContractError::ReferenceMismatch);
        }
        Ok(ResolvedResultProjector {
            revision: self.clone(),
            runtime,
        })
    }
}

fn validate_projector(document: &ResultProjectorDocument) -> Result<(), ContractError> {
    if document.schema != "worldstream/result-projector-revision/v1"
        || document.input.listing_schema != "worldstream/activity-listing-revision/v1"
        || document.input.complete_head_schema != "worldstream/complete-head/v1"
        || document.program.schema != "worldstream/result-projector-program/v1"
        || document.output.canonicalizer != "worldstream/canonical-json/v1"
    {
        return Err(ContractError::Unsupported);
    }
    let _ = resolve_runtime(&document.runtime)?;
    validate_identifier(&document.projector_id, 128)?;
    validate_version(&document.version)?;
    validate_pack(&document.input.pack)?;
    validate_schema_reference(&document.input.projection)?;
    validate_identifier(&document.output.schema, 128)?;
    validate_digest(&document.output.schema_digest, "blake3")?;
    if document.input.projection.schema != PROJECTION_SCHEMA
        || document.input.projection.digest != PROJECTION_SCHEMA_DIGEST
        || document.output.schema != RESULT_SCHEMA
        || document.output.schema_digest != RESULT_SCHEMA_DIGEST
    {
        return Err(ContractError::Unsupported);
    }
    if !(1..=MAX_REVISION_BYTES).contains(&document.maximum_input_bytes)
        || !(128..=MAX_RESULT_OUTPUT_BYTES).contains(&document.output.maximum_bytes)
    {
        return Err(ContractError::Unbounded);
    }
    validate_json_key(&document.program.terminal.field)?;
    validate_text(&document.program.terminal.equals, 64)?;
    validate_json_key(&document.program.outcome_field)?;
    if document.program.summary_fields.is_empty() || document.program.summary_fields.len() > 32 {
        return Err(ContractError::Unbounded);
    }
    let mut outputs = BTreeSet::new();
    let mut sources = BTreeSet::new();
    for field in &document.program.summary_fields {
        validate_json_key(field.output())?;
        validate_json_key(field.source())?;
        if !outputs.insert(field.output()) || !sources.insert(field.source()) {
            return Err(ContractError::InvalidShape);
        }
        match field {
            SummaryField::Enum { values, .. } => {
                if values.is_empty()
                    || values.len() > 32
                    || values.iter().collect::<BTreeSet<_>>().len() != values.len()
                {
                    return Err(ContractError::InvalidShape);
                }
                for value in values {
                    validate_text(value, 128)?;
                }
            }
            SummaryField::NullableIdentifier { maximum_bytes, .. } => {
                if !(1..=128).contains(maximum_bytes) {
                    return Err(ContractError::Unbounded);
                }
            }
            SummaryField::Integer {
                minimum, maximum, ..
            } => {
                if minimum > maximum || *maximum > u64::from(u32::MAX) {
                    return Err(ContractError::Unbounded);
                }
            }
        }
    }
    let expected = BTreeSet::from(["outcome", "reason", "score", "selected_plan_id"]);
    if outputs != expected
        || document.program.terminal.field != "phase"
        || document.program.terminal.equals != "complete"
        || document.program.outcome_field != "outcome"
    {
        return Err(ContractError::InvalidShape);
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompleteHead {
    room_id: String,
    room_seq: u64,
    genesis_or_transition_hash: String,
    core_schema_version: String,
    pack_digest: String,
    core_state_hash: String,
    activity_state_hash: String,
    authoritative_state_hash: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResultProjectorInput {
    schema: String,
    listing_revision_digest: String,
    projector_revision_digest: String,
    pack: PackReference,
    projection_schema: String,
    source_head: CompleteHead,
    public_projection: Value,
}

/// Canonical bounded output from a Result Projector.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectedResult(Vec<u8>);

impl ProjectedResult {
    /// Returns the retained canonical public result bytes.
    ///
    /// # Errors
    /// Returns [`ContractError::InvalidShape`] if retained bytes cannot be decoded.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ContractError> {
        canonicalize(
            &serde_json::from_slice::<Value>(&self.0).map_err(|_| ContractError::InvalidShape)?,
        )
    }
}

/// Runs one deterministic, networkless declarative projector.
///
/// # Errors
/// Returns a closed contract error for malformed input, identity mismatch,
/// unreviewed projection data, or an invalid or oversized output.
pub fn project_result(
    listing: &ListingRevision,
    projector: &ResolvedResultProjector,
    input_bytes: &[u8],
) -> Result<ProjectedResult, ContractError> {
    let revision = &projector.revision;
    listing.verify_projector(revision)?;
    let (input, _) = decode_canonical::<ResultProjectorInput>(
        input_bytes,
        revision.document.maximum_input_bytes,
    )?;
    if input.schema != "worldstream/result-projector-input/v1" {
        return Err(ContractError::Unsupported);
    }
    if input.listing_revision_digest != listing.digest
        || input.projector_revision_digest != revision.digest
        || input.pack != revision.document.input.pack
        || input.projection_schema != revision.document.input.projection.schema
        || input.source_head.pack_digest != input.pack.digest
    {
        return Err(ContractError::ReferenceMismatch);
    }
    validate_complete_head(&input.source_head)?;
    validate_agent_heist_public_projection(&input.public_projection)?;
    let output = match projector.runtime {
        RetainedRuntime::DeclarativeV1 => {
            runtime_v1::interpret(&revision.document, &input.public_projection)?
        }
    };
    validate_result_output(&output)?;
    let bytes = canonicalize(&output)?;
    if bytes.len() > revision.document.output.maximum_bytes {
        return Err(ContractError::OutputTooLarge);
    }
    Ok(ProjectedResult(bytes))
}

fn validate_complete_head(head: &CompleteHead) -> Result<(), ContractError> {
    validate_public_reference(&head.room_id, 128)?;
    for digest in [
        &head.genesis_or_transition_hash,
        &head.pack_digest,
        &head.core_state_hash,
        &head.activity_state_hash,
        &head.authoritative_state_hash,
    ] {
        validate_digest(digest, "blake3")?;
    }
    if head.core_schema_version != "worldstream.core-room-state.v1" {
        return Err(ContractError::Unsupported);
    }
    if head.room_seq > 9_007_199_254_740_991 {
        return Err(ContractError::Unbounded);
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PublicSeat {
    role: String,
    present: bool,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PublicClaim {
    clue_id: String,
    claim_code: String,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PublicPlan {
    plan_id: String,
    proposer_role: String,
    created_room_seq: u64,
    route: String,
    entry_window: String,
    required_tool: String,
    extraction: String,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PublicChallenge {
    role: String,
    plan_id: String,
    reason: String,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
struct PublicChecks {
    route: bool,
    entry_window: bool,
    required_tool: bool,
    extraction: bool,
    resource_contributed: bool,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PublicOutcome {
    outcome: String,
    selected_plan_id: Option<String>,
    vote_counts: BTreeMap<String, u32>,
    missing_roles: Vec<String>,
    checks: Option<PublicChecks>,
    score: u8,
    reason: String,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentHeistPublicProjection {
    phase: String,
    phase_generation: u32,
    phase_start: String,
    phase_deadline: Option<String>,
    seats: Vec<PublicSeat>,
    public_claims: Vec<PublicClaim>,
    plans: Vec<PublicPlan>,
    endorsements: BTreeMap<String, String>,
    challenges: Vec<PublicChallenge>,
    commitment_count: u32,
    outcome: Option<PublicOutcome>,
}

#[allow(clippy::too_many_lines)]
fn validate_agent_heist_public_projection(value: &Value) -> Result<(), ContractError> {
    exact_keys(
        value,
        &[
            "phase",
            "phase_generation",
            "phase_start",
            "phase_deadline",
            "seats",
            "public_claims",
            "plans",
            "endorsements",
            "challenges",
            "commitment_count",
            "outcome",
        ],
    )?;
    let projection: AgentHeistPublicProjection =
        serde_json::from_value(value.clone()).map_err(|_| ContractError::InvalidShape)?;
    if ![
        "lobby",
        "briefing",
        "negotiation",
        "commitment",
        "resolution",
        "result",
        "complete",
    ]
    .contains(&projection.phase.as_str())
    {
        return Err(ContractError::Unsupported);
    }
    validate_timestamp(&projection.phase_start)?;
    if let Some(deadline) = &projection.phase_deadline {
        validate_timestamp(deadline)?;
    }
    if projection.seats.len() > 32
        || projection.public_claims.len() > 64
        || projection.plans.len() > 64
        || projection.endorsements.len() > 32
        || projection.challenges.len() > 64
        || projection.commitment_count > 32
    {
        return Err(ContractError::Unbounded);
    }
    for (index, item) in value["seats"]
        .as_array()
        .ok_or(ContractError::InvalidShape)?
        .iter()
        .enumerate()
    {
        exact_keys(item, &["role", "present"])?;
        validate_role(&projection.seats[index].role)?;
        let _ = projection.seats[index].present;
    }
    for (index, item) in value["public_claims"]
        .as_array()
        .ok_or(ContractError::InvalidShape)?
        .iter()
        .enumerate()
    {
        exact_keys(item, &["clue_id", "claim_code"])?;
        validate_public_reference(&projection.public_claims[index].clue_id, 128)?;
        validate_public_reference(&projection.public_claims[index].claim_code, 128)?;
    }
    for (index, item) in value["plans"]
        .as_array()
        .ok_or(ContractError::InvalidShape)?
        .iter()
        .enumerate()
    {
        exact_keys(
            item,
            &[
                "plan_id",
                "proposer_role",
                "created_room_seq",
                "route",
                "entry_window",
                "required_tool",
                "extraction",
            ],
        )?;
        let plan = &projection.plans[index];
        validate_public_reference(&plan.plan_id, 128)?;
        validate_role(&plan.proposer_role)?;
        validate_enum(&plan.route, &["canal", "service", "roof"])?;
        validate_enum(&plan.entry_window, &["late", "early", "middle"])?;
        validate_enum(&plan.required_tool, &["disguise", "thermal_key", "jammer"])?;
        validate_enum(&plan.extraction, &["van", "boat", "motorbike"])?;
        if plan.created_room_seq > 9_007_199_254_740_991 {
            return Err(ContractError::Unbounded);
        }
    }
    for (role, plan_id) in &projection.endorsements {
        validate_role(role)?;
        validate_public_reference(plan_id, 128)?;
    }
    for (index, item) in value["challenges"]
        .as_array()
        .ok_or(ContractError::InvalidShape)?
        .iter()
        .enumerate()
    {
        exact_keys(item, &["role", "plan_id", "reason"])?;
        let challenge = &projection.challenges[index];
        validate_role(&challenge.role)?;
        validate_public_reference(&challenge.plan_id, 128)?;
        validate_enum(
            &challenge.reason,
            &[
                "route_conflict",
                "timing_conflict",
                "tool_conflict",
                "extraction_conflict",
            ],
        )?;
    }
    if let Some(outcome_value) = value.get("outcome").filter(|item| !item.is_null()) {
        exact_keys(
            outcome_value,
            &[
                "outcome",
                "selected_plan_id",
                "vote_counts",
                "missing_roles",
                "checks",
                "score",
                "reason",
            ],
        )?;
        let outcome = projection
            .outcome
            .as_ref()
            .ok_or(ContractError::InvalidShape)?;
        validate_enum(&outcome.outcome, &["success", "partial_failure", "failure"])?;
        if let Some(plan_id) = &outcome.selected_plan_id {
            validate_public_reference(plan_id, 128)?;
        }
        if outcome.vote_counts.len() > 64 || outcome.missing_roles.len() > 3 || outcome.score > 5 {
            return Err(ContractError::Unbounded);
        }
        for (plan_id, votes) in &outcome.vote_counts {
            validate_public_reference(plan_id, 128)?;
            if *votes > 32 {
                return Err(ContractError::Unbounded);
            }
        }
        let roles = outcome.missing_roles.iter().collect::<BTreeSet<_>>();
        if roles.len() != outcome.missing_roles.len() {
            return Err(ContractError::InvalidShape);
        }
        for role in &outcome.missing_roles {
            validate_role(role)?;
        }
        if let Some(checks_value) = outcome_value.get("checks").filter(|item| !item.is_null()) {
            exact_keys(
                checks_value,
                &[
                    "route",
                    "entry_window",
                    "required_tool",
                    "extraction",
                    "resource_contributed",
                ],
            )?;
        }
        if let Some(checks) = &outcome.checks {
            let _ = (
                checks.route,
                checks.entry_window,
                checks.required_tool,
                checks.extraction,
                checks.resource_contributed,
            );
        }
        validate_enum(
            &outcome.reason,
            &["no_strict_majority", "scored_selected_plan"],
        )?;
    }
    let _ = projection.phase_generation;
    Ok(())
}

fn validate_result_output(value: &Value) -> Result<(), ContractError> {
    let object = value.as_object().ok_or(ContractError::InvalidShape)?;
    match object.get("status").and_then(Value::as_str) {
        Some("not_terminal" | "terminal_without_outcome") if object.len() == 1 => Ok(()),
        Some("summary") if object.len() == 2 => {
            let summary = object.get("summary").ok_or(ContractError::InvalidShape)?;
            exact_keys(
                summary,
                &["schema", "outcome", "selected_plan_id", "score", "reason"],
            )?;
            let summary = summary.as_object().ok_or(ContractError::InvalidShape)?;
            if summary.get("schema").and_then(Value::as_str) != Some(RESULT_SCHEMA) {
                return Err(ContractError::InvalidShape);
            }
            validate_enum(
                summary
                    .get("outcome")
                    .and_then(Value::as_str)
                    .ok_or(ContractError::InvalidShape)?,
                &["success", "partial_failure", "failure"],
            )?;
            if let Some(plan_id) = summary
                .get("selected_plan_id")
                .filter(|item| !item.is_null())
            {
                validate_public_reference(
                    plan_id.as_str().ok_or(ContractError::InvalidShape)?,
                    128,
                )?;
            }
            if summary
                .get("score")
                .and_then(Value::as_u64)
                .is_none_or(|score| score > 5)
            {
                return Err(ContractError::InvalidShape);
            }
            validate_enum(
                summary
                    .get("reason")
                    .and_then(Value::as_str)
                    .ok_or(ContractError::InvalidShape)?,
                &["no_strict_majority", "scored_selected_plan"],
            )?;
            Ok(())
        }
        _ => Err(ContractError::InvalidShape),
    }
}

fn exact_keys(value: &Value, expected: &[&str]) -> Result<(), ContractError> {
    let object = value.as_object().ok_or(ContractError::InvalidShape)?;
    if object.len() == expected.len() && object.keys().all(|key| expected.contains(&key.as_str())) {
        Ok(())
    } else {
        Err(ContractError::InvalidShape)
    }
}

fn decode_canonical<T: DeserializeOwned>(
    bytes: &[u8],
    maximum_bytes: usize,
) -> Result<(T, Vec<u8>), ContractError> {
    if bytes.len() > maximum_bytes {
        return Err(ContractError::TooLarge);
    }
    let value =
        CanonicalJsonV1::from_canonical_bytes(bytes).map_err(|_| ContractError::NonCanonical)?;
    let canonical_bytes = value.to_bytes().map_err(|_| ContractError::InvalidShape)?;
    let document =
        serde_json::from_slice(&canonical_bytes).map_err(|_| ContractError::InvalidShape)?;
    Ok((document, canonical_bytes))
}

fn canonicalize<T: Serialize>(value: &T) -> Result<Vec<u8>, ContractError> {
    let json = serde_json::to_vec(value).map_err(|_| ContractError::InvalidShape)?;
    CanonicalJsonV1::parse(&json)
        .and_then(|canonical| canonical.to_bytes())
        .map_err(|_| ContractError::InvalidShape)
}

fn canonical_document_digest(bytes: &[u8]) -> Result<String, ContractError> {
    if bytes.len() > MAX_REVISION_BYTES {
        return Err(ContractError::TooLarge);
    }
    CanonicalJsonV1::from_canonical_bytes(bytes).map_err(|_| ContractError::NonCanonical)?;
    Ok(blake3_digest(bytes))
}

fn resolve_runtime(reference: &RuntimeReference) -> Result<RetainedRuntime, ContractError> {
    validate_identifier(&reference.id, 128)?;
    validate_version(&reference.version)?;
    validate_digest(&reference.digest, "blake3")?;
    if reference.id == RUNTIME_ID
        && reference.version == RUNTIME_VERSION
        && reference.digest == RUNTIME_DIGEST
    {
        Ok(RetainedRuntime::DeclarativeV1)
    } else {
        Err(ContractError::Unsupported)
    }
}

fn resolve_runtime_artifact(
    reference: &RuntimeReference,
    bytes: &[u8],
) -> Result<RetainedRuntime, ContractError> {
    let runtime = resolve_runtime(reference)?;
    if canonical_document_digest(bytes)? != reference.digest {
        return Err(ContractError::ReferenceMismatch);
    }
    let (artifact, _) = decode_canonical::<RuntimeArtifact>(bytes, MAX_REVISION_BYTES)?;
    if artifact.schema != "worldstream/result-projector-runtime-artifact/v1"
        || artifact.runtime_id != reference.id
        || artifact.version != reference.version
    {
        return Err(ContractError::ReferenceMismatch);
    }
    let expected_semantics = BTreeSet::from([
        "canonicalization",
        "capabilities",
        "input",
        "missing_outcome",
        "summary",
        "terminal",
    ]);
    if artifact
        .semantics
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>()
        != expected_semantics
    {
        return Err(ContractError::InvalidShape);
    }
    for description in artifact.semantics.values() {
        validate_text(description, 1_024)?;
    }
    let expected_implementations = BTreeMap::from([
        (
            "rust",
            (
                RUST_RUNTIME_PATH,
                RUST_RUNTIME_SOURCE_DIGEST,
                RUST_RUNTIME_SOURCE,
            ),
        ),
        (
            "typescript",
            (
                TYPESCRIPT_RUNTIME_PATH,
                TYPESCRIPT_RUNTIME_SOURCE_DIGEST,
                TYPESCRIPT_RUNTIME_SOURCE,
            ),
        ),
    ]);
    if artifact.implementations.len() != expected_implementations.len() {
        return Err(ContractError::InvalidShape);
    }
    let mut languages = BTreeSet::new();
    for implementation in &artifact.implementations {
        validate_digest(&implementation.digest, "blake3")?;
        if !languages.insert(implementation.language.as_str()) {
            return Err(ContractError::InvalidShape);
        }
        let Some((path, digest, source)) =
            expected_implementations.get(implementation.language.as_str())
        else {
            return Err(ContractError::Unsupported);
        };
        if implementation.path != *path
            || implementation.digest != *digest
            || blake3_digest(source) != *digest
        {
            return Err(ContractError::ReferenceMismatch);
        }
    }
    Ok(runtime)
}

fn blake3_digest(bytes: &[u8]) -> String {
    format!("blake3:{}", blake3::hash(bytes).to_hex())
}

fn sha256_digest(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut value = String::with_capacity(71);
    value.push_str("sha256:");
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(value, "{byte:02x}");
    }
    value
}

fn validate_pack(pack: &PackReference) -> Result<(), ContractError> {
    validate_identifier(&pack.id, 128)?;
    validate_version(&pack.version)?;
    validate_digest(&pack.digest, "blake3")
}

fn validate_schema_reference(reference: &SchemaReference) -> Result<(), ContractError> {
    validate_identifier(&reference.schema, 128)?;
    validate_digest(&reference.digest, "blake3")
}

fn validate_projector_reference(reference: &ProjectorReference) -> Result<(), ContractError> {
    validate_identifier(&reference.id, 128)?;
    validate_version(&reference.version)?;
    validate_digest(&reference.digest, "blake3")
}

fn validate_digest(value: &str, algorithm: &str) -> Result<(), ContractError> {
    let Some(hex) = value.strip_prefix(&format!("{algorithm}:")) else {
        return Err(ContractError::InvalidShape);
    };
    if hex.len() == 64
        && hex
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        Ok(())
    } else {
        Err(ContractError::InvalidShape)
    }
}

fn validate_identifier(value: &str, maximum_bytes: usize) -> Result<(), ContractError> {
    if value.is_empty()
        || value.len() > maximum_bytes
        || !value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        || !value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-' | b'/')
        })
    {
        Err(ContractError::InvalidShape)
    } else {
        Ok(())
    }
}

fn validate_public_reference(value: &str, maximum_bytes: usize) -> Result<(), ContractError> {
    if value.is_empty()
        || value.len() > maximum_bytes
        || !value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
    {
        Err(ContractError::InvalidShape)
    } else {
        Ok(())
    }
}

fn validate_json_key(value: &str) -> Result<(), ContractError> {
    validate_public_reference(value, 128)
}

fn validate_seat_label(value: &str) -> Result<(), ContractError> {
    if !value.is_empty()
        && value.len() <= 64
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        Ok(())
    } else {
        Err(ContractError::InvalidShape)
    }
}

fn validate_version(value: &str) -> Result<(), ContractError> {
    if value.is_empty()
        || value.len() > 32
        || !value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        Err(ContractError::InvalidShape)
    } else {
        Ok(())
    }
}

fn validate_text(value: &str, maximum_bytes: usize) -> Result<(), ContractError> {
    if value.is_empty() || value.len() > maximum_bytes || value.chars().any(char::is_control) {
        Err(ContractError::Unbounded)
    } else {
        Ok(())
    }
}

fn validate_timestamp(value: &str) -> Result<(), ContractError> {
    validate_text(value, 64)?;
    if value.bytes().all(|byte| {
        byte.is_ascii_digit() || matches!(byte, b'-' | b':' | b'.' | b'T' | b'Z' | b'+')
    }) {
        Ok(())
    } else {
        Err(ContractError::InvalidShape)
    }
}

fn validate_role(value: &str) -> Result<(), ContractError> {
    validate_enum(value, &["navigator", "insider", "broker"])
}

fn validate_enum(value: &str, allowed: &[&str]) -> Result<(), ContractError> {
    if allowed.contains(&value) {
        Ok(())
    } else {
        Err(ContractError::InvalidShape)
    }
}

fn validate_json(value: &Value) -> Result<(), ContractError> {
    let mut remaining = MAX_JSON_NODES;
    if validate_json_node(value, 0, &mut remaining) {
        Ok(())
    } else {
        Err(ContractError::Unbounded)
    }
}

fn validate_json_node(value: &Value, depth: usize, remaining: &mut usize) -> bool {
    if depth > MAX_JSON_DEPTH || *remaining == 0 {
        return false;
    }
    *remaining -= 1;
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => true,
        Value::String(text) => text.len() <= 4_096 && !text.chars().any(char::is_control),
        Value::Array(items) => items
            .iter()
            .all(|item| validate_json_node(item, depth + 1, remaining)),
        Value::Object(fields) => fields.iter().all(|(key, item)| {
            validate_json_key(key).is_ok() && validate_json_node(item, depth + 1, remaining)
        }),
    }
}
