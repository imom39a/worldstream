//! Deep PostgreSQL adapter for the Core-owned operational authority seam.
//!
//! The public surface is intentionally small: a runtime can authenticate one
//! bearer and can hand the cloned [`PostgresRoomStore`] to Core's
//! [`worldstream_core::AuthorityV1`].  All row shape validation, transaction
//! ordering, generation fencing, receipt replay, and audit persistence stay
//! behind that seam.

use std::{fmt, str::FromStr};

use postgres::{Client, GenericClient, Transaction};
use thiserror::Error;
use worldstream_core::{
    AuthorityBootstrapStateV1, AuthorityChangeReceiptV1, AuthorityChangeResultV1,
    AuthorityChangeStatePartsV1, AuthorityChangeStateV1, AuthorityChangeTargetV1,
    AuthorityChangeV1, AuthorityCheckedAt, AuthorityGenerationV1, AuthorityReasonCodeV1,
    AuthoritySnapshotQueryV1, AuthoritySnapshotV1, AuthorityStoreErrorV1, AuthorityStoreV1,
    CapabilityAuthoritySnapshotPartsV1, CapabilityAuthoritySnapshotV1, CapabilityBearerV1,
    CapabilityExpiresAt, CapabilityId, CapabilityProfileV1, CapabilityRevokedAt,
    CapabilityScopeSetV1, CapabilityScopeV1, CapabilityTokenHashV1, CoreRoomStateV1,
    MembershipAuthoritySnapshotV1, MembershipGenerationV1, MembershipV1,
    PreparedAuthorityBootstrapV1, PreparedAuthorityChangeV1, PresentedCapabilityV1,
    PrincipalAuthoritySnapshotV1, PrincipalAuthorityStatusV1, PrincipalGenerationV1, PrincipalId,
    PrincipalKindV1, RoomId, RoomMembershipKeyV1, RunnerAuthoritySnapshotV1,
    RunnerAuthorityStatusV1, RunnerGenerationV1, RunnerId, RunnerMembershipSetV1,
    ValidatedAuthorityBootstrapV1, ValidatedAuthorityChangeV1,
};

use super::PostgresRoomStore;

const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;
const AUTHORITY_STATE_ID: bool = true;

/// Capability selector and principal metadata resolved from a bearer.
///
/// The bearer is retained only inside the returned Core presentation. Its
/// `Debug` implementation is redacted, and this type never carries a DSN or
/// provider error text.
pub struct PostgresAuthenticatedCapabilityV1 {
    presented: PresentedCapabilityV1,
    principal_id: PrincipalId,
    principal_kind: PrincipalKindV1,
}

impl fmt::Debug for PostgresAuthenticatedCapabilityV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PostgresAuthenticatedCapabilityV1")
            .field("presented", &self.presented)
            .field("principal_id", &self.principal_id)
            .field("principal_kind", &self.principal_kind)
            .finish()
    }
}

impl PostgresAuthenticatedCapabilityV1 {
    /// Returns the Core presentation containing the public Capability selector.
    #[must_use]
    pub fn presented(&self) -> &PresentedCapabilityV1 {
        &self.presented
    }

    /// Consumes the metadata wrapper and returns the Core presentation.
    #[must_use]
    pub fn into_presented(self) -> PresentedCapabilityV1 {
        self.presented
    }

    /// Returns the authenticated Principal identity.
    #[must_use]
    pub fn principal_id(&self) -> &PrincipalId {
        &self.principal_id
    }

    /// Returns the authenticated Principal kind.
    #[must_use]
    pub const fn principal_kind(&self) -> PrincipalKindV1 {
        self.principal_kind
    }
}

/// Closed failures for production bearer authentication.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
pub enum PostgresAuthorityAuthenticationError {
    #[error("PostgreSQL authority storage is unavailable")]
    Unavailable,
    #[error("bearer capability is not authenticated")]
    Unauthenticated,
    #[error("PostgreSQL authority state is corrupt")]
    Corrupt,
}

impl PostgresRoomStore {
    /// Resolves an opaque bearer through the durable PostgreSQL capability
    /// table. The token is hashed in memory and only the hash is sent to the
    /// provider; neither the bearer nor the DSN appears in returned errors.
    pub fn authenticate_bearer(
        &self,
        bearer: CapabilityBearerV1,
    ) -> Result<PostgresAuthenticatedCapabilityV1, PostgresAuthorityAuthenticationError> {
        let token_hash = bearer.token_hash();
        let mut client = self.connect().map_err(|error| {
            self.record_error(&error);
            PostgresAuthorityAuthenticationError::Unavailable
        })?;
        let checked_at = super::postgres_authority_checked_at(&mut client)
            .map_err(|_| PostgresAuthorityAuthenticationError::Unavailable)?;
        let row = client
            .query_opt(
                "SELECT c.capability_id, c.principal_id, p.principal_kind, c.expires_at \
                 FROM worldstream_authority_capabilities c \
                 JOIN worldstream_authority_principals p ON p.principal_id = c.principal_id \
                 WHERE c.token_hash = $1 AND p.authority_status = 'enabled' \
                   AND c.revoked_at IS NULL",
                &[&token_hash.storage_bytes().as_slice()],
            )
            .map_err(|error| {
                self.record_error(&error);
                PostgresAuthorityAuthenticationError::Unavailable
            })?;
        let Some(row) = row else {
            return Err(PostgresAuthorityAuthenticationError::Unauthenticated);
        };
        let capability_id: String = row
            .try_get(0)
            .map_err(|_| PostgresAuthorityAuthenticationError::Corrupt)?;
        let principal_id: String = row
            .try_get(1)
            .map_err(|_| PostgresAuthorityAuthenticationError::Corrupt)?;
        let principal_kind: String = row
            .try_get(2)
            .map_err(|_| PostgresAuthorityAuthenticationError::Corrupt)?;
        let expires_at: Option<String> = row
            .try_get(3)
            .map_err(|_| PostgresAuthorityAuthenticationError::Corrupt)?;
        if let Some(expires_at) = expires_at
            && timestamp_is_expired(&checked_at, &expires_at)
                .map_err(|_| PostgresAuthorityAuthenticationError::Corrupt)?
        {
            return Err(PostgresAuthorityAuthenticationError::Unauthenticated);
        }
        let capability_id = capability_id
            .parse::<CapabilityId>()
            .map_err(|_| PostgresAuthorityAuthenticationError::Corrupt)?;
        let principal_id = principal_id
            .parse::<PrincipalId>()
            .map_err(|_| PostgresAuthorityAuthenticationError::Corrupt)?;
        let principal_kind = principal_kind_from_storage(&principal_kind)
            .map_err(|_| PostgresAuthorityAuthenticationError::Corrupt)?;
        Ok(PostgresAuthenticatedCapabilityV1 {
            presented: PresentedCapabilityV1::new(capability_id, bearer),
            principal_id,
            principal_kind,
        })
    }
}

impl Clone for PostgresRoomStore {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            telemetry: self.telemetry.clone(),
            failpoint: std::sync::Mutex::new(None),
            last_failure: std::sync::Mutex::new(None),
            #[cfg(feature = "conformance-tracer")]
            conformance_capability_commit: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

impl AuthorityStoreV1 for PostgresRoomStore {
    fn snapshot(
        &self,
        query: &AuthoritySnapshotQueryV1,
    ) -> Result<Option<AuthoritySnapshotV1>, AuthorityStoreErrorV1> {
        let mut client = self.connect().map_err(|error| {
            self.record_error(&error);
            AuthorityStoreErrorV1::Unavailable
        })?;
        let mut transaction = begin_consistent_transaction(&mut client)?;
        let snapshot = load_authority_snapshot(&mut transaction, query, false)?;
        transaction
            .commit()
            .map_err(|error| map_provider_error(self, &error))?;
        Ok(snapshot)
    }

