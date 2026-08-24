//! Storage-neutral operational authority decisions.
//!
//! The external seam is deliberately small: callers authorize one closed use
//! or apply one closed authority change at an explicitly supplied trusted
//! time. Bearer verification, profile/scope policy, Principal and Membership
//! binding, expiry, revocation, generation fencing, and safe error projection
//! remain behind that seam.

use std::{collections::BTreeSet, fmt, sync::Arc};

#[cfg(any(test, feature = "conformance-tracer"))]
use std::{collections::BTreeMap, sync::Mutex};

use serde::{Deserialize, Deserializer, Serialize, de};
use thiserror::Error;

use crate::{
    AccessModeV1, AdministrationOperationIdentityV1, AuthorityChangeId, AuthorityCheckedAt,
    AuthorityGenerationV1, Blake3DigestV1, CREATE_ROOM_OPERATION_KIND, CanonicalJsonError,
    CanonicalRequestHashV1, CapabilityExpiresAt, CapabilityId, CapabilityRevokedAt,
    CoreAdministrationRequestV1, CoreAuthorityAttributionV1, CoreAuthorityKindV1,
    CoreProposedKindV1, MemberId, MembershipGenerationV1, MembershipStandingV1, MembershipV1,
    OperationIdentityV1, ParticipantActionOperationIdentityV1, ParticipantActionRequestV1,
    PrincipalGenerationV1, PrincipalId, PrincipalKindV1, RoomCreationRequestV1, RoomId,
    RoomSequenceV1, RunnerGenerationV1, RunnerId, TimerFiredRequestV1, canonical::encode,
    primitives::compare_timestamp_text,
};

const CAPABILITY_TOKEN_HASH_DOMAIN: &[u8] = b"worldstream/capability-token-hash/v1\0";
const AUTHORITY_FENCE_DOMAIN: &str = "worldstream/authority-fence/v1";
const AUTHORITY_PURPOSE_DOMAIN: &str = "worldstream/authority-purpose/v1";
const MAX_SCOPES: usize = 16;
const MAX_RUNNER_MEMBERSHIPS: usize = 256;
const MAX_ACTION_TYPE_BYTES: usize = 256;
const MAX_REASON_CODE_BYTES: usize = 128;

/// One 256-bit bearer secret. Formatting never exposes its bytes.
pub struct CapabilityBearerV1([u8; 32]);

impl CapabilityBearerV1 {
    /// Wraps exactly 256 bits generated outside Core.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Derives the only token hash accepted by the authority Module.
    #[must_use]
    pub fn token_hash(&self) -> CapabilityTokenHashV1 {
        let mut hasher = blake3::Hasher::new();
        hasher.update(CAPABILITY_TOKEN_HASH_DOMAIN);
        hasher.update(&self.0);
        CapabilityTokenHashV1(*hasher.finalize().as_bytes())
    }
}

impl fmt::Debug for CapabilityBearerV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CapabilityBearerV1([REDACTED])")
    }
}

impl Drop for CapabilityBearerV1 {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

/// Persisted 256-bit token hash. Formatting never exposes its bytes.
#[derive(Clone)]
pub struct CapabilityTokenHashV1([u8; 32]);

impl CapabilityTokenHashV1 {
    /// Reconstructs a stored token hash from exact database bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Returns exact bytes for a storage Adapter. Never log this value.
    #[must_use]
    pub const fn storage_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    fn constant_time_eq(&self, other: &Self) -> bool {
        self.0
            .iter()
            .zip(other.0.iter())
            .fold(0_u8, |difference, (left, right)| {
                difference | (left ^ right)
            })
            == 0
    }
}

impl fmt::Debug for CapabilityTokenHashV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CapabilityTokenHashV1([REDACTED])")
    }
}

impl PartialEq for CapabilityTokenHashV1 {
    fn eq(&self, other: &Self) -> bool {
        self.constant_time_eq(other)
    }
}

impl Eq for CapabilityTokenHashV1 {}

/// One presented bearer and its non-secret selector.
pub struct PresentedCapabilityV1 {
    capability_id: CapabilityId,
    bearer: CapabilityBearerV1,
}

impl PresentedCapabilityV1 {
    /// Binds one presented secret to its public Capability selector.
    #[must_use]
    pub const fn new(capability_id: CapabilityId, bearer: CapabilityBearerV1) -> Self {
        Self {
            capability_id,
            bearer,
        }
    }

    /// Returns the public Capability selector.
    #[must_use]
    pub const fn capability_id(&self) -> &CapabilityId {
        &self.capability_id
    }
}

impl fmt::Debug for PresentedCapabilityV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PresentedCapabilityV1")
            .field("capability_id", &self.capability_id)
            .field("bearer", &"[REDACTED]")
            .finish()
    }
}

/// Closed v1 Capability scopes.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub enum CapabilityScopeV1 {
    #[serde(rename = "room:attach")]
    RoomAttach,
    #[serde(rename = "room:act")]
    RoomAct,
    #[serde(rename = "room:observe_public")]
    RoomObservePublic,
    #[serde(rename = "room:observe_member")]
    RoomObserveMember,
    #[serde(rename = "room:replay")]
    RoomReplay,
    #[serde(rename = "activation:offer_receive")]
    ActivationOfferReceive,
    #[serde(rename = "activation:claim")]
    ActivationClaim,
    #[serde(rename = "activation:complete")]
    ActivationComplete,
    #[serde(rename = "operator:room_admin")]
    OperatorRoomAdmin,
    #[serde(rename = "operator:backup")]
    OperatorBackup,
}

/// A bounded, sorted, duplicate-free Capability scope set.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct CapabilityScopeSetV1(BTreeSet<CapabilityScopeV1>);

impl CapabilityScopeSetV1 {
    /// Canonicalizes a nonempty bounded set and rejects duplicate input.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty, duplicate, or oversized set.
    pub fn new(
        scopes: impl IntoIterator<Item = CapabilityScopeV1>,
    ) -> Result<Self, AuthorityShapeErrorV1> {
        let mut canonical = BTreeSet::new();
        for scope in scopes {
            if !canonical.insert(scope) {
                return Err(AuthorityShapeErrorV1::DuplicateScope);
            }
            if canonical.len() > MAX_SCOPES {
                return Err(AuthorityShapeErrorV1::TooManyScopes);
            }
        }
        if canonical.is_empty() {
            return Err(AuthorityShapeErrorV1::EmptyScopes);
        }
        Ok(Self(canonical))
    }

    /// Returns whether this exact closed scope is present.
    #[must_use]
    pub fn contains(&self, scope: CapabilityScopeV1) -> bool {
        self.0.contains(&scope)
    }

    /// Iterates in canonical scope order.
    pub fn iter(&self) -> impl Iterator<Item = CapabilityScopeV1> + '_ {
        self.0.iter().copied()
    }

    /// Encodes the exact canonical storage representation.
    ///
    /// # Errors
    ///
    /// Returns an error only if the closed typed value cannot be encoded.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, CanonicalJsonError> {
        encode(self)
    }

    fn is_subset(&self, other: &Self) -> bool {
        self.0.is_subset(&other.0)
    }
}

impl<'de> Deserialize<'de> for CapabilityScopeSetV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let scopes = Vec::<CapabilityScopeV1>::deserialize(deserializer)?;
        Self::new(scopes).map_err(de::Error::custom)
    }
}

/// Exact Room-local Membership address.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoomMembershipKeyV1 {
    pub room_id: RoomId,
    pub member_id: MemberId,
}

/// A bounded, sorted, duplicate-free Runner Membership allowlist.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct RunnerMembershipSetV1(BTreeSet<RoomMembershipKeyV1>);

impl RunnerMembershipSetV1 {
    /// Canonicalizes a nonempty bounded set and rejects duplicate input.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty, duplicate, or oversized set.
    pub fn new(
        memberships: impl IntoIterator<Item = RoomMembershipKeyV1>,
    ) -> Result<Self, AuthorityShapeErrorV1> {
        let mut canonical = BTreeSet::new();
        for membership in memberships {
            if !canonical.insert(membership) {
                return Err(AuthorityShapeErrorV1::DuplicateRunnerMembership);
            }
            if canonical.len() > MAX_RUNNER_MEMBERSHIPS {
                return Err(AuthorityShapeErrorV1::TooManyRunnerMemberships);
            }
        }
        if canonical.is_empty() {
            return Err(AuthorityShapeErrorV1::EmptyRunnerMemberships);
        }
        Ok(Self(canonical))
    }

    /// Returns whether the Runner is scoped to one exact Membership.
    #[must_use]
    pub fn contains(&self, membership: &RoomMembershipKeyV1) -> bool {
        self.0.contains(membership)
    }

    /// Iterates in canonical Room/Member order.
    pub fn iter(&self) -> impl Iterator<Item = &RoomMembershipKeyV1> {
        self.0.iter()
    }
}

impl<'de> Deserialize<'de> for RunnerMembershipSetV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let memberships = Vec::<RoomMembershipKeyV1>::deserialize(deserializer)?;
        Self::new(memberships).map_err(de::Error::custom)
    }
}

/// Mutually exclusive Capability authority families.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "profile", rename_all = "snake_case")]
pub enum CapabilityProfileV1 {
    RoomMember {
        room_id: RoomId,
        member_id: MemberId,
    },
    HostOperator {
        room_id: Option<RoomId>,
    },
    RunnerControl {
        runner_id: RunnerId,
        permitted_memberships: RunnerMembershipSetV1,
    },
}

impl CapabilityProfileV1 {
    /// Encodes the exact canonical storage representation.
    ///
    /// # Errors
    ///
    /// Returns an error only if the closed typed value cannot be encoded.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, CanonicalJsonError> {
        encode(self)
    }

    fn validate_scopes(&self, scopes: &CapabilityScopeSetV1) -> Result<(), AuthorityShapeErrorV1> {
        let compatible = scopes.iter().all(|scope| match self {
            Self::RoomMember { .. } => matches!(
                scope,
                CapabilityScopeV1::RoomAttach
                    | CapabilityScopeV1::RoomAct
                    | CapabilityScopeV1::RoomObservePublic
                    | CapabilityScopeV1::RoomObserveMember
                    | CapabilityScopeV1::RoomReplay
            ),
            Self::HostOperator { .. } => matches!(
                scope,
                CapabilityScopeV1::OperatorRoomAdmin | CapabilityScopeV1::OperatorBackup
            ),
            Self::RunnerControl { .. } => matches!(
                scope,
                CapabilityScopeV1::ActivationOfferReceive
                    | CapabilityScopeV1::ActivationClaim
                    | CapabilityScopeV1::ActivationComplete
            ),
        });
        if compatible {
            Ok(())
        } else {
            Err(AuthorityShapeErrorV1::IncompatibleScopeProfile)
        }
    }
}

/// Operational Principal status.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PrincipalAuthorityStatusV1 {
    Enabled,
    Disabled,
}

/// Operational Runner status.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunnerAuthorityStatusV1 {
    Enabled,
    Revoked,
}

/// One durable Principal authority snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrincipalAuthoritySnapshotV1 {
    principal_id: PrincipalId,
    kind: PrincipalKindV1,
    status: PrincipalAuthorityStatusV1,
    generation: PrincipalGenerationV1,
}

impl PrincipalAuthoritySnapshotV1 {
    #[must_use]
    pub const fn new(
        principal_id: PrincipalId,
        kind: PrincipalKindV1,
        status: PrincipalAuthorityStatusV1,
        generation: PrincipalGenerationV1,
    ) -> Self {
        Self {
            principal_id,
            kind,
            status,
            generation,
        }
    }

    #[must_use]
    pub const fn principal_id(&self) -> &PrincipalId {
        &self.principal_id
    }

    #[must_use]
    pub const fn kind(&self) -> PrincipalKindV1 {
        self.kind
    }

    #[must_use]
    pub const fn status(&self) -> PrincipalAuthorityStatusV1 {
        self.status
    }

    #[must_use]
    pub const fn generation(&self) -> PrincipalGenerationV1 {
        self.generation
    }
}

/// Construction fields for one durable Capability snapshot.
pub struct CapabilityAuthoritySnapshotPartsV1 {
    pub capability_id: CapabilityId,
    pub token_hash: CapabilityTokenHashV1,
    pub principal_id: PrincipalId,
    pub profile: CapabilityProfileV1,
    pub scopes: CapabilityScopeSetV1,
    pub generation: AuthorityGenerationV1,
    pub expires_at: Option<CapabilityExpiresAt>,
    pub revoked_at: Option<CapabilityRevokedAt>,
}

/// One durable Capability scope, target, expiry, and revocation snapshot.
#[derive(Clone, Eq, PartialEq)]
pub struct CapabilityAuthoritySnapshotV1 {
    capability_id: CapabilityId,
    token_hash: CapabilityTokenHashV1,
    principal_id: PrincipalId,
    profile: CapabilityProfileV1,
    scopes: CapabilityScopeSetV1,
    generation: AuthorityGenerationV1,
    expires_at: Option<CapabilityExpiresAt>,
    revoked_at: Option<CapabilityRevokedAt>,
}

impl CapabilityAuthoritySnapshotV1 {
    /// Validates the immutable profile/scope family.
    ///
    /// # Errors
    ///
    /// Returns an error when a scope belongs to another authority family.
    pub fn new(parts: CapabilityAuthoritySnapshotPartsV1) -> Result<Self, AuthorityShapeErrorV1> {
        parts.profile.validate_scopes(&parts.scopes)?;
        Ok(Self {
            capability_id: parts.capability_id,
            token_hash: parts.token_hash,
            principal_id: parts.principal_id,
            profile: parts.profile,
            scopes: parts.scopes,
            generation: parts.generation,
            expires_at: parts.expires_at,
            revoked_at: parts.revoked_at,
        })
    }

    #[must_use]
    pub const fn capability_id(&self) -> &CapabilityId {
        &self.capability_id
    }

    #[must_use]
    pub const fn token_hash(&self) -> &CapabilityTokenHashV1 {
        &self.token_hash
    }

    #[must_use]
    pub const fn principal_id(&self) -> &PrincipalId {
        &self.principal_id
    }

    #[must_use]
    pub const fn profile(&self) -> &CapabilityProfileV1 {
        &self.profile
    }

    #[must_use]
    pub const fn scopes(&self) -> &CapabilityScopeSetV1 {
        &self.scopes
    }

    #[must_use]
    pub const fn generation(&self) -> AuthorityGenerationV1 {
        self.generation
    }

    #[must_use]
    pub const fn expires_at(&self) -> Option<&CapabilityExpiresAt> {
        self.expires_at.as_ref()
    }

    #[must_use]
    pub const fn revoked_at(&self) -> Option<&CapabilityRevokedAt> {
        self.revoked_at.as_ref()
    }
}

impl fmt::Debug for CapabilityAuthoritySnapshotV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CapabilityAuthoritySnapshotV1")
            .field("capability_id", &self.capability_id)
            .field("token_hash", &"[REDACTED]")
            .field("principal_id", &self.principal_id)
            .field("profile", &"[REDACTED]")
            .field("scopes", &"[REDACTED]")
            .field("generation", &self.generation)
            .field("expires_at", &self.expires_at)
            .field("revoked", &self.revoked_at.is_some())
            .finish()
    }
}

/// One current Membership plus its operational authority generation.
#[derive(Clone, Eq, PartialEq)]
pub struct MembershipAuthoritySnapshotV1 {
    room_id: RoomId,
    membership: MembershipV1,
    generation: MembershipGenerationV1,
}

impl fmt::Debug for MembershipAuthoritySnapshotV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MembershipAuthoritySnapshotV1")
            .field("room_id", &"[REDACTED]")
            .field("membership", &"[REDACTED]")
            .field("generation", &self.generation)
            .finish()
    }
}

impl MembershipAuthoritySnapshotV1 {
    #[must_use]
    pub const fn new(
        room_id: RoomId,
        membership: MembershipV1,
        generation: MembershipGenerationV1,
    ) -> Self {
        Self {
            room_id,
            membership,
            generation,
        }
    }

    #[must_use]
    pub const fn room_id(&self) -> &RoomId {
        &self.room_id
    }

    #[must_use]
    pub const fn membership(&self) -> &MembershipV1 {
        &self.membership
    }

    #[must_use]
    pub const fn generation(&self) -> MembershipGenerationV1 {
        self.generation
    }
}

/// One durable Runner authority snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunnerAuthoritySnapshotV1 {
    runner_id: RunnerId,
    owner_principal_id: PrincipalId,
    status: RunnerAuthorityStatusV1,
    generation: RunnerGenerationV1,
}

impl RunnerAuthoritySnapshotV1 {
    #[must_use]
    pub const fn new(
        runner_id: RunnerId,
        owner_principal_id: PrincipalId,
        status: RunnerAuthorityStatusV1,
        generation: RunnerGenerationV1,
    ) -> Self {
        Self {
            runner_id,
            owner_principal_id,
            status,
            generation,
        }
    }

    #[must_use]
    pub const fn runner_id(&self) -> &RunnerId {
        &self.runner_id
    }

    #[must_use]
    pub const fn owner_principal_id(&self) -> &PrincipalId {
        &self.owner_principal_id
    }

    #[must_use]
    pub const fn status(&self) -> RunnerAuthorityStatusV1 {
        self.status
    }

    #[must_use]
    pub const fn generation(&self) -> RunnerGenerationV1 {
        self.generation
    }
}

/// Coherent current authority facts returned by one storage snapshot.
#[derive(Clone, Eq, PartialEq)]
pub struct AuthoritySnapshotV1 {
    capability: CapabilityAuthoritySnapshotV1,
    principal: PrincipalAuthoritySnapshotV1,
    membership: Option<MembershipAuthoritySnapshotV1>,
    runner: Option<RunnerAuthoritySnapshotV1>,
}

impl fmt::Debug for AuthoritySnapshotV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AuthoritySnapshotV1([REDACTED])")
    }
}

impl AuthoritySnapshotV1 {
    /// Constructs one coherent Adapter snapshot.
    ///
    /// # Errors
    ///
    /// Returns an error if Capability and Principal identities disagree.
    pub fn new(
        capability: CapabilityAuthoritySnapshotV1,
        principal: PrincipalAuthoritySnapshotV1,
        membership: Option<MembershipAuthoritySnapshotV1>,
        runner: Option<RunnerAuthoritySnapshotV1>,
    ) -> Result<Self, AuthorityShapeErrorV1> {
        if capability.principal_id != principal.principal_id {
            return Err(AuthorityShapeErrorV1::CapabilityPrincipalMismatch);
        }
        Ok(Self {
            capability,
            principal,
            membership,
            runner,
        })
    }

    #[must_use]
    pub const fn capability(&self) -> &CapabilityAuthoritySnapshotV1 {
        &self.capability
    }

    #[must_use]
    pub const fn principal(&self) -> &PrincipalAuthoritySnapshotV1 {
        &self.principal
    }

    #[must_use]
    pub const fn membership(&self) -> Option<&MembershipAuthoritySnapshotV1> {
        self.membership.as_ref()
    }

    #[must_use]
    pub const fn runner(&self) -> Option<&RunnerAuthoritySnapshotV1> {
        self.runner.as_ref()
    }
}

/// Exact optional facts one authority decision asks an Adapter to snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthoritySnapshotQueryV1 {
    capability_id: CapabilityId,
    membership: Option<RoomMembershipKeyV1>,
    runner_id: Option<RunnerId>,
}

impl AuthoritySnapshotQueryV1 {
    #[must_use]
    pub const fn capability_id(&self) -> &CapabilityId {
        &self.capability_id
    }

    #[must_use]
    pub const fn membership(&self) -> Option<&RoomMembershipKeyV1> {
        self.membership.as_ref()
    }

    #[must_use]
    pub const fn runner_id(&self) -> Option<&RunnerId> {
        self.runner_id.as_ref()
    }
}

/// Transaction-current target facts needed to validate one authority change.
/// The shape is redacted and bounded; no bearer material is present.
pub struct AuthorityChangeStatePartsV1 {
    pub principal: Option<PrincipalAuthoritySnapshotV1>,
    pub capability: Option<CapabilityAuthoritySnapshotV1>,
    pub membership: Option<MembershipAuthoritySnapshotV1>,
    pub runner: Option<RunnerAuthoritySnapshotV1>,
    pub runner_memberships: Vec<MembershipAuthoritySnapshotV1>,
}

