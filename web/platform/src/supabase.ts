import { createClient, type User } from "@supabase/supabase-js";

import {
  PlatformActivityCapacityUnavailableError,
  PlatformCredentialRejectedError,
  PlatformDependencyUnavailableError,
  PlatformRateLimitExceededError,
} from "./bff.js";
import type {
  AuthIdentity,
  AuthSession,
  AuthUser,
  BffDependencies,
  OAuthAttempt,
  PlatformAccount,
  PlatformAuthClient,
  PlatformDataClient,
  MyGamesIndex,
  RefreshedAuthSession,
  SessionRefreshAdmission,
} from "./bff.js";
import type {
  ReconciliationWriteReceipt,
  ResultIntegrityStatus,
  ResultReconciliationCandidate,
  ResultReconciliationData,
  ResultReconciliationState,
  PrestartHouseRunnerRetirementCandidate,
  TerminalHouseRunnerRetirementCandidate,
  TerminalReconciliationState,
} from "./result-reconciliation.js";
import type { OwnedRunMembershipCorrespondence } from "./browser-sessions.js";
import type {
  AccountParticipation,
  FormationLaunchRecord,
  GenesisReconciliation,
  HostedFormationData,
  HostedLaunchMaterial,
  HouseFillChoice,
  HouseFillRecord,
  LaunchCreationResult,
  OwnedRunRecord,
  PrestartAbandonmentCandidate,
} from "./hosted-formation.js";
import {
  readPublicRunDto,
  readRecentResultsDto,
  type PublicRunData,
  type PublicRunRecord,
  type RecentResults,
} from "./public-runs.js";

interface RpcResult {
  readonly data: unknown;
  readonly error: { readonly code?: string; readonly message: string } | null;
}

interface RpcClient {
  rpc(name: string, args?: Record<string, unknown>): Promise<RpcResult>;
}

export interface SupabasePlatformConfig {
  readonly url: string;
  readonly publishableKey: string;
  readonly dataSecretKey: string;
}

export function createSupabaseSchemaHeadReader(url: string, dataSecretKey: string) {
  validateSupabaseUrl(url);
  validateSupabaseKey(dataSecretKey, "secret");
  const client = createClient(url, dataSecretKey, { ...serverClientOptions(), db: { schema: "platform_api" } });
  return async (): Promise<string> => {
    const head = await requiredRpc({ rpc: async (name, args) => client.rpc(name, args) }, "read_hosted_schema_head_v1", {});
    if (typeof head !== "string" || !/^\d{14}$/u.test(head)) throw new PlatformDependencyUnavailableError();
    return head;
  };
}

export interface PlatformAuthAdminClient {
  deleteAuthUser(authUserId: string): Promise<void>;
}

/**
 * Constructs the request Auth factory and the independent secret-key data
 * client. The data client is never given a user access token.
 */
export function createSupabaseBffDependencies(
  config: SupabasePlatformConfig,
): BffDependencies {
  validateSupabaseUrl(config.url);
  if (config.publishableKey === config.dataSecretKey) {
    throw new Error("supabase_key_separation_required");
  }
  validateSupabaseKey(config.publishableKey, "publishable");
  validateSupabaseKey(config.dataSecretKey, "secret");
  const dataClient = new SupabasePlatformDataClient(config.url, config.dataSecretKey);
  return {
    authClient: () => new SupabasePlatformAuthClient(config.url, config.publishableKey),
    dataClient,
    publicRunData: dataClient,
    hostedFormationData: dataClient,
  };
}

/** Builds a separately constructed Auth Admin client for the erasure worker. */
export function createSupabaseAuthAdminClient(
  url: string,
  adminSecretKey: string,
): PlatformAuthAdminClient {
  validateSupabaseUrl(url);
  validateSupabaseKey(adminSecretKey, "secret");
  const client = createClient(url, adminSecretKey, serverClientOptions());
  return {
    async deleteAuthUser(authUserId: string): Promise<void> {
      const { error } = await dependencyCall(() => client.auth.admin.deleteUser(authUserId, false));
      if (error !== null) throw new PlatformDependencyUnavailableError();
    },
  };
}

/** Builds the server-only RPC adapter for terminal and result reconciliation. */
export function createSupabaseResultReconciliationData(
  url: string,
  dataSecretKey: string,
): ResultReconciliationData {
  validateSupabaseUrl(url);
  validateSupabaseKey(dataSecretKey, "secret");
  const client = createClient(url, dataSecretKey, {
    ...serverClientOptions(),
    db: { schema: "platform_api" },
  });
  return new SupabaseResultReconciliationDataClient(
    client.schema("platform_api") as unknown as RpcClient,
  );
}

class SupabasePlatformAuthClient implements PlatformAuthClient {
  readonly #url: string;
  readonly #publishableKey: string;
  readonly #client: ReturnType<typeof createClient>;

  constructor(url: string, publishableKey: string) {
    this.#url = url.replace(/\/+$/u, "");
    this.#publishableKey = publishableKey;
    this.#client = createClient(url, publishableKey, serverClientOptions());
  }

  githubAuthorizeUrl(input: {
    callbackUrl: string;
    codeChallenge: string;
  }): string {
    const url = new URL(`${this.#url}/auth/v1/authorize`);
    url.searchParams.set("provider", "github");
    url.searchParams.set("redirect_to", input.callbackUrl);
    url.searchParams.set("code_challenge", input.codeChallenge);
    url.searchParams.set("code_challenge_method", "s256");
    return url.toString();
  }

  async exchangeCodeForSession(code: string, verifier: string): Promise<AuthSession> {
    let response: Response;
    try {
      response = await fetch(`${this.#url}/auth/v1/token?grant_type=pkce`, {
        method: "POST",
        headers: {
          apikey: this.#publishableKey,
          "content-type": "application/json",
        },
        body: JSON.stringify({ auth_code: code, code_verifier: verifier }),
        redirect: "error",
      });
    } catch (error) {
      if (error instanceof PlatformCredentialRejectedError) throw error;
      throw new PlatformDependencyUnavailableError();
    }
    if (!response.ok) throwForAuthStatus(response.status);
    try {
      const value: unknown = await response.json();
      return readSession(value);
    } catch (error) {
      if (error instanceof PlatformCredentialRejectedError) throw error;
      throw new PlatformDependencyUnavailableError();
    }
  }

