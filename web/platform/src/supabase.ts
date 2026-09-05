import { createClient, type User } from "@supabase/supabase-js";

import {
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
  RefreshedAuthSession,
  SessionRefreshAdmission,
} from "./bff.js";

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
  return {
    authClient: () => new SupabasePlatformAuthClient(config.url, config.publishableKey),
    dataClient: new SupabasePlatformDataClient(config.url, config.dataSecretKey),
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

class SupabasePlatformDataClient implements PlatformDataClient {
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
    if (error.code !== undefined && ["22023", "23505", "55000"].includes(error.code)) {
      throw new PlatformCredentialRejectedError();
    }
    throw new PlatformDependencyUnavailableError();
  }
  return data;
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

function requiredExpiry(value: unknown): number {
  if (typeof value !== "number" || !Number.isSafeInteger(value)) {
    throw new PlatformDependencyUnavailableError();
  }
  return value;
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