    fn apply_bootstrap(
        &self,
        bootstrap: &PreparedAuthorityBootstrapV1,
    ) -> Result<AuthorityChangeReceiptV1, AuthorityStoreErrorV1> {
        let mut client = self.connect().map_err(|error| {
            self.record_error(&error);
            AuthorityStoreErrorV1::Unavailable
        })?;
        let mut transaction = begin_consistent_transaction(&mut client)?;
        lock_authority_state(&mut transaction)?;

        if let Some(receipt) = lookup_bootstrap_receipt(&mut transaction, bootstrap)? {
            transaction
                .commit()
                .map_err(|error| map_provider_error(self, &error))?;
            return Ok(receipt);
        }

        let occupied: bool = transaction
            .query_one(
                "SELECT EXISTS (SELECT 1 FROM worldstream_authority_principals) \
                 OR EXISTS (SELECT 1 FROM worldstream_authority_capabilities) \
                 OR EXISTS (SELECT 1 FROM worldstream_authority_runners) \
                 OR EXISTS (SELECT 1 FROM worldstream_authority_change_receipts)",
                &[],
            )
            .map_err(|error| map_provider_error(self, &error))?
            .try_get(0)
            .map_err(|_| AuthorityStoreErrorV1::Corrupt)?;
        let state = if occupied {
            AuthorityBootstrapStateV1::Occupied
        } else {
            AuthorityBootstrapStateV1::Empty
        };
        let commit_checked_at = super::postgres_authority_checked_at(&mut transaction)?;
        let validated = bootstrap.validate_install(state, &commit_checked_at)?;
        persist_validated_bootstrap(&mut transaction, &validated)?;
        let receipt = AuthorityChangeReceiptV1::from_applied_bootstrap(
            bootstrap,
            AuthorityChangeResultV1::AuthorityBootstrapped,
            1,
            commit_checked_at,
        )?;
        insert_bootstrap_receipt(&mut transaction, &receipt)?;
        transaction
            .commit()
            .map_err(|error| map_provider_error(self, &error))?;
        Ok(receipt)
    }