  async getClaims(accessToken: string): Promise<{ subject: string }> {
    const { data, error } = await authCall(() => this.#client.auth.getClaims(accessToken));
    const subject = data?.claims.sub;
    if (error !== null) throwForAuthError(error);
    if (typeof subject !== "string" || subject.length === 0) {
      throw new PlatformDependencyUnavailableError();
    }
    return { subject };
  }

  async getUser(accessToken: string): Promise<AuthUser> {
    const { data, error } = await authCall(() => this.#client.auth.getUser(accessToken));
    if (error !== null) throwForAuthError(error);
    if (data.user === null) throw new PlatformDependencyUnavailableError();
    try {
      return readUser(data.user);
    } catch (error) {
      if (error instanceof PlatformCredentialRejectedError) throw error;
      throw new PlatformDependencyUnavailableError();
    }
  }

  async refreshSession(refreshToken: string): Promise<RefreshedAuthSession> {
    const { data, error } = await authCall(() =>
      this.#client.auth.refreshSession({ refresh_token: refreshToken }),
    );
    if (error !== null) throwForAuthError(error);
    if (data.session === null || data.user === null) {
      throw new PlatformDependencyUnavailableError();
    }
    try {
      return {
        accessToken: data.session.access_token,
        refreshToken: data.session.refresh_token,
        expiresAt: requiredExpiry(data.session.expires_at),
        user: readUser(data.user),
      };
    } catch (error) {
      if (error instanceof PlatformCredentialRejectedError) throw error;
      throw new PlatformDependencyUnavailableError();
    }
  }

  async signOut(accessToken: string): Promise<void> {
    let response: Response;
    try {
      response = await fetch(`${this.#url}/auth/v1/logout?scope=global`, {
        method: "POST",
        headers: {
          apikey: this.#publishableKey,
          authorization: `Bearer ${accessToken}`,
        },
        redirect: "error",
      });
    } catch {
      throw new PlatformDependencyUnavailableError();
    }
    if (!response.ok && ![401, 403, 404].includes(response.status)) {
      throw new PlatformDependencyUnavailableError();
    }
  }
}

class SupabasePlatformDataClient implements PlatformDataClient, HostedFormationData, PublicRunData {
  readonly #rpc: RpcClient;

  constructor(url: string, secretKey: string) {
    const client = createClient(url, secretKey, {
      ...serverClientOptions(),
      db: { schema: "platform_api" },
    });
    this.#rpc = client.schema("platform_api") as unknown as RpcClient;
  }

  async beginGithubOAuth(attempt: OAuthAttempt): Promise<boolean> {
    const data = await requiredRpc(this.#rpc, "begin_github_oauth_v1", {
      p_state_digest: attempt.stateDigest,
      p_encrypted_pkce_verifier: attempt.encryptedPkceVerifier,
      p_browser_nonce_digest: attempt.browserNonceDigest,
      p_return_target: attempt.returnTarget,
    });
    const rows = readRows(data);
    if (rows.length > 1) throw new Error("oauth_attempt_ambiguous");
    return rows.length === 1;
  }

  async readPublicRun(publicId: string): Promise<PublicRunRecord> {
    return readPublicRunDto(await requiredRpc(this.#rpc, "read_public_run_v1", {
      p_public_id: publicId,
    }));
  }