/// Coherent target-side transaction snapshot for authority mutation policy.
#[derive(Clone, Eq, PartialEq)]
pub struct AuthorityChangeStateV1 {
    principal: Option<PrincipalAuthoritySnapshotV1>,
    capability: Option<CapabilityAuthoritySnapshotV1>,
    membership: Option<MembershipAuthoritySnapshotV1>,
    runner: Option<RunnerAuthoritySnapshotV1>,
    runner_memberships: Vec<MembershipAuthoritySnapshotV1>,
}

impl fmt::Debug for AuthorityChangeStateV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AuthorityChangeStateV1([REDACTED])")
    }
}

impl AuthorityChangeStateV1 {
    /// Bounds a transaction-current target snapshot before validation.
    ///
    /// # Errors
    ///
    /// Returns an error for an oversized or duplicate Runner Membership set.
    pub fn new(parts: AuthorityChangeStatePartsV1) -> Result<Self, AuthorityShapeErrorV1> {
        if parts.runner_memberships.len() > MAX_RUNNER_MEMBERSHIPS {
            return Err(AuthorityShapeErrorV1::TooManyRunnerMemberships);
        }
        let mut addresses = BTreeSet::new();
        for membership in &parts.runner_memberships {
            let key = RoomMembershipKeyV1 {
                room_id: membership.room_id().clone(),
                member_id: membership.membership().member_id().clone(),
            };
            if !addresses.insert(key) {
                return Err(AuthorityShapeErrorV1::DuplicateRunnerMembership);
            }
        }
        Ok(Self {
            principal: parts.principal,
            capability: parts.capability,
            membership: parts.membership,
            runner: parts.runner,
            runner_memberships: parts.runner_memberships,
        })
    }
}

/// Core-validated full target replacement for one durable authority change.
#[derive(Clone, Eq, PartialEq)]
pub enum ValidatedAuthorityChangeV1 {
    Principal {
        result: AuthorityChangeResultV1,
        principal: PrincipalAuthoritySnapshotV1,
    },
    Capability {
        result: AuthorityChangeResultV1,
        capability: CapabilityAuthoritySnapshotV1,
    },
    Runner {
        result: AuthorityChangeResultV1,
        runner: RunnerAuthoritySnapshotV1,
    },
}

impl fmt::Debug for ValidatedAuthorityChangeV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Principal { .. } => "ValidatedAuthorityChangeV1::Principal([REDACTED])",
            Self::Capability { .. } => "ValidatedAuthorityChangeV1::Capability([REDACTED])",
            Self::Runner { .. } => "ValidatedAuthorityChangeV1::Runner([REDACTED])",
        })
    }
}

impl ValidatedAuthorityChangeV1 {
    /// Returns the audited result family.
    #[must_use]
    pub const fn result(&self) -> AuthorityChangeResultV1 {
        match self {
            Self::Principal { result, .. }
            | Self::Capability { result, .. }
            | Self::Runner { result, .. } => *result,
        }
    }

    /// Returns a full Principal replacement when this is a Principal change.
    #[must_use]
    pub const fn principal(&self) -> Option<&PrincipalAuthoritySnapshotV1> {
        match self {
            Self::Principal { principal, .. } => Some(principal),
            Self::Capability { .. } | Self::Runner { .. } => None,
        }
    }

    /// Returns a full Capability replacement when this is a Capability change.
    #[must_use]
    pub const fn capability(&self) -> Option<&CapabilityAuthoritySnapshotV1> {
        match self {
            Self::Capability { capability, .. } => Some(capability),
            Self::Principal { .. } | Self::Runner { .. } => None,
        }
    }

    /// Returns a full Runner replacement when this is a Runner change.
    #[must_use]
    pub const fn runner(&self) -> Option<&RunnerAuthoritySnapshotV1> {
        match self {
            Self::Runner { runner, .. } => Some(runner),
            Self::Principal { .. } | Self::Capability { .. } => None,
        }
    }

    /// Returns the validated semantic-change generation.
    #[must_use]
    pub const fn resulting_generation(&self) -> u64 {
        match self {
            Self::Principal { principal, .. } => principal.generation().get(),
            Self::Capability { capability, .. } => capability.generation().get(),
            Self::Runner { runner, .. } => runner.generation().get(),
        }
    }
}

/// Failure at the authority storage seam. No sensitive detail is carried.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum AuthorityStoreErrorV1 {
    #[error("authority storage is unavailable")]
    Unavailable,
    #[error("authority change identity conflicts with another request")]
    Conflict,
    #[error("authority generation changed")]
    StaleGeneration,
    #[error("stored authority facts are invalid")]
    Corrupt,
    #[error("authority change is invalid")]
    InvalidChange,
}

/// Internal persistence seam implemented by durable and in-memory Adapters.
pub trait AuthorityStoreV1: Send + Sync {
    /// Loads one coherent current snapshot.
    ///
    /// # Errors
    ///
    /// Returns a safe Adapter failure when current facts cannot be read or
    /// reconstructed coherently.
    fn snapshot(
        &self,
        query: &AuthoritySnapshotQueryV1,
    ) -> Result<Option<AuthoritySnapshotV1>, AuthorityStoreErrorV1>;

    /// Atomically installs the sole bootstrap Principal and Capability.
    ///
    /// The Adapter must first return an already durable receipt only when its
    /// change identity and request hash match. Otherwise it must report a
    /// conflict for a reused identity or any nonempty authority store. A fresh
    /// install samples its Adapter-owned trusted clock, validates
    /// [`AuthorityBootstrapStateV1::Empty`], writes both generation-one
    /// replacements, and records their receipt in one durable transaction.
    ///
    /// # Errors
    ///
    /// Returns conflict, invalid-change, corrupt, or unavailable without
    /// exposing bootstrap material.
    fn apply_bootstrap(
        &self,
        bootstrap: &PreparedAuthorityBootstrapV1,
    ) -> Result<AuthorityChangeReceiptV1, AuthorityStoreErrorV1>;

    /// Atomically revalidates and applies one prepared authority change. The
    /// Adapter must load the actor under its transaction guard, sample its own
    /// trusted clock there, and pass that sample to actor and target validation;
    /// no caller-supplied value is authoritative commit time. Capability
    /// registration must also reject a token hash already assigned to any
    /// other Capability.
    ///
    /// # Errors
    ///
    /// Returns conflict, stale-generation, invalid-change, corrupt, or
    /// unavailable without exposing sensitive stored facts.
    fn apply_change(
        &self,
        change: &PreparedAuthorityChangeV1,
    ) -> Result<AuthorityChangeReceiptV1, AuthorityStoreErrorV1>;
}

/// Closed shape failures caught before any authority grant exists.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum AuthorityShapeErrorV1 {
    #[error("Capability scope set is empty")]
    EmptyScopes,
    #[error("Capability scope input repeats a scope")]
    DuplicateScope,
    #[error("Capability scope set exceeds its bound")]
    TooManyScopes,
    #[error("Runner Membership set is empty")]
    EmptyRunnerMemberships,
    #[error("Runner Membership input repeats an address")]
    DuplicateRunnerMembership,
    #[error("Runner Membership set exceeds its bound")]
    TooManyRunnerMemberships,
    #[error("Capability scope is incompatible with its profile")]
    IncompatibleScopeProfile,
    #[error("Capability and Principal identities disagree")]
    CapabilityPrincipalMismatch,
    #[error("authority reason code is invalid")]
    InvalidReasonCode,
}

/// Member-facing uses whose Access Mode and Standing are derived from Core.
#[derive(Clone, Eq, PartialEq, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum MemberAuthorityUseV1 {
    Attach,
    CurrentProjection,
    CatchUp,
    AcknowledgeObservation,
    SubmitAction {
        identity: ParticipantActionOperationIdentityV1,
        request_hash: CanonicalRequestHashV1,
        action_type: String,
    },
}

/// Closed nonmutating Member operation accepted by the normal caller facade.
/// Participant Action authorization is deliberately absent because it must use
/// the receipt-first Action ingress coordinator.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MemberReadOperationV1 {
    Attach,
    CurrentProjection,
    CatchUp,
    AcknowledgeObservation,
}

impl From<MemberReadOperationV1> for MemberAuthorityUseV1 {
    fn from(operation: MemberReadOperationV1) -> Self {
        match operation {
            MemberReadOperationV1::Attach => Self::Attach,
            MemberReadOperationV1::CurrentProjection => Self::CurrentProjection,
            MemberReadOperationV1::CatchUp => Self::CatchUp,
            MemberReadOperationV1::AcknowledgeObservation => Self::AcknowledgeObservation,
        }
    }
}

impl fmt::Debug for MemberAuthorityUseV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Attach => "MemberAuthorityUseV1::Attach",
            Self::CurrentProjection => "MemberAuthorityUseV1::CurrentProjection",
            Self::CatchUp => "MemberAuthorityUseV1::CatchUp",
            Self::AcknowledgeObservation => "MemberAuthorityUseV1::AcknowledgeObservation",
            Self::SubmitAction { .. } => "MemberAuthorityUseV1::SubmitAction([REDACTED])",
        })
    }
}

/// Whether one Core proposal is pack-vetoable or mandatory.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CoreAdministrationClassV1 {
    Vetoable,
    Mandatory,
}

/// Core-derived administration shape. Callers cannot self-declare this class.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ClassifiedCoreAdministrationV1 {
    kind: CoreProposedKindV1,
    class: CoreAdministrationClassV1,
    affected_memberships: u16,
}

impl ClassifiedCoreAdministrationV1 {
    #[allow(dead_code)]
    pub(crate) const fn new(
        kind: CoreProposedKindV1,
        class: CoreAdministrationClassV1,
        affected_memberships: u16,
    ) -> Self {
        Self {
            kind,
            class,
            affected_memberships,
        }
    }

    #[must_use]
    pub const fn kind(&self) -> CoreProposedKindV1 {
        self.kind
    }

    #[must_use]
    pub const fn class(&self) -> CoreAdministrationClassV1 {
        self.class
    }

    #[must_use]
    pub const fn affected_memberships(&self) -> u16 {
        self.affected_memberships
    }
}

/// Presently requested Replay projection policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayProjectionKindV1 {
    HistoricalMembership,
    FinalReveal,
}

/// Closed host-diagnostic target.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "target", content = "room_id", rename_all = "snake_case")]
pub enum DiagnosticTargetV1 {
    Deployment,
    Room(RoomId),
}

/// Closed diagnostic operations; there is no generic state read.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticOperationV1 {
    SafeRoomSummary,
    ActivityPackCatalog,
    Verify,
    RawExport,
    Restore,
    Backup,
}

/// Closed Runner-control operations, separate from Participant Actions.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunnerControlOperationV1 {
    ReceiveOffer,
    Claim,
    Renew,
    Release,
    Complete,
}

/// Every purpose the authority Module can grant in v1.
#[derive(Clone, Eq, PartialEq, Serialize)]
#[serde(tag = "use", rename_all = "snake_case")]
pub enum AuthorityUseV1 {
    ReadRoomOperationResult {
        identity: OperationIdentityV1,
        request_hash: CanonicalRequestHashV1,
        target_room_id: Option<RoomId>,
    },
    Member {
        room_id: RoomId,
        member_id: MemberId,
        operation: MemberAuthorityUseV1,
    },
    CreateRoom {
        identity: AdministrationOperationIdentityV1,
        request_hash: CanonicalRequestHashV1,
    },
    CoreAdministration {
        room_id: RoomId,
        classified: ClassifiedCoreAdministrationV1,
        identity: AdministrationOperationIdentityV1,
        request_hash: CanonicalRequestHashV1,
    },
    TimerFired {
        room_id: RoomId,
        request_hash: CanonicalRequestHashV1,
    },
    ExternalInput {
        room_id: RoomId,
        request_hash: CanonicalRequestHashV1,
    },
    Replay {
        room_id: RoomId,
        member_id: MemberId,
        at_room_seq: RoomSequenceV1,
        projection_kind: ReplayProjectionKindV1,
    },
    HostDiagnostic {
        target: DiagnosticTargetV1,
        operation: DiagnosticOperationV1,
    },
    RunnerControl {
        runner_id: RunnerId,
        operation: RunnerControlOperationV1,
        target: Option<RoomMembershipKeyV1>,
    },
}

impl fmt::Debug for AuthorityUseV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ReadRoomOperationResult { .. } => {
                "AuthorityUseV1::ReadRoomOperationResult([REDACTED])"
            }
            Self::Member { operation, .. } => match operation {
                MemberAuthorityUseV1::Attach => "AuthorityUseV1::Member(Attach)",
                MemberAuthorityUseV1::CurrentProjection => {
                    "AuthorityUseV1::Member(CurrentProjection)"
                }
                MemberAuthorityUseV1::CatchUp => "AuthorityUseV1::Member(CatchUp)",
                MemberAuthorityUseV1::AcknowledgeObservation => {
                    "AuthorityUseV1::Member(AcknowledgeObservation)"
                }
                MemberAuthorityUseV1::SubmitAction { .. } => {
                    "AuthorityUseV1::Member(SubmitAction([REDACTED]))"
                }
            },
            Self::CreateRoom { .. } => "AuthorityUseV1::CreateRoom([REDACTED])",
            Self::CoreAdministration { .. } => "AuthorityUseV1::CoreAdministration([REDACTED])",
            Self::TimerFired { .. } => "AuthorityUseV1::TimerFired([REDACTED])",
            Self::ExternalInput { .. } => "AuthorityUseV1::ExternalInput([REDACTED])",
            Self::Replay { .. } => "AuthorityUseV1::Replay([REDACTED])",
            Self::HostDiagnostic { .. } => "AuthorityUseV1::HostDiagnostic([REDACTED])",
            Self::RunnerControl { .. } => "AuthorityUseV1::RunnerControl([REDACTED])",
        })
    }
}

impl AuthorityUseV1 {
    fn query(&self, capability_id: CapabilityId) -> AuthoritySnapshotQueryV1 {
        let membership = self.membership_query();
        let runner_id = match self {
            Self::RunnerControl { runner_id, .. } => Some(runner_id.clone()),
            _ => None,
        };
        AuthoritySnapshotQueryV1 {
            capability_id,
            membership,
            runner_id,
        }
    }

    fn membership_query(&self) -> Option<RoomMembershipKeyV1> {
        match self {
            Self::Member {
                room_id, member_id, ..
            }
            | Self::Replay {
                room_id, member_id, ..
            } => Some(RoomMembershipKeyV1 {
                room_id: room_id.clone(),
                member_id: member_id.clone(),
            }),
            Self::ReadRoomOperationResult { identity, .. } => match identity {
                OperationIdentityV1::ParticipantAction(identity) => Some(RoomMembershipKeyV1 {
                    room_id: identity.room_id.clone(),
                    member_id: identity.member_id.clone(),
                }),
                _ => None,
            },
            Self::RunnerControl { target, .. } => target.clone(),
            Self::CreateRoom { .. }
            | Self::CoreAdministration { .. }
            | Self::TimerFired { .. }
            | Self::ExternalInput { .. }
            | Self::HostDiagnostic { .. } => None,
        }
    }
}

#[derive(Clone, Eq, PartialEq)]
pub(crate) struct AuthorityFenceFactsV1 {
    capability_id: CapabilityId,
    authenticated_principal: PrincipalId,
    principal_generation: PrincipalGenerationV1,
    authority_generation: AuthorityGenerationV1,
    canonical_scope_revocation_bytes: Vec<u8>,
    scope_revocation_hash: Blake3DigestV1,
    purpose_hash: Blake3DigestV1,
    authorized_at: AuthorityCheckedAt,
    expires_at: Option<CapabilityExpiresAt>,
    membership: Option<(RoomMembershipKeyV1, MembershipGenerationV1)>,
    runner: Option<(RunnerId, RunnerGenerationV1)>,
}

impl fmt::Debug for AuthorityFenceFactsV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthorityFenceFactsV1")
            .field("capability_id", &self.capability_id)
            .field("authenticated_principal", &self.authenticated_principal)
            .field("principal_generation", &self.principal_generation)
            .field("authority_generation", &self.authority_generation)
            .field("canonical_scope_revocation_bytes", &"[REDACTED]")
            .field("scope_revocation_hash", &"[REDACTED]")
            .field("purpose_hash", &"[REDACTED]")
            .field("authorized_at", &self.authorized_at)
            .field("expires_at", &self.expires_at)
            .field("membership_fenced", &self.membership.is_some())
            .field("runner_fenced", &self.runner.is_some())
            .finish()
    }
}

impl AuthorityFenceFactsV1 {
    pub(crate) fn snapshot_query(&self) -> AuthoritySnapshotQueryV1 {
        AuthoritySnapshotQueryV1 {
            capability_id: self.capability_id.clone(),
            membership: self.membership.as_ref().map(|(key, _)| key.clone()),
            runner_id: self.runner.as_ref().map(|(runner_id, _)| runner_id.clone()),
        }
    }

    pub(crate) const fn capability_id(&self) -> &CapabilityId {
        &self.capability_id
    }

    pub(crate) const fn authenticated_principal(&self) -> &PrincipalId {
        &self.authenticated_principal
    }

    pub(crate) const fn principal_generation(&self) -> PrincipalGenerationV1 {
        self.principal_generation
    }

    pub(crate) const fn authority_generation(&self) -> AuthorityGenerationV1 {
        self.authority_generation
    }

    pub(crate) fn canonical_scope_revocation_bytes(&self) -> &[u8] {
        &self.canonical_scope_revocation_bytes
    }

    pub(crate) const fn scope_revocation_hash(&self) -> &Blake3DigestV1 {
        &self.scope_revocation_hash
    }

    pub(crate) const fn purpose_hash(&self) -> &Blake3DigestV1 {
        &self.purpose_hash
    }

    pub(crate) const fn authorized_at(&self) -> &AuthorityCheckedAt {
        &self.authorized_at
    }

    pub(crate) const fn expires_at(&self) -> Option<&CapabilityExpiresAt> {
        self.expires_at.as_ref()
    }

    #[allow(dead_code)]
    pub(crate) const fn membership(
        &self,
    ) -> Option<&(RoomMembershipKeyV1, MembershipGenerationV1)> {
        self.membership.as_ref()
    }

    #[allow(dead_code)]
    pub(crate) const fn runner(&self) -> Option<&(RunnerId, RunnerGenerationV1)> {
        self.runner.as_ref()
    }

    pub(crate) fn binds_use(&self, use_: &AuthorityUseV1) -> bool {
        authority_purpose_bytes(use_)
            .is_ok_and(|bytes| Blake3DigestV1::hash(&bytes) == self.purpose_hash)
    }

    pub(crate) fn revalidate_current(
        &self,
        snapshot: &AuthoritySnapshotV1,
        checked_at: &AuthorityCheckedAt,
    ) -> Result<(), AuthorityErrorV1> {
        if compare_timestamp_text(checked_at.as_str(), self.authorized_at.as_str()).is_lt() {
            return Err(AuthorityErrorV1::InvalidAuthorityRequest);
        }
        let capability = snapshot.capability();
        capability
            .profile()
            .validate_scopes(capability.scopes())
            .map_err(|_| AuthorityErrorV1::Unavailable)?;
        if snapshot.principal().status() != PrincipalAuthorityStatusV1::Enabled
            || capability.revoked_at().is_some()
            || is_expired(capability.expires_at(), checked_at)
            || snapshot
                .runner()
                .is_some_and(|runner| runner.status() != RunnerAuthorityStatusV1::Enabled)
        {
            return Err(AuthorityErrorV1::StaleAuthorityGeneration);
        }
        let current = build_fence_with_purpose_hash(
            snapshot,
            snapshot.membership(),
            snapshot.runner(),
            self.purpose_hash.clone(),
            self.authorized_at.clone(),
        )?;
        if &current == self {
            Ok(())
        } else {
            Err(AuthorityErrorV1::StaleAuthorityGeneration)
        }
    }
}

macro_rules! opaque_grant_debug {
    ($name:ident) => {
        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(concat!(stringify!($name), "([OPAQUE])"))
            }
        }
    };
}

/// Present authority to read one exact stored Room operation result.
pub struct AuthorizedReceiptReadV1 {
    fence: AuthorityFenceFactsV1,
    #[allow(dead_code)]
    identity: OperationIdentityV1,
    #[allow(dead_code)]
    request_hash: CanonicalRequestHashV1,
    #[allow(dead_code)]
    target_policy: ReceiptReadTargetPolicyV1,
}
opaque_grant_debug!(AuthorizedReceiptReadV1);