    fn apply_change(
        &self,
        change: &PreparedAuthorityChangeV1,
    ) -> Result<AuthorityChangeReceiptV1, AuthorityStoreErrorV1> {
        let mut client = self.connect().map_err(|error| {
            self.record_error(&error);
            AuthorityStoreErrorV1::Unavailable
        })?;
        let mut transaction = begin_consistent_transaction(&mut client)?;
        lock_authority_state(&mut transaction)?;

        let actor =
            load_authority_snapshot(&mut transaction, &change.actor_snapshot_query(), true)?
                .ok_or(AuthorityStoreErrorV1::StaleGeneration)?;
        let commit_checked_at = super::postgres_authority_checked_at(&mut transaction)?;
        change.revalidate_actor(&actor, &commit_checked_at)?;

        if let Some(receipt) = lookup_change_receipt(&mut transaction, change)? {
            transaction
                .commit()
                .map_err(|error| map_provider_error(self, &error))?;
            return Ok(receipt);
        }

        if let AuthorityChangeV1::RegisterCapability { capability, .. } = change.command() {
            let token_owner = transaction
                .query_opt(
                    "SELECT capability_id FROM worldstream_authority_capabilities \
                     WHERE token_hash = $1",
                    &[&capability.token_hash().storage_bytes().as_slice()],
                )
                .map_err(|error| map_provider_error(self, &error))?;
            if token_owner.is_some() {
                return Err(AuthorityStoreErrorV1::Conflict);
            }
        }

        let target_state = load_authority_change_state(&mut transaction, change)?;
        let validated = change.validate_target(&target_state, &commit_checked_at)?;
        persist_validated_change(&mut transaction, change, &validated)?;
        let receipt = AuthorityChangeReceiptV1::from_applied_change(
            change,
            validated.result(),
            validated.resulting_generation(),
            commit_checked_at,
        )?;
        insert_change_receipt(&mut transaction, change, &receipt)?;
        transaction
            .commit()
            .map_err(|error| map_provider_error(self, &error))?;
        Ok(receipt)
    }
}

fn begin_consistent_transaction(
    client: &mut Client,
) -> Result<Transaction<'_>, AuthorityStoreErrorV1> {
    let mut transaction = client
        .transaction()
        .map_err(|_| AuthorityStoreErrorV1::Unavailable)?;
    transaction
        .batch_execute("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .map_err(|_| AuthorityStoreErrorV1::Unavailable)?;
    Ok(transaction)
}

fn lock_authority_state<C: GenericClient>(client: &mut C) -> Result<(), AuthorityStoreErrorV1> {
    let row = client
        .query_opt(
            "SELECT authority_id FROM worldstream_authority_state \
             WHERE authority_id = $1 FOR UPDATE",
            &[&AUTHORITY_STATE_ID],
        )
        .map_err(|_| AuthorityStoreErrorV1::Unavailable)?;
    let Some(row) = row else {
        return Err(AuthorityStoreErrorV1::Corrupt);
    };
    let present: bool = row.try_get(0).map_err(|_| AuthorityStoreErrorV1::Corrupt)?;
    if !present {
        return Err(AuthorityStoreErrorV1::Corrupt);
    }
    Ok(())
}

fn timestamp_is_expired(
    checked_at: &AuthorityCheckedAt,
    expires_at: &str,
) -> Result<bool, AuthorityStoreErrorV1> {
    let checked_at = timestamp_key(checked_at.as_str())?;
    let expires_at = timestamp_key(expires_at)?;
    Ok(checked_at >= expires_at)
}

fn timestamp_key(
    value: &str,
) -> Result<(u32, u32, u32, u32, u32, u32, u32), AuthorityStoreErrorV1> {
    let parsed = value
        .parse::<AuthorityCheckedAt>()
        .map_err(|_| AuthorityStoreErrorV1::Corrupt)?;
    let bytes = parsed.as_str().as_bytes();
    let digit = |index: usize| -> Result<u32, AuthorityStoreErrorV1> {
        let high = bytes
            .get(index)
            .copied()
            .filter(u8::is_ascii_digit)
            .ok_or(AuthorityStoreErrorV1::Corrupt)?;
        let low = bytes
            .get(index + 1)
            .copied()
            .filter(u8::is_ascii_digit)
            .ok_or(AuthorityStoreErrorV1::Corrupt)?;
        Ok(u32::from(high - b'0') * 10 + u32::from(low - b'0'))
    };
    let year = bytes
        .get(0..4)
        .and_then(|digits| {
            if digits.iter().all(u8::is_ascii_digit) {
                Some(
                    digits
                        .iter()
                        .fold(0_u32, |value, digit| value * 10 + u32::from(*digit - b'0')),
                )
            } else {
                None
            }
        })
        .ok_or(AuthorityStoreErrorV1::Corrupt)?;
    let month = digit(5)?;
    let day = digit(8)?;
    let hour = digit(11)?;
    let minute = digit(14)?;
    let second = digit(17)?;
    let fraction = if bytes.len() == 20 {
        0
    } else {
        let fraction_bytes = bytes
            .get(20..bytes.len() - 1)
            .ok_or(AuthorityStoreErrorV1::Corrupt)?;
        let mut value = 0_u32;
        for digit in fraction_bytes {
            if !digit.is_ascii_digit() {
                return Err(AuthorityStoreErrorV1::Corrupt);
            }
            value = value * 10 + u32::from(*digit - b'0');
        }
        value * 10_u32.pow(u32::try_from(9_usize.saturating_sub(fraction_bytes.len())).unwrap_or(0))
    };
    Ok((year, month, day, hour, minute, second, fraction))
}

#[allow(clippy::too_many_lines)]
fn load_authority_snapshot<C: GenericClient>(
    client: &mut C,
    query: &AuthoritySnapshotQueryV1,
    lock_rows: bool,
) -> Result<Option<AuthoritySnapshotV1>, AuthorityStoreErrorV1> {
    let Some(capability) = load_capability(client, query.capability_id(), lock_rows)? else {
        return Ok(None);
    };
    let principal = load_principal(client, capability.principal_id(), lock_rows)?
        .ok_or(AuthorityStoreErrorV1::Corrupt)?;
    let membership = query
        .membership()
        .map(|key| load_membership(client, key, lock_rows))
        .transpose()?
        .flatten();
    let runner = query
        .runner_id()
        .map(|runner_id| load_runner(client, runner_id, lock_rows))
        .transpose()?
        .flatten();
    AuthoritySnapshotV1::new(capability, principal, membership, runner)
        .map(Some)
        .map_err(|_| AuthorityStoreErrorV1::Corrupt)
}

pub(super) fn load_authority_snapshot_for_adapter<C: GenericClient>(
    client: &mut C,
    query: &AuthoritySnapshotQueryV1,
) -> Result<Option<AuthoritySnapshotV1>, AuthorityStoreErrorV1> {
    load_authority_snapshot(client, query, true)
}

#[allow(clippy::too_many_lines)]
fn load_capability<C: GenericClient>(
    client: &mut C,
    requested_id: &CapabilityId,
    lock_rows: bool,
) -> Result<Option<CapabilityAuthoritySnapshotV1>, AuthorityStoreErrorV1> {
    let suffix = if lock_rows { " FOR UPDATE" } else { "" };
    let sql = format!(
        "SELECT capability_id, token_hash, principal_id, profile_kind, target_room_id, \
         target_member_id, runner_id, authority_generation, expires_at, revoked_at \
         FROM worldstream_authority_capabilities WHERE capability_id = $1{suffix}"
    );
    let row = client
        .query_opt(&sql, &[&requested_id.as_str()])
        .map_err(|_| AuthorityStoreErrorV1::Unavailable)?;
    let Some(row) = row else {
        return Ok(None);
    };
    let capability_id: String = row.try_get(0).map_err(|_| AuthorityStoreErrorV1::Corrupt)?;
    let token_hash: Vec<u8> = row.try_get(1).map_err(|_| AuthorityStoreErrorV1::Corrupt)?;
    let principal_id: String = row.try_get(2).map_err(|_| AuthorityStoreErrorV1::Corrupt)?;
    let profile_kind: String = row.try_get(3).map_err(|_| AuthorityStoreErrorV1::Corrupt)?;
    let target_room_id: Option<String> =
        row.try_get(4).map_err(|_| AuthorityStoreErrorV1::Corrupt)?;
    let target_member_id: Option<String> =
        row.try_get(5).map_err(|_| AuthorityStoreErrorV1::Corrupt)?;
    let runner_id: Option<String> = row.try_get(6).map_err(|_| AuthorityStoreErrorV1::Corrupt)?;
    let generation: i64 = row.try_get(7).map_err(|_| AuthorityStoreErrorV1::Corrupt)?;
    let expires_at: Option<String> = row.try_get(8).map_err(|_| AuthorityStoreErrorV1::Corrupt)?;
    let revoked_at: Option<String> = row.try_get(9).map_err(|_| AuthorityStoreErrorV1::Corrupt)?;

    let capability_id = parse_text::<CapabilityId>(&capability_id)?;
    if capability_id != *requested_id {
        return Err(AuthorityStoreErrorV1::Corrupt);
    }
    let token_hash = CapabilityTokenHashV1::from_bytes(
        token_hash
            .try_into()
            .map_err(|_| AuthorityStoreErrorV1::Corrupt)?,
    );
    let principal_id = parse_text::<PrincipalId>(&principal_id)?;
    let generation = AuthorityGenerationV1::new(parse_counter(generation)?)
        .map_err(|_| AuthorityStoreErrorV1::Corrupt)?;
    let expires_at = expires_at
        .as_deref()
        .map(parse_text::<CapabilityExpiresAt>)
        .transpose()?;
    let revoked_at = revoked_at
        .as_deref()
        .map(parse_text::<CapabilityRevokedAt>)
        .transpose()?;
    let scopes = load_scopes(client, &capability_id)?;
    let runner_memberships = load_runner_memberships(client, &capability_id)?;
    let profile = match profile_kind.as_str() {
        "room_member" => {
            if runner_id.is_some() || !runner_memberships.is_empty() {
                return Err(AuthorityStoreErrorV1::Corrupt);
            }
            CapabilityProfileV1::RoomMember {
                room_id: parse_text(
                    target_room_id
                        .as_deref()
                        .ok_or(AuthorityStoreErrorV1::Corrupt)?,
                )?,
                member_id: parse_text(
                    target_member_id
                        .as_deref()
                        .ok_or(AuthorityStoreErrorV1::Corrupt)?,
                )?,
            }
        }
        "host_operator" => {
            if target_member_id.is_some() || runner_id.is_some() || !runner_memberships.is_empty() {
                return Err(AuthorityStoreErrorV1::Corrupt);
            }
            CapabilityProfileV1::HostOperator {
                room_id: target_room_id
                    .as_deref()
                    .map(parse_text::<RoomId>)
                    .transpose()?,
            }
        }
        "runner_control" => {
            if target_room_id.is_some() || target_member_id.is_some() {
                return Err(AuthorityStoreErrorV1::Corrupt);
            }
            CapabilityProfileV1::RunnerControl {
                runner_id: parse_text(runner_id.as_deref().ok_or(AuthorityStoreErrorV1::Corrupt)?)?,
                permitted_memberships: RunnerMembershipSetV1::new(runner_memberships)
                    .map_err(|_| AuthorityStoreErrorV1::Corrupt)?,
            }
        }
        _ => return Err(AuthorityStoreErrorV1::Corrupt),
    };
    CapabilityAuthoritySnapshotV1::new(CapabilityAuthoritySnapshotPartsV1 {
        capability_id,
        token_hash,
        principal_id,
        profile,
        scopes,
        generation,
        expires_at,
        revoked_at,
    })
    .map(Some)
    .map_err(|_| AuthorityStoreErrorV1::Corrupt)
}

fn load_scopes<C: GenericClient>(
    client: &mut C,
    capability_id: &CapabilityId,
) -> Result<CapabilityScopeSetV1, AuthorityStoreErrorV1> {
    let rows = client
        .query(
            "SELECT scope FROM worldstream_authority_capability_scopes \
             WHERE capability_id = $1 ORDER BY scope",
            &[&capability_id.as_str()],
        )
        .map_err(|_| AuthorityStoreErrorV1::Unavailable)?;
    let scopes = rows
        .iter()
        .map(|row| {
            let value: String = row.try_get(0).map_err(|_| AuthorityStoreErrorV1::Corrupt)?;
            scope_from_storage(&value)
        })
        .collect::<Result<Vec<_>, _>>()?;
    CapabilityScopeSetV1::new(scopes).map_err(|_| AuthorityStoreErrorV1::Corrupt)
}

fn load_runner_memberships<C: GenericClient>(
    client: &mut C,
    capability_id: &CapabilityId,
) -> Result<Vec<RoomMembershipKeyV1>, AuthorityStoreErrorV1> {
    let rows = client
        .query(
            "SELECT room_id, member_id \
             FROM worldstream_authority_runner_capability_memberships \
             WHERE capability_id = $1 ORDER BY room_id, member_id",
            &[&capability_id.as_str()],
        )
        .map_err(|_| AuthorityStoreErrorV1::Unavailable)?;
    rows.iter()
        .map(|row| {
            let room_id: String = row.try_get(0).map_err(|_| AuthorityStoreErrorV1::Corrupt)?;
            let member_id: String = row.try_get(1).map_err(|_| AuthorityStoreErrorV1::Corrupt)?;
            Ok(RoomMembershipKeyV1 {
                room_id: parse_text(&room_id)?,
                member_id: parse_text(&member_id)?,
            })
        })
        .collect()
}

fn load_principal<C: GenericClient>(
    client: &mut C,
    principal_id: &PrincipalId,
    lock_rows: bool,
) -> Result<Option<PrincipalAuthoritySnapshotV1>, AuthorityStoreErrorV1> {
    let suffix = if lock_rows { " FOR UPDATE" } else { "" };
    let sql = format!(
        "SELECT principal_kind, authority_status, principal_generation \
         FROM worldstream_authority_principals WHERE principal_id = $1{suffix}"
    );
    let row = client
        .query_opt(&sql, &[&principal_id.as_str()])
        .map_err(|_| AuthorityStoreErrorV1::Unavailable)?;
    let Some(row) = row else {
        return Ok(None);
    };
    let kind: String = row.try_get(0).map_err(|_| AuthorityStoreErrorV1::Corrupt)?;
    let status: String = row.try_get(1).map_err(|_| AuthorityStoreErrorV1::Corrupt)?;
    let generation: i64 = row.try_get(2).map_err(|_| AuthorityStoreErrorV1::Corrupt)?;
    let kind = principal_kind_from_storage(&kind)?;
    let status = principal_status_from_storage(&status)?;
    let generation = PrincipalGenerationV1::new(parse_counter(generation)?)
        .map_err(|_| AuthorityStoreErrorV1::Corrupt)?;
    Ok(Some(PrincipalAuthoritySnapshotV1::new(
        principal_id.clone(),
        kind,
        status,
        generation,
    )))
}

fn load_runner<C: GenericClient>(
    client: &mut C,
    runner_id: &RunnerId,
    lock_rows: bool,
) -> Result<Option<RunnerAuthoritySnapshotV1>, AuthorityStoreErrorV1> {
    let suffix = if lock_rows { " FOR UPDATE" } else { "" };
    let sql = format!(
        "SELECT owner_principal_id, authority_status, runner_generation \
         FROM worldstream_authority_runners WHERE runner_id = $1{suffix}"
    );
    let row = client
        .query_opt(&sql, &[&runner_id.as_str()])
        .map_err(|_| AuthorityStoreErrorV1::Unavailable)?;
    let Some(row) = row else {
        return Ok(None);
    };
    let owner: String = row.try_get(0).map_err(|_| AuthorityStoreErrorV1::Corrupt)?;
    let status: String = row.try_get(1).map_err(|_| AuthorityStoreErrorV1::Corrupt)?;
    let generation: i64 = row.try_get(2).map_err(|_| AuthorityStoreErrorV1::Corrupt)?;
    let owner = parse_text::<PrincipalId>(&owner)?;
    let status = runner_status_from_storage(&status)?;
    let generation = RunnerGenerationV1::new(parse_counter(generation)?)
        .map_err(|_| AuthorityStoreErrorV1::Corrupt)?;
    Ok(Some(RunnerAuthoritySnapshotV1::new(
        runner_id.clone(),
        owner,
        status,
        generation,
    )))
}

fn load_membership<C: GenericClient>(
    client: &mut C,
    key: &RoomMembershipKeyV1,
    lock_rows: bool,
) -> Result<Option<MembershipAuthoritySnapshotV1>, AuthorityStoreErrorV1> {
    let suffix = if lock_rows { " FOR SHARE" } else { "" };
    let sql = format!(
        "SELECT membership_bytes, membership_generation \
         FROM worldstream_members WHERE room_id = $1 AND member_id = $2{suffix}"
    );
    let row = client
        .query_opt(&sql, &[&key.room_id.as_str(), &key.member_id.as_str()])
        .map_err(|_| AuthorityStoreErrorV1::Unavailable)?;
    let Some(row) = row else {
        return Ok(None);
    };
    let bytes: Vec<u8> = row.try_get(0).map_err(|_| AuthorityStoreErrorV1::Corrupt)?;
    let generation: i64 = row.try_get(1).map_err(|_| AuthorityStoreErrorV1::Corrupt)?;
    let membership = worldstream_core::CanonicalJsonV1::decode_canonical::<MembershipV1>(&bytes)
        .map_err(|_| AuthorityStoreErrorV1::Corrupt)?;
    if membership.member_id() != &key.member_id {
        return Err(AuthorityStoreErrorV1::Corrupt);
    }
    let core_sql = format!(
        "SELECT core_state_bytes FROM worldstream_materializations \
         WHERE room_id = $1{suffix}"
    );
    let core_row = client
        .query_opt(&core_sql, &[&key.room_id.as_str()])
        .map_err(|_| AuthorityStoreErrorV1::Unavailable)?
        .ok_or(AuthorityStoreErrorV1::Corrupt)?;
    let core_bytes: Vec<u8> = core_row
        .try_get(0)
        .map_err(|_| AuthorityStoreErrorV1::Corrupt)?;
    let core = worldstream_core::CanonicalJsonV1::decode_canonical::<CoreRoomStateV1>(&core_bytes)
        .map_err(|_| AuthorityStoreErrorV1::Corrupt)?;
    if core.membership(&key.member_id) != Some(&membership) {
        return Err(AuthorityStoreErrorV1::Corrupt);
    }
    let generation = MembershipGenerationV1::new(parse_counter(generation)?)
        .map_err(|_| AuthorityStoreErrorV1::Corrupt)?;
    Ok(Some(MembershipAuthoritySnapshotV1::new(
        key.room_id.clone(),
        membership,
        generation,
    )))
}

fn load_authority_change_state<C: GenericClient>(
    client: &mut C,
    change: &PreparedAuthorityChangeV1,
) -> Result<AuthorityChangeStateV1, AuthorityStoreErrorV1> {
    let mut parts = AuthorityChangeStatePartsV1 {
        principal: None,
        capability: None,
        membership: None,
        runner: None,
        runner_memberships: Vec::new(),
    };
    match change.command() {
        AuthorityChangeV1::CreatePrincipal { principal_id, .. }
        | AuthorityChangeV1::SetPrincipalStatus { principal_id, .. } => {
            parts.principal = load_principal(client, principal_id, true)?;
        }
        AuthorityChangeV1::RegisterCapability { capability, .. } => {
            parts.principal = load_principal(client, capability.principal_id(), true)?;
            parts.capability = load_capability(client, capability.capability_id(), true)?;
            match capability.profile() {
                CapabilityProfileV1::RoomMember { room_id, member_id } => {
                    parts.membership = load_membership(
                        client,
                        &RoomMembershipKeyV1 {
                            room_id: room_id.clone(),
                            member_id: member_id.clone(),
                        },
                        true,
                    )?;
                }
                CapabilityProfileV1::HostOperator { .. } => {}
                CapabilityProfileV1::RunnerControl {
                    runner_id,
                    permitted_memberships,
                } => {
                    parts.runner = load_runner(client, runner_id, true)?;
                    for key in permitted_memberships.iter() {
                        if let Some(membership) = load_membership(client, key, true)? {
                            parts.runner_memberships.push(membership);
                        }
                    }
                }
            }
        }
        AuthorityChangeV1::RegisterRunner {
            runner_id,
            owner_principal_id,
            ..
        } => {
            parts.principal = load_principal(client, owner_principal_id, true)?;
            parts.runner = load_runner(client, runner_id, true)?;
        }
        AuthorityChangeV1::NarrowCapability { capability_id, .. }
        | AuthorityChangeV1::RevokeCapability { capability_id, .. } => {
            parts.capability = load_capability(client, capability_id, true)?;
        }
        AuthorityChangeV1::RevokeRunner { runner_id, .. } => {
            parts.runner = load_runner(client, runner_id, true)?;
        }
    }
    AuthorityChangeStateV1::new(parts).map_err(|_| AuthorityStoreErrorV1::Corrupt)
}

#[allow(clippy::too_many_lines)]
fn persist_validated_bootstrap<C: GenericClient>(
    client: &mut C,
    validated: &ValidatedAuthorityBootstrapV1,
) -> Result<(), AuthorityStoreErrorV1> {
    insert_principal(client, validated.principal())?;
    insert_capability(client, validated.capability())
}

#[allow(clippy::too_many_lines)]
fn persist_validated_change<C: GenericClient>(
    client: &mut C,
    change: &PreparedAuthorityChangeV1,
    validated: &ValidatedAuthorityChangeV1,
) -> Result<(), AuthorityStoreErrorV1> {
    match change.command() {
        AuthorityChangeV1::CreatePrincipal { .. } => insert_principal(
            client,
            validated
                .principal()
                .ok_or(AuthorityStoreErrorV1::Corrupt)?,
        )?,
        AuthorityChangeV1::SetPrincipalStatus {
            principal_id,
            expected_generation,
            ..
        } => {
            let principal = validated
                .principal()
                .ok_or(AuthorityStoreErrorV1::Corrupt)?;
            let changed = client
                .execute(
                    "UPDATE worldstream_authority_principals \
                     SET authority_status = $1, principal_generation = $2 \
                     WHERE principal_id = $3 AND principal_generation = $4",
                    &[
                        &principal_status_storage(principal.status()),
                        &to_i64(principal.generation().get())?,
                        &principal_id.as_str(),
                        &to_i64(expected_generation.get())?,
                    ],
                )
                .map_err(|error| map_write_error(&error))?;
            if changed != 1 {
                return Err(AuthorityStoreErrorV1::StaleGeneration);
            }
        }
        AuthorityChangeV1::RegisterCapability { .. } => insert_capability(
            client,
            validated
                .capability()
                .ok_or(AuthorityStoreErrorV1::Corrupt)?,
        )?,
        AuthorityChangeV1::RegisterRunner { .. } => {
            let runner = validated.runner().ok_or(AuthorityStoreErrorV1::Corrupt)?;
            client
                .execute(
                    "INSERT INTO worldstream_authority_runners \
                     (runner_id, owner_principal_id, authority_status, runner_generation) \
                     VALUES ($1, $2, $3, $4)",
                    &[
                        &runner.runner_id().as_str(),
                        &runner.owner_principal_id().as_str(),
                        &runner_status_storage(runner.status()),
                        &to_i64(runner.generation().get())?,
                    ],
                )
                .map_err(|error| map_write_error(&error))?;
        }
        AuthorityChangeV1::NarrowCapability {
            capability_id,
            expected_generation,
            ..
        } => {
            let capability = validated
                .capability()
                .ok_or(AuthorityStoreErrorV1::Corrupt)?;
            update_capability(client, capability_id, expected_generation.get(), capability)?;
            client
                .execute(
                    "DELETE FROM worldstream_authority_capability_scopes \
                     WHERE capability_id = $1",
                    &[&capability_id.as_str()],
                )
                .map_err(|error| map_write_error(&error))?;
            insert_scopes(client, capability)?;
        }
        AuthorityChangeV1::RevokeCapability {
            capability_id,
            expected_generation,
            ..
        } => {
            let capability = validated
                .capability()
                .ok_or(AuthorityStoreErrorV1::Corrupt)?;
            update_capability(client, capability_id, expected_generation.get(), capability)?;
        }
        AuthorityChangeV1::RevokeRunner {
            runner_id,
            expected_generation,
            ..
        } => {
            let runner = validated.runner().ok_or(AuthorityStoreErrorV1::Corrupt)?;
            if runner.runner_id() != runner_id {
                return Err(AuthorityStoreErrorV1::Corrupt);
            }
            let changed = client
                .execute(
                    "UPDATE worldstream_authority_runners \
                     SET authority_status = $1, runner_generation = $2 \
                     WHERE runner_id = $3 AND runner_generation = $4",
                    &[
                        &runner_status_storage(runner.status()),
                        &to_i64(runner.generation().get())?,
                        &runner_id.as_str(),
                        &to_i64(expected_generation.get())?,
                    ],
                )
                .map_err(|error| map_write_error(&error))?;
            if changed != 1 {
                return Err(AuthorityStoreErrorV1::StaleGeneration);
            }
        }
    }
    Ok(())
}

fn insert_principal<C: GenericClient>(
    client: &mut C,
    principal: &PrincipalAuthoritySnapshotV1,
) -> Result<(), AuthorityStoreErrorV1> {
    client
        .execute(
            "INSERT INTO worldstream_authority_principals \
             (principal_id, principal_kind, authority_status, principal_generation) \
             VALUES ($1, $2, $3, $4)",
            &[
                &principal.principal_id().as_str(),
                &principal_kind_storage(principal.kind()),
                &principal_status_storage(principal.status()),
                &to_i64(principal.generation().get())?,
            ],
        )
        .map_err(|error| map_write_error(&error))?;
    Ok(())
}

fn insert_capability<C: GenericClient>(
    client: &mut C,
    capability: &CapabilityAuthoritySnapshotV1,
) -> Result<(), AuthorityStoreErrorV1> {
    let (profile_kind, target_room_id, target_member_id, runner_id) =
        capability_profile_storage(capability.profile());
    client
        .execute(
            "INSERT INTO worldstream_authority_capabilities \
             (capability_id, token_hash, principal_id, profile_kind, target_room_id, \
              target_member_id, runner_id, authority_generation, expires_at, revoked_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
            &[
                &capability.capability_id().as_str(),
                &capability.token_hash().storage_bytes().as_slice(),
                &capability.principal_id().as_str(),
                &profile_kind,
                &target_room_id,
                &target_member_id,
                &runner_id,
                &to_i64(capability.generation().get())?,
                &capability.expires_at().map(CapabilityExpiresAt::as_str),
                &capability.revoked_at().map(CapabilityRevokedAt::as_str),
            ],
        )
        .map_err(|error| map_write_error(&error))?;
    insert_scopes(client, capability)?;
    if let CapabilityProfileV1::RunnerControl {
        permitted_memberships,
        ..
    } = capability.profile()
    {
        for membership in permitted_memberships.iter() {
            client
                .execute(
                    "INSERT INTO worldstream_authority_runner_capability_memberships \
                     (capability_id, room_id, member_id) VALUES ($1, $2, $3)",
                    &[
                        &capability.capability_id().as_str(),
                        &membership.room_id.as_str(),
                        &membership.member_id.as_str(),
                    ],
                )
                .map_err(|error| map_write_error(&error))?;
        }
    }
    Ok(())
}

fn insert_scopes<C: GenericClient>(
    client: &mut C,
    capability: &CapabilityAuthoritySnapshotV1,
) -> Result<(), AuthorityStoreErrorV1> {
    for scope in capability.scopes().iter() {
        client
            .execute(
                "INSERT INTO worldstream_authority_capability_scopes \
                 (capability_id, scope) VALUES ($1, $2)",
                &[&capability.capability_id().as_str(), &scope_storage(scope)],
            )
            .map_err(|error| map_write_error(&error))?;
    }
    Ok(())
}

fn update_capability<C: GenericClient>(
    client: &mut C,
    capability_id: &CapabilityId,
    expected_generation: u64,
    capability: &CapabilityAuthoritySnapshotV1,
) -> Result<(), AuthorityStoreErrorV1> {
    if capability.capability_id() != capability_id {
        return Err(AuthorityStoreErrorV1::Corrupt);
    }
    let changed = client
        .execute(
            "UPDATE worldstream_authority_capabilities \
             SET authority_generation = $1, expires_at = $2, revoked_at = $3 \
             WHERE capability_id = $4 AND authority_generation = $5",
            &[
                &to_i64(capability.generation().get())?,
                &capability.expires_at().map(CapabilityExpiresAt::as_str),
                &capability.revoked_at().map(CapabilityRevokedAt::as_str),
                &capability_id.as_str(),
                &to_i64(expected_generation)?,
            ],
        )
        .map_err(|error| map_write_error(&error))?;
    if changed != 1 {
        return Err(AuthorityStoreErrorV1::StaleGeneration);
    }
    Ok(())
}

fn insert_bootstrap_receipt<C: GenericClient>(
    client: &mut C,
    receipt: &AuthorityChangeReceiptV1,
) -> Result<(), AuthorityStoreErrorV1> {
    let (target_kind, target_id, secondary_target_id) = target_storage(receipt.target());
    let secondary_target_id = secondary_target_id.ok_or(AuthorityStoreErrorV1::Corrupt)?;
    insert_receipt_rows(
        client,
        None,
        receipt,
        target_kind,
        &target_id,
        Some(&secondary_target_id),
        "bootstrap_authority",
        None,
        None,
    )
}

fn insert_change_receipt<C: GenericClient>(
    client: &mut C,
    change: &PreparedAuthorityChangeV1,
    receipt: &AuthorityChangeReceiptV1,
) -> Result<(), AuthorityStoreErrorV1> {
    let (target_kind, target_id, secondary_target_id) = target_storage(receipt.target());
    if secondary_target_id.is_some() {
        return Err(AuthorityStoreErrorV1::Corrupt);
    }
    let (change_kind, prior_generation, reason_code) = audit_facts(change);
    insert_receipt_rows(
        client,
        Some(change.actor_principal_id().as_str()),
        receipt,
        target_kind,
        &target_id,
        None,
        change_kind,
        prior_generation,
        reason_code,
    )
}

#[allow(clippy::too_many_arguments)]
fn insert_receipt_rows<C: GenericClient>(
    client: &mut C,
    actor_principal_id: Option<&str>,
    receipt: &AuthorityChangeReceiptV1,
    target_kind: &str,
    target_id: &str,
    secondary_target_id: Option<&str>,
    change_kind: &str,
    prior_generation: Option<u64>,
    reason_code: Option<&str>,
) -> Result<(), AuthorityStoreErrorV1> {
    let actor_principal_id = actor_principal_id.map(str::to_owned);
    let secondary_target_id = secondary_target_id.map(str::to_owned);
    let reason_code = reason_code.map(str::to_owned);
    client
        .execute(
            "INSERT INTO worldstream_authority_change_receipts \
             (change_id, authenticated_principal, request_hash, result_kind, target_kind, \
              target_id, secondary_target_id, resulting_generation, checked_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
            &[
                &receipt.change_id().as_str(),
                &actor_principal_id,
                &receipt.request_hash().as_bytes().as_slice(),
                &result_storage(receipt.result()),
                &target_kind,
                &target_id,
                &secondary_target_id,
                &to_i64(receipt.resulting_generation())?,
                &receipt.changed_at().as_str(),
            ],
        )
        .map_err(|error| map_write_error(&error))?;
    client
        .execute(
            "INSERT INTO worldstream_authority_audit \
             (change_id, actor_principal_id, target_kind, target_id, secondary_target_id, \
              change_kind, prior_generation, resulting_generation, checked_at, reason_code, \
              request_hash) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
            &[
                &receipt.change_id().as_str(),
                &actor_principal_id,
                &target_kind,
                &target_id,
                &secondary_target_id,
                &change_kind,
                &prior_generation.map(to_i64).transpose()?,
                &to_i64(receipt.resulting_generation())?,
                &receipt.changed_at().as_str(),
                &reason_code,
                &receipt.request_hash().as_bytes().as_slice(),
            ],
        )
        .map_err(|error| map_write_error(&error))?;
    Ok(())
}

fn lookup_bootstrap_receipt<C: GenericClient>(
    client: &mut C,
    bootstrap: &PreparedAuthorityBootstrapV1,
) -> Result<Option<AuthorityChangeReceiptV1>, AuthorityStoreErrorV1> {
    let Some((receipt, audit)) = load_receipt_pair(client, bootstrap.change_id().as_str())? else {
        return Ok(None);
    };
    validate_receipt_pair(&receipt, &audit)?;
    if receipt.request_hash != bootstrap.request_hash().as_bytes() {
        return Err(AuthorityStoreErrorV1::Conflict);
    }
    let result = result_from_storage(&receipt.result_kind)?;
    let generation = parse_counter(receipt.resulting_generation)?;
    let changed_at = parse_text::<AuthorityCheckedAt>(&receipt.checked_at)?;
    let rebuilt = AuthorityChangeReceiptV1::from_applied_bootstrap(
        bootstrap, result, generation, changed_at,
    )?;
    let (target_kind, target_id, secondary_target_id) = target_storage(rebuilt.target());
    if receipt.authenticated_principal.is_some()
        || receipt.target_kind != target_kind
        || receipt.target_id != target_id
        || receipt.secondary_target_id != secondary_target_id
        || audit.change_kind != "bootstrap_authority"
        || audit.prior_generation.is_some()
        || audit.reason_code.is_some()
    {
        return Err(AuthorityStoreErrorV1::Corrupt);
    }
    Ok(Some(rebuilt))
}

fn lookup_change_receipt<C: GenericClient>(
    client: &mut C,
    change: &PreparedAuthorityChangeV1,
) -> Result<Option<AuthorityChangeReceiptV1>, AuthorityStoreErrorV1> {
    let Some((receipt, audit)) = load_receipt_pair(client, change.command().change_id().as_str())?
    else {
        return Ok(None);
    };
    validate_receipt_pair(&receipt, &audit)?;
    if receipt.request_hash != change.request_hash().as_bytes() {
        return Err(AuthorityStoreErrorV1::Conflict);
    }
    let result = result_from_storage(&receipt.result_kind)?;
    let generation = parse_counter(receipt.resulting_generation)?;
    let changed_at = parse_text::<AuthorityCheckedAt>(&receipt.checked_at)?;
    let rebuilt =
        AuthorityChangeReceiptV1::from_applied_change(change, result, generation, changed_at)?;
    let (target_kind, target_id, secondary_target_id) = target_storage(rebuilt.target());
    let (change_kind, prior_generation, reason_code) = audit_facts(change);
    if receipt.authenticated_principal.as_deref() != Some(change.actor_principal_id().as_str())
        || receipt.target_kind != target_kind
        || receipt.target_id != target_id
        || receipt.secondary_target_id != secondary_target_id
        || audit.actor_principal_id.as_deref() != Some(change.actor_principal_id().as_str())
        || audit.change_kind != change_kind
        || audit.prior_generation != prior_generation.map(to_i64).transpose()?
        || audit.reason_code.as_deref() != reason_code
    {
        return Err(AuthorityStoreErrorV1::Corrupt);
    }
    Ok(Some(rebuilt))
}

struct StoredReceipt {
    authenticated_principal: Option<String>,
    request_hash: Vec<u8>,
    result_kind: String,
    target_kind: String,
    target_id: String,
    secondary_target_id: Option<String>,
    resulting_generation: i64,
    checked_at: String,
}

struct StoredAudit {
    actor_principal_id: Option<String>,
    target_kind: String,
    target_id: String,
    secondary_target_id: Option<String>,
    change_kind: String,
    prior_generation: Option<i64>,
    resulting_generation: i64,
    checked_at: String,
    reason_code: Option<String>,
    request_hash: Vec<u8>,
}

fn load_receipt_pair<C: GenericClient>(
    client: &mut C,
    change_id: &str,
) -> Result<Option<(StoredReceipt, StoredAudit)>, AuthorityStoreErrorV1> {
    let receipt_row = client
        .query_opt(
            "SELECT authenticated_principal, request_hash, result_kind, target_kind, \
             target_id, secondary_target_id, resulting_generation, checked_at \
             FROM worldstream_authority_change_receipts WHERE change_id = $1 FOR SHARE",
            &[&change_id],
        )
        .map_err(|_| AuthorityStoreErrorV1::Unavailable)?;
    let Some(receipt_row) = receipt_row else {
        return Ok(None);
    };
    let receipt = StoredReceipt {
        authenticated_principal: receipt_row
            .try_get(0)
            .map_err(|_| AuthorityStoreErrorV1::Corrupt)?,
        request_hash: receipt_row
            .try_get(1)
            .map_err(|_| AuthorityStoreErrorV1::Corrupt)?,
        result_kind: receipt_row
            .try_get(2)
            .map_err(|_| AuthorityStoreErrorV1::Corrupt)?,
        target_kind: receipt_row
            .try_get(3)
            .map_err(|_| AuthorityStoreErrorV1::Corrupt)?,
        target_id: receipt_row
            .try_get(4)
            .map_err(|_| AuthorityStoreErrorV1::Corrupt)?,
        secondary_target_id: receipt_row
            .try_get(5)
            .map_err(|_| AuthorityStoreErrorV1::Corrupt)?,
        resulting_generation: receipt_row
            .try_get(6)
            .map_err(|_| AuthorityStoreErrorV1::Corrupt)?,
        checked_at: receipt_row
            .try_get(7)
            .map_err(|_| AuthorityStoreErrorV1::Corrupt)?,
    };
    let audit_row = client
        .query_opt(
            "SELECT actor_principal_id, target_kind, target_id, secondary_target_id, \
             change_kind, prior_generation, resulting_generation, checked_at, reason_code, \
             request_hash FROM worldstream_authority_audit WHERE change_id = $1 FOR SHARE",
            &[&change_id],
        )
        .map_err(|_| AuthorityStoreErrorV1::Unavailable)?
        .ok_or(AuthorityStoreErrorV1::Corrupt)?;
    let audit = StoredAudit {
        actor_principal_id: audit_row
            .try_get(0)
            .map_err(|_| AuthorityStoreErrorV1::Corrupt)?,
        target_kind: audit_row
            .try_get(1)
            .map_err(|_| AuthorityStoreErrorV1::Corrupt)?,
        target_id: audit_row
            .try_get(2)
            .map_err(|_| AuthorityStoreErrorV1::Corrupt)?,
        secondary_target_id: audit_row
            .try_get(3)
            .map_err(|_| AuthorityStoreErrorV1::Corrupt)?,
        change_kind: audit_row
            .try_get(4)
            .map_err(|_| AuthorityStoreErrorV1::Corrupt)?,
        prior_generation: audit_row
            .try_get(5)
            .map_err(|_| AuthorityStoreErrorV1::Corrupt)?,
        resulting_generation: audit_row
            .try_get(6)
            .map_err(|_| AuthorityStoreErrorV1::Corrupt)?,
        checked_at: audit_row
            .try_get(7)
            .map_err(|_| AuthorityStoreErrorV1::Corrupt)?,
        reason_code: audit_row
            .try_get(8)
            .map_err(|_| AuthorityStoreErrorV1::Corrupt)?,
        request_hash: audit_row
            .try_get(9)
            .map_err(|_| AuthorityStoreErrorV1::Corrupt)?,
    };
    Ok(Some((receipt, audit)))
}

#[allow(clippy::too_many_lines)]
fn validate_receipt_pair(
    receipt: &StoredReceipt,
    audit: &StoredAudit,
) -> Result<(), AuthorityStoreErrorV1> {
    if receipt.request_hash.len() != 32
        || audit.request_hash != receipt.request_hash
        || audit.actor_principal_id != receipt.authenticated_principal
        || audit.target_kind != receipt.target_kind
        || audit.target_id != receipt.target_id
        || audit.secondary_target_id != receipt.secondary_target_id
        || audit.resulting_generation != receipt.resulting_generation
        || audit.checked_at != receipt.checked_at
    {
        return Err(AuthorityStoreErrorV1::Corrupt);
    }
    if let Some(actor) = receipt.authenticated_principal.as_deref() {
        parse_text::<PrincipalId>(actor)?;
    }
    let checked_at = parse_text::<AuthorityCheckedAt>(&receipt.checked_at)?;
    parse_text::<AuthorityCheckedAt>(&audit.checked_at)?;
    let generation = parse_counter(receipt.resulting_generation)?;
    let prior = audit.prior_generation.map(parse_counter).transpose()?;
    let result = result_from_storage(&receipt.result_kind)?;
    let (
        target_kind,
        change_kind,
        expected_prior,
        needs_reason,
        actor_required,
        secondary_required,
    ) = match result {
        AuthorityChangeResultV1::AuthorityBootstrapped => {
            ("bootstrap", "bootstrap_authority", None, false, false, true)
        }
        AuthorityChangeResultV1::PrincipalCreated => {
            ("principal", "create_principal", None, false, true, false)
        }
        AuthorityChangeResultV1::CapabilityRegistered => (
            "capability",
            "register_capability",
            None,
            false,
            true,
            false,
        ),
        AuthorityChangeResultV1::RunnerRegistered => {
            ("runner", "register_runner", None, false, true, false)
        }
        AuthorityChangeResultV1::CapabilityNarrowed => (
            "capability",
            "narrow_capability",
            generation.checked_sub(1),
            true,
            true,
            false,
        ),
        AuthorityChangeResultV1::CapabilityRevoked => (
            "capability",
            "revoke_capability",
            generation.checked_sub(1),
            true,
            true,
            false,
        ),
        AuthorityChangeResultV1::PrincipalStatusChanged => (
            "principal",
            "set_principal_status",
            generation.checked_sub(1),
            true,
            true,
            false,
        ),
        AuthorityChangeResultV1::RunnerRevoked => (
            "runner",
            "revoke_runner",
            generation.checked_sub(1),
            true,
            true,
            false,
        ),
    };
    if receipt.target_kind != target_kind
        || audit.change_kind != change_kind
        || prior != expected_prior
        || audit.reason_code.is_some() != needs_reason
        || receipt.authenticated_principal.is_some() != actor_required
        || receipt.secondary_target_id.is_some() != secondary_required
        || (prior.is_none() && generation != 1)
        || !actor_required && receipt.authenticated_principal.is_some()
    {
        return Err(AuthorityStoreErrorV1::Corrupt);
    }
    match receipt.target_kind.as_str() {
        "principal" => {
            let _: PrincipalId = parse_text(&receipt.target_id)?;
        }
        "capability" => {
            let _: CapabilityId = parse_text(&receipt.target_id)?;
        }
        "runner" => {
            let _: RunnerId = parse_text(&receipt.target_id)?;
        }
        "bootstrap" => {
            parse_text::<PrincipalId>(&receipt.target_id)?;
            parse_text::<CapabilityId>(
                receipt
                    .secondary_target_id
                    .as_deref()
                    .ok_or(AuthorityStoreErrorV1::Corrupt)?,
            )?;
        }
        _ => return Err(AuthorityStoreErrorV1::Corrupt),
    }
    if let Some(reason) = audit.reason_code.as_deref() {
        AuthorityReasonCodeV1::new(reason.to_owned())
            .map_err(|_| AuthorityStoreErrorV1::Corrupt)?;
    }
    let _ = checked_at;
    Ok(())
}

fn capability_profile_storage(
    profile: &CapabilityProfileV1,
) -> (&'static str, Option<String>, Option<String>, Option<String>) {
    match profile {
        CapabilityProfileV1::RoomMember { room_id, member_id } => (
            "room_member",
            Some(room_id.to_string()),
            Some(member_id.to_string()),
            None,
        ),
        CapabilityProfileV1::HostOperator { room_id } => (
            "host_operator",
            room_id.as_ref().map(ToString::to_string),
            None,
            None,
        ),
        CapabilityProfileV1::RunnerControl { runner_id, .. } => {
            ("runner_control", None, None, Some(runner_id.to_string()))
        }
    }
}

fn target_storage(target: &AuthorityChangeTargetV1) -> (&'static str, String, Option<String>) {
    match target {
        AuthorityChangeTargetV1::Bootstrap {
            principal_id,
            capability_id,
        } => (
            "bootstrap",
            principal_id.to_string(),
            Some(capability_id.to_string()),
        ),
        AuthorityChangeTargetV1::Principal(principal_id) => {
            ("principal", principal_id.to_string(), None)
        }
        AuthorityChangeTargetV1::Capability(capability_id) => {
            ("capability", capability_id.to_string(), None)
        }
        AuthorityChangeTargetV1::Runner(runner_id) => ("runner", runner_id.to_string(), None),
    }
}

fn audit_facts(change: &PreparedAuthorityChangeV1) -> (&'static str, Option<u64>, Option<&str>) {
    match change.command() {
        AuthorityChangeV1::CreatePrincipal { .. } => ("create_principal", None, None),
        AuthorityChangeV1::RegisterCapability { .. } => ("register_capability", None, None),
        AuthorityChangeV1::RegisterRunner { .. } => ("register_runner", None, None),
        AuthorityChangeV1::NarrowCapability {
            expected_generation,
            reason_code,
            ..
        } => (
            "narrow_capability",
            Some(expected_generation.get()),
            Some(reason_code.as_str()),
        ),
        AuthorityChangeV1::RevokeCapability {
            expected_generation,
            reason_code,
            ..
        } => (
            "revoke_capability",
            Some(expected_generation.get()),
            Some(reason_code.as_str()),
        ),
        AuthorityChangeV1::RevokeRunner {
            expected_generation,
            reason_code,
            ..
        } => (
            "revoke_runner",
            Some(expected_generation.get()),
            Some(reason_code.as_str()),
        ),
        AuthorityChangeV1::SetPrincipalStatus {
            expected_generation,
            reason_code,
            ..
        } => (
            "set_principal_status",
            Some(expected_generation.get()),
            Some(reason_code.as_str()),
        ),
    }
}

fn principal_kind_storage(kind: PrincipalKindV1) -> &'static str {
    match kind {
        PrincipalKindV1::Human => "human",
        PrincipalKindV1::Agent => "agent",
    }
}