  async listRecentResults(limit: number): Promise<RecentResults> {
    return readRecentResultsDto(await requiredRpc(this.#rpc, "list_recent_results_v1", {
      p_limit: limit,
    }));
  }

  async consumeGithubOAuth(input: {
    stateDigest: string;
    browserNonceDigest: string;
    returnTarget: string;
  }): Promise<OAuthAttempt | null> {
    const data = await requiredRpc(this.#rpc, "consume_github_oauth_v1", {
      p_state_digest: input.stateDigest,
      p_browser_nonce_digest: input.browserNonceDigest,
      p_return_target: input.returnTarget,
    });
    const rows = readRows(data);
    if (rows.length === 0) return null;
    if (rows.length !== 1) throw new Error("oauth_attempt_ambiguous");
    const row = rows[0];
    if (row === undefined) throw new Error("oauth_attempt_invalid");
    return {
      stateDigest: input.stateDigest,
      encryptedPkceVerifier: requiredString(row.encrypted_pkce_verifier),
      browserNonceDigest: input.browserNonceDigest,
      returnTarget: requiredString(row.return_target),
    };
  }

  async syncGithubIdentity(input: {
    authUserId: string;
    providerSubject: string;
    githubLogin: string | null;
    avatarUrl: string | null;
  }): Promise<PlatformAccount> {
    const data = await requiredRpc(this.#rpc, "sync_github_identity_v1", {
      p_auth_user_id: input.authUserId,
      p_provider_subject: input.providerSubject,
      p_github_login: input.githubLogin,
      p_avatar_url: input.avatarUrl,
    });
    const account = this.#accountResult(data, true);
    if (account === null) throw new PlatformRateLimitExceededError();
    return account;
  }

  async resolveGithubAccount(input: {
    authUserId: string;
    providerSubject: string;
  }): Promise<PlatformAccount | null> {
    const data = await requiredRpc(this.#rpc, "sync_github_identity_v1", {
      p_auth_user_id: input.authUserId,
      p_provider_subject: input.providerSubject,
    });
    return this.#accountResult(data, true);
  }

  async beginSessionRefresh(input: {
    authUserId: string;
    providerSubject: string;
  }): Promise<SessionRefreshAdmission | null> {
    const data = await requiredRpc(this.#rpc, "sync_github_identity_v1", {
      p_auth_user_id: input.authUserId,
      p_provider_subject: input.providerSubject,
      p_admit_session_refresh: true,
    });
    const rows = readRows(data);
    if (rows.length === 0) return null;
    if (rows.length !== 1 || rows[0] === undefined) {
      throw new PlatformDependencyUnavailableError();
    }
    const account = this.#accountResult(data, false);
    if (account === null) throw new PlatformDependencyUnavailableError();
    return { ...account, admitted: requiredBoolean(rows[0].admitted) };
  }

  async syncGithubIdentityForMutation(input: {
    authUserId: string;
    providerSubject: string;
    githubLogin: string | null;
    avatarUrl: string | null;
  }): Promise<PlatformAccount | null> {
    const data = await requiredRpc(this.#rpc, "sync_github_identity_v1", {
      p_auth_user_id: input.authUserId,
      p_provider_subject: input.providerSubject,
      p_github_login: input.githubLogin,
      p_avatar_url: input.avatarUrl,
    });
    return this.#accountResult(data, true);
  }

  #accountResult(data: unknown, allowEmpty: boolean): PlatformAccount | null {
    const rows = readRows(data);
    if (rows.length === 0 && allowEmpty) return null;
    if (rows.length !== 1 || rows[0] === undefined) {
      throw new PlatformDependencyUnavailableError();
    }
    const row = rows[0];
    if (row.erased_at !== null && row.erased_at !== undefined) {
      throw new PlatformCredentialRejectedError();
    }
    return {
      accountId: requiredString(row.account_id),
      publicProfileEnabled: requiredBoolean(row.public_profile_enabled),
    };
  }

  async setPublicProfile(authUserId: string, enabled: boolean): Promise<boolean> {
    const data = await requiredRpc(this.#rpc, "set_public_profile_v1", {
      p_auth_user_id: authUserId,
      p_enabled: enabled,
    });
    return requiredBoolean(data);
  }

  async beginAccountErasure(authUserId: string): Promise<boolean> {
    const data = await requiredRpc(this.#rpc, "begin_account_erasure_v1", {
      p_auth_user_id: authUserId,
    });
    return readRows(data).length === 1;
  }

  async resolveOwnedRunMembership(input: {
    accountId: string;
    runId: string;
    entrySelector: string;
  }): Promise<OwnedRunMembershipCorrespondence | null> {
    const data = await requiredRpc(this.#rpc, "resolve_owned_run_membership_v1", {
      p_requesting_account_id: input.accountId,
      p_activity_run_id: input.runId,
      p_entry_selector: input.entrySelector,
    });
    if (data === null) return null;
    const value = requiredRecord(data);
    if (
      value.version !== "platform_owned_run_membership.v1" ||
      (value.principal_kind !== "human" && value.principal_kind !== "agent") ||
      (value.purpose !== "participant" && value.purpose !== "creator_spectator") ||
      (value.principal_kind === "agent" && value.purpose !== "participant")
    ) {
      return null;
    }
    const pack = requiredRecord(value.pack);
    return {
      runId: requiredString(value.run_id),
      listingRevisionDigest: requiredString(value.listing_revision_digest),
      hostInstallationId: requiredString(value.host_installation_id),
      roomSetupOperationId: requiredString(value.room_setup_operation_id),
      roomId: requiredString(value.room_id),
      pack: {
        id: requiredString(pack.id),
        version: requiredString(pack.version),
        digest: requiredString(pack.digest),
      },
      clientReleaseDigest: requiredString(value.client_release_digest),
      clientSurfaceId: requiredString(value.client_surface_id),
      accessMode: requiredEnum(value.access_mode, ["participant", "spectator"]),
      purpose: value.purpose,
      seatId: nullableString(value.seat_id),
      role: nullableString(value.role),
      principalKind: value.principal_kind,
      principalId: requiredString(value.principal_id),
      membershipId: requiredString(value.membership_id),
    };
  }

  async createLaunchRequest(input: {
    accountId: string;
    listingRevisionDigest: string;
    idempotencyNamespace: string;
    idempotencyKeyDigest: Uint8Array;
    canonicalLaunchInput: Uint8Array;
    launchInputDigest: Uint8Array;
    houseFillChoice: HouseFillChoice;
    creatorAccessChoice: "seat" | "spectator";
    creatorSeatId: string | null;
  }): Promise<LaunchCreationResult> {
    const data = await requiredRpc(this.#rpc, "create_launch_request_v1", {
      p_creator_account_id: input.accountId,
      p_listing_revision_digest: input.listingRevisionDigest,
      p_idempotency_namespace: input.idempotencyNamespace,
      p_idempotency_key_digest: bytea(input.idempotencyKeyDigest),
      p_canonical_launch_input: bytea(input.canonicalLaunchInput),
      p_launch_input_digest: bytea(input.launchInputDigest),
      p_canonicalizer_version: "worldstream/canonical-json/v1",
      p_house_fill_choice: input.houseFillChoice,
      p_creator_access_choice: input.creatorAccessChoice,
      p_creator_seat_id: input.creatorSeatId,
      p_creator_participation_kind: "account_human",
    });
    const row = singleRow(data);
    return {
      launchRequestId: requiredString(row.launch_request_id),
      state: requiredString(row.launch_state),
      expiresAt: requiredString(row.expires_at),
      wasCreated: requiredBoolean(row.was_created),
    };
  }

  async readLaunchRequest(
    accountId: string,
    launchRequestId: string,
  ): Promise<FormationLaunchRecord | null> {
    const data = await requiredRpc(this.#rpc, "read_launch_request_v1", {
      p_requesting_account_id: accountId,
      p_launch_request_id: launchRequestId,
    });
    if (data === null) return null;
    const value = requiredRecord(data);
    if (value.version !== "platform_launch_request.v1") {
      throw new PlatformDependencyUnavailableError();
    }
    return {
      launchRequestId: requiredString(value.launch_request_id),
      listingRevisionDigest: requiredString(value.listing_revision_digest),
      state: requiredString(value.state),
      expiresAt: requiredString(value.expires_at),
      creatorAccessChoice: requiredEnum(value.creator_access_choice, ["seat", "spectator"]),
      creatorSeatId: nullableString(value.creator_seat_id),
      houseFillChoice: requiredEnum(value.house_fill_choice, ["disabled", "fill_unclaimed"]),
      rosterFrozen: requiredBoolean(value.roster_frozen),
      canManage: requiredBoolean(value.can_manage),
      seats: requiredRecords(value.seats).map((seat) => ({
        seatId: requiredString(seat.seat_id),
        displayName: requiredString(seat.display_name),
        required: requiredBoolean(seat.required),
        claimed: requiredBoolean(seat.claimed),
        claimedByRequester: requiredBoolean(seat.claimed_by_requester),
        participationKind: nullableEnum(seat.participation_kind, [
          "account_human",
          "account_external_agent",
        ]),
      })),
    };
  }

  async rotateSeatInvitation(
    accountId: string,
    launchRequestId: string,
    seatId: string,
  ): Promise<{ token: string; expiresAt: string } | null> {
    const data = await requiredRpc(this.#rpc, "rotate_seat_invitation_v1", {
      p_creator_account_id: accountId,
      p_launch_request_id: launchRequestId,
      p_seat_id: seatId,
    });
    const rows = readRows(data);
    if (rows.length === 0) return null;
    const row = singleRow(data);
    return {
      token: requiredString(row.invitation_token),
      expiresAt: requiredString(row.expires_at),
    };
  }

  async claimInvitedSeat(
    accountId: string,
    tokenDigest: Uint8Array,
    participation: AccountParticipation,
  ): Promise<{ launchRequestId: string; seatId: string } | null> {
    const data = await requiredRpc(this.#rpc, "claim_invited_seat_v1", {
      p_claiming_account_id: accountId,
      p_invitation_token_digest: bytea(tokenDigest),
      p_participation_kind: participation,
    });
    const rows = readRows(data);
    if (rows.length === 0) return null;
    const row = singleRow(data);
    return {
      launchRequestId: requiredString(row.launch_request_id),
      seatId: requiredString(row.seat_id),
    };
  }

  async releaseSeatClaim(accountId: string, launchRequestId: string, seatId: string) {
    return requiredBoolean(await requiredRpc(this.#rpc, "release_seat_claim_v1", {
      p_claiming_account_id: accountId,
      p_launch_request_id: launchRequestId,
      p_seat_id: seatId,
    }));
  }

  async resetSeatClaim(accountId: string, launchRequestId: string, seatId: string) {
    return requiredBoolean(await requiredRpc(this.#rpc, "reset_seat_claim_v1", {
      p_creator_account_id: accountId,
      p_launch_request_id: launchRequestId,
      p_seat_id: seatId,
    }));
  }

  async cancelLaunchRequest(accountId: string, launchRequestId: string) {
    return requiredBoolean(await requiredRpc(this.#rpc, "cancel_launch_request_v1", {
      p_creator_account_id: accountId,
      p_launch_request_id: launchRequestId,
    }));
  }

  async startHouseFill(accountId: string, launchRequestId: string) {
    return parseHouseFill(await requiredRpc(this.#rpc, "start_house_fill_v1", {
      p_creator_account_id: accountId,
      p_launch_request_id: launchRequestId,
    }));
  }

  async readHouseFill(accountId: string, launchRequestId: string) {
    return parseHouseFill(await requiredRpc(this.#rpc, "read_house_fill_v1", {
      p_requesting_account_id: accountId,
      p_launch_request_id: launchRequestId,
    }));
  }

  async retainHouseFillSelection(launchRequestId: string, hostInstallationId: string) {
    return parseHouseFill(await requiredRpc(this.#rpc, "retain_house_fill_selection_v1", {
      p_launch_request_id: launchRequestId,
      p_host_installation_id: hostInstallationId,
    }));
  }

  async recordHouseRunnerReservation(input: {
    reservationOperationId: string;
    outcome: "succeeded" | "terminal_failed";
    runnerUnitId: string | null;
    canonicalReceipt: Uint8Array;
    receiptDigest: Uint8Array;
    failureCode: string | null;
  }) {
    return parseHouseFill(await requiredRpc(this.#rpc, "record_house_runner_reservation_v1", {
      p_reservation_operation_id: input.reservationOperationId,
      p_outcome: input.outcome,
      p_runner_unit_id: input.runnerUnitId,
      p_reservation_receipt: bytea(input.canonicalReceipt),
      p_reservation_receipt_digest: bytea(input.receiptDigest),
      p_failure_code: input.failureCode,
    }));
  }

  async completeHouseFill(launchRequestId: string) {
    return parseHouseFill(await requiredRpc(this.#rpc, "complete_house_fill_v1", {
      p_launch_request_id: launchRequestId,
    }));
  }

  async readHostedLaunchMaterial(
    accountId: string,
    launchRequestId: string,
  ): Promise<HostedLaunchMaterial | null> {
    const data = await requiredRpc(this.#rpc, "read_hosted_launch_material_v1", {
      p_creator_account_id: accountId,
      p_launch_request_id: launchRequestId,
    });
    return this.parseLaunchMaterial(data);
  }

  async readHostedRecoveryMaterial(launchRequestId: string): Promise<HostedLaunchMaterial | null> {
    return this.parseLaunchMaterial(await requiredRpc(this.#rpc, "read_hosted_recovery_material_v1", {
      p_launch_request_id: launchRequestId,
    }));
  }

  private parseLaunchMaterial(data: unknown): HostedLaunchMaterial | null {
    if (data === null) return null;
    const value = requiredRecord(data);
    if (value.version !== "platform_hosted_launch_material.v1") {
      throw new PlatformDependencyUnavailableError();
    }
    return {
      launchRequestId: requiredString(value.launch_request_id),
      listingRevisionDigest: requiredString(value.listing_revision_digest),
      state: requiredString(value.state),
      expiresAt: requiredString(value.expires_at),
      launchInputs: requiredRecord(value.launch_inputs) as never,
      houseFillChoice: requiredEnum(value.house_fill_choice, ["disabled", "fill_unclaimed"]),
      creatorAccessChoice: requiredEnum(value.creator_access_choice, ["seat", "spectator"]),
      creatorSeatId: nullableString(value.creator_seat_id),
      hostInstallationId: nullableString(value.host_installation_id),
      roomSetupOperationId: nullableString(value.room_setup_operation_id),
      rosterFrozen: requiredBoolean(value.roster_frozen),
      hostMutationStarted: requiredBoolean(value.host_mutation_started),
      claims: requiredRecords(value.claims).map((claim) => ({
        seatId: requiredString(claim.seat_id),
        displayName: requiredString(claim.display_name),
        participationKind: requiredEnum(claim.participation_kind, [
          "account_human",
          "account_external_agent",
        ]),
        principalReference: requiredString(claim.principal_reference),
      })),
      houseAssignments: requiredRecords(value.house_assignments).map((assignment) => {
        const profile = requiredRecord(assignment.agent_profile);
        const runner = requiredRecord(assignment.runner_template);
        return {
          assignmentId: requiredString(assignment.house_agent_assignment_id),
          seatId: requiredString(assignment.seat_id),
          displayName: requiredString(assignment.display_name),
          principalReference: requiredString(assignment.principal_reference),
          houseAgentRevisionDigest: requiredString(assignment.house_agent_revision_digest),
          agentProfile: {
            profileId: requiredString(profile.profile_id),
            revision: requiredString(profile.revision),
          },
          runnerTemplate: {
            templateId: requiredString(runner.template_id),
            revision: requiredString(runner.revision),
          },
          reservationReceipt: requiredRecord(assignment.reservation_receipt) as never,
        };
      }),
    };
  }

  async freezeLaunch(input: {
    accountId: string;
    launchRequestId: string;
    frozenRoster: Uint8Array;
    frozenRosterDigest: Uint8Array;
    frozenRoomSetup: Uint8Array;
    frozenRoomSetupDigest: string;
    hostInstallationId: string;
    roomSetupOperationId: string;
  }) {
    return requiredBoolean(await requiredRpc(this.#rpc, "freeze_launch_request_v1", {
      p_creator_account_id: input.accountId,
      p_launch_request_id: input.launchRequestId,
      p_frozen_roster: bytea(input.frozenRoster),
      p_frozen_roster_digest: bytea(input.frozenRosterDigest),
      p_frozen_room_setup_specification: bytea(input.frozenRoomSetup),
      p_frozen_room_setup_specification_digest: input.frozenRoomSetupDigest,
      p_host_installation_id: input.hostInstallationId,
      p_room_setup_operation_id: input.roomSetupOperationId,
    }));
  }

  async authorizeHostMutation(
    accountId: string,
    launchRequestId: string,
    hostInstallationId: string,
    roomSetupOperationId: string,
  ) {
    return requiredBoolean(await requiredRpc(this.#rpc, "authorize_host_mutation_v1", {
      p_creator_account_id: accountId,
      p_launch_request_id: launchRequestId,
      p_host_installation_id: hostInstallationId,
      p_room_setup_operation_id: roomSetupOperationId,
    }));
  }

  async readGenesisReconciliation(
    launchRequestId: string,
  ): Promise<GenesisReconciliation | null> {
    const data = await requiredRpc(this.#rpc, "read_genesis_reconciliation_v1", {
      p_launch_request_id: launchRequestId,
    });
    if (data === null) return null;
    const value = requiredRecord(data);
    if (value.version !== "platform_genesis_reconciliation.v1") {
      throw new PlatformDependencyUnavailableError();
    }
    return {
      launchState: requiredString(value.launch_state),
      runId: nullableString(value.activity_run_id),
      reconciliationState: requiredEnum(value.reconciliation_state, [
        "pending",
        "ready",
        "quarantined",
      ]),
      needsGenesisPull: requiredBoolean(value.needs_genesis_pull),
    };
  }

  async recordGenesis(
    launchRequestId: string,
    canonicalEvidence: Uint8Array,
    evidenceDigest: Uint8Array,
  ): Promise<{ runId: string; reconciliationState: "ready" | "quarantined" } | null> {
    const data = await requiredRpc(this.#rpc, "record_genesis_v1", {
      p_launch_request_id: launchRequestId,
      p_canonical_genesis_evidence: bytea(canonicalEvidence),
      p_genesis_evidence_digest: bytea(evidenceDigest),
    });
    const rows = readRows(data);
    if (rows.length === 0) return null;
    const row = singleRow(data);
    return {
      runId: requiredString(row.activity_run_id),
      reconciliationState: requiredEnum(row.reconciliation_state, ["ready", "quarantined"]),
    };
  }

  async recordPrestartAbandonment(
    launchRequestId: string,
    canonicalEvidence: Uint8Array,
    evidenceDigest: Uint8Array,
  ): Promise<boolean> {
    return requiredBoolean(await requiredRpc(this.#rpc, "record_prestart_abandonment_v1", {
      p_launch_request_id: launchRequestId,
      p_canonical_abandonment_evidence: bytea(canonicalEvidence),
      p_abandonment_evidence_digest: bytea(evidenceDigest),
    }));
  }

  async recordProvisioningAbandonment(
    launchRequestId: string,
    canonicalEvidence: Uint8Array,
    evidenceDigest: Uint8Array,
  ): Promise<boolean> {
    return requiredBoolean(await requiredRpc(this.#rpc, "record_provisioning_abandonment_v1", {
      p_launch_request_id: launchRequestId,
      p_canonical_abandonment_evidence: bytea(canonicalEvidence),
      p_abandonment_evidence_digest: bytea(evidenceDigest),
    }));
  }

  async listPrestartAbandonmentCandidates(
    limit: number,
  ): Promise<readonly PrestartAbandonmentCandidate[]> {
    const rows = readRows(await requiredRpc(this.#rpc, "list_prestart_abandonment_candidates_v1", {
      p_limit: limit,
    }));
    return rows.map((row) => ({
      runId: requiredString(row.activity_run_id),
      launchRequestId: requiredString(row.launch_request_id),
      listingRevisionDigest: requiredString(row.listing_revision_digest),
      hostInstallationId: requiredString(row.host_installation_id),
      roomSetupOperationId: requiredString(row.room_setup_operation_id),
    }));
  }

  async readPublicRelayBindingCandidate(runId: string) {
    const data = await requiredRpc(
      this.#rpc,
      "read_public_relay_binding_candidate_v1",
      { p_activity_run_id: runId },
    );
    if (data === null) return null;
    return requiredRecord(data) as never;
  }

  async recordPublicRelayBinding(input: {
    runId: string;
    canonicalRequest: Uint8Array;
    requestDigest: Uint8Array;
    canonicalReceipt: Uint8Array;
    receiptDigest: Uint8Array;
  }) {
    return requiredBoolean(await requiredRpc(this.#rpc, "record_public_relay_binding_v1", {
      p_activity_run_id: input.runId,
      p_canonical_binding_request: bytea(input.canonicalRequest),
      p_binding_request_digest: bytea(input.requestDigest),
      p_canonical_binding_receipt: bytea(input.canonicalReceipt),
      p_binding_receipt_digest: bytea(input.receiptDigest),
    }));
  }

  async readOwnedRun(accountId: string, runId: string): Promise<OwnedRunRecord | null> {
    const data = await requiredRpc(this.#rpc, "read_owned_run_v1", {
      p_requesting_account_id: accountId,
      p_activity_run_id: runId,
    });
    if (data === null) return null;
    const value = requiredRecord(data);
    if (value.version !== "platform_owned_run.v1") {
      throw new PlatformDependencyUnavailableError();
    }
    return {
      runId: requiredString(value.run_id),
      publicId: nullableString(value.public_id),
      reconciliationState: requiredEnum(value.reconciliation_state, ["ready", "quarantined"]),
      canEnter: requiredBoolean(value.can_enter),
      memberships: requiredRecords(value.memberships)
        .filter((membership) =>
          membership.purpose === "participant" || membership.purpose === "creator_spectator",
        )
        .map((membership) => ({
          purpose: requiredEnum(membership.purpose, ["participant", "creator_spectator"]),
          seatId: nullableString(membership.seat_id),
          entrySelector: nullableString(membership.entry_selector),
        })),
    };
  }

  async listMyGames(input: {
    accountId: string;
    beforeAt: string | null;
    beforeLaunchId: string | null;
    limit: number;
  }): Promise<MyGamesIndex | null> {
    const data = await requiredRpc(this.#rpc, "list_my_games_v1", {
      p_requesting_account_id: input.accountId,
      p_before_at: input.beforeAt,
      p_before_launch_id: input.beforeLaunchId,
      p_limit: input.limit,
    });
    if (data === null) return null;
    const value = requiredRecord(data);
    if (value.version !== "platform_my_games.v1") throw new PlatformDependencyUnavailableError();
    const next = value.next === null || value.next === undefined ? null : requiredRecord(value.next);
    return {
      version: "platform_my_games.v1",
      items: requiredRecords(value.items).map((item) => ({
        launchId: requiredString(item.launch_id),
        title: requiredString(item.title),
        state: requiredEnum(item.state, [
          "setup_pending",
          "setup_cancelled",
          "setup_abandoned",
          "setup_failed",
          "live",
          "publication_pending",
          "terminal_private",
          "terminal_without_outcome",
          "result_suppressed",
          "dependency_failure",
          "verified_result",
        ]) as MyGamesIndex["items"][number]["state"],
        updatedAt: requiredString(item.updated_at),
        participation: requiredEnum(item.participation, ["human", "external_agent"]),
        action: requiredEnum(item.action, ["continue_setup", "return_to_game", "view_result", "none"]),
        resultPublicId: nullableString(item.result_public_id),
      })),
      next: next === null ? null : {
        beforeAt: requiredString(next.before_at),
        beforeLaunchId: requiredString(next.before_launch_id),
      },
    };
  }
}