/// Exact Room selection policy sealed into an authorized receipt read.
#[derive(Clone, Eq, PartialEq)]
pub(crate) enum ReceiptReadTargetPolicyV1 {
    ExactRoom(RoomId),
    GlobalCreate,
}

impl fmt::Debug for ReceiptReadTargetPolicyV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ExactRoom(_) => "ReceiptReadTargetPolicyV1::ExactRoom([REDACTED])",
            Self::GlobalCreate => "ReceiptReadTargetPolicyV1::GlobalCreate",
        })
    }
}

/// Trusted-Adapter input obtained by consuming one receipt-read grant.
///
/// This value is not a second authorization grant. An Adapter must reread and
/// revalidate the sealed fence at its own trusted clock sample in the same
/// serialized transaction that performs the guarded receipt lookup.
#[doc(hidden)]
pub struct ReceiptReadAdapterInputV1 {
    pub(crate) fence: AuthorityFenceFactsV1,
    pub(crate) identity: OperationIdentityV1,
    pub(crate) request_hash: CanonicalRequestHashV1,
    pub(crate) target_policy: ReceiptReadTargetPolicyV1,
}

opaque_grant_debug!(ReceiptReadAdapterInputV1);

impl AuthorizedReceiptReadV1 {
    #[must_use]
    pub const fn authenticated_principal(&self) -> &PrincipalId {
        self.fence.authenticated_principal()
    }

    /// Transfers this grant into the trusted Adapter SPI. Merely extracting
    /// this value does not establish current authorization.
    #[doc(hidden)]
    #[must_use]
    pub fn into_adapter_input(self) -> ReceiptReadAdapterInputV1 {
        ReceiptReadAdapterInputV1 {
            fence: self.fence,
            identity: self.identity,
            request_hash: self.request_hash,
            target_policy: self.target_policy,
        }
    }
}

impl ReceiptReadAdapterInputV1 {
    /// Returns the exact authority rows the Adapter must reread inside the
    /// guarded receipt transaction.
    #[doc(hidden)]
    #[must_use]
    pub fn authority_snapshot_query(&self) -> AuthoritySnapshotQueryV1 {
        self.fence.snapshot_query()
    }

    /// Checks the sealed fence against an Adapter-owned current snapshot and
    /// trusted clock sample. Success must not be cached across transactions.
    ///
    /// # Errors
    ///
    /// Returns a safe authority failure if any fenced fact, revocation,
    /// expiry, or generation changed.
    #[doc(hidden)]
    pub fn revalidate_current(
        &self,
        snapshot: &AuthoritySnapshotV1,
        checked_at: &AuthorityCheckedAt,
    ) -> Result<(), AuthorityErrorV1> {
        self.fence.revalidate_current(snapshot, checked_at)
    }
}

/// Present authority for one enabled Membership read/delivery operation.
pub struct AuthorizedViewerV1 {
    fence: AuthorityFenceFactsV1,
    room_id: RoomId,
    membership: MembershipV1,
    operation: MemberReadOperationV1,
}
opaque_grant_debug!(AuthorizedViewerV1);

/// Trusted-Adapter input obtained by consuming one Member-read grant.
#[doc(hidden)]
pub struct ViewerAdapterInputV1 {
    fence: AuthorityFenceFactsV1,
    room_id: RoomId,
    membership: MembershipV1,
    operation: MemberReadOperationV1,
}
opaque_grant_debug!(ViewerAdapterInputV1);

impl AuthorizedViewerV1 {
    #[must_use]
    pub const fn membership(&self) -> &MembershipV1 {
        &self.membership
    }

    #[must_use]
    pub const fn room_id(&self) -> &RoomId {
        &self.room_id
    }

    #[must_use]
    pub const fn operation(&self) -> MemberReadOperationV1 {
        self.operation
    }

    /// Transfers this grant into a trusted projection/delivery Adapter. Merely
    /// extracting it does not establish current authorization.
    #[doc(hidden)]
    #[must_use]
    pub fn into_adapter_input(self) -> ViewerAdapterInputV1 {
        ViewerAdapterInputV1 {
            fence: self.fence,
            room_id: self.room_id,
            membership: self.membership,
            operation: self.operation,
        }
    }
}

impl ViewerAdapterInputV1 {
    #[doc(hidden)]
    #[must_use]
    pub fn authority_snapshot_query(&self) -> AuthoritySnapshotQueryV1 {
        self.fence.snapshot_query()
    }

    /// # Errors
    ///
    /// Returns a safe authority failure if any current fenced fact changed.
    #[doc(hidden)]
    pub fn revalidate_current(
        &self,
        snapshot: &AuthoritySnapshotV1,
        checked_at: &AuthorityCheckedAt,
    ) -> Result<(), AuthorityErrorV1> {
        self.fence.revalidate_current(snapshot, checked_at)
    }

    #[must_use]
    pub const fn room_id(&self) -> &RoomId {
        &self.room_id
    }

    #[must_use]
    pub const fn membership(&self) -> &MembershipV1 {
        &self.membership
    }

    #[must_use]
    pub const fn operation(&self) -> MemberReadOperationV1 {
        self.operation
    }
}

/// Present authority for an enabled Participant Action path.
pub struct AuthorizedParticipantActionV1 {
    fence: AuthorityFenceFactsV1,
    membership: MembershipV1,
}
opaque_grant_debug!(AuthorizedParticipantActionV1);

impl AuthorizedParticipantActionV1 {
    #[must_use]
    pub const fn membership(&self) -> &MembershipV1 {
        &self.membership
    }

    pub(crate) fn into_fence_facts(self) -> AuthorityFenceFactsV1 {
        self.fence
    }
}

/// Present authority for a stable no-Transition Action disposition.
pub struct AuthorizedStableActionDispositionV1 {
    fence: AuthorityFenceFactsV1,
    membership: MembershipV1,
}
opaque_grant_debug!(AuthorizedStableActionDispositionV1);

impl AuthorizedStableActionDispositionV1 {
    #[must_use]
    pub const fn membership(&self) -> &MembershipV1 {
        &self.membership
    }

    pub(crate) fn into_fence_facts(self) -> AuthorityFenceFactsV1 {
        self.fence
    }
}

/// Purpose-sealed Action authority, including stable disabled-Membership paths.
#[derive(Debug)]
pub enum ParticipantActionAuthorityV1 {
    EnabledParticipant(AuthorizedParticipantActionV1),
    StableMembershipNotEnabled(AuthorizedStableActionDispositionV1),
}

/// Present creation authority; it carries no Room Membership.
pub struct AuthorizedRoomCreationV1 {
    fence: AuthorityFenceFactsV1,
    attribution: CoreAuthorityAttributionV1,
}
opaque_grant_debug!(AuthorizedRoomCreationV1);

impl AuthorizedRoomCreationV1 {
    #[must_use]
    pub const fn attribution(&self) -> &CoreAuthorityAttributionV1 {
        &self.attribution
    }

    pub(crate) fn into_fence_facts(self) -> AuthorityFenceFactsV1 {
        self.fence
    }
}

/// Present host authority for one already-classified Core proposal.
pub struct AuthorizedCoreAdministrationV1 {
    #[allow(dead_code)]
    fence: AuthorityFenceFactsV1,
    attribution: CoreAuthorityAttributionV1,
    classified: ClassifiedCoreAdministrationV1,
}
opaque_grant_debug!(AuthorizedCoreAdministrationV1);

impl AuthorizedCoreAdministrationV1 {
    #[must_use]
    pub const fn attribution(&self) -> &CoreAuthorityAttributionV1 {
        &self.attribution
    }

    #[must_use]
    pub const fn classified(&self) -> &ClassifiedCoreAdministrationV1 {
        &self.classified
    }

    #[allow(dead_code)]
    pub(crate) fn into_fence_facts(self) -> AuthorityFenceFactsV1 {
        self.fence
    }
}

/// Present `HostOperator` authority for one exact Timer firing request.
pub struct AuthorizedTimerFiredV1 {
    fence: AuthorityFenceFactsV1,
    room_id: RoomId,
    request_hash: CanonicalRequestHashV1,
}
opaque_grant_debug!(AuthorizedTimerFiredV1);

impl AuthorizedTimerFiredV1 {
    pub(crate) const fn room_id(&self) -> &RoomId {
        &self.room_id
    }

    pub(crate) const fn request_hash(&self) -> &CanonicalRequestHashV1 {
        &self.request_hash
    }

    pub(crate) fn into_fence_facts(self) -> AuthorityFenceFactsV1 {
        self.fence
    }
}

/// Present `HostOperator` authority for one exact `ExternalInput` request.
pub struct AuthorizedExternalInputV1 {
    fence: AuthorityFenceFactsV1,
    room_id: RoomId,
    request_hash: CanonicalRequestHashV1,
}
opaque_grant_debug!(AuthorizedExternalInputV1);

impl AuthorizedExternalInputV1 {
    pub(crate) const fn room_id(&self) -> &RoomId {
        &self.room_id
    }

    pub(crate) const fn request_hash(&self) -> &CanonicalRequestHashV1 {
        &self.request_hash
    }

    pub(crate) fn into_fence_facts(self) -> AuthorityFenceFactsV1 {
        self.fence
    }
}

/// Present gate plus current Membership; historical Replay still reauthorizes N.
pub struct AuthorizedReplayV1 {
    #[allow(dead_code)]
    fence: AuthorityFenceFactsV1,
    room_id: RoomId,
    member_id: MemberId,
    at_room_seq: RoomSequenceV1,
    membership: MembershipV1,
    projection_kind: ReplayProjectionKindV1,
}
opaque_grant_debug!(AuthorizedReplayV1);

/// Trusted-Adapter input obtained by consuming one Replay grant.
///
/// The Adapter must check its sealed current-authority fence when capturing an
/// immutable upper bound and again immediately before releasing the result.
/// History between those short serialized boundaries is read in bounded pages.
/// This value is not itself proof that either check has occurred.
#[doc(hidden)]
pub struct ReplayAdapterInputV1 {
    pub(crate) fence: AuthorityFenceFactsV1,
    room_id: RoomId,
    member_id: MemberId,
    at_room_seq: RoomSequenceV1,
    projection_kind: ReplayProjectionKindV1,
}
opaque_grant_debug!(ReplayAdapterInputV1);

impl AuthorizedReplayV1 {
    #[must_use]
    pub const fn room_id(&self) -> &RoomId {
        &self.room_id
    }

    #[must_use]
    pub const fn member_id(&self) -> &MemberId {
        &self.member_id
    }

    #[must_use]
    pub const fn at_room_seq(&self) -> RoomSequenceV1 {
        self.at_room_seq
    }

    #[must_use]
    pub const fn membership(&self) -> &MembershipV1 {
        &self.membership
    }

    #[must_use]
    pub const fn projection_kind(&self) -> ReplayProjectionKindV1 {
        self.projection_kind
    }

    /// Transfers this grant into the trusted Adapter SPI. Merely extracting
    /// this value does not establish current authorization.
    #[doc(hidden)]
    #[must_use]
    pub fn into_adapter_input(self) -> ReplayAdapterInputV1 {
        ReplayAdapterInputV1 {
            fence: self.fence,
            room_id: self.room_id,
            member_id: self.member_id,
            at_room_seq: self.at_room_seq,
            projection_kind: self.projection_kind,
        }
    }
}

impl ReplayAdapterInputV1 {
    /// Returns the exact authority rows the Adapter must reread inside the
    /// guarded history-capture transaction.
    #[doc(hidden)]
    #[must_use]
    pub fn authority_snapshot_query(&self) -> AuthoritySnapshotQueryV1 {
        self.fence.snapshot_query()
    }

    /// Checks the sealed fence against an Adapter-owned current snapshot and
    /// trusted clock sample. Success must not be cached across transactions.
    ///
    /// # Errors
    ///
    /// Returns a safe authority failure if any fenced fact, revocation,
    /// expiry, or generation changed.
    #[doc(hidden)]
    pub fn revalidate_current(
        &self,
        snapshot: &AuthoritySnapshotV1,
        checked_at: &AuthorityCheckedAt,
    ) -> Result<(), AuthorityErrorV1> {
        self.fence.revalidate_current(snapshot, checked_at)
    }

    #[must_use]
    pub const fn room_id(&self) -> &RoomId {
        &self.room_id
    }

    #[must_use]
    pub const fn member_id(&self) -> &MemberId {
        &self.member_id
    }

    #[must_use]
    pub const fn at_room_seq(&self) -> RoomSequenceV1 {
        self.at_room_seq
    }

    #[must_use]
    pub const fn projection_kind(&self) -> ReplayProjectionKindV1 {
        self.projection_kind
    }

    /// Derives the pure deterministic projection request while retaining the
    /// sealed Adapter input for mandatory final authority revalidation.
    ///
    /// The returned request is not a present-authorization result. A trusted
    /// Adapter must recheck this input at its final release boundary.
    #[doc(hidden)]
    #[must_use]
    pub fn projection_request(
        &self,
        integrity: crate::RoomIntegrityStateV1,
    ) -> crate::HistoricalReplayProjectionRequestV1 {
        crate::HistoricalReplayProjectionRequestV1::new(
            self.room_id.clone(),
            self.member_id.clone(),
            self.at_room_seq,
            self.projection_kind,
            integrity,
        )
    }
}

/// Present host authority for one explicit diagnostic operation.
pub struct AuthorizedDiagnosticV1 {
    fence: AuthorityFenceFactsV1,
    target: DiagnosticTargetV1,
    operation: DiagnosticOperationV1,
}
opaque_grant_debug!(AuthorizedDiagnosticV1);

/// Trusted-Adapter input obtained by consuming one diagnostic grant.
#[doc(hidden)]
pub struct DiagnosticAdapterInputV1 {
    fence: AuthorityFenceFactsV1,
    target: DiagnosticTargetV1,
    operation: DiagnosticOperationV1,
}
opaque_grant_debug!(DiagnosticAdapterInputV1);

impl AuthorizedDiagnosticV1 {
    #[must_use]
    pub const fn target(&self) -> &DiagnosticTargetV1 {
        &self.target
    }

    #[must_use]
    pub const fn operation(&self) -> DiagnosticOperationV1 {
        self.operation
    }

    /// Transfers this grant into a trusted diagnostic Adapter. Merely
    /// extracting it does not establish current authorization.
    #[doc(hidden)]
    #[must_use]
    pub fn into_adapter_input(self) -> DiagnosticAdapterInputV1 {
        DiagnosticAdapterInputV1 {
            fence: self.fence,
            target: self.target,
            operation: self.operation,
        }
    }
}

impl DiagnosticAdapterInputV1 {
    #[doc(hidden)]
    #[must_use]
    pub fn authority_snapshot_query(&self) -> AuthoritySnapshotQueryV1 {
        self.fence.snapshot_query()
    }

    /// # Errors
    ///
    /// Returns a safe authority failure if any current fenced fact changed.
    #[doc(hidden)]
    pub fn revalidate_current(
        &self,
        snapshot: &AuthoritySnapshotV1,
        checked_at: &AuthorityCheckedAt,
    ) -> Result<(), AuthorityErrorV1> {
        self.fence.revalidate_current(snapshot, checked_at)
    }

    #[must_use]
    pub const fn target(&self) -> &DiagnosticTargetV1 {
        &self.target
    }

    #[must_use]
    pub const fn operation(&self) -> DiagnosticOperationV1 {
        self.operation
    }
}

/// Present Runner-control authority; it contains no Invocation Context.
pub struct AuthorizedRunnerControlV1 {
    fence: AuthorityFenceFactsV1,
    runner_id: RunnerId,
    operation: RunnerControlOperationV1,
    target: RoomMembershipKeyV1,
}
opaque_grant_debug!(AuthorizedRunnerControlV1);

/// Trusted-Adapter input obtained by consuming one Runner-control grant.
#[doc(hidden)]
pub struct RunnerControlAdapterInputV1 {
    fence: AuthorityFenceFactsV1,
    runner_id: RunnerId,
    operation: RunnerControlOperationV1,
    target: RoomMembershipKeyV1,
}
opaque_grant_debug!(RunnerControlAdapterInputV1);

impl AuthorizedRunnerControlV1 {
    #[must_use]
    pub const fn runner_id(&self) -> &RunnerId {
        &self.runner_id
    }

    #[must_use]
    pub const fn operation(&self) -> RunnerControlOperationV1 {
        self.operation
    }

    #[must_use]
    pub const fn target(&self) -> &RoomMembershipKeyV1 {
        &self.target
    }

    /// Returns the capability authority generation sealed into this grant.
    /// Adapters use it as a witness in the exact Invocation Context.
    #[must_use]
    pub const fn authority_generation(&self) -> AuthorityGenerationV1 {
        self.fence.authority_generation()
    }

    /// Transfers this grant into a trusted scheduler/activation Adapter.
    /// Merely extracting it does not establish current authorization.
    #[doc(hidden)]
    #[must_use]
    pub fn into_adapter_input(self) -> RunnerControlAdapterInputV1 {
        RunnerControlAdapterInputV1 {
            fence: self.fence,
            runner_id: self.runner_id,
            operation: self.operation,
            target: self.target,
        }
    }
}

impl RunnerControlAdapterInputV1 {
    #[doc(hidden)]
    #[must_use]
    pub fn authority_snapshot_query(&self) -> AuthoritySnapshotQueryV1 {
        self.fence.snapshot_query()
    }

    /// # Errors
    ///
    /// Returns a safe authority failure if any Principal, Capability, Runner,
    /// target Membership, revocation, expiry, or generation fact changed.
    #[doc(hidden)]
    pub fn revalidate_current(
        &self,
        snapshot: &AuthoritySnapshotV1,
        checked_at: &AuthorityCheckedAt,
    ) -> Result<(), AuthorityErrorV1> {
        self.fence.revalidate_current(snapshot, checked_at)
    }

    #[must_use]
    pub const fn runner_id(&self) -> &RunnerId {
        &self.runner_id
    }

    #[must_use]
    pub const fn operation(&self) -> RunnerControlOperationV1 {
        self.operation
    }

    #[must_use]
    pub const fn target(&self) -> &RoomMembershipKeyV1 {
        &self.target
    }
}

/// Exactly one purpose-sealed successful authority result.
pub enum AuthorityGrantV1 {
    ReceiptRead(AuthorizedReceiptReadV1),
    MemberRead(AuthorizedViewerV1),
    ParticipantAction(ParticipantActionAuthorityV1),
    RoomCreation(AuthorizedRoomCreationV1),
    CoreAdministration(AuthorizedCoreAdministrationV1),
    TimerFired(AuthorizedTimerFiredV1),
    ExternalInput(AuthorizedExternalInputV1),
    Replay(AuthorizedReplayV1),
    Diagnostic(AuthorizedDiagnosticV1),
    RunnerControl(AuthorizedRunnerControlV1),
}

impl fmt::Debug for AuthorityGrantV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ReceiptRead(_) => "AuthorityGrantV1::ReceiptRead([OPAQUE])",
            Self::MemberRead(_) => "AuthorityGrantV1::MemberRead([OPAQUE])",
            Self::ParticipantAction(_) => "AuthorityGrantV1::ParticipantAction([OPAQUE])",
            Self::RoomCreation(_) => "AuthorityGrantV1::RoomCreation([OPAQUE])",
            Self::CoreAdministration(_) => "AuthorityGrantV1::CoreAdministration([OPAQUE])",
            Self::TimerFired(_) => "AuthorityGrantV1::TimerFired([OPAQUE])",
            Self::ExternalInput(_) => "AuthorityGrantV1::ExternalInput([OPAQUE])",
            Self::Replay(_) => "AuthorityGrantV1::Replay([OPAQUE])",
            Self::Diagnostic(_) => "AuthorityGrantV1::Diagnostic([OPAQUE])",
            Self::RunnerControl(_) => "AuthorityGrantV1::RunnerControl([OPAQUE])",
        })
    }
}

/// A bounded machine-readable operational authority reason.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct AuthorityReasonCodeV1(String);