fn principal_kind_from_storage(value: &str) -> Result<PrincipalKindV1, AuthorityStoreErrorV1> {
    match value {
        "human" => Ok(PrincipalKindV1::Human),
        "agent" => Ok(PrincipalKindV1::Agent),
        _ => Err(AuthorityStoreErrorV1::Corrupt),
    }
}

fn principal_status_storage(status: PrincipalAuthorityStatusV1) -> &'static str {
    match status {
        PrincipalAuthorityStatusV1::Enabled => "enabled",
        PrincipalAuthorityStatusV1::Disabled => "disabled",
    }
}

fn principal_status_from_storage(
    value: &str,
) -> Result<PrincipalAuthorityStatusV1, AuthorityStoreErrorV1> {
    match value {
        "enabled" => Ok(PrincipalAuthorityStatusV1::Enabled),
        "disabled" => Ok(PrincipalAuthorityStatusV1::Disabled),
        _ => Err(AuthorityStoreErrorV1::Corrupt),
    }
}

fn runner_status_storage(status: RunnerAuthorityStatusV1) -> &'static str {
    match status {
        RunnerAuthorityStatusV1::Enabled => "enabled",
        RunnerAuthorityStatusV1::Revoked => "revoked",
    }
}

fn runner_status_from_storage(
    value: &str,
) -> Result<RunnerAuthorityStatusV1, AuthorityStoreErrorV1> {
    match value {
        "enabled" => Ok(RunnerAuthorityStatusV1::Enabled),
        "revoked" => Ok(RunnerAuthorityStatusV1::Revoked),
        _ => Err(AuthorityStoreErrorV1::Corrupt),
    }
}