class SupabaseResultReconciliationDataClient implements ResultReconciliationData {
  constructor(private readonly rpc: RpcClient) {}

  async reconcileTerminalActivityCapacity(limit: number): Promise<number> {
    const released = await requiredRpc(this.rpc, "reconcile_terminal_activity_capacity_v1", {
      p_limit: limit,
    });
    if (
      typeof released !== "number" ||
      !Number.isSafeInteger(released) ||
      released < 0 ||
      released > limit
    ) {
      throw new PlatformDependencyUnavailableError();
    }
    return released;
  }

  async markAttempt(launchRequestId: string): Promise<void> {
    await requiredRpc(this.rpc, "mark_reconciliation_attempt_v1", { p_launch_request_id: launchRequestId });
  }

  async listCandidates(limit: number): Promise<readonly ResultReconciliationCandidate[]> {
    const data = await requiredRpc(this.rpc, "list_reconciliation_candidates_v1", {
      p_limit: limit,
    });
    return readRows(data).map((row): ResultReconciliationCandidate => ({
      candidateKind: requiredEnum(row.candidate_kind, ["genesis", "result_source"]),
      launchRequestId: requiredString(row.launch_request_id),
      runId: nullableString(row.activity_run_id),
      listingRevisionDigest: requiredString(row.listing_revision_digest),
      launchRequestDigest: nullableString(row.launch_request_digest),
      hostInstallationId: requiredString(row.host_installation_id),
      roomSetupOperationId: requiredString(row.room_setup_operation_id),
    }));
  }

