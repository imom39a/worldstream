use std::collections::BTreeSet;

use thiserror::Error;

use crate::{
    AccessModeV1, CoreProposedKindV1, CoreProposedV1, CoreRoomStateV1, MemberId,
    MembershipChangeKindV1, MembershipChangeV1, MembershipStandingV1, MembershipV1, PrincipalId,
    RoomStatusV1, model::CoreProposalClassV1,
};

/// Frozen versioned operation kind used in Core administration identities.
pub const CORE_OPERATION_KIND: &str = "worldstream/core-proposed/v1";

pub(crate) struct ProposedCoreV1 {
    pub state: CoreRoomStateV1,
    pub class: CoreProposalClassV1,
    pub no_change: bool,
}

pub(crate) enum RoleValidationFailureV1 {
    Rejected(String),
    Panicked,
}

pub(crate) enum CheckedCoreErrorV1 {
    Validation(CoreValidationErrorV1),
    RoleValidatorPanicked,
}

impl From<CoreValidationErrorV1> for CheckedCoreErrorV1 {
    fn from(error: CoreValidationErrorV1) -> Self {
        Self::Validation(error)
    }
}

pub(crate) fn validate_core_state<F>(
    state: &CoreRoomStateV1,
    validate_roles: &F,
) -> Result<(), CheckedCoreErrorV1>
where
    F: Fn(&CoreRoomStateV1) -> Result<(), RoleValidationFailureV1>,
{
    validate_core_host_shape(state)?;
    match validate_roles(state) {
        Ok(()) => Ok(()),
        Err(RoleValidationFailureV1::Rejected(detail)) => {
            Err(CoreValidationErrorV1::RolePolicy(detail).into())
        }
        Err(RoleValidationFailureV1::Panicked) => Err(CheckedCoreErrorV1::RoleValidatorPanicked),
    }
}

fn validate_core_host_shape(state: &CoreRoomStateV1) -> Result<(), CoreValidationErrorV1> {
    let mut live_principals = BTreeSet::<PrincipalId>::new();
    for (key, membership) in &state.memberships {
        if key != &membership.member_id {
            return Err(CoreValidationErrorV1::MemberMapKeyMismatch {
                key: key.clone(),
                embedded: membership.member_id.clone(),
            });
        }
        validate_access_role_shape(membership)?;
        if membership.standing != MembershipStandingV1::Departed
            && !live_principals.insert(membership.principal_id.clone())
        {
            return Err(CoreValidationErrorV1::DuplicateLivePrincipal(
                membership.principal_id.clone(),
            ));
        }
    }
    Ok(())
}

pub(crate) fn propose_core<F>(
    before: &CoreRoomStateV1,
    proposal: &CoreProposedV1,
    validate_roles: &F,
) -> Result<ProposedCoreV1, CheckedCoreErrorV1>
where
    F: Fn(&CoreRoomStateV1) -> Result<(), RoleValidationFailureV1>,
{
    validate_core_host_shape(before)?;
    validate_attribution(proposal)?;
    validate_changeset_shape(proposal)?;

    if proposal.kind == CoreProposedKindV1::Archive {
        return Ok(propose_archive(before, proposal)?);
    }

    let changes = &proposal.canonical_changeset.membership_changes;
    let class = classify_changes(changes)?;
    validate_observed_memberships(before, changes)?;

    if before.room_status == RoomStatusV1::Archived
        && changes.iter().any(|change| {
            !matches!(
                change.kind,
                MembershipChangeKindV1::Suspend | MembershipChangeKindV1::Depart
            )
        })
    {
        return Err(CoreValidationErrorV1::ArchivedMutationForbidden.into());
    }

    if changes.iter().all(|change| {
        change.before.as_ref() == Some(&change.after)
            && before.memberships.get(&change.member_id) == Some(&change.after)
    }) {
        for change in changes {
            validate_no_change_target(change)?;
        }
        return Ok(ProposedCoreV1 {
            state: before.clone(),
            class,
            no_change: true,
        });
    }

    let mut after = before.clone();
    for change in changes {
        validate_component_semantics(change)?;
        after
            .memberships
            .insert(change.member_id.clone(), change.after.clone());
    }
    validate_core_host_shape(&after)?;
    match validate_roles(&after) {
        Ok(()) => {}
        Err(RoleValidationFailureV1::Rejected(detail)) => {
            return Err(CoreValidationErrorV1::RolePolicy(detail).into());
        }
        Err(RoleValidationFailureV1::Panicked) => {
            return Err(CheckedCoreErrorV1::RoleValidatorPanicked);
        }
    }

    Ok(ProposedCoreV1 {
        state: after,
        class,
        no_change: false,
    })
}