fn scope_storage(scope: CapabilityScopeV1) -> &'static str {
    match scope {
        CapabilityScopeV1::RoomAttach => "room:attach",
        CapabilityScopeV1::RoomAct => "room:act",
        CapabilityScopeV1::RoomObservePublic => "room:observe_public",
        CapabilityScopeV1::RoomObserveMember => "room:observe_member",
        CapabilityScopeV1::RoomReplay => "room:replay",
        CapabilityScopeV1::ActivationOfferReceive => "activation:offer_receive",
        CapabilityScopeV1::ActivationClaim => "activation:claim",
        CapabilityScopeV1::ActivationComplete => "activation:complete",
        CapabilityScopeV1::OperatorRoomAdmin => "operator:room_admin",
        CapabilityScopeV1::OperatorBackup => "operator:backup",
    }
}

fn scope_from_storage(value: &str) -> Result<CapabilityScopeV1, AuthorityStoreErrorV1> {
    match value {
        "room:attach" => Ok(CapabilityScopeV1::RoomAttach),
        "room:act" => Ok(CapabilityScopeV1::RoomAct),
        "room:observe_public" => Ok(CapabilityScopeV1::RoomObservePublic),
        "room:observe_member" => Ok(CapabilityScopeV1::RoomObserveMember),
        "room:replay" => Ok(CapabilityScopeV1::RoomReplay),
        "activation:offer_receive" => Ok(CapabilityScopeV1::ActivationOfferReceive),
        "activation:claim" => Ok(CapabilityScopeV1::ActivationClaim),
        "activation:complete" => Ok(CapabilityScopeV1::ActivationComplete),
        "operator:room_admin" => Ok(CapabilityScopeV1::OperatorRoomAdmin),
        "operator:backup" => Ok(CapabilityScopeV1::OperatorBackup),
        _ => Err(AuthorityStoreErrorV1::Corrupt),
    }
}