impl AuthorityReasonCodeV1 {
    /// Validates one lower-case machine reason.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty, oversized, or noncanonical reason.
    pub fn new(value: impl Into<String>) -> Result<Self, AuthorityShapeErrorV1> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_REASON_CODE_BYTES
            || !value.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'.' | b'_' | b'-')
            })
        {
            return Err(AuthorityShapeErrorV1::InvalidReasonCode);
        }
        Ok(Self(value))
    }

    /// Returns the stable machine reason.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for AuthorityReasonCodeV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(de::Error::custom)
    }
}

/// One newly registered Capability. Plaintext bearer material is absent.
#[derive(Clone, Eq, PartialEq)]
pub struct NewCapabilityV1 {
    capability_id: CapabilityId,
    token_hash: CapabilityTokenHashV1,
    principal_id: PrincipalId,
    profile: CapabilityProfileV1,
    scopes: CapabilityScopeSetV1,
    expires_at: Option<CapabilityExpiresAt>,
}

impl NewCapabilityV1 {
    /// Validates the closed profile/scope family for a new generation-one Capability.
    ///
    /// # Errors
    ///
    /// Returns an error for an incompatible scope family.
    pub fn new(
        capability_id: CapabilityId,
        token_hash: CapabilityTokenHashV1,
        principal_id: PrincipalId,
        profile: CapabilityProfileV1,
        scopes: CapabilityScopeSetV1,
        expires_at: Option<CapabilityExpiresAt>,
    ) -> Result<Self, AuthorityShapeErrorV1> {
        profile.validate_scopes(&scopes)?;
        Ok(Self {
            capability_id,
            token_hash,
            principal_id,
            profile,
            scopes,
            expires_at,
        })
    }

    #[must_use]
    pub const fn capability_id(&self) -> &CapabilityId {
        &self.capability_id
    }

    #[must_use]
    pub const fn token_hash(&self) -> &CapabilityTokenHashV1 {
        &self.token_hash
    }

    #[must_use]
    pub const fn principal_id(&self) -> &PrincipalId {
        &self.principal_id
    }

    #[must_use]
    pub const fn profile(&self) -> &CapabilityProfileV1 {
        &self.profile
    }

    #[must_use]
    pub const fn scopes(&self) -> &CapabilityScopeSetV1 {
        &self.scopes
    }

    #[must_use]
    pub const fn expires_at(&self) -> Option<&CapabilityExpiresAt> {
        self.expires_at.as_ref()
    }
}

impl fmt::Debug for NewCapabilityV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("NewCapabilityV1([REDACTED])")
    }
}

/// The sole unauthenticated production bootstrap request.
///
/// Construction seals the Capability to the deployment-wide
/// `HostOperator` profile and exactly the `operator:room_admin` scope. The
/// bearer itself is generated outside Core and is never carried here.
#[derive(Clone, Eq, PartialEq)]
pub struct AuthorityBootstrapV1 {
    change_id: AuthorityChangeId,
    principal_kind: PrincipalKindV1,
    capability: NewCapabilityV1,
}

impl AuthorityBootstrapV1 {
    /// Constructs the one allowed bootstrap authority pair.
    ///
    /// # Errors
    ///
    /// Returns an error only if Core cannot construct its fixed nonempty scope
    /// set or fixed profile/scope pairing.
    pub fn new(
        change_id: AuthorityChangeId,
        principal_id: PrincipalId,
        principal_kind: PrincipalKindV1,
        capability_id: CapabilityId,
        token_hash: CapabilityTokenHashV1,
        expires_at: Option<CapabilityExpiresAt>,
    ) -> Result<Self, AuthorityShapeErrorV1> {
        let scopes = CapabilityScopeSetV1::new([CapabilityScopeV1::OperatorRoomAdmin])?;
        let capability = NewCapabilityV1::new(
            capability_id,
            token_hash,
            principal_id,
            CapabilityProfileV1::HostOperator { room_id: None },
            scopes,
            expires_at,
        )?;
        Ok(Self {
            change_id,
            principal_kind,
            capability,
        })
    }

    /// Returns the durable idempotency identity.
    #[must_use]
    pub const fn change_id(&self) -> &AuthorityChangeId {
        &self.change_id
    }

    /// Returns the first Principal identity.
    #[must_use]
    pub const fn principal_id(&self) -> &PrincipalId {
        self.capability.principal_id()
    }

    /// Returns the first Principal kind.
    #[must_use]
    pub const fn principal_kind(&self) -> PrincipalKindV1 {
        self.principal_kind
    }

    /// Returns the sealed generation-one Capability input.
    #[must_use]
    pub const fn capability(&self) -> &NewCapabilityV1 {
        &self.capability
    }

    fn request_hash(&self) -> Blake3DigestV1 {
        let mut bytes = Vec::new();
        push_hash_field(&mut bytes, b"worldstream/authority-bootstrap/v1");
        push_hash_field(&mut bytes, self.change_id.as_str().as_bytes());
        push_hash_field(&mut bytes, self.capability.principal_id.as_str().as_bytes());
        bytes.push(match self.principal_kind {
            PrincipalKindV1::Human => 0,
            PrincipalKindV1::Agent => 1,
        });
        push_hash_field(
            &mut bytes,
            self.capability.capability_id.as_str().as_bytes(),
        );
        push_hash_field(&mut bytes, self.capability.token_hash.storage_bytes());
        push_optional_time(&mut bytes, self.capability.expires_at.as_ref());
        Blake3DigestV1::hash(&bytes)
    }
}

impl fmt::Debug for AuthorityBootstrapV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AuthorityBootstrapV1([REDACTED])")
    }
}

/// Transaction-current occupancy supplied by an authority Adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthorityBootstrapStateV1 {
    /// No Principal, Capability, Runner, or authority-change receipt exists.
    Empty,
    /// At least one durable authority fact already exists.
    Occupied,
}

/// Prepared one-time bootstrap consumed atomically by an Adapter.
#[derive(Clone)]
pub struct PreparedAuthorityBootstrapV1 {
    bootstrap: AuthorityBootstrapV1,
    request_hash: Blake3DigestV1,
    prepared_at: AuthorityCheckedAt,
}

impl fmt::Debug for PreparedAuthorityBootstrapV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PreparedAuthorityBootstrapV1([REDACTED])")
    }
}

impl PreparedAuthorityBootstrapV1 {
    /// Returns the sealed bootstrap request.
    #[must_use]
    pub const fn bootstrap(&self) -> &AuthorityBootstrapV1 {
        &self.bootstrap
    }

    /// Returns the durable idempotency identity.
    #[must_use]
    pub const fn change_id(&self) -> &AuthorityChangeId {
        self.bootstrap.change_id()
    }

    /// Returns the hash that distinguishes replay from identity conflict.
    #[must_use]
    pub const fn request_hash(&self) -> &Blake3DigestV1 {
        &self.request_hash
    }

    /// Returns the trusted time at which the request was prepared.
    #[must_use]
    pub const fn prepared_at(&self) -> &AuthorityCheckedAt {
        &self.prepared_at
    }

    /// Validates the one-time target state and returns both complete durable
    /// generation-one replacements.
    ///
    /// # Errors
    ///
    /// Returns conflict when any authority state already exists. Returns an
    /// invalid change for backwards commit time or a Capability expired by
    /// the authoritative commit time.
    pub fn validate_install(
        &self,
        state: AuthorityBootstrapStateV1,
        commit_checked_at: &AuthorityCheckedAt,
    ) -> Result<ValidatedAuthorityBootstrapV1, AuthorityStoreErrorV1> {
        if compare_timestamp_text(commit_checked_at.as_str(), self.prepared_at.as_str()).is_lt() {
            return Err(AuthorityStoreErrorV1::InvalidChange);
        }
        if state == AuthorityBootstrapStateV1::Occupied {
            return Err(AuthorityStoreErrorV1::Conflict);
        }
        if is_expired(self.bootstrap.capability.expires_at(), commit_checked_at) {
            return Err(AuthorityStoreErrorV1::InvalidChange);
        }
        let principal_generation =
            PrincipalGenerationV1::new(1).map_err(|_| AuthorityStoreErrorV1::Corrupt)?;
        let authority_generation =
            AuthorityGenerationV1::new(1).map_err(|_| AuthorityStoreErrorV1::Corrupt)?;
        let principal = PrincipalAuthoritySnapshotV1::new(
            self.bootstrap.principal_id().clone(),
            self.bootstrap.principal_kind,
            PrincipalAuthorityStatusV1::Enabled,
            principal_generation,
        );
        let capability = CapabilityAuthoritySnapshotV1::new(CapabilityAuthoritySnapshotPartsV1 {
            capability_id: self.bootstrap.capability.capability_id().clone(),
            token_hash: self.bootstrap.capability.token_hash().clone(),
            principal_id: self.bootstrap.principal_id().clone(),
            profile: CapabilityProfileV1::HostOperator { room_id: None },
            scopes: CapabilityScopeSetV1::new([CapabilityScopeV1::OperatorRoomAdmin])
                .map_err(|_| AuthorityStoreErrorV1::Corrupt)?,
            generation: authority_generation,
            expires_at: self.bootstrap.capability.expires_at().cloned(),
            revoked_at: None,
        })
        .map_err(|_| AuthorityStoreErrorV1::Corrupt)?;
        Ok(ValidatedAuthorityBootstrapV1 {
            principal,
            capability,
        })
    }
}

/// Core-validated atomic bootstrap replacements.
#[derive(Clone, Eq, PartialEq)]
pub struct ValidatedAuthorityBootstrapV1 {
    principal: PrincipalAuthoritySnapshotV1,
    capability: CapabilityAuthoritySnapshotV1,
}

impl fmt::Debug for ValidatedAuthorityBootstrapV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ValidatedAuthorityBootstrapV1([REDACTED])")
    }
}

impl ValidatedAuthorityBootstrapV1 {
    /// Returns the enabled generation-one Principal replacement.
    #[must_use]
    pub const fn principal(&self) -> &PrincipalAuthoritySnapshotV1 {
        &self.principal
    }

    /// Returns the global `HostOperator` generation-one Capability replacement.
    #[must_use]
    pub const fn capability(&self) -> &CapabilityAuthoritySnapshotV1 {
        &self.capability
    }
}

/// Closed operational authority changes. Membership changes remain Core Stimuli.
#[derive(Clone, Eq, PartialEq)]
pub enum AuthorityChangeV1 {
    CreatePrincipal {
        change_id: AuthorityChangeId,
        principal_id: PrincipalId,
        kind: PrincipalKindV1,
    },
    RegisterCapability {
        change_id: AuthorityChangeId,
        capability: NewCapabilityV1,
    },
    RegisterRunner {
        change_id: AuthorityChangeId,
        runner_id: RunnerId,
        owner_principal_id: PrincipalId,
    },
    NarrowCapability {
        change_id: AuthorityChangeId,
        capability_id: CapabilityId,
        expected_generation: AuthorityGenerationV1,
        scopes: CapabilityScopeSetV1,
        expires_at: Option<CapabilityExpiresAt>,
        reason_code: AuthorityReasonCodeV1,
    },
    RevokeCapability {
        change_id: AuthorityChangeId,
        capability_id: CapabilityId,
        expected_generation: AuthorityGenerationV1,
        reason_code: AuthorityReasonCodeV1,
    },
    RevokeRunner {
        change_id: AuthorityChangeId,
        runner_id: RunnerId,
        expected_generation: RunnerGenerationV1,
        reason_code: AuthorityReasonCodeV1,
    },
    SetPrincipalStatus {
        change_id: AuthorityChangeId,
        principal_id: PrincipalId,
        expected_generation: PrincipalGenerationV1,
        status: PrincipalAuthorityStatusV1,
        reason_code: AuthorityReasonCodeV1,
    },
}

impl AuthorityChangeV1 {
    /// Returns the idempotent operational change identity.
    #[must_use]
    pub const fn change_id(&self) -> &AuthorityChangeId {
        match self {
            Self::CreatePrincipal { change_id, .. }
            | Self::RegisterCapability { change_id, .. }
            | Self::RegisterRunner { change_id, .. }
            | Self::NarrowCapability { change_id, .. }
            | Self::RevokeCapability { change_id, .. }
            | Self::RevokeRunner { change_id, .. }
            | Self::SetPrincipalStatus { change_id, .. } => change_id,
        }
    }

    fn request_hash(&self) -> Result<Blake3DigestV1, AuthorityErrorV1> {
        let mut bytes = Vec::new();
        push_hash_field(&mut bytes, b"worldstream/authority-change/v1");
        push_hash_field(&mut bytes, self.change_id().as_str().as_bytes());
        match self {
            Self::CreatePrincipal {
                principal_id, kind, ..
            } => {
                push_hash_field(&mut bytes, b"create_principal");
                push_hash_field(&mut bytes, principal_id.as_str().as_bytes());
                bytes.push(match kind {
                    PrincipalKindV1::Human => 0,
                    PrincipalKindV1::Agent => 1,
                });
            }
            Self::RegisterCapability { capability, .. } => {
                push_hash_field(&mut bytes, b"register_capability");
                push_hash_field(&mut bytes, capability.capability_id.as_str().as_bytes());
                push_hash_field(&mut bytes, capability.token_hash.storage_bytes());
                push_hash_field(&mut bytes, capability.principal_id.as_str().as_bytes());
                push_hash_field(
                    &mut bytes,
                    &encode(&capability.profile).map_err(|_| AuthorityErrorV1::Unavailable)?,
                );
                push_hash_field(
                    &mut bytes,
                    &encode(&capability.scopes).map_err(|_| AuthorityErrorV1::Unavailable)?,
                );
                push_optional_time(&mut bytes, capability.expires_at.as_ref());
            }
            Self::RegisterRunner {
                runner_id,
                owner_principal_id,
                ..
            } => {
                push_hash_field(&mut bytes, b"register_runner");
                push_hash_field(&mut bytes, runner_id.as_str().as_bytes());
                push_hash_field(&mut bytes, owner_principal_id.as_str().as_bytes());
            }
            Self::NarrowCapability {
                capability_id,
                expected_generation,
                scopes,
                expires_at,
                reason_code,
                ..
            } => {
                push_hash_field(&mut bytes, b"narrow_capability");
                push_hash_field(&mut bytes, capability_id.as_str().as_bytes());
                bytes.extend_from_slice(&expected_generation.get().to_be_bytes());
                push_hash_field(
                    &mut bytes,
                    &encode(scopes).map_err(|_| AuthorityErrorV1::Unavailable)?,
                );
                push_optional_time(&mut bytes, expires_at.as_ref());
                push_hash_field(&mut bytes, reason_code.as_str().as_bytes());
            }
            Self::RevokeCapability {
                capability_id,
                expected_generation,
                reason_code,
                ..
            } => {
                push_hash_field(&mut bytes, b"revoke_capability");
                push_hash_field(&mut bytes, capability_id.as_str().as_bytes());
                bytes.extend_from_slice(&expected_generation.get().to_be_bytes());
                push_hash_field(&mut bytes, reason_code.as_str().as_bytes());
            }
            Self::RevokeRunner {
                runner_id,
                expected_generation,
                reason_code,
                ..
            } => {
                push_hash_field(&mut bytes, b"revoke_runner");
                push_hash_field(&mut bytes, runner_id.as_str().as_bytes());
                bytes.extend_from_slice(&expected_generation.get().to_be_bytes());
                push_hash_field(&mut bytes, reason_code.as_str().as_bytes());
            }
            Self::SetPrincipalStatus {
                principal_id,
                expected_generation,
                status,
                reason_code,
                ..
            } => {
                push_hash_field(&mut bytes, b"set_principal_status");
                push_hash_field(&mut bytes, principal_id.as_str().as_bytes());
                bytes.extend_from_slice(&expected_generation.get().to_be_bytes());
                bytes.push(match status {
                    PrincipalAuthorityStatusV1::Enabled => 0,
                    PrincipalAuthorityStatusV1::Disabled => 1,
                });
                push_hash_field(&mut bytes, reason_code.as_str().as_bytes());
            }
        }
        Ok(Blake3DigestV1::hash(&bytes))
    }
}

impl fmt::Debug for AuthorityChangeV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::CreatePrincipal { .. } => "AuthorityChangeV1::CreatePrincipal([REDACTED])",
            Self::RegisterCapability { .. } => "AuthorityChangeV1::RegisterCapability([REDACTED])",
            Self::RegisterRunner { .. } => "AuthorityChangeV1::RegisterRunner([REDACTED])",
            Self::NarrowCapability { .. } => "AuthorityChangeV1::NarrowCapability([REDACTED])",
            Self::RevokeCapability { .. } => "AuthorityChangeV1::RevokeCapability([REDACTED])",
            Self::RevokeRunner { .. } => "AuthorityChangeV1::RevokeRunner([REDACTED])",
            Self::SetPrincipalStatus { .. } => "AuthorityChangeV1::SetPrincipalStatus([REDACTED])",
        })
    }
}

fn push_hash_field(target: &mut Vec<u8>, field: &[u8]) {
    target.extend_from_slice(&u64::try_from(field.len()).unwrap_or(u64::MAX).to_be_bytes());
    target.extend_from_slice(field);
}

fn push_optional_time(target: &mut Vec<u8>, value: Option<&CapabilityExpiresAt>) {
    if let Some(value) = value {
        target.push(1);
        push_hash_field(target, value.as_str().as_bytes());
    } else {
        target.push(0);
    }
}

/// Prepared authority mutation consumed atomically by an Adapter.
#[derive(Clone)]
pub struct PreparedAuthorityChangeV1 {
    actor_fence: AuthorityFenceFactsV1,
    command: AuthorityChangeV1,
    request_hash: Blake3DigestV1,
    authorized_at: AuthorityCheckedAt,
}

impl fmt::Debug for PreparedAuthorityChangeV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PreparedAuthorityChangeV1([REDACTED])")
    }
}

impl PreparedAuthorityChangeV1 {
    /// Returns the exact transaction query needed to revalidate this actor.
    #[must_use]
    pub fn actor_snapshot_query(&self) -> AuthoritySnapshotQueryV1 {
        self.actor_fence.snapshot_query()
    }

    #[must_use]
    pub const fn command(&self) -> &AuthorityChangeV1 {
        &self.command
    }

    #[must_use]
    pub const fn request_hash(&self) -> &Blake3DigestV1 {
        &self.request_hash
    }

    #[must_use]
    pub const fn authorized_at(&self) -> &AuthorityCheckedAt {
        &self.authorized_at
    }

    #[must_use]
    pub const fn actor_capability_id(&self) -> &CapabilityId {
        self.actor_fence.capability_id()
    }

    #[must_use]
    pub const fn actor_principal_id(&self) -> &PrincipalId {
        self.actor_fence.authenticated_principal()
    }

    #[must_use]
    pub const fn actor_principal_generation(&self) -> PrincipalGenerationV1 {
        self.actor_fence.principal_generation()
    }

    #[must_use]
    pub const fn actor_authority_generation(&self) -> AuthorityGenerationV1 {
        self.actor_fence.authority_generation()
    }

    #[must_use]
    pub fn actor_scope_revocation_bytes(&self) -> &[u8] {
        self.actor_fence.canonical_scope_revocation_bytes()
    }

    #[must_use]
    pub const fn actor_scope_revocation_hash(&self) -> &Blake3DigestV1 {
        self.actor_fence.scope_revocation_hash()
    }

    #[must_use]
    pub const fn actor_expires_at(&self) -> Option<&CapabilityExpiresAt> {
        self.actor_fence.expires_at()
    }

    /// Revalidates the actor fence against one coherent transaction snapshot
    /// at an explicitly supplied authoritative commit time.
    ///
    /// # Errors
    ///
    /// Returns a stale-generation failure when scope, revocation, expiry,
    /// Principal status, or generation no longer matches. A backwards time
    /// sample or malformed snapshot is rejected as an invalid change.
    pub fn revalidate_actor(
        &self,
        snapshot: &AuthoritySnapshotV1,
        checked_at: &AuthorityCheckedAt,
    ) -> Result<(), AuthorityStoreErrorV1> {
        if Blake3DigestV1::hash(self.request_hash.as_bytes()) != *self.actor_fence.purpose_hash() {
            return Err(AuthorityStoreErrorV1::InvalidChange);
        }
        self.actor_fence
            .revalidate_current(snapshot, checked_at)
            .map_err(|error| match error {
                AuthorityErrorV1::InvalidAuthorityRequest => AuthorityStoreErrorV1::InvalidChange,
                AuthorityErrorV1::Unavailable => AuthorityStoreErrorV1::Corrupt,
                AuthorityErrorV1::Unauthenticated
                | AuthorityErrorV1::Forbidden
                | AuthorityErrorV1::MembershipNotEnabled
                | AuthorityErrorV1::StaleAuthorityGeneration
                | AuthorityErrorV1::Conflict => AuthorityStoreErrorV1::StaleGeneration,
            })
    }