  async listTerminalHouseRunnerRetirementRuns(limit: number): Promise<readonly string[]> {
    const data = await requiredRpc(
      this.rpc,
      "list_terminal_house_runner_retirement_candidates_v1",
      { p_limit: limit },
    );
    return readRows(data).map((row) => requiredString(row.activity_run_id));
  }

  async listPrestartHouseRunnerRetirementRuns(limit: number): Promise<readonly string[]> {
    const data = await requiredRpc(
      this.rpc,
      "list_prestart_house_runner_retirement_candidates_v1",
      { p_limit: limit },
    );
    return readRows(data).map((row) => requiredString(row.activity_run_id));
  }

  async listPrestartAbandonmentLaunches(limit: number): Promise<readonly string[]> {
    const data = await requiredRpc(
      this.rpc,
      "list_prestart_abandonment_candidates_v1",
      { p_limit: limit },
    );
    return readRows(data).map((row) => requiredString(row.launch_request_id));
  }

  async readTerminal(runId: string): Promise<TerminalReconciliationState | null> {
    const data = await requiredRpc(this.rpc, "read_terminal_reconciliation_v1", {
      p_activity_run_id: runId,
    });
    if (data === null) return null;
    const value = requiredRecord(data);
    return {
      terminalRecorded: requiredBoolean(value.terminal_recorded),
      projectorStatus: nullableEnum(value.projector_status, [
        "terminal_without_outcome",
        "summary",
      ]),
      reconciliationState: requiredEnum(value.reconciliation_state, [
        "pending",
        "terminal",
        "quarantined",
      ]),
    };
  }