fn result_storage(result: AuthorityChangeResultV1) -> &'static str {
    match result {
        AuthorityChangeResultV1::AuthorityBootstrapped => "authority_bootstrapped",
        AuthorityChangeResultV1::PrincipalCreated => "principal_created",
        AuthorityChangeResultV1::CapabilityRegistered => "capability_registered",
        AuthorityChangeResultV1::CapabilityNarrowed => "capability_narrowed",
        AuthorityChangeResultV1::CapabilityRevoked => "capability_revoked",
        AuthorityChangeResultV1::PrincipalStatusChanged => "principal_status_changed",
        AuthorityChangeResultV1::RunnerRegistered => "runner_registered",
        AuthorityChangeResultV1::RunnerRevoked => "runner_revoked",
    }
}

fn result_from_storage(value: &str) -> Result<AuthorityChangeResultV1, AuthorityStoreErrorV1> {
    match value {
        "authority_bootstrapped" => Ok(AuthorityChangeResultV1::AuthorityBootstrapped),
        "principal_created" => Ok(AuthorityChangeResultV1::PrincipalCreated),
        "capability_registered" => Ok(AuthorityChangeResultV1::CapabilityRegistered),
        "capability_narrowed" => Ok(AuthorityChangeResultV1::CapabilityNarrowed),
        "capability_revoked" => Ok(AuthorityChangeResultV1::CapabilityRevoked),
        "principal_status_changed" => Ok(AuthorityChangeResultV1::PrincipalStatusChanged),
        "runner_registered" => Ok(AuthorityChangeResultV1::RunnerRegistered),
        "runner_revoked" => Ok(AuthorityChangeResultV1::RunnerRevoked),
        _ => Err(AuthorityStoreErrorV1::Corrupt),
    }
}