    /// Validates target-side mutation policy against transaction-current
    /// facts and returns the complete replacement selected by Core.
    ///
    /// # Errors
    ///
    /// Returns conflict for an occupied create target, stale generation for a
    /// failed compare-and-set, and invalid change for incoherent bindings,
    /// widening/no-op updates, or missing target facts.
    #[allow(clippy::too_many_lines)]
    pub fn validate_target(
        &self,
        state: &AuthorityChangeStateV1,
        commit_checked_at: &AuthorityCheckedAt,
    ) -> Result<ValidatedAuthorityChangeV1, AuthorityStoreErrorV1> {
        if compare_timestamp_text(commit_checked_at.as_str(), self.authorized_at.as_str()).is_lt() {
            return Err(AuthorityStoreErrorV1::InvalidChange);
        }
        match self.command() {
            AuthorityChangeV1::CreatePrincipal {
                principal_id, kind, ..
            } => {
                if state.principal.is_some() {
                    return Err(AuthorityStoreErrorV1::Conflict);
                }
                if state.capability.is_some()
                    || state.membership.is_some()
                    || state.runner.is_some()
                    || !state.runner_memberships.is_empty()
                {
                    return Err(AuthorityStoreErrorV1::InvalidChange);
                }
                let generation =
                    PrincipalGenerationV1::new(1).map_err(|_| AuthorityStoreErrorV1::Corrupt)?;
                Ok(ValidatedAuthorityChangeV1::Principal {
                    result: AuthorityChangeResultV1::PrincipalCreated,
                    principal: PrincipalAuthoritySnapshotV1::new(
                        principal_id.clone(),
                        *kind,
                        PrincipalAuthorityStatusV1::Enabled,
                        generation,
                    ),
                })
            }
            AuthorityChangeV1::RegisterCapability { capability, .. } => {
                if state.capability.is_some() {
                    return Err(AuthorityStoreErrorV1::Conflict);
                }
                let principal = state
                    .principal
                    .as_ref()
                    .ok_or(AuthorityStoreErrorV1::InvalidChange)?;
                if principal.principal_id() != capability.principal_id() {
                    return Err(AuthorityStoreErrorV1::InvalidChange);
                }
                validate_registration_context(capability, principal, state)?;
                let generation =
                    AuthorityGenerationV1::new(1).map_err(|_| AuthorityStoreErrorV1::Corrupt)?;
                let replacement =
                    CapabilityAuthoritySnapshotV1::new(CapabilityAuthoritySnapshotPartsV1 {
                        capability_id: capability.capability_id().clone(),
                        token_hash: capability.token_hash().clone(),
                        principal_id: capability.principal_id().clone(),
                        profile: capability.profile().clone(),
                        scopes: capability.scopes().clone(),
                        generation,
                        expires_at: capability.expires_at().cloned(),
                        revoked_at: None,
                    })
                    .map_err(|_| AuthorityStoreErrorV1::InvalidChange)?;
                Ok(ValidatedAuthorityChangeV1::Capability {
                    result: AuthorityChangeResultV1::CapabilityRegistered,
                    capability: replacement,
                })
            }
            AuthorityChangeV1::RegisterRunner {
                runner_id,
                owner_principal_id,
                ..
            } => {
                if state.runner.is_some() {
                    return Err(AuthorityStoreErrorV1::Conflict);
                }
                let principal = state
                    .principal
                    .as_ref()
                    .ok_or(AuthorityStoreErrorV1::InvalidChange)?;
                if state.capability.is_some()
                    || state.membership.is_some()
                    || !state.runner_memberships.is_empty()
                    || principal.principal_id() != owner_principal_id
                    || principal.status() != PrincipalAuthorityStatusV1::Enabled
                {
                    return Err(AuthorityStoreErrorV1::InvalidChange);
                }
                let generation =
                    RunnerGenerationV1::new(1).map_err(|_| AuthorityStoreErrorV1::Corrupt)?;
                Ok(ValidatedAuthorityChangeV1::Runner {
                    result: AuthorityChangeResultV1::RunnerRegistered,
                    runner: RunnerAuthoritySnapshotV1::new(
                        runner_id.clone(),
                        owner_principal_id.clone(),
                        RunnerAuthorityStatusV1::Enabled,
                        generation,
                    ),
                })
            }
            AuthorityChangeV1::NarrowCapability {
                capability_id,
                expected_generation,
                scopes,
                expires_at,
                ..
            } => {
                let current = state
                    .capability
                    .as_ref()
                    .ok_or(AuthorityStoreErrorV1::InvalidChange)?;
                if current.capability_id() != capability_id {
                    return Err(AuthorityStoreErrorV1::InvalidChange);
                }
                if current.generation() != *expected_generation {
                    return Err(AuthorityStoreErrorV1::StaleGeneration);
                }
                if current.revoked_at().is_some() {
                    return Err(AuthorityStoreErrorV1::StaleGeneration);
                }
                current
                    .profile()
                    .validate_scopes(scopes)
                    .map_err(|_| AuthorityStoreErrorV1::InvalidChange)?;
                if !scopes.is_subset(current.scopes())
                    || !expiry_is_narrower(current.expires_at(), expires_at.as_ref())
                {
                    return Err(AuthorityStoreErrorV1::InvalidChange);
                }
                if scopes == current.scopes() && expires_at.as_ref() == current.expires_at() {
                    return Err(AuthorityStoreErrorV1::InvalidChange);
                }
                let generation = current
                    .generation()
                    .checked_successor()
                    .map_err(|_| AuthorityStoreErrorV1::InvalidChange)?;
                let replacement =
                    CapabilityAuthoritySnapshotV1::new(CapabilityAuthoritySnapshotPartsV1 {
                        capability_id: current.capability_id().clone(),
                        token_hash: current.token_hash().clone(),
                        principal_id: current.principal_id().clone(),
                        profile: current.profile().clone(),
                        scopes: scopes.clone(),
                        generation,
                        expires_at: expires_at.clone(),
                        revoked_at: None,
                    })
                    .map_err(|_| AuthorityStoreErrorV1::InvalidChange)?;
                Ok(ValidatedAuthorityChangeV1::Capability {
                    result: AuthorityChangeResultV1::CapabilityNarrowed,
                    capability: replacement,
                })
            }
            AuthorityChangeV1::RevokeCapability {
                capability_id,
                expected_generation,
                ..
            } => {
                let current = state
                    .capability
                    .as_ref()
                    .ok_or(AuthorityStoreErrorV1::InvalidChange)?;
                if current.capability_id() != capability_id {
                    return Err(AuthorityStoreErrorV1::InvalidChange);
                }
                if current.generation() != *expected_generation || current.revoked_at().is_some() {
                    return Err(AuthorityStoreErrorV1::StaleGeneration);
                }
                let generation = current
                    .generation()
                    .checked_successor()
                    .map_err(|_| AuthorityStoreErrorV1::InvalidChange)?;
                let revoked_at = commit_checked_at
                    .as_str()
                    .parse::<CapabilityRevokedAt>()
                    .map_err(|_| AuthorityStoreErrorV1::Corrupt)?;
                let replacement =
                    CapabilityAuthoritySnapshotV1::new(CapabilityAuthoritySnapshotPartsV1 {
                        capability_id: current.capability_id().clone(),
                        token_hash: current.token_hash().clone(),
                        principal_id: current.principal_id().clone(),
                        profile: current.profile().clone(),
                        scopes: current.scopes().clone(),
                        generation,
                        expires_at: current.expires_at().cloned(),
                        revoked_at: Some(revoked_at),
                    })
                    .map_err(|_| AuthorityStoreErrorV1::InvalidChange)?;
                Ok(ValidatedAuthorityChangeV1::Capability {
                    result: AuthorityChangeResultV1::CapabilityRevoked,
                    capability: replacement,
                })
            }
            AuthorityChangeV1::RevokeRunner {
                runner_id,
                expected_generation,
                ..
            } => {
                let current = state
                    .runner
                    .as_ref()
                    .ok_or(AuthorityStoreErrorV1::InvalidChange)?;
                if state.principal.is_some()
                    || state.capability.is_some()
                    || state.membership.is_some()
                    || !state.runner_memberships.is_empty()
                    || current.runner_id() != runner_id
                {
                    return Err(AuthorityStoreErrorV1::InvalidChange);
                }
                if current.generation() != *expected_generation
                    || current.status() != RunnerAuthorityStatusV1::Enabled
                {
                    return Err(AuthorityStoreErrorV1::StaleGeneration);
                }
                let generation = current
                    .generation()
                    .checked_successor()
                    .map_err(|_| AuthorityStoreErrorV1::InvalidChange)?;
                Ok(ValidatedAuthorityChangeV1::Runner {
                    result: AuthorityChangeResultV1::RunnerRevoked,
                    runner: RunnerAuthoritySnapshotV1::new(
                        current.runner_id().clone(),
                        current.owner_principal_id().clone(),
                        RunnerAuthorityStatusV1::Revoked,
                        generation,
                    ),
                })
            }
            AuthorityChangeV1::SetPrincipalStatus {
                principal_id,
                expected_generation,
                status,
                ..
            } => {
                let current = state
                    .principal
                    .as_ref()
                    .ok_or(AuthorityStoreErrorV1::InvalidChange)?;
                if current.principal_id() != principal_id {
                    return Err(AuthorityStoreErrorV1::InvalidChange);
                }
                if current.generation() != *expected_generation {
                    return Err(AuthorityStoreErrorV1::StaleGeneration);
                }
                if current.status() == *status {
                    return Err(AuthorityStoreErrorV1::InvalidChange);
                }
                let generation = current
                    .generation()
                    .checked_successor()
                    .map_err(|_| AuthorityStoreErrorV1::InvalidChange)?;
                Ok(ValidatedAuthorityChangeV1::Principal {
                    result: AuthorityChangeResultV1::PrincipalStatusChanged,
                    principal: PrincipalAuthoritySnapshotV1::new(
                        current.principal_id().clone(),
                        current.kind(),
                        *status,
                        generation,
                    ),
                })
            }
        }
    }
}

fn expiry_is_narrower(
    current: Option<&CapabilityExpiresAt>,
    replacement: Option<&CapabilityExpiresAt>,
) -> bool {
    match (current, replacement) {
        (None, _) => true,
        (Some(_), None) => false,
        (Some(current), Some(replacement)) => {
            !compare_timestamp_text(replacement.as_str(), current.as_str()).is_gt()
        }
    }
}

fn validate_registration_context(
    capability: &NewCapabilityV1,
    principal: &PrincipalAuthoritySnapshotV1,
    state: &AuthorityChangeStateV1,
) -> Result<(), AuthorityStoreErrorV1> {
    if principal.status() != PrincipalAuthorityStatusV1::Enabled {
        return Err(AuthorityStoreErrorV1::InvalidChange);
    }
    match capability.profile() {
        CapabilityProfileV1::RoomMember { room_id, member_id } => {
            let membership = state
                .membership
                .as_ref()
                .ok_or(AuthorityStoreErrorV1::InvalidChange)?;
            if state.runner.is_some()
                || !state.runner_memberships.is_empty()
                || membership.room_id() != room_id
                || membership.membership().member_id() != member_id
                || membership.membership().principal_id() != principal.principal_id()
                || membership.membership().principal_kind() != principal.kind()
            {
                return Err(AuthorityStoreErrorV1::InvalidChange);
            }
        }
        CapabilityProfileV1::HostOperator { .. } => {
            if state.membership.is_some()
                || state.runner.is_some()
                || !state.runner_memberships.is_empty()
            {
                return Err(AuthorityStoreErrorV1::InvalidChange);
            }
        }
        CapabilityProfileV1::RunnerControl {
            runner_id,
            permitted_memberships,
        } => {
            let runner = state
                .runner
                .as_ref()
                .ok_or(AuthorityStoreErrorV1::InvalidChange)?;
            if state.membership.is_some()
                || runner.runner_id() != runner_id
                || runner.owner_principal_id() != principal.principal_id()
                || runner.status() != RunnerAuthorityStatusV1::Enabled
                || state.runner_memberships.len() != permitted_memberships.0.len()
            {
                return Err(AuthorityStoreErrorV1::InvalidChange);
            }
            for target in permitted_memberships.iter() {
                let membership = state.runner_memberships.iter().find(|membership| {
                    membership.room_id() == &target.room_id
                        && membership.membership().member_id() == &target.member_id
                });
                let Some(membership) = membership else {
                    return Err(AuthorityStoreErrorV1::InvalidChange);
                };
                if membership.membership().principal_kind() != PrincipalKindV1::Agent
                    || membership.membership().access_mode() != AccessModeV1::Participant
                    || membership.membership().role().is_none()
                {
                    return Err(AuthorityStoreErrorV1::InvalidChange);
                }
            }
        }
    }
    Ok(())
}

/// Authority mutation result family.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthorityChangeResultV1 {
    AuthorityBootstrapped,
    PrincipalCreated,
    CapabilityRegistered,
    CapabilityNarrowed,
    CapabilityRevoked,
    PrincipalStatusChanged,
    RunnerRegistered,
    RunnerRevoked,
}

/// Authority mutation target.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AuthorityChangeTargetV1 {
    Bootstrap {
        principal_id: PrincipalId,
        capability_id: CapabilityId,
    },
    Principal(PrincipalId),
    Capability(CapabilityId),
    Runner(RunnerId),
}

/// Durable operational authority-change receipt.
#[derive(Clone, Eq, PartialEq)]
pub struct AuthorityChangeReceiptV1 {
    change_id: AuthorityChangeId,
    request_hash: Blake3DigestV1,
    result: AuthorityChangeResultV1,
    target: AuthorityChangeTargetV1,
    resulting_generation: u64,
    changed_at: AuthorityCheckedAt,
}

impl fmt::Debug for AuthorityChangeReceiptV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AuthorityChangeReceiptV1([REDACTED])")
    }
}

impl AuthorityChangeReceiptV1 {
    /// Reconstructs a fresh or idempotently replayed durable receipt while
    /// checking that result, target, generation family, identity, and hash
    /// agree with the prepared command.
    ///
    /// # Errors
    ///
    /// Returns `InvalidChange` if any supplied receipt fact cannot describe
    /// this prepared command.
    pub fn from_applied_change(
        change: &PreparedAuthorityChangeV1,
        result: AuthorityChangeResultV1,
        resulting_generation: u64,
        changed_at: AuthorityCheckedAt,
    ) -> Result<Self, AuthorityStoreErrorV1> {
        if compare_timestamp_text(changed_at.as_str(), change.authorized_at.as_str()).is_lt() {
            return Err(AuthorityStoreErrorV1::InvalidChange);
        }
        let (expected_result, target) = match change.command() {
            AuthorityChangeV1::CreatePrincipal { principal_id, .. } => (
                AuthorityChangeResultV1::PrincipalCreated,
                AuthorityChangeTargetV1::Principal(principal_id.clone()),
            ),
            AuthorityChangeV1::RegisterCapability { capability, .. } => (
                AuthorityChangeResultV1::CapabilityRegistered,
                AuthorityChangeTargetV1::Capability(capability.capability_id().clone()),
            ),
            AuthorityChangeV1::RegisterRunner { runner_id, .. } => (
                AuthorityChangeResultV1::RunnerRegistered,
                AuthorityChangeTargetV1::Runner(runner_id.clone()),
            ),
            AuthorityChangeV1::NarrowCapability { capability_id, .. } => (
                AuthorityChangeResultV1::CapabilityNarrowed,
                AuthorityChangeTargetV1::Capability(capability_id.clone()),
            ),
            AuthorityChangeV1::RevokeCapability { capability_id, .. } => (
                AuthorityChangeResultV1::CapabilityRevoked,
                AuthorityChangeTargetV1::Capability(capability_id.clone()),
            ),
            AuthorityChangeV1::RevokeRunner { runner_id, .. } => (
                AuthorityChangeResultV1::RunnerRevoked,
                AuthorityChangeTargetV1::Runner(runner_id.clone()),
            ),
            AuthorityChangeV1::SetPrincipalStatus { principal_id, .. } => (
                AuthorityChangeResultV1::PrincipalStatusChanged,
                AuthorityChangeTargetV1::Principal(principal_id.clone()),
            ),
        };
        if result != expected_result {
            return Err(AuthorityStoreErrorV1::InvalidChange);
        }
        match target {
            AuthorityChangeTargetV1::Bootstrap { .. } => {
                return Err(AuthorityStoreErrorV1::InvalidChange);
            }
            AuthorityChangeTargetV1::Principal(_) => {
                PrincipalGenerationV1::new(resulting_generation)
                    .map_err(|_| AuthorityStoreErrorV1::InvalidChange)?;
            }
            AuthorityChangeTargetV1::Capability(_) => {
                AuthorityGenerationV1::new(resulting_generation)
                    .map_err(|_| AuthorityStoreErrorV1::InvalidChange)?;
            }
            AuthorityChangeTargetV1::Runner(_) => {
                RunnerGenerationV1::new(resulting_generation)
                    .map_err(|_| AuthorityStoreErrorV1::InvalidChange)?;
            }
        }
        Ok(Self {
            change_id: change.command.change_id().clone(),
            request_hash: change.request_hash.clone(),
            result,
            target,
            resulting_generation,
            changed_at,
        })
    }

    /// Reconstructs a fresh or replayed bootstrap receipt while checking the
    /// fixed result and the generation shared by both installed records.
    ///
    /// # Errors
    ///
    /// Returns `InvalidChange` for the wrong result or a generation other than
    /// one. Fresh installs validate that the commit time is not before
    /// preparation in [`PreparedAuthorityBootstrapV1::validate_install`]; a
    /// replay must be allowed to reconstruct its already durable receipt even
    /// when the caller prepares the same idempotent request at a later time.
    pub fn from_applied_bootstrap(
        bootstrap: &PreparedAuthorityBootstrapV1,
        result: AuthorityChangeResultV1,
        resulting_generation: u64,
        changed_at: AuthorityCheckedAt,
    ) -> Result<Self, AuthorityStoreErrorV1> {
        if result != AuthorityChangeResultV1::AuthorityBootstrapped || resulting_generation != 1 {
            return Err(AuthorityStoreErrorV1::InvalidChange);
        }
        Ok(Self {
            change_id: bootstrap.change_id().clone(),
            request_hash: bootstrap.request_hash.clone(),
            result,
            target: AuthorityChangeTargetV1::Bootstrap {
                principal_id: bootstrap.bootstrap.principal_id().clone(),
                capability_id: bootstrap.bootstrap.capability.capability_id().clone(),
            },
            resulting_generation,
            changed_at,
        })
    }

    #[must_use]
    pub const fn change_id(&self) -> &AuthorityChangeId {
        &self.change_id
    }

    #[must_use]
    pub const fn request_hash(&self) -> &Blake3DigestV1 {
        &self.request_hash
    }

    #[must_use]
    pub const fn result(&self) -> AuthorityChangeResultV1 {
        self.result
    }

    #[must_use]
    pub const fn target(&self) -> &AuthorityChangeTargetV1 {
        &self.target
    }

    #[must_use]
    pub const fn resulting_generation(&self) -> u64 {
        self.resulting_generation
    }

    #[must_use]
    pub const fn changed_at(&self) -> &AuthorityCheckedAt {
        &self.changed_at
    }
}

/// Safe caller-visible authority failures.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum AuthorityErrorV1 {
    #[error("unauthenticated")]
    Unauthenticated,
    #[error("forbidden")]
    Forbidden,
    #[error("Membership is not enabled")]
    MembershipNotEnabled,
    #[error("authority request is invalid")]
    InvalidAuthorityRequest,
    #[error("authority generation changed")]
    StaleAuthorityGeneration,
    #[error("authority change conflicts with another request")]
    Conflict,
    #[error("authority is unavailable")]
    Unavailable,
}

/// Deep operational authority Module.
pub struct AuthorityV1 {
    store: Arc<dyn AuthorityStoreV1>,
}