fn validate_no_change_target(change: &MembershipChangeV1) -> Result<(), CoreValidationErrorV1> {
    let valid = match change.kind {
        MembershipChangeKindV1::Join => false,
        MembershipChangeKindV1::Resume => change.after.standing == MembershipStandingV1::Enabled,
        MembershipChangeKindV1::Suspend => change.after.standing == MembershipStandingV1::Suspended,
        MembershipChangeKindV1::Depart => change.after.standing == MembershipStandingV1::Departed,
        MembershipChangeKindV1::AccessModeChange => true,
        MembershipChangeKindV1::RoleChange => {
            change.after.access_mode == AccessModeV1::Participant && change.after.role.is_some()
        }
    };
    if valid {
        Ok(())
    } else {
        Err(CoreValidationErrorV1::InvalidNoChangeTarget(
            change.member_id.clone(),
        ))
    }
}

fn validate_access_role_shape(membership: &MembershipV1) -> Result<(), CoreValidationErrorV1> {
    match (membership.access_mode, membership.role.is_some()) {
        (AccessModeV1::Participant, true)
        | (AccessModeV1::Spectator | AccessModeV1::Operator, false) => Ok(()),
        (AccessModeV1::Participant, false) => Err(CoreValidationErrorV1::ParticipantMissingRole(
            membership.member_id.clone(),
        )),
        (AccessModeV1::Spectator | AccessModeV1::Operator, true) => Err(
            CoreValidationErrorV1::NonParticipantHasRole(membership.member_id.clone()),
        ),
    }
}

fn validate_attribution(proposal: &CoreProposedV1) -> Result<(), CoreValidationErrorV1> {
    if proposal.canonical_authority_attribution.principal_id
        != proposal.operation_identity.authenticated_principal
    {
        return Err(CoreValidationErrorV1::AuthorityIdentityMismatch);
    }
    if proposal.operation_identity.versioned_operation_kind != CORE_OPERATION_KIND {
        return Err(CoreValidationErrorV1::OperationKindMismatch);
    }
    if proposal.operation_identity.idempotency_key.is_empty() {
        return Err(CoreValidationErrorV1::EmptyIdempotencyKey);
    }
    if proposal.reason_code.is_empty() {
        return Err(CoreValidationErrorV1::EmptyReasonCode);
    }
    Ok(())
}

fn validate_changeset_shape(proposal: &CoreProposedV1) -> Result<(), CoreValidationErrorV1> {
    let changeset = &proposal.canonical_changeset;
    if changeset
        .membership_changes
        .windows(2)
        .any(|pair| pair[0].member_id >= pair[1].member_id)
    {
        return Err(CoreValidationErrorV1::MembershipChangesNotStrictlySorted);
    }

    match proposal.kind {
        CoreProposedKindV1::Archive => {
            if changeset.room_status_change.is_none() || !changeset.membership_changes.is_empty() {
                return Err(CoreValidationErrorV1::ChangesetShapeMismatch);
            }
        }
        CoreProposedKindV1::MembershipChangeSet => {
            if changeset.room_status_change.is_some() || changeset.membership_changes.len() < 2 {
                return Err(CoreValidationErrorV1::ChangesetShapeMismatch);
            }
        }
        kind => {
            let expected = kind
                .component_kind()
                .ok_or(CoreValidationErrorV1::ChangesetShapeMismatch)?;
            if changeset.room_status_change.is_some()
                || changeset.membership_changes.len() != 1
                || changeset.membership_changes[0].kind != expected
            {
                return Err(CoreValidationErrorV1::ChangesetShapeMismatch);
            }
        }
    }
    Ok(())
}