  async readResult(runId: string): Promise<ResultReconciliationState | null> {
    const data = await requiredRpc(this.rpc, "read_result_reconciliation_v1", {
      p_activity_run_id: runId,
    });
    if (data === null) return null;
    const value = requiredRecord(data);
    return {
      resultRecorded: requiredBoolean(value.result_recorded),
      resultPayloadDigest: nullableString(value.result_payload_digest),
      integrityStatus: nullableEnum(value.integrity_status, [
        "healthy",
        "faulted",
        "quarantined",
      ]) as ResultIntegrityStatus | null,
      integrityGeneration: nullableSafeInteger(value.integrity_generation),
      publishable: requiredBoolean(value.publishable),
    };
  }

  async readTerminalHouseRunnerRetirements(
    runId: string,
  ): Promise<readonly TerminalHouseRunnerRetirementCandidate[]> {
    const data = await requiredRpc(this.rpc, "read_terminal_house_runner_retirements_v1", {
      p_activity_run_id: runId,
    });
    return readRows(data).map((row): TerminalHouseRunnerRetirementCandidate => ({
      hostInstallationId: requiredString(row.host_installation_id),
      reservationOperationId: requiredString(row.reservation_operation_id),
      launchRequestId: requiredString(row.launch_request_id),
      houseAgentAssignmentId: requiredString(row.house_agent_assignment_id),
      terminalEvidenceDigest: requiredString(row.terminal_evidence_digest),
    }));
  }