impl fmt::Debug for AuthorityV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AuthorityV1([OPAQUE])")
    }
}

impl AuthorityV1 {
    /// Wires one selected authority Adapter.
    #[must_use]
    pub fn new(store: Arc<dyn AuthorityStoreV1>) -> Self {
        Self { store }
    }

    /// Installs the first enabled Principal and the sole bootstrap Capability.
    ///
    /// This is the only path without an authenticated actor. The Adapter owns
    /// the trusted clock, enforces durable one-time occupancy and idempotency,
    /// and samples commit time inside its guarded transaction.
    ///
    /// # Errors
    ///
    /// Returns conflict after any authority state exists, or a safe invalid,
    /// corrupt, or unavailable failure from the Adapter.
    pub fn bootstrap(
        &self,
        bootstrap: AuthorityBootstrapV1,
        prepared_at: AuthorityCheckedAt,
    ) -> Result<AuthorityChangeReceiptV1, AuthorityErrorV1> {
        let request_hash = bootstrap.request_hash();
        let prepared = PreparedAuthorityBootstrapV1 {
            bootstrap,
            request_hash,
            prepared_at,
        };
        self.store
            .apply_bootstrap(&prepared)
            .map_err(map_store_error)
    }

    /// Authorizes one closed use at caller-supplied trusted operational time.
    ///
    /// # Errors
    ///
    /// Fails closed for invalid credentials, scope/target mismatch, disabled
    /// authority, expiry, malformed purpose, or unavailable storage.
    #[cfg(any(test, feature = "conformance-tracer"))]
    pub fn authorize(
        &self,
        presented: &PresentedCapabilityV1,
        use_: AuthorityUseV1,
        checked_at: AuthorityCheckedAt,
    ) -> Result<AuthorityGrantV1, AuthorityErrorV1> {
        self.authorize_inner(presented, use_, checked_at)
    }

    #[cfg(not(any(test, feature = "conformance-tracer")))]
    pub(crate) fn authorize(
        &self,
        presented: &PresentedCapabilityV1,
        use_: AuthorityUseV1,
        checked_at: AuthorityCheckedAt,
    ) -> Result<AuthorityGrantV1, AuthorityErrorV1> {
        self.authorize_inner(presented, use_, checked_at)
    }

    fn authorize_inner(
        &self,
        presented: &PresentedCapabilityV1,
        use_: AuthorityUseV1,
        checked_at: AuthorityCheckedAt,
    ) -> Result<AuthorityGrantV1, AuthorityErrorV1> {
        let query = use_.query(presented.capability_id.clone());
        let snapshot = self.authenticate(presented, &query, &checked_at)?;
        let purpose_bytes = authority_purpose_bytes(&use_)?;
        Self::authorize_use(&snapshot, use_, checked_at, &purpose_bytes)
    }

    #[cfg(test)]
    pub(crate) fn receipt_adapter_input_for_test(
        &self,
        authority: AuthorizedReceiptReadV1,
        checked_at: &AuthorityCheckedAt,
    ) -> Result<ReceiptReadAdapterInputV1, AuthorityErrorV1> {
        let input = authority.into_adapter_input();
        let snapshot = self
            .store
            .snapshot(&input.authority_snapshot_query())
            .map_err(map_store_error)?
            .ok_or(AuthorityErrorV1::Unauthenticated)?;
        input.revalidate_current(&snapshot, checked_at)?;
        Ok(input)
    }

    #[cfg(test)]
    pub(crate) fn replay_adapter_input_for_test(
        &self,
        authority: AuthorizedReplayV1,
        checked_at: &AuthorityCheckedAt,
    ) -> Result<ReplayAdapterInputV1, AuthorityErrorV1> {
        let input = authority.into_adapter_input();
        let snapshot = self
            .store
            .snapshot(&input.authority_snapshot_query())
            .map_err(map_store_error)?
            .ok_or(AuthorityErrorV1::Unauthenticated)?;
        input.revalidate_current(&snapshot, checked_at)?;
        Ok(input)
    }

    /// Authorizes present access to one exact immutable operation result.
    /// The returned grant must be consumed by an
    /// [`crate::AuthorizedReceiptResolverV1`] so current authority and the
    /// result's actual Room are rechecked before any bytes are released.
    ///
    /// # Errors
    ///
    /// Returns a safe authentication, scope, target, binding, or storage
    /// failure.
    pub fn authorize_receipt_read(
        &self,
        presented: &PresentedCapabilityV1,
        identity: OperationIdentityV1,
        request_hash: CanonicalRequestHashV1,
        target_room_id: Option<RoomId>,
        checked_at: AuthorityCheckedAt,
    ) -> Result<AuthorizedReceiptReadV1, AuthorityErrorV1> {
        match self.authorize_inner(
            presented,
            AuthorityUseV1::ReadRoomOperationResult {
                identity,
                request_hash,
                target_room_id,
            },
            checked_at,
        )? {
            AuthorityGrantV1::ReceiptRead(grant) => Ok(grant),
            AuthorityGrantV1::MemberRead(_)
            | AuthorityGrantV1::ParticipantAction(_)
            | AuthorityGrantV1::RoomCreation(_)
            | AuthorityGrantV1::CoreAdministration(_)
            | AuthorityGrantV1::TimerFired(_)
            | AuthorityGrantV1::Replay(_)
            | AuthorityGrantV1::Diagnostic(_)
            | AuthorityGrantV1::RunnerControl(_)
            | AuthorityGrantV1::ExternalInput(_) => Err(AuthorityErrorV1::Unavailable),
        }
    }

    /// Authorizes one exact typed Room creation request and operation identity.
    /// Core derives the canonical request hash; generated Room/Member IDs,
    /// seed, and timestamps cannot enter this purpose.
    ///
    /// # Errors
    ///
    /// Returns an invalid request for malformed creation semantics or identity,
    /// or a safe authentication, scope, expiry, or storage failure.
    pub(crate) fn authorize_room_creation(
        &self,
        presented: &PresentedCapabilityV1,
        identity: AdministrationOperationIdentityV1,
        request: &RoomCreationRequestV1,
        checked_at: AuthorityCheckedAt,
    ) -> Result<AuthorizedRoomCreationV1, AuthorityErrorV1> {
        if identity.versioned_operation_kind != CREATE_ROOM_OPERATION_KIND
            || identity.idempotency_key.is_empty()
        {
            return Err(AuthorityErrorV1::InvalidAuthorityRequest);
        }
        let request_hash = request
            .canonical_request_hash()
            .map_err(|_| AuthorityErrorV1::InvalidAuthorityRequest)?;
        match self.authorize_inner(
            presented,
            AuthorityUseV1::CreateRoom {
                identity,
                request_hash,
            },
            checked_at,
        )? {
            AuthorityGrantV1::RoomCreation(grant) => Ok(grant),
            AuthorityGrantV1::ReceiptRead(_)
            | AuthorityGrantV1::MemberRead(_)
            | AuthorityGrantV1::ParticipantAction(_)
            | AuthorityGrantV1::CoreAdministration(_)
            | AuthorityGrantV1::TimerFired(_)
            | AuthorityGrantV1::Replay(_)
            | AuthorityGrantV1::Diagnostic(_)
            | AuthorityGrantV1::RunnerControl(_)
            | AuthorityGrantV1::ExternalInput(_) => Err(AuthorityErrorV1::Unavailable),
        }
    }

    /// Authorizes one exact nonmutating Member operation. Action submission is
    /// intentionally absent and must use the receipt-first Action ingress.
    ///
    /// # Errors
    ///
    /// Returns a safe authentication, Membership-binding, Standing, scope,
    /// expiry, or storage failure.
    pub fn authorize_member_read(
        &self,
        presented: &PresentedCapabilityV1,
        room_id: RoomId,
        member_id: MemberId,
        operation: MemberReadOperationV1,
        checked_at: AuthorityCheckedAt,
    ) -> Result<AuthorizedViewerV1, AuthorityErrorV1> {
        match self.authorize_inner(
            presented,
            AuthorityUseV1::Member {
                room_id,
                member_id,
                operation: operation.into(),
            },
            checked_at,
        )? {
            AuthorityGrantV1::MemberRead(grant) => Ok(grant),
            AuthorityGrantV1::ReceiptRead(_)
            | AuthorityGrantV1::ParticipantAction(_)
            | AuthorityGrantV1::RoomCreation(_)
            | AuthorityGrantV1::CoreAdministration(_)
            | AuthorityGrantV1::TimerFired(_)
            | AuthorityGrantV1::Replay(_)
            | AuthorityGrantV1::Diagnostic(_)
            | AuthorityGrantV1::RunnerControl(_)
            | AuthorityGrantV1::ExternalInput(_) => Err(AuthorityErrorV1::Unavailable),
        }
    }

    /// Authorizes one exact diagnostic target and operation. The returned
    /// grant must be consumed by a diagnostic Adapter that revalidates its
    /// current fence before releasing even a safe summary.
    ///
    /// # Errors
    ///
    /// Returns a safe authentication, scope, target, expiry, or storage
    /// failure.
    pub fn authorize_diagnostic(
        &self,
        presented: &PresentedCapabilityV1,
        target: DiagnosticTargetV1,
        operation: DiagnosticOperationV1,
        checked_at: AuthorityCheckedAt,
    ) -> Result<AuthorizedDiagnosticV1, AuthorityErrorV1> {
        match self.authorize_inner(
            presented,
            AuthorityUseV1::HostDiagnostic { target, operation },
            checked_at,
        )? {
            AuthorityGrantV1::Diagnostic(grant) => Ok(grant),
            AuthorityGrantV1::ReceiptRead(_)
            | AuthorityGrantV1::MemberRead(_)
            | AuthorityGrantV1::ParticipantAction(_)
            | AuthorityGrantV1::RoomCreation(_)
            | AuthorityGrantV1::CoreAdministration(_)
            | AuthorityGrantV1::TimerFired(_)
            | AuthorityGrantV1::Replay(_)
            | AuthorityGrantV1::RunnerControl(_)
            | AuthorityGrantV1::ExternalInput(_) => Err(AuthorityErrorV1::Unavailable),
        }
    }

    /// Authorizes one exact Runner operation against an exact permitted Agent
    /// Participant Membership target, including `ReceiveOffer`.
    /// Runner capabilities cannot be used for Action, read, Replay,
    /// administration, or diagnostics.
    ///
    /// # Errors
    ///
    /// Returns a safe authentication, Runner/Membership binding, scope,
    /// expiry, operation-shape, or storage failure.
    pub fn authorize_runner_control(
        &self,
        presented: &PresentedCapabilityV1,
        runner_id: RunnerId,
        operation: RunnerControlOperationV1,
        target: RoomMembershipKeyV1,
        checked_at: AuthorityCheckedAt,
    ) -> Result<AuthorizedRunnerControlV1, AuthorityErrorV1> {
        match self.authorize_inner(
            presented,
            AuthorityUseV1::RunnerControl {
                runner_id,
                operation,
                target: Some(target),
            },
            checked_at,
        )? {
            AuthorityGrantV1::RunnerControl(grant) => Ok(grant),
            AuthorityGrantV1::ReceiptRead(_)
            | AuthorityGrantV1::MemberRead(_)
            | AuthorityGrantV1::ParticipantAction(_)
            | AuthorityGrantV1::RoomCreation(_)
            | AuthorityGrantV1::CoreAdministration(_)
            | AuthorityGrantV1::TimerFired(_)
            | AuthorityGrantV1::Replay(_)
            | AuthorityGrantV1::Diagnostic(_)
            | AuthorityGrantV1::ExternalInput(_) => Err(AuthorityErrorV1::Unavailable),
        }
    }

    /// Authorizes one exact typed Participant Action request.
    ///
    /// Core derives the operation identity, canonical request hash, Action
    /// type, Room, and Member from the request so callers cannot reconstruct
    /// or mismatch its authority purpose.
    ///
    /// # Errors
    ///
    /// Returns an invalid request for malformed Action shape, or the same safe
    /// authentication, authorization, and storage failures as [`Self::authorize`].
    pub(crate) fn authorize_action(
        &self,
        presented: &PresentedCapabilityV1,
        request: &ParticipantActionRequestV1,
        checked_at: AuthorityCheckedAt,
    ) -> Result<ParticipantActionAuthorityV1, AuthorityErrorV1> {
        let request_hash = request
            .canonical_request_hash()
            .map_err(|_| AuthorityErrorV1::InvalidAuthorityRequest)?;
        let use_ = AuthorityUseV1::Member {
            room_id: request.room_id().clone(),
            member_id: request.member_id().clone(),
            operation: MemberAuthorityUseV1::SubmitAction {
                identity: ParticipantActionOperationIdentityV1 {
                    room_id: request.room_id().clone(),
                    member_id: request.member_id().clone(),
                    action_id: request.action_id().clone(),
                },
                request_hash,
                action_type: request.action_type().to_owned(),
            },
        };
        match self.authorize(presented, use_, checked_at)? {
            AuthorityGrantV1::ParticipantAction(grant) => Ok(grant),
            AuthorityGrantV1::ReceiptRead(_)
            | AuthorityGrantV1::MemberRead(_)
            | AuthorityGrantV1::RoomCreation(_)
            | AuthorityGrantV1::CoreAdministration(_)
            | AuthorityGrantV1::TimerFired(_)
            | AuthorityGrantV1::Replay(_)
            | AuthorityGrantV1::Diagnostic(_)
            | AuthorityGrantV1::RunnerControl(_)
            | AuthorityGrantV1::ExternalInput(_) => Err(AuthorityErrorV1::Unavailable),
        }
    }

    /// Authorizes receipt resolution for one exact typed Participant Action
    /// request without consulting current Standing, Access Mode, Role, or Room
    /// lifecycle eligibility.
    ///
    /// This is the mandatory first half of Action ingress: an existing
    /// identity/hash result is returned before new-work eligibility is tested.
    ///
    /// # Errors
    ///
    /// Returns an invalid request for malformed Action shape, or the same safe
    /// authentication, scope, binding, and storage failures as
    /// [`Self::authorize`].
    pub(crate) fn authorize_action_receipt_read(
        &self,
        presented: &PresentedCapabilityV1,
        request: &ParticipantActionRequestV1,
        checked_at: AuthorityCheckedAt,
    ) -> Result<AuthorizedReceiptReadV1, AuthorityErrorV1> {
        let request_hash = request
            .canonical_request_hash()
            .map_err(|_| AuthorityErrorV1::InvalidAuthorityRequest)?;
        let identity = OperationIdentityV1::ParticipantAction(Box::new(
            ParticipantActionOperationIdentityV1 {
                room_id: request.room_id().clone(),
                member_id: request.member_id().clone(),
                action_id: request.action_id().clone(),
            },
        ));
        match self.authorize(
            presented,
            AuthorityUseV1::ReadRoomOperationResult {
                identity,
                request_hash,
                target_room_id: Some(request.room_id().clone()),
            },
            checked_at,
        )? {
            AuthorityGrantV1::ReceiptRead(grant) => Ok(grant),
            AuthorityGrantV1::MemberRead(_)
            | AuthorityGrantV1::ParticipantAction(_)
            | AuthorityGrantV1::RoomCreation(_)
            | AuthorityGrantV1::CoreAdministration(_)
            | AuthorityGrantV1::TimerFired(_)
            | AuthorityGrantV1::Replay(_)
            | AuthorityGrantV1::Diagnostic(_)
            | AuthorityGrantV1::RunnerControl(_)
            | AuthorityGrantV1::ExternalInput(_) => Err(AuthorityErrorV1::Unavailable),
        }
    }

    /// Authorizes one exact typed existing-Room Core administration request.
    ///
    /// Core derives the canonical request hash and mandatory/vetoable class;
    /// callers cannot self-declare either authority fact.
    ///
    /// # Errors
    ///
    /// Returns an invalid request for malformed administration shape, or the
    /// same safe failures as [`Self::authorize`].
    pub(crate) fn authorize_core_administration(
        &self,
        presented: &PresentedCapabilityV1,
        request: &CoreAdministrationRequestV1,
        checked_at: AuthorityCheckedAt,
    ) -> Result<AuthorizedCoreAdministrationV1, AuthorityErrorV1> {
        let classified = request
            .classified()
            .map_err(|_| AuthorityErrorV1::InvalidAuthorityRequest)?;
        let request_hash = request
            .canonical_request_hash()
            .map_err(|_| AuthorityErrorV1::InvalidAuthorityRequest)?;
        let use_ = AuthorityUseV1::CoreAdministration {
            room_id: request.room_id().clone(),
            classified,
            identity: request.operation_identity().clone(),
            request_hash,
        };
        match self.authorize(presented, use_, checked_at)? {
            AuthorityGrantV1::CoreAdministration(grant) => Ok(grant),
            AuthorityGrantV1::ReceiptRead(_)
            | AuthorityGrantV1::MemberRead(_)
            | AuthorityGrantV1::ParticipantAction(_)
            | AuthorityGrantV1::RoomCreation(_)
            | AuthorityGrantV1::TimerFired(_)
            | AuthorityGrantV1::Replay(_)
            | AuthorityGrantV1::Diagnostic(_)
            | AuthorityGrantV1::RunnerControl(_)
            | AuthorityGrantV1::ExternalInput(_) => Err(AuthorityErrorV1::Unavailable),
        }
    }

    /// Authorizes one exact scheduled Timer firing for its Room root.
    /// Core derives the request hash; the returned grant exposes no Timer
    /// payload and can only be consumed by the typed Room Commit sealer.
    ///
    /// # Errors
    ///
    /// Returns an invalid request for malformed Timer semantics, or the same
    /// safe authentication, Room-scope, expiry, and storage failures as
    /// [`Self::authorize`].
    pub fn authorize_timer_fired(
        &self,
        presented: &PresentedCapabilityV1,
        request: &TimerFiredRequestV1,
        checked_at: AuthorityCheckedAt,
    ) -> Result<AuthorizedTimerFiredV1, AuthorityErrorV1> {
        let request_hash = request
            .canonical_request_hash()
            .map_err(|_| AuthorityErrorV1::InvalidAuthorityRequest)?;
        match self.authorize(
            presented,
            AuthorityUseV1::TimerFired {
                room_id: request.room_id().clone(),
                request_hash,
            },
            checked_at,
        )? {
            AuthorityGrantV1::TimerFired(grant) => Ok(grant),
            AuthorityGrantV1::ReceiptRead(_)
            | AuthorityGrantV1::MemberRead(_)
            | AuthorityGrantV1::ParticipantAction(_)
            | AuthorityGrantV1::RoomCreation(_)
            | AuthorityGrantV1::CoreAdministration(_)
            | AuthorityGrantV1::Replay(_)
            | AuthorityGrantV1::Diagnostic(_)
            | AuthorityGrantV1::RunnerControl(_)
            | AuthorityGrantV1::ExternalInput(_) => Err(AuthorityErrorV1::Unavailable),
        }
    }

    /// Authorizes one exact host `ExternalInput` request for its Room root.
    ///
    /// # Errors
    ///
    /// Returns the same closed authority failures as [`Self::authorize`].
    pub fn authorize_external_input(
        &self,
        presented: &PresentedCapabilityV1,
        room_id: RoomId,
        request_hash: CanonicalRequestHashV1,
        checked_at: AuthorityCheckedAt,
    ) -> Result<AuthorizedExternalInputV1, AuthorityErrorV1> {
        match self.authorize(
            presented,
            AuthorityUseV1::ExternalInput {
                room_id,
                request_hash,
            },
            checked_at,
        )? {
            AuthorityGrantV1::ExternalInput(grant) => Ok(grant),
            _ => Err(AuthorityErrorV1::Unavailable),
        }
    }