fn classify_changes(
    changes: &[MembershipChangeV1],
) -> Result<CoreProposalClassV1, CoreValidationErrorV1> {
    let first = changes
        .first()
        .ok_or(CoreValidationErrorV1::ChangesetShapeMismatch)?
        .kind
        .veto_class();
    if changes
        .iter()
        .skip(1)
        .any(|change| change.kind.veto_class() != first)
    {
        return Err(CoreValidationErrorV1::MixedVetoClasses);
    }
    Ok(first)
}

fn validate_observed_memberships(
    before: &CoreRoomStateV1,
    changes: &[MembershipChangeV1],
) -> Result<(), CoreValidationErrorV1> {
    for change in changes {
        if change.after.member_id != change.member_id {
            return Err(CoreValidationErrorV1::ChangedMemberIdentity(
                change.member_id.clone(),
            ));
        }
        let current = before.memberships.get(&change.member_id);
        match change.kind {
            MembershipChangeKindV1::Join => {
                if current.is_some() || change.before.is_some() {
                    return Err(CoreValidationErrorV1::JoinIdentityAlreadyExists(
                        change.member_id.clone(),
                    ));
                }
            }
            _ => {
                if current != change.before.as_ref() {
                    return Err(CoreValidationErrorV1::BeforeWitnessMismatch(
                        change.member_id.clone(),
                    ));
                }
            }
        }
    }
    Ok(())
}

fn validate_component_semantics(change: &MembershipChangeV1) -> Result<(), CoreValidationErrorV1> {
    if change.kind == MembershipChangeKindV1::Join {
        if change.after.standing != MembershipStandingV1::Enabled {
            return Err(CoreValidationErrorV1::InvalidStandingTransition(
                change.member_id.clone(),
            ));
        }
        return Ok(());
    }

    let before = change
        .before
        .as_ref()
        .ok_or_else(|| CoreValidationErrorV1::BeforeWitnessMismatch(change.member_id.clone()))?;
    if before.standing == MembershipStandingV1::Departed {
        return Err(CoreValidationErrorV1::DepartedIsTerminal(
            change.member_id.clone(),
        ));
    }
    if before.member_id != change.after.member_id
        || before.principal_id != change.after.principal_id
        || before.principal_kind != change.after.principal_kind
    {
        return Err(CoreValidationErrorV1::ImmutableBindingChanged(
            change.member_id.clone(),
        ));
    }

    let valid = match change.kind {
        MembershipChangeKindV1::Join => false,
        MembershipChangeKindV1::Resume => {
            before.standing == MembershipStandingV1::Suspended
                && change.after.standing == MembershipStandingV1::Enabled
                && same_access_and_role(before, &change.after)
        }
        MembershipChangeKindV1::Suspend => {
            before.standing == MembershipStandingV1::Enabled
                && change.after.standing == MembershipStandingV1::Suspended
                && same_access_and_role(before, &change.after)
        }
        MembershipChangeKindV1::Depart => {
            matches!(
                before.standing,
                MembershipStandingV1::Enabled | MembershipStandingV1::Suspended
            ) && change.after.standing == MembershipStandingV1::Departed
                && same_access_and_role(before, &change.after)
        }
        MembershipChangeKindV1::AccessModeChange => {
            before.standing == change.after.standing
                && before.access_mode != change.after.access_mode
        }
        MembershipChangeKindV1::RoleChange => {
            before.standing == change.after.standing
                && before.access_mode == AccessModeV1::Participant
                && change.after.access_mode == AccessModeV1::Participant
                && before.role != change.after.role
        }
    };
    if !valid {
        return Err(CoreValidationErrorV1::InvalidComponentDelta(
            change.member_id.clone(),
        ));
    }
    Ok(())
}

fn same_access_and_role(before: &MembershipV1, after: &MembershipV1) -> bool {
    before.access_mode == after.access_mode && before.role == after.role
}