  async recordTerminalHouseRunnerRetirement(input: {
    runId: string;
    candidate: TerminalHouseRunnerRetirementCandidate;
    canonicalReceipt: Uint8Array;
    receiptDigest: Uint8Array;
  }): Promise<boolean> {
    return requiredBoolean(await requiredRpc(
      this.rpc,
      "record_terminal_house_runner_retirement_v1",
      {
        p_activity_run_id: input.runId,
        p_house_agent_assignment_id: input.candidate.houseAgentAssignmentId,
        p_canonical_retirement_receipt: bytea(input.canonicalReceipt),
        p_retirement_receipt_digest: bytea(input.receiptDigest),
      },
    ));
  }

  async readPrestartHouseRunnerRetirements(
    runId: string,
  ): Promise<readonly PrestartHouseRunnerRetirementCandidate[]> {
    const data = await requiredRpc(this.rpc, "read_prestart_house_runner_retirements_v1", {
      p_activity_run_id: runId,
    });
    return readRows(data).map((row): PrestartHouseRunnerRetirementCandidate => ({
      hostInstallationId: requiredString(row.host_installation_id),
      reservationOperationId: requiredString(row.reservation_operation_id),
      launchRequestId: requiredString(row.launch_request_id),
      houseAgentAssignmentId: requiredString(row.house_agent_assignment_id),
      abandonmentEvidenceDigest: requiredString(row.abandonment_evidence_digest),
    }));
  }

  async recordPrestartHouseRunnerRetirement(input: {
    runId: string;
    candidate: PrestartHouseRunnerRetirementCandidate;
    canonicalReceipt: Uint8Array;
    receiptDigest: Uint8Array;
  }): Promise<boolean> {
    return requiredBoolean(await requiredRpc(
      this.rpc,
      "record_prestart_house_runner_retirement_v1",
      {
        p_activity_run_id: input.runId,
        p_house_agent_assignment_id: input.candidate.houseAgentAssignmentId,
        p_canonical_retirement_receipt: bytea(input.canonicalReceipt),
        p_retirement_receipt_digest: bytea(input.receiptDigest),
      },
    ));
  }

  async recordTerminal(
    runId: string,
    evidence: Uint8Array,
    evidenceDigest: Uint8Array,
  ): Promise<ReconciliationWriteReceipt> {
    return this.write("record_run_terminal_v1", {
      p_activity_run_id: runId,
      p_canonical_terminal_evidence: bytea(evidence),
      p_terminal_evidence_digest: bytea(evidenceDigest),
    });
  }

  async recordTerminalConflict(
    runId: string,
    evidence: Uint8Array,
    evidenceDigest: Uint8Array,
  ): Promise<ReconciliationWriteReceipt> {
    return this.write("record_terminal_conflict_v1", {
      p_activity_run_id: runId,
      p_canonical_conflict_evidence: bytea(evidence),
      p_conflict_evidence_digest: bytea(evidenceDigest),
    });
  }

  async recordResult(
    runId: string,
    evidence: Uint8Array,
    evidenceDigest: Uint8Array,
    payload: Uint8Array,
    payloadDigest: Uint8Array,
  ): Promise<ReconciliationWriteReceipt> {
    return this.write("record_result_v1", {
      p_activity_run_id: runId,
      p_canonical_result_evidence: bytea(evidence),
      p_result_evidence_digest: bytea(evidenceDigest),
      p_canonical_result_payload: bytea(payload),
      p_result_payload_digest: bytea(payloadDigest),
    });
  }

  async recordIntegrity(
    runId: string,
    evidence: Uint8Array,
    evidenceDigest: Uint8Array,
  ): Promise<ReconciliationWriteReceipt> {
    return this.write("record_integrity_observation_v1", {
      p_activity_run_id: runId,
      p_canonical_integrity_evidence: bytea(evidence),
      p_integrity_evidence_digest: bytea(evidenceDigest),
    });
  }

  private async write(
    name: string,
    args: Record<string, unknown>,
  ): Promise<ReconciliationWriteReceipt> {
    const value = requiredRecord(await requiredRpc(this.rpc, name, args));
    return {
      disposition: requiredEnum(value.disposition, [
        "applied",
        "duplicate",
        "conflict",
        "blocked",
      ]),
      safeCode: requiredString(value.safe_code),
    };
  }
}

function serverClientOptions() {
  return {
    auth: {
      persistSession: false,
      autoRefreshToken: false,
      detectSessionInUrl: false,
    },
  } as const;
}

async function requiredRpc(
  rpc: RpcClient,
  name: string,
  args: Record<string, unknown>,
): Promise<unknown> {
  let result: RpcResult;
  try {
    result = await rpc.rpc(name, args);
  } catch {
    throw new PlatformDependencyUnavailableError();
  }
  const { data, error } = result;
  if (error !== null) {
    if (
      name === "create_launch_request_v1" &&
      error.code === "55000" &&
      error.message === "hosted_launches_closed"
    ) {
      throw new PlatformDependencyUnavailableError();
    }
    if (capacityGateScope(name, error.code, error.message) !== null) {
      throwCapacityUnavailable(error.message);
    }
    if (error.code !== undefined && ["22023", "23505", "55000"].includes(error.code)) {
      throw new PlatformCredentialRejectedError();
    }
    throw new PlatformDependencyUnavailableError();
  }
  return data;
}

function capacityGateScope(
  name: string,
  code: string | undefined,
  message: string | undefined,
): "account" | "platform" | null {
  if (code !== "55000") return null;
  if (name === "create_launch_request_v1" && message === "pre_genesis_capacity_unavailable") {
    return "account";
  }
  if (name === "authorize_host_mutation_v1") {
    if (message === "account_active_run_capacity_unavailable") return "account";
    if (message === "global_active_run_capacity_unavailable") return "platform";
  }
  return null;
}

function throwCapacityUnavailable(message: string): never {
  // These are exact durable gate decisions. Do not turn similarly shaped
  // errors into a retryable capacity condition: a caller may infer only that
  // capacity is held, never another Launch Request or Activity Run identity.
  throw new PlatformActivityCapacityUnavailableError(
    message === "global_active_run_capacity_unavailable" ? "platform" : "account",
  );
}

function readSession(value: unknown): AuthSession {
  const record = requiredRecord(value);
  return {
    accessToken: requiredString(record.access_token),
    refreshToken: requiredString(record.refresh_token),
    expiresAt: requiredExpiry(record.expires_at),
  };
}