    /// Authorizes one exact historical Replay address and projection policy.
    /// The returned grant must be consumed by a storage adapter's
    /// present-authorized Replay facade, which revalidates it immediately
    /// before capturing lineage for pure historical projection.
    ///
    /// # Errors
    ///
    /// Returns a safe authentication, scope, Membership-binding, expiry, or
    /// storage failure.
    pub fn authorize_replay(
        &self,
        presented: &PresentedCapabilityV1,
        room_id: RoomId,
        member_id: MemberId,
        at_room_seq: RoomSequenceV1,
        projection_kind: ReplayProjectionKindV1,
        checked_at: AuthorityCheckedAt,
    ) -> Result<AuthorizedReplayV1, AuthorityErrorV1> {
        match self.authorize_inner(
            presented,
            AuthorityUseV1::Replay {
                room_id,
                member_id,
                at_room_seq,
                projection_kind,
            },
            checked_at,
        )? {
            AuthorityGrantV1::Replay(grant) => Ok(grant),
            AuthorityGrantV1::ReceiptRead(_)
            | AuthorityGrantV1::MemberRead(_)
            | AuthorityGrantV1::ParticipantAction(_)
            | AuthorityGrantV1::RoomCreation(_)
            | AuthorityGrantV1::CoreAdministration(_)
            | AuthorityGrantV1::TimerFired(_)
            | AuthorityGrantV1::Diagnostic(_)
            | AuthorityGrantV1::RunnerControl(_)
            | AuthorityGrantV1::ExternalInput(_) => Err(AuthorityErrorV1::Unavailable),
        }
    }

    /// Applies one host-authorized operational authority change.
    /// The Adapter independently samples trusted commit time after acquiring
    /// its durable transaction guard.
    ///
    /// # Errors
    ///
    /// Returns a safe denial, conflict, stale-generation, or storage error.
    pub fn change(
        &self,
        presented: &PresentedCapabilityV1,
        command: AuthorityChangeV1,
        authorized_at: AuthorityCheckedAt,
    ) -> Result<AuthorityChangeReceiptV1, AuthorityErrorV1> {
        let query = AuthoritySnapshotQueryV1 {
            capability_id: presented.capability_id.clone(),
            membership: None,
            runner_id: None,
        };
        let snapshot = self.authenticate(presented, &query, &authorized_at)?;
        require_global_host_scope(&snapshot, CapabilityScopeV1::OperatorRoomAdmin)?;
        let request_hash = command.request_hash()?;
        let fence = build_fence(
            &snapshot,
            None,
            None,
            request_hash.as_bytes(),
            authorized_at.clone(),
        )?;
        let prepared = PreparedAuthorityChangeV1 {
            actor_fence: fence,
            request_hash,
            command,
            authorized_at,
        };
        self.store.apply_change(&prepared).map_err(map_store_error)
    }

    fn authenticate(
        &self,
        presented: &PresentedCapabilityV1,
        query: &AuthoritySnapshotQueryV1,
        checked_at: &AuthorityCheckedAt,
    ) -> Result<AuthoritySnapshotV1, AuthorityErrorV1> {
        let snapshot = self
            .store
            .snapshot(query)
            .map_err(map_store_error)?
            .ok_or(AuthorityErrorV1::Unauthenticated)?;
        let capability = snapshot.capability();
        if capability.capability_id() != presented.capability_id()
            || !capability
                .token_hash()
                .constant_time_eq(&presented.bearer.token_hash())
            || capability.principal_id() != snapshot.principal().principal_id()
            || snapshot.principal().status() != PrincipalAuthorityStatusV1::Enabled
            || capability.revoked_at().is_some()
            || is_expired(capability.expires_at(), checked_at)
        {
            return Err(AuthorityErrorV1::Unauthenticated);
        }
        capability
            .profile()
            .validate_scopes(capability.scopes())
            .map_err(|_| AuthorityErrorV1::Unavailable)?;
        Ok(snapshot)
    }

    fn authorize_use(
        snapshot: &AuthoritySnapshotV1,
        use_: AuthorityUseV1,
        checked_at: AuthorityCheckedAt,
        purpose_bytes: &[u8],
    ) -> Result<AuthorityGrantV1, AuthorityErrorV1> {
        match use_ {
            AuthorityUseV1::ReadRoomOperationResult {
                identity,
                request_hash,
                target_room_id,
            } => authorize_receipt_read(
                snapshot,
                &identity,
                &request_hash,
                target_room_id.as_ref(),
                checked_at,
                purpose_bytes,
            ),
            AuthorityUseV1::Member {
                room_id,
                member_id,
                operation,
            } => authorize_member(
                snapshot,
                &RoomMembershipKeyV1 { room_id, member_id },
                &operation,
                checked_at,
                purpose_bytes,
            ),
            AuthorityUseV1::CreateRoom { identity, .. } => {
                authorize_creation(snapshot, &identity, checked_at, purpose_bytes)
            }
            AuthorityUseV1::CoreAdministration {
                room_id,
                classified,
                identity,
                ..
            } => authorize_core_administration(
                snapshot,
                &room_id,
                classified,
                &identity,
                checked_at,
                purpose_bytes,
            ),
            AuthorityUseV1::TimerFired {
                room_id,
                request_hash,
            } => authorize_timer_fired(snapshot, &room_id, request_hash, checked_at, purpose_bytes),
            AuthorityUseV1::ExternalInput {
                room_id,
                request_hash,
            } => authorize_external_input(
                snapshot,
                &room_id,
                request_hash,
                checked_at,
                purpose_bytes,
            ),
            AuthorityUseV1::Replay {
                room_id,
                member_id,
                at_room_seq,
                projection_kind,
            } => authorize_replay(
                snapshot,
                &RoomMembershipKeyV1 { room_id, member_id },
                at_room_seq,
                projection_kind,
                checked_at,
                purpose_bytes,
            ),
            AuthorityUseV1::HostDiagnostic { target, operation } => {
                authorize_diagnostic(snapshot, target, operation, checked_at, purpose_bytes)
            }
            AuthorityUseV1::RunnerControl {
                runner_id,
                operation,
                target,
            } => authorize_runner(
                snapshot,
                runner_id,
                operation,
                target,
                checked_at,
                purpose_bytes,
            ),
        }
    }
}

/// Deterministic authority Adapter for conformance and focused tests.
#[cfg(any(test, feature = "conformance-tracer"))]
pub struct InMemoryAuthorityStoreV1 {
    state: Mutex<InMemoryAuthorityStateV1>,
}

#[cfg(any(test, feature = "conformance-tracer"))]
struct InMemoryAuthorityStateV1 {
    principals: BTreeMap<PrincipalId, PrincipalAuthoritySnapshotV1>,
    capabilities: BTreeMap<CapabilityId, CapabilityAuthoritySnapshotV1>,
    memberships: BTreeMap<RoomMembershipKeyV1, MembershipAuthoritySnapshotV1>,
    runners: BTreeMap<RunnerId, RunnerAuthoritySnapshotV1>,
    receipts: BTreeMap<AuthorityChangeId, AuthorityChangeReceiptV1>,
    commit_checked_at: AuthorityCheckedAt,
}

#[cfg(any(test, feature = "conformance-tracer"))]
impl fmt::Debug for InMemoryAuthorityStoreV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("InMemoryAuthorityStoreV1([REDACTED])")
    }
}

#[cfg(any(test, feature = "conformance-tracer"))]
impl Default for InMemoryAuthorityStoreV1 {
    fn default() -> Self {
        let commit_checked_at = "2026-08-15T12:10:00Z"
            .parse()
            .unwrap_or_else(|_| unreachable!("fixed in-memory authority time is valid"));
        Self {
            state: Mutex::new(InMemoryAuthorityStateV1 {
                principals: BTreeMap::new(),
                capabilities: BTreeMap::new(),
                memberships: BTreeMap::new(),
                runners: BTreeMap::new(),
                receipts: BTreeMap::new(),
                commit_checked_at,
            }),
        }
    }
}

#[cfg(any(test, feature = "conformance-tracer"))]
impl InMemoryAuthorityStoreV1 {
    /// Creates an empty deterministic Adapter.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the Adapter-owned trusted clock sample used by later mutations.
    ///
    /// This deterministic hook exists only on the conformance Adapter. Core
    /// still validates the sampled time against the prepared authorization
    /// time inside each guarded mutation.
    ///
    /// # Errors
    ///
    /// Returns unavailable if the Adapter lock is poisoned.
    pub fn set_commit_checked_at(
        &self,
        checked_at: AuthorityCheckedAt,
    ) -> Result<(), AuthorityStoreErrorV1> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| AuthorityStoreErrorV1::Unavailable)?;
        state.commit_checked_at = checked_at;
        Ok(())
    }

    /// Seeds one Principal fixture without bypassing duplicate checks.
    ///
    /// # Errors
    ///
    /// Returns conflict if the Principal already exists.
    pub fn seed_principal(
        &self,
        principal: PrincipalAuthoritySnapshotV1,
    ) -> Result<(), AuthorityStoreErrorV1> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| AuthorityStoreErrorV1::Unavailable)?;
        if state.principals.contains_key(principal.principal_id()) {
            return Err(AuthorityStoreErrorV1::Conflict);
        }
        state
            .principals
            .insert(principal.principal_id().clone(), principal);
        Ok(())
    }

    /// Seeds one Capability fixture without bypassing duplicate checks.
    ///
    /// # Errors
    ///
    /// Returns conflict if the Capability already exists.
    pub fn seed_capability(
        &self,
        capability: CapabilityAuthoritySnapshotV1,
    ) -> Result<(), AuthorityStoreErrorV1> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| AuthorityStoreErrorV1::Unavailable)?;
        if state.capabilities.contains_key(capability.capability_id())
            || state.capabilities.values().any(|stored| {
                stored
                    .token_hash()
                    .constant_time_eq(capability.token_hash())
            })
        {
            return Err(AuthorityStoreErrorV1::Conflict);
        }
        state
            .capabilities
            .insert(capability.capability_id().clone(), capability);
        Ok(())
    }

    /// Seeds one Membership fixture without bypassing duplicate checks.
    ///
    /// # Errors
    ///
    /// Returns conflict if the exact Room/Member address already exists.
    pub fn seed_membership(
        &self,
        membership: MembershipAuthoritySnapshotV1,
    ) -> Result<(), AuthorityStoreErrorV1> {
        let key = RoomMembershipKeyV1 {
            room_id: membership.room_id().clone(),
            member_id: membership.membership().member_id().clone(),
        };
        let mut state = self
            .state
            .lock()
            .map_err(|_| AuthorityStoreErrorV1::Unavailable)?;
        if state.memberships.contains_key(&key) {
            return Err(AuthorityStoreErrorV1::Conflict);
        }
        state.memberships.insert(key, membership);
        Ok(())
    }

    /// Seeds one Runner fixture without bypassing duplicate checks.
    ///
    /// # Errors
    ///
    /// Returns conflict if the Runner already exists.
    pub fn seed_runner(
        &self,
        runner: RunnerAuthoritySnapshotV1,
    ) -> Result<(), AuthorityStoreErrorV1> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| AuthorityStoreErrorV1::Unavailable)?;
        if state.runners.contains_key(runner.runner_id()) {
            return Err(AuthorityStoreErrorV1::Conflict);
        }
        state.runners.insert(runner.runner_id().clone(), runner);
        Ok(())
    }
}

#[cfg(any(test, feature = "conformance-tracer"))]
impl AuthorityStoreV1 for InMemoryAuthorityStoreV1 {
    fn snapshot(
        &self,
        query: &AuthoritySnapshotQueryV1,
    ) -> Result<Option<AuthoritySnapshotV1>, AuthorityStoreErrorV1> {
        let state = self
            .state
            .lock()
            .map_err(|_| AuthorityStoreErrorV1::Unavailable)?;
        snapshot_from_memory(&state, query)
    }

    fn apply_bootstrap(
        &self,
        bootstrap: &PreparedAuthorityBootstrapV1,
    ) -> Result<AuthorityChangeReceiptV1, AuthorityStoreErrorV1> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| AuthorityStoreErrorV1::Unavailable)?;
        if let Some(receipt) = state.receipts.get(bootstrap.change_id()) {
            if receipt.request_hash() != bootstrap.request_hash() {
                return Err(AuthorityStoreErrorV1::Conflict);
            }
            let expected = AuthorityChangeReceiptV1::from_applied_bootstrap(
                bootstrap,
                receipt.result(),
                receipt.resulting_generation(),
                receipt.changed_at().clone(),
            )
            .map_err(|_| AuthorityStoreErrorV1::Corrupt)?;
            return if &expected == receipt {
                Ok(receipt.clone())
            } else {
                Err(AuthorityStoreErrorV1::Corrupt)
            };
        }

        let occupancy = if state.principals.is_empty()
            && state.capabilities.is_empty()
            && state.runners.is_empty()
            && state.receipts.is_empty()
        {
            AuthorityBootstrapStateV1::Empty
        } else {
            AuthorityBootstrapStateV1::Occupied
        };
        let commit_checked_at = state.commit_checked_at.clone();
        let validated = bootstrap.validate_install(occupancy, &commit_checked_at)?;
        state.principals.insert(
            validated.principal().principal_id().clone(),
            validated.principal().clone(),
        );
        state.capabilities.insert(
            validated.capability().capability_id().clone(),
            validated.capability().clone(),
        );
        let receipt = AuthorityChangeReceiptV1::from_applied_bootstrap(
            bootstrap,
            AuthorityChangeResultV1::AuthorityBootstrapped,
            1,
            commit_checked_at,
        )?;
        state
            .receipts
            .insert(bootstrap.change_id().clone(), receipt.clone());
        Ok(receipt)
    }

    fn apply_change(
        &self,
        change: &PreparedAuthorityChangeV1,
    ) -> Result<AuthorityChangeReceiptV1, AuthorityStoreErrorV1> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| AuthorityStoreErrorV1::Unavailable)?;
        let actor = snapshot_from_memory(&state, &change.actor_snapshot_query())?
            .ok_or(AuthorityStoreErrorV1::StaleGeneration)?;
        let commit_checked_at = state.commit_checked_at.clone();
        change.revalidate_actor(&actor, &commit_checked_at)?;

        if let Some(receipt) = state.receipts.get(change.command().change_id()) {
            if receipt.request_hash() == change.request_hash() {
                return Ok(receipt.clone());
            }
            return Err(AuthorityStoreErrorV1::Conflict);
        }

        if let AuthorityChangeV1::RegisterCapability { capability, .. } = change.command()
            && state.capabilities.values().any(|stored| {
                stored.capability_id() != capability.capability_id()
                    && stored
                        .token_hash()
                        .constant_time_eq(capability.token_hash())
            })
        {
            return Err(AuthorityStoreErrorV1::Conflict);
        }

        let target_state = change_state_from_memory(&state, change.command())?;
        let validated = change.validate_target(&target_state, &commit_checked_at)?;
        match &validated {
            ValidatedAuthorityChangeV1::Principal { principal, .. } => {
                state
                    .principals
                    .insert(principal.principal_id().clone(), principal.clone());
            }
            ValidatedAuthorityChangeV1::Capability { capability, .. } => {
                state
                    .capabilities
                    .insert(capability.capability_id().clone(), capability.clone());
            }
            ValidatedAuthorityChangeV1::Runner { runner, .. } => {
                state
                    .runners
                    .insert(runner.runner_id().clone(), runner.clone());
            }
        }
        let receipt = AuthorityChangeReceiptV1::from_applied_change(
            change,
            validated.result(),
            validated.resulting_generation(),
            commit_checked_at,
        )?;
        state
            .receipts
            .insert(change.command().change_id().clone(), receipt.clone());
        Ok(receipt)
    }
}

#[cfg(any(test, feature = "conformance-tracer"))]
fn snapshot_from_memory(
    state: &InMemoryAuthorityStateV1,
    query: &AuthoritySnapshotQueryV1,
) -> Result<Option<AuthoritySnapshotV1>, AuthorityStoreErrorV1> {
    let Some(capability) = state.capabilities.get(query.capability_id()) else {
        return Ok(None);
    };
    let principal = state
        .principals
        .get(capability.principal_id())
        .ok_or(AuthorityStoreErrorV1::Corrupt)?;
    let membership = query
        .membership()
        .and_then(|key| state.memberships.get(key))
        .cloned();
    let runner = query
        .runner_id()
        .and_then(|runner_id| state.runners.get(runner_id))
        .cloned();
    AuthoritySnapshotV1::new(capability.clone(), principal.clone(), membership, runner)
        .map(Some)
        .map_err(|_| AuthorityStoreErrorV1::Corrupt)
}

#[cfg(any(test, feature = "conformance-tracer"))]
fn change_state_from_memory(
    state: &InMemoryAuthorityStateV1,
    command: &AuthorityChangeV1,
) -> Result<AuthorityChangeStateV1, AuthorityStoreErrorV1> {
    let mut parts = AuthorityChangeStatePartsV1 {
        principal: None,
        capability: None,
        membership: None,
        runner: None,
        runner_memberships: Vec::new(),
    };
    match command {
        AuthorityChangeV1::CreatePrincipal { principal_id, .. }
        | AuthorityChangeV1::SetPrincipalStatus { principal_id, .. } => {
            parts.principal = state.principals.get(principal_id).cloned();
        }
        AuthorityChangeV1::RegisterCapability { capability, .. } => {
            parts.principal = state.principals.get(capability.principal_id()).cloned();
            parts.capability = state.capabilities.get(capability.capability_id()).cloned();
            match capability.profile() {
                CapabilityProfileV1::RoomMember { room_id, member_id } => {
                    parts.membership = state
                        .memberships
                        .get(&RoomMembershipKeyV1 {
                            room_id: room_id.clone(),
                            member_id: member_id.clone(),
                        })
                        .cloned();
                }
                CapabilityProfileV1::HostOperator { .. } => {}
                CapabilityProfileV1::RunnerControl {
                    runner_id,
                    permitted_memberships,
                } => {
                    parts.runner = state.runners.get(runner_id).cloned();
                    parts.runner_memberships = permitted_memberships
                        .iter()
                        .filter_map(|key| state.memberships.get(key).cloned())
                        .collect();
                }
            }
        }
        AuthorityChangeV1::RegisterRunner {
            runner_id,
            owner_principal_id,
            ..
        } => {
            parts.principal = state.principals.get(owner_principal_id).cloned();
            parts.runner = state.runners.get(runner_id).cloned();
        }
        AuthorityChangeV1::NarrowCapability { capability_id, .. }
        | AuthorityChangeV1::RevokeCapability { capability_id, .. } => {
            parts.capability = state.capabilities.get(capability_id).cloned();
        }
        AuthorityChangeV1::RevokeRunner { runner_id, .. } => {
            parts.runner = state.runners.get(runner_id).cloned();
        }
    }
    AuthorityChangeStateV1::new(parts).map_err(|_| AuthorityStoreErrorV1::InvalidChange)
}

fn map_store_error(error: AuthorityStoreErrorV1) -> AuthorityErrorV1 {
    match error {
        AuthorityStoreErrorV1::Conflict => AuthorityErrorV1::Conflict,
        AuthorityStoreErrorV1::StaleGeneration => AuthorityErrorV1::StaleAuthorityGeneration,
        AuthorityStoreErrorV1::InvalidChange => AuthorityErrorV1::InvalidAuthorityRequest,
        AuthorityStoreErrorV1::Unavailable | AuthorityStoreErrorV1::Corrupt => {
            AuthorityErrorV1::Unavailable
        }
    }
}

fn is_expired(expires_at: Option<&CapabilityExpiresAt>, checked_at: &AuthorityCheckedAt) -> bool {
    expires_at.is_some_and(|expires_at| {
        compare_timestamp_text(checked_at.as_str(), expires_at.as_str()).is_ge()
    })
}

fn authority_purpose_bytes(use_: &AuthorityUseV1) -> Result<Vec<u8>, AuthorityErrorV1> {
    #[derive(Serialize)]
    struct Purpose<'a> {
        domain: &'static str,
        use_: &'a AuthorityUseV1,
    }
    encode(&Purpose {
        domain: AUTHORITY_PURPOSE_DOMAIN,
        use_,
    })
    .map_err(|_| AuthorityErrorV1::Unavailable)
}

fn build_fence(
    snapshot: &AuthoritySnapshotV1,
    membership: Option<&MembershipAuthoritySnapshotV1>,
    runner: Option<&RunnerAuthoritySnapshotV1>,
    purpose: impl AsRef<[u8]>,
    authorized_at: AuthorityCheckedAt,
) -> Result<AuthorityFenceFactsV1, AuthorityErrorV1> {
    build_fence_with_purpose_hash(
        snapshot,
        membership,
        runner,
        Blake3DigestV1::hash(purpose.as_ref()),
        authorized_at,
    )
}