fn propose_archive(
    before: &CoreRoomStateV1,
    proposal: &CoreProposedV1,
) -> Result<ProposedCoreV1, CoreValidationErrorV1> {
    let status = proposal
        .canonical_changeset
        .room_status_change
        .as_ref()
        .ok_or(CoreValidationErrorV1::ChangesetShapeMismatch)?;
    if status.before != before.room_status {
        return Err(CoreValidationErrorV1::RoomStatusBeforeMismatch);
    }
    match (status.before, status.after) {
        (RoomStatusV1::Active, RoomStatusV1::Archived) => {
            let mut after = before.clone();
            after.room_status = RoomStatusV1::Archived;
            Ok(ProposedCoreV1 {
                state: after,
                class: CoreProposalClassV1::Mandatory,
                no_change: false,
            })
        }
        (RoomStatusV1::Archived, RoomStatusV1::Archived) => Ok(ProposedCoreV1 {
            state: before.clone(),
            class: CoreProposalClassV1::Mandatory,
            no_change: true,
        }),
        _ => Err(CoreValidationErrorV1::InvalidArchiveTransition),
    }
}

/// A semantic Core state or proposal invariant failure.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum CoreValidationErrorV1 {
    /// The map key and embedded immutable identity disagree.
    #[error("Membership map key {key} differs from embedded Member ID {embedded}")]
    MemberMapKeyMismatch { key: MemberId, embedded: MemberId },
    /// A participant did not carry exactly one Role.
    #[error("participant Membership {0} has no Role")]
    ParticipantMissingRole(MemberId),
    /// A spectator or operator carried a Role.
    #[error("nonparticipant Membership {0} carries a Role")]
    NonParticipantHasRole(MemberId),
    /// Two non-departed Memberships bind the same Principal.
    #[error("Principal {0} has more than one non-departed Membership")]
    DuplicateLivePrincipal(PrincipalId),
    /// The injected pack-fixture Role/cardinality rule rejected the final state.
    #[error("final Role validation failed: {0}")]
    RolePolicy(String),
    /// Attribution and authenticated administration identity disagree.
    #[error("authority attribution and authenticated Principal differ")]
    AuthorityIdentityMismatch,
    /// The administration operation-kind identity is not the frozen Core v1 kind.
    #[error("administration operation kind is not worldstream/core-proposed/v1")]
    OperationKindMismatch,
    /// Idempotency identity must be nonempty.
    #[error("idempotency key must not be empty")]
    EmptyIdempotencyKey,
    /// Stable reason code must be nonempty.
    #[error("reason code must not be empty")]
    EmptyReasonCode,
    /// Proposal kind and exact changeset shape disagree.
    #[error("Core proposal kind does not match its exact changeset shape")]
    ChangesetShapeMismatch,
    /// Membership components must be strictly sorted with no duplicate identity.
    #[error("Membership changes are not strictly sorted by Member ID")]
    MembershipChangesNotStrictlySorted,
    /// Mandatory and vetoable component kinds cannot be atomic together.
    #[error("Membership changeset mixes vetoable and mandatory components")]
    MixedVetoClasses,
    /// After archive only suspend/depart may change Core.
    #[error("archived Room permits only suspend or depart")]
    ArchivedMutationForbidden,
    /// The embedded after value tried to change its Member ID.
    #[error("Membership change for {0} changes its embedded Member ID")]
    ChangedMemberIdentity(MemberId),
    /// Join cannot reuse any historical Member ID.
    #[error("Join reuses existing historical Member ID {0}")]
    JoinIdentityAlreadyExists(MemberId),
    /// Exact before value did not match canonical Core.
    #[error("Membership before witness does not match {0}")]
    BeforeWitnessMismatch(MemberId),
    /// Departed is terminal.
    #[error("departed Membership {0} cannot transition")]
    DepartedIsTerminal(MemberId),
    /// Member/Principal/kind binding is immutable.
    #[error("immutable binding changed for Membership {0}")]
    ImmutableBindingChanged(MemberId),
    /// Standing transition did not match its component kind.
    #[error("invalid standing transition for Membership {0}")]
    InvalidStandingTransition(MemberId),
    /// The exact before/after delta did not match its component kind.
    #[error("invalid component delta for Membership {0}")]
    InvalidComponentDelta(MemberId),
    /// `NoChange` did not already equal the component kind's desired target.
    #[error("invalid NoChange target for Membership {0}")]
    InvalidNoChangeTarget(MemberId),
    /// Status before did not match Core.
    #[error("Room status before witness does not match Core")]
    RoomStatusBeforeMismatch,
    /// Archive is only active-to-archived, with archived-to-archived `NoChange`.
    #[error("invalid irreversible archive transition")]
    InvalidArchiveTransition,
}