fn parse_counter(value: i64) -> Result<u64, AuthorityStoreErrorV1> {
    u64::try_from(value)
        .ok()
        .filter(|value| *value <= MAX_SAFE_INTEGER as u64)
        .ok_or(AuthorityStoreErrorV1::Corrupt)
}

fn to_i64(value: u64) -> Result<i64, AuthorityStoreErrorV1> {
    i64::try_from(value)
        .ok()
        .filter(|value| *value <= MAX_SAFE_INTEGER)
        .ok_or(AuthorityStoreErrorV1::Corrupt)
}

fn parse_text<T: FromStr>(value: &str) -> Result<T, AuthorityStoreErrorV1> {
    value.parse().map_err(|_| AuthorityStoreErrorV1::Corrupt)
}

fn map_provider_error(store: &PostgresRoomStore, error: &postgres::Error) -> AuthorityStoreErrorV1 {
    store.record_error(error);
    match error.as_db_error().map(|db| db.code().code()) {
        Some("23505") => AuthorityStoreErrorV1::Conflict,
        Some("23503" | "23514" | "22P02") => AuthorityStoreErrorV1::Corrupt,
        _ => AuthorityStoreErrorV1::Unavailable,
    }
}

fn map_write_error(error: &postgres::Error) -> AuthorityStoreErrorV1 {
    match error.as_db_error().map(|db| db.code().code()) {
        Some("23505") => AuthorityStoreErrorV1::Conflict,
        Some("23503" | "23514" | "22P02") => AuthorityStoreErrorV1::Corrupt,
        _ => AuthorityStoreErrorV1::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_format_is_accepted_by_core() {
        let timestamp = "2026-08-22T04:12:13.123456Z".parse::<AuthorityCheckedAt>();
        assert!(timestamp.is_ok());
        if let Ok(timestamp) = timestamp {
            assert!(timestamp.as_str().ends_with('Z'));
        }
    }

    #[test]
    fn storage_vocabulary_is_closed() {
        assert_eq!(principal_kind_storage(PrincipalKindV1::Agent), "agent");
        assert_eq!(
            runner_status_storage(RunnerAuthorityStatusV1::Revoked),
            "revoked"
        );
        assert_eq!(
            scope_storage(CapabilityScopeV1::OperatorRoomAdmin),
            "operator:room_admin"
        );
        assert_eq!(
            result_storage(AuthorityChangeResultV1::RunnerRevoked),
            "runner_revoked"
        );
    }

    #[test]
    fn continued_sql_literals_preserve_token_boundaries() {
        let source = include_str!("authority.rs");
        let mut continuation_count = 0;
        for line in source.lines() {
            let trimmed = line.trim_end();
            if trimmed.ends_with('\\') {
                continuation_count += 1;
                assert!(
                    trimmed
                        .as_bytes()
                        .get(trimmed.len().saturating_sub(2))
                        .is_some_and(u8::is_ascii_whitespace),
                    "SQL continuation must leave whitespace before its backslash: {trimmed}"
                );
            }
        }
        assert!(continuation_count > 0);
        assert!(source.contains("worldstream_authority_state \\\n"));
        assert!(source.contains("worldstream_authority_capabilities \\\n"));
        assert!(source.contains("worldstream_authority_audit \\\n"));
    }
}