fn build_fence_with_purpose_hash(
    snapshot: &AuthoritySnapshotV1,
    membership: Option<&MembershipAuthoritySnapshotV1>,
    runner: Option<&RunnerAuthoritySnapshotV1>,
    purpose_hash: Blake3DigestV1,
    authorized_at: AuthorityCheckedAt,
) -> Result<AuthorityFenceFactsV1, AuthorityErrorV1> {
    #[derive(Serialize)]
    struct ScopeRevocation<'a> {
        domain: &'static str,
        capability_id: &'a CapabilityId,
        authenticated_principal: &'a PrincipalId,
        principal_generation: PrincipalGenerationV1,
        principal_kind: PrincipalKindV1,
        principal_status: PrincipalAuthorityStatusV1,
        profile: &'a CapabilityProfileV1,
        scopes: &'a CapabilityScopeSetV1,
        authority_generation: AuthorityGenerationV1,
        expires_at: Option<&'a CapabilityExpiresAt>,
        revoked: bool,
        membership: Option<(&'a RoomId, &'a MembershipV1, MembershipGenerationV1)>,
        runner: Option<(
            &'a RunnerId,
            &'a PrincipalId,
            RunnerAuthorityStatusV1,
            RunnerGenerationV1,
        )>,
    }
    let capability = snapshot.capability();
    let membership_fence =
        membership.map(|member| (member.room_id(), member.membership(), member.generation()));
    let runner_fence = runner.map(|runner| {
        (
            runner.runner_id(),
            runner.owner_principal_id(),
            runner.status(),
            runner.generation(),
        )
    });
    let canonical_scope_revocation_bytes = encode(&ScopeRevocation {
        domain: AUTHORITY_FENCE_DOMAIN,
        capability_id: capability.capability_id(),
        authenticated_principal: snapshot.principal().principal_id(),
        principal_generation: snapshot.principal().generation(),
        principal_kind: snapshot.principal().kind(),
        principal_status: snapshot.principal().status(),
        profile: capability.profile(),
        scopes: capability.scopes(),
        authority_generation: capability.generation(),
        expires_at: capability.expires_at(),
        revoked: capability.revoked_at().is_some(),
        membership: membership_fence,
        runner: runner_fence,
    })
    .map_err(|_| AuthorityErrorV1::Unavailable)?;
    let scope_revocation_hash = Blake3DigestV1::hash(&canonical_scope_revocation_bytes);
    Ok(AuthorityFenceFactsV1 {
        capability_id: capability.capability_id().clone(),
        authenticated_principal: snapshot.principal().principal_id().clone(),
        principal_generation: snapshot.principal().generation(),
        authority_generation: capability.generation(),
        canonical_scope_revocation_bytes,
        scope_revocation_hash,
        purpose_hash,
        authorized_at,
        expires_at: capability.expires_at().cloned(),
        membership: membership.map(|member| {
            (
                RoomMembershipKeyV1 {
                    room_id: member.room_id().clone(),
                    member_id: member.membership().member_id().clone(),
                },
                member.generation(),
            )
        }),
        runner: runner.map(|runner| (runner.runner_id().clone(), runner.generation())),
    })
}

fn require_scope(
    snapshot: &AuthoritySnapshotV1,
    scope: CapabilityScopeV1,
) -> Result<(), AuthorityErrorV1> {
    if snapshot.capability().scopes().contains(scope) {
        Ok(())
    } else {
        Err(AuthorityErrorV1::Forbidden)
    }
}

fn require_host_scope(
    snapshot: &AuthoritySnapshotV1,
    target_room_id: Option<&RoomId>,
    scope: CapabilityScopeV1,
) -> Result<(), AuthorityErrorV1> {
    require_scope(snapshot, scope)?;
    let CapabilityProfileV1::HostOperator { room_id } = snapshot.capability().profile() else {
        return Err(AuthorityErrorV1::Forbidden);
    };
    match (room_id.as_ref(), target_room_id) {
        (None, _) => Ok(()),
        (Some(bound), Some(target)) if bound == target => Ok(()),
        _ => Err(AuthorityErrorV1::Forbidden),
    }
}

fn require_global_host_scope(
    snapshot: &AuthoritySnapshotV1,
    scope: CapabilityScopeV1,
) -> Result<(), AuthorityErrorV1> {
    require_host_scope(snapshot, None, scope)
}

fn bound_membership<'a>(
    snapshot: &'a AuthoritySnapshotV1,
    key: &RoomMembershipKeyV1,
) -> Result<&'a MembershipAuthoritySnapshotV1, AuthorityErrorV1> {
    let CapabilityProfileV1::RoomMember { room_id, member_id } = snapshot.capability().profile()
    else {
        return Err(AuthorityErrorV1::Forbidden);
    };
    let membership = snapshot.membership().ok_or(AuthorityErrorV1::Forbidden)?;
    if room_id != &key.room_id
        || member_id != &key.member_id
        || membership.room_id() != &key.room_id
        || membership.membership().member_id() != &key.member_id
        || membership.membership().principal_id() != snapshot.principal().principal_id()
        || membership.membership().principal_kind() != snapshot.principal().kind()
    {
        return Err(AuthorityErrorV1::Forbidden);
    }
    Ok(membership)
}

fn ensure_enabled(membership: &MembershipV1) -> Result<(), AuthorityErrorV1> {
    if membership.standing() == MembershipStandingV1::Enabled {
        Ok(())
    } else {
        Err(AuthorityErrorV1::MembershipNotEnabled)
    }
}

fn observation_scope(membership: &MembershipV1) -> CapabilityScopeV1 {
    match membership.access_mode() {
        AccessModeV1::Spectator => CapabilityScopeV1::RoomObservePublic,
        AccessModeV1::Participant | AccessModeV1::Operator => CapabilityScopeV1::RoomObserveMember,
    }
}

fn authorize_receipt_read(
    snapshot: &AuthoritySnapshotV1,
    identity: &OperationIdentityV1,
    request_hash: &CanonicalRequestHashV1,
    target_room_id: Option<&RoomId>,
    checked_at: AuthorityCheckedAt,
    purpose: &[u8],
) -> Result<AuthorityGrantV1, AuthorityErrorV1> {
    match identity {
        OperationIdentityV1::ParticipantAction(identity) => {
            let target_room_id = target_room_id.ok_or(AuthorityErrorV1::InvalidAuthorityRequest)?;
            if target_room_id != &identity.room_id {
                return Err(AuthorityErrorV1::InvalidAuthorityRequest);
            }
            let key = RoomMembershipKeyV1 {
                room_id: identity.room_id.clone(),
                member_id: identity.member_id.clone(),
            };
            let membership = bound_membership(snapshot, &key)?;
            require_scope(snapshot, CapabilityScopeV1::RoomAct)?;
            let fence = build_fence(snapshot, Some(membership), None, purpose, checked_at)?;
            Ok(AuthorityGrantV1::ReceiptRead(AuthorizedReceiptReadV1 {
                fence,
                identity: OperationIdentityV1::ParticipantAction(identity.clone()),
                request_hash: request_hash.clone(),
                target_policy: ReceiptReadTargetPolicyV1::ExactRoom(target_room_id.clone()),
            }))
        }
        OperationIdentityV1::Administration(identity) => {
            if identity.authenticated_principal != *snapshot.principal().principal_id() {
                return Err(AuthorityErrorV1::Forbidden);
            }
            let target_policy = if identity.versioned_operation_kind == CREATE_ROOM_OPERATION_KIND {
                if target_room_id.is_some() {
                    return Err(AuthorityErrorV1::InvalidAuthorityRequest);
                }
                require_global_host_scope(snapshot, CapabilityScopeV1::OperatorRoomAdmin)?;
                ReceiptReadTargetPolicyV1::GlobalCreate
            } else {
                let room_id = target_room_id.ok_or(AuthorityErrorV1::InvalidAuthorityRequest)?;
                require_host_scope(
                    snapshot,
                    Some(room_id),
                    CapabilityScopeV1::OperatorRoomAdmin,
                )?;
                ReceiptReadTargetPolicyV1::ExactRoom(room_id.clone())
            };
            let fence = build_fence(snapshot, None, None, purpose, checked_at)?;
            Ok(AuthorityGrantV1::ReceiptRead(AuthorizedReceiptReadV1 {
                fence,
                identity: OperationIdentityV1::Administration(identity.clone()),
                request_hash: request_hash.clone(),
                target_policy,
            }))
        }
        OperationIdentityV1::TimerFired(_) | OperationIdentityV1::ExternalInput(_) => {
            let room_id = identity
                .room_id()
                .ok_or(AuthorityErrorV1::InvalidAuthorityRequest)?;
            let target_room_id = target_room_id.ok_or(AuthorityErrorV1::InvalidAuthorityRequest)?;
            if target_room_id != room_id {
                return Err(AuthorityErrorV1::InvalidAuthorityRequest);
            }
            require_host_scope(
                snapshot,
                Some(room_id),
                CapabilityScopeV1::OperatorRoomAdmin,
            )?;
            let fence = build_fence(snapshot, None, None, purpose, checked_at)?;
            Ok(AuthorityGrantV1::ReceiptRead(AuthorizedReceiptReadV1 {
                fence,
                identity: identity.clone(),
                request_hash: request_hash.clone(),
                target_policy: ReceiptReadTargetPolicyV1::ExactRoom(target_room_id.clone()),
            }))
        }
    }
}

fn authorize_member(
    snapshot: &AuthoritySnapshotV1,
    key: &RoomMembershipKeyV1,
    operation: &MemberAuthorityUseV1,
    checked_at: AuthorityCheckedAt,
    purpose: &[u8],
) -> Result<AuthorityGrantV1, AuthorityErrorV1> {
    let membership_snapshot = bound_membership(snapshot, key)?;
    let membership = membership_snapshot.membership();
    if let MemberAuthorityUseV1::SubmitAction {
        identity,
        action_type,
        ..
    } = &operation
    {
        if identity.room_id != key.room_id
            || identity.member_id != key.member_id
            || action_type.is_empty()
            || action_type.len() > MAX_ACTION_TYPE_BYTES
        {
            return Err(AuthorityErrorV1::InvalidAuthorityRequest);
        }
        require_scope(snapshot, CapabilityScopeV1::RoomAct)?;
        if membership.access_mode() != AccessModeV1::Participant {
            return Err(AuthorityErrorV1::Forbidden);
        }
        let fence = build_fence(
            snapshot,
            Some(membership_snapshot),
            None,
            purpose,
            checked_at,
        )?;
        if membership.standing() != MembershipStandingV1::Enabled {
            return Ok(AuthorityGrantV1::ParticipantAction(
                ParticipantActionAuthorityV1::StableMembershipNotEnabled(
                    AuthorizedStableActionDispositionV1 {
                        fence,
                        membership: membership.clone(),
                    },
                ),
            ));
        }
        return Ok(AuthorityGrantV1::ParticipantAction(
            ParticipantActionAuthorityV1::EnabledParticipant(AuthorizedParticipantActionV1 {
                fence,
                membership: membership.clone(),
            }),
        ));
    }

    ensure_enabled(membership)?;
    let (scope, read_operation) = match operation {
        MemberAuthorityUseV1::Attach => {
            (CapabilityScopeV1::RoomAttach, MemberReadOperationV1::Attach)
        }
        MemberAuthorityUseV1::CurrentProjection => (
            observation_scope(membership),
            MemberReadOperationV1::CurrentProjection,
        ),
        MemberAuthorityUseV1::CatchUp => (
            observation_scope(membership),
            MemberReadOperationV1::CatchUp,
        ),
        MemberAuthorityUseV1::AcknowledgeObservation => (
            observation_scope(membership),
            MemberReadOperationV1::AcknowledgeObservation,
        ),
        MemberAuthorityUseV1::SubmitAction { .. } => {
            return Err(AuthorityErrorV1::InvalidAuthorityRequest);
        }
    };
    require_scope(snapshot, scope)?;
    let fence = build_fence(
        snapshot,
        Some(membership_snapshot),
        None,
        purpose,
        checked_at,
    )?;
    Ok(AuthorityGrantV1::MemberRead(AuthorizedViewerV1 {
        fence,
        room_id: key.room_id.clone(),
        membership: membership.clone(),
        operation: read_operation,
    }))
}

fn authorize_creation(
    snapshot: &AuthoritySnapshotV1,
    identity: &AdministrationOperationIdentityV1,
    checked_at: AuthorityCheckedAt,
    purpose: &[u8],
) -> Result<AuthorityGrantV1, AuthorityErrorV1> {
    require_global_host_scope(snapshot, CapabilityScopeV1::OperatorRoomAdmin)?;
    if identity.authenticated_principal != *snapshot.principal().principal_id() {
        return Err(AuthorityErrorV1::Forbidden);
    }
    let fence = build_fence(snapshot, None, None, purpose, checked_at)?;
    Ok(AuthorityGrantV1::RoomCreation(AuthorizedRoomCreationV1 {
        fence,
        attribution: CoreAuthorityAttributionV1 {
            principal_id: snapshot.principal().principal_id().clone(),
            authority_kind: CoreAuthorityKindV1::HostOperator,
        },
    }))
}

fn authorize_core_administration(
    snapshot: &AuthoritySnapshotV1,
    room_id: &RoomId,
    classified: ClassifiedCoreAdministrationV1,
    identity: &AdministrationOperationIdentityV1,
    checked_at: AuthorityCheckedAt,
    purpose: &[u8],
) -> Result<AuthorityGrantV1, AuthorityErrorV1> {
    require_host_scope(
        snapshot,
        Some(room_id),
        CapabilityScopeV1::OperatorRoomAdmin,
    )?;
    if identity.authenticated_principal != *snapshot.principal().principal_id() {
        return Err(AuthorityErrorV1::Forbidden);
    }
    let authority_kind = match snapshot.capability().profile() {
        CapabilityProfileV1::HostOperator { room_id: None } => CoreAuthorityKindV1::HostOperator,
        CapabilityProfileV1::HostOperator { room_id: Some(_) } => {
            CoreAuthorityKindV1::RoomAdministrator
        }
        CapabilityProfileV1::RoomMember { .. } | CapabilityProfileV1::RunnerControl { .. } => {
            return Err(AuthorityErrorV1::Forbidden);
        }
    };
    let fence = build_fence(snapshot, None, None, purpose, checked_at)?;
    Ok(AuthorityGrantV1::CoreAdministration(
        AuthorizedCoreAdministrationV1 {
            fence,
            attribution: CoreAuthorityAttributionV1 {
                principal_id: snapshot.principal().principal_id().clone(),
                authority_kind,
            },
            classified,
        },
    ))
}

fn authorize_timer_fired(
    snapshot: &AuthoritySnapshotV1,
    room_id: &RoomId,
    request_hash: CanonicalRequestHashV1,
    checked_at: AuthorityCheckedAt,
    purpose: &[u8],
) -> Result<AuthorityGrantV1, AuthorityErrorV1> {
    require_host_scope(
        snapshot,
        Some(room_id),
        CapabilityScopeV1::OperatorRoomAdmin,
    )?;
    let fence = build_fence(snapshot, None, None, purpose, checked_at)?;
    Ok(AuthorityGrantV1::TimerFired(AuthorizedTimerFiredV1 {
        fence,
        room_id: room_id.clone(),
        request_hash,
    }))
}

fn authorize_external_input(
    snapshot: &AuthoritySnapshotV1,
    room_id: &RoomId,
    request_hash: CanonicalRequestHashV1,
    checked_at: AuthorityCheckedAt,
    purpose: &[u8],
) -> Result<AuthorityGrantV1, AuthorityErrorV1> {
    require_host_scope(
        snapshot,
        Some(room_id),
        CapabilityScopeV1::OperatorRoomAdmin,
    )?;
    let fence = build_fence(snapshot, None, None, purpose, checked_at)?;
    Ok(AuthorityGrantV1::ExternalInput(AuthorizedExternalInputV1 {
        fence,
        room_id: room_id.clone(),
        request_hash,
    }))
}

fn authorize_replay(
    snapshot: &AuthoritySnapshotV1,
    key: &RoomMembershipKeyV1,
    at_room_seq: RoomSequenceV1,
    projection_kind: ReplayProjectionKindV1,
    checked_at: AuthorityCheckedAt,
    purpose: &[u8],
) -> Result<AuthorityGrantV1, AuthorityErrorV1> {
    if projection_kind == ReplayProjectionKindV1::FinalReveal {
        return Err(AuthorityErrorV1::Forbidden);
    }
    let membership = bound_membership(snapshot, key)?;
    ensure_enabled(membership.membership())?;
    require_scope(snapshot, CapabilityScopeV1::RoomReplay)?;
    let fence = build_fence(snapshot, Some(membership), None, purpose, checked_at)?;
    Ok(AuthorityGrantV1::Replay(AuthorizedReplayV1 {
        fence,
        room_id: key.room_id.clone(),
        member_id: key.member_id.clone(),
        at_room_seq,
        membership: membership.membership().clone(),
        projection_kind,
    }))
}

fn authorize_diagnostic(
    snapshot: &AuthoritySnapshotV1,
    target: DiagnosticTargetV1,
    operation: DiagnosticOperationV1,
    checked_at: AuthorityCheckedAt,
    purpose: &[u8],
) -> Result<AuthorityGrantV1, AuthorityErrorV1> {
    let room_id = match &target {
        DiagnosticTargetV1::Deployment => None,
        DiagnosticTargetV1::Room(room_id) => Some(room_id),
    };
    let scope = match operation {
        DiagnosticOperationV1::SafeRoomSummary
        | DiagnosticOperationV1::ActivityPackCatalog
        | DiagnosticOperationV1::Verify => CapabilityScopeV1::OperatorRoomAdmin,
        DiagnosticOperationV1::RawExport
        | DiagnosticOperationV1::Restore
        | DiagnosticOperationV1::Backup => CapabilityScopeV1::OperatorBackup,
    };
    require_host_scope(snapshot, room_id, scope)?;
    let fence = build_fence(snapshot, None, None, purpose, checked_at)?;
    Ok(AuthorityGrantV1::Diagnostic(AuthorizedDiagnosticV1 {
        fence,
        target,
        operation,
    }))
}

fn authorize_runner(
    snapshot: &AuthoritySnapshotV1,
    runner_id: RunnerId,
    operation: RunnerControlOperationV1,
    target: Option<RoomMembershipKeyV1>,
    checked_at: AuthorityCheckedAt,
    purpose: &[u8],
) -> Result<AuthorityGrantV1, AuthorityErrorV1> {
    let CapabilityProfileV1::RunnerControl {
        runner_id: bound_runner_id,
        permitted_memberships,
    } = snapshot.capability().profile()
    else {
        return Err(AuthorityErrorV1::Forbidden);
    };
    let runner = snapshot.runner().ok_or(AuthorityErrorV1::Forbidden)?;
    if bound_runner_id != &runner_id
        || runner.runner_id() != &runner_id
        || runner.owner_principal_id() != snapshot.principal().principal_id()
        || runner.status() != RunnerAuthorityStatusV1::Enabled
    {
        return Err(AuthorityErrorV1::Forbidden);
    }
    let scope = match operation {
        RunnerControlOperationV1::ReceiveOffer => CapabilityScopeV1::ActivationOfferReceive,
        RunnerControlOperationV1::Claim
        | RunnerControlOperationV1::Renew
        | RunnerControlOperationV1::Release => CapabilityScopeV1::ActivationClaim,
        RunnerControlOperationV1::Complete => CapabilityScopeV1::ActivationComplete,
    };
    require_scope(snapshot, scope)?;

    let target = target.ok_or(AuthorityErrorV1::InvalidAuthorityRequest)?;
    if !permitted_memberships.contains(&target) {
        return Err(AuthorityErrorV1::Forbidden);
    }
    let membership = snapshot.membership().ok_or(AuthorityErrorV1::Forbidden)?;
    if membership.room_id() != &target.room_id
        || membership.membership().member_id() != &target.member_id
        || membership.membership().principal_kind() != PrincipalKindV1::Agent
        || membership.membership().standing() != MembershipStandingV1::Enabled
        || membership.membership().access_mode() != AccessModeV1::Participant
        || membership.membership().role().is_none()
    {
        return Err(AuthorityErrorV1::Forbidden);
    }
    let fence = build_fence(
        snapshot,
        Some(membership),
        Some(runner),
        purpose,
        checked_at,
    )?;
    Ok(AuthorityGrantV1::RunnerControl(AuthorizedRunnerControlV1 {
        fence,
        runner_id,
        operation,
        target,
    }))
}