function readUser(user: User): AuthUser {
  const identities = (user.identities ?? [])
    .filter((identity) => identity.provider === "github")
    .map((identity): AuthIdentity => {
      const data = requiredRecord(identity.identity_data);
      const subject = requiredString(data.sub);
      if (identity.id !== subject) throw new PlatformCredentialRejectedError();
      return {
        provider: "github",
        subject,
        login: optionalString(data.user_name ?? data.preferred_username),
        avatarUrl: optionalHttpsUrl(data.avatar_url),
      };
    });
  return { id: user.id, identities };
}

function readRows(value: unknown): Record<string, unknown>[] {
  if (!Array.isArray(value) || !value.every(isRecord)) {
    throw new PlatformDependencyUnavailableError();
  }
  return value;
}

function singleRow(value: unknown): Record<string, unknown> {
  const rows = readRows(value);
  if (rows.length !== 1 || rows[0] === undefined) {
    throw new PlatformDependencyUnavailableError();
  }
  return rows[0];
}

function requiredRecords(value: unknown): Record<string, unknown>[] {
  if (!Array.isArray(value) || !value.every(isRecord)) {
    throw new PlatformDependencyUnavailableError();
  }
  return value;
}

function parseHouseFill(value: unknown): HouseFillRecord | null {
  if (value === null) return null;
  const operation = requiredRecord(value);
  if (operation.version !== "platform_house_fill_operation.v1") {
    throw new PlatformDependencyUnavailableError();
  }
  return {
    state: requiredEnum(operation.state, [
      "claim_window_open",
      "reserving",
      "assignments_complete",
      "failed_pre_genesis",
    ]),
    claimWindowClosesAt: requiredString(operation.claim_window_closes_at),
    failureCode: nullableString(operation.failure_code),
    reservations: requiredRecords(operation.reservations).map((reservation) => ({
      reservationOperationId: requiredString(reservation.reservation_operation_id),
      seatId: requiredString(reservation.seat_id),
      houseAgentRevisionDigest: requiredString(reservation.house_agent_revision_digest),
      state: requiredEnum(reservation.state, [
        "pending",
        "ambiguous",
        "succeeded",
        "terminal_failed",
        "released",
      ]),
    })),
    assignments: requiredRecords(operation.assignments).map((assignment) => ({
      seatId: requiredString(assignment.seat_id),
      displayName: requiredString(assignment.display_name),
    })),
  };
}

function throwForAuthError(error: unknown): never {
  const status = isRecord(error) ? error.status : undefined;
  if (typeof status === "number") throwForAuthStatus(status);
  throw new PlatformDependencyUnavailableError();
}

async function dependencyCall<T>(operation: () => Promise<T>): Promise<T> {
  try {
    return await operation();
  } catch {
    throw new PlatformDependencyUnavailableError();
  }
}

async function authCall<T>(operation: () => Promise<T>): Promise<T> {
  try {
    return await operation();
  } catch (error) {
    throwForAuthError(error);
  }
}

function throwForAuthStatus(status: number): never {
  if ([400, 401, 403, 422].includes(status)) throw new PlatformCredentialRejectedError();
  throw new PlatformDependencyUnavailableError();
}

function requiredRecord(value: unknown): Record<string, unknown> {
  if (!isRecord(value)) throw new PlatformDependencyUnavailableError();
  return value;
}

function requiredString(value: unknown): string {
  if (typeof value !== "string" || value.length === 0) {
    throw new PlatformDependencyUnavailableError();
  }
  return value;
}

function optionalString(value: unknown): string | null {
  return typeof value === "string" && value.length > 0 ? value : null;
}

function optionalHttpsUrl(value: unknown): string | null {
  const result = optionalString(value);
  return result !== null && result.startsWith("https://") ? result : null;
}

function requiredBoolean(value: unknown): boolean {
  if (typeof value !== "boolean") throw new PlatformDependencyUnavailableError();
  return value;
}

function nullableString(value: unknown): string | null {
  return value === null || value === undefined ? null : requiredString(value);
}

function nullableSafeInteger(value: unknown): number | null {
  if (value === null || value === undefined) return null;
  if (typeof value !== "number" || !Number.isSafeInteger(value) || value < 0) {
    throw new PlatformDependencyUnavailableError();
  }
  return value;
}

function requiredEnum<const T extends string>(value: unknown, values: readonly T[]): T {
  if (typeof value !== "string" || !values.includes(value as T)) {
    throw new PlatformDependencyUnavailableError();
  }
  return value as T;
}

function nullableEnum<const T extends string>(
  value: unknown,
  values: readonly T[],
): T | null {
  return value === null || value === undefined ? null : requiredEnum(value, values);
}

function requiredExpiry(value: unknown): number {
  if (typeof value !== "number" || !Number.isSafeInteger(value)) {
    throw new PlatformDependencyUnavailableError();
  }
  return value;
}

function bytea(value: Uint8Array): string {
  return `\\x${Buffer.from(value).toString("hex")}`;
}

function validateSupabaseUrl(url: string): void {
  const parsed = new URL(url);
  const local = parsed.hostname === "127.0.0.1" || parsed.hostname === "localhost";
  if (
    (parsed.protocol !== "https:" && !(local && parsed.protocol === "http:")) ||
    parsed.username !== "" ||
    parsed.password !== "" ||
    parsed.pathname !== "/" ||
    parsed.search !== "" ||
    parsed.hash !== ""
  ) {
    throw new Error("invalid_supabase_configuration");
  }
}

function validateSupabaseKey(key: string, expected: "publishable" | "secret"): void {
  if (key.length < 20 || key.length > 4096 || !/^[\x21-\x7e]+$/u.test(key)) {
    throw new Error("invalid_supabase_configuration");
  }
  const currentClass = key.startsWith("sb_publishable_")
    ? "publishable"
    : key.startsWith("sb_secret_")
      ? "secret"
      : legacySupabaseKeyClass(key);
  if (currentClass !== expected) throw new Error("invalid_supabase_key_class");
}

function legacySupabaseKeyClass(key: string): "publishable" | "secret" | null {
  const parts = key.split(".");
  if (parts.length !== 3 || parts.some((part) => part.length === 0)) return null;
  try {
    const payload: unknown = JSON.parse(Buffer.from(parts[1] ?? "", "base64url").toString());
    if (!isRecord(payload)) return null;
    if (payload.role === "anon") return "publishable";
    if (payload.role === "service_role") return "secret";
    return null;
  } catch {
    return null;
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
