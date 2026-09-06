import { useCallback, useEffect, useState } from "react";

export interface HostedActivitySummary {
  readonly slug: string;
  readonly title: string;
  readonly description: string;
  readonly availability: "available" | "coming_soon" | "dependency_unavailable";
  readonly availabilityMessage: string;
  readonly seatSummary: string;
  readonly seats: readonly { readonly key: string; readonly label: string; readonly required: boolean }[];
  readonly creatorMaySpectate: boolean;
  readonly houseFillAvailable: boolean;
  readonly publicViewingAvailable: boolean;
  readonly resultPublication: string;
  readonly attribution: string;
  readonly clientPath: string | null;
  readonly houseTerms: {
    readonly exhibition: true;
    readonly maximumAgents: number;
    readonly maximumCallsPerAgent: number;
    readonly maximumInputTokensPerAgent: number;
    readonly maximumOutputTokensPerAgent: number;
    readonly callTimeoutSeconds: number;
  } | null;
}

export interface HostedLaunchSeat {
  readonly seat_key: string;
  readonly label: string;
  readonly required: boolean;
  readonly status: "open" | "yours" | "claimed" | "house";
  readonly participation: "human" | "external_agent" | "house_agent" | null;
  readonly house_display_name?: string;
}

export interface HostedLaunch {
  readonly version: "hosted_launch.v1";
  readonly launch_id: string;
  readonly activity_slug: string;
  readonly activity_title: string;
  readonly state: "collecting" | "provisioning" | "reconciling" | "run_created" | "cancelled" | "expired" | "failed_pre_genesis";
  readonly expires_at: string;
  readonly can_manage: boolean;
  readonly fill_mode: "people_only" | "house_agents";
  readonly house_fill: {
    readonly state: string;
    readonly claim_window_closes_at: string;
    readonly failure_code: string | null;
  } | null;
  readonly seats: readonly HostedLaunchSeat[];
  readonly run: {
    readonly run_id: string;
    readonly public_id: string | null;
    readonly can_enter: boolean;
    readonly entries: readonly {
      readonly label: string;
      readonly entry_selector: string | null;
    }[];
  } | null;
  readonly retry_after_seconds?: number | null;
}

export type PublicJson = null | boolean | number | string | readonly PublicJson[] | {
  readonly [key: string]: PublicJson;
};

export interface PublicRunParticipant {
  readonly seat_label: string;
  readonly role: string;
  readonly kind: "human" | "external_agent" | "house_agent";
  readonly identity?:
    | { readonly kind: "pseudonym"; readonly label: string }
    | {
        readonly kind: "github";
        readonly login: string;
        readonly avatar_url?: string;
        readonly fallback_label: string;
      };
  readonly notice?: string;
  readonly house_agent?: {
    readonly display_name: string;
    readonly revision_digest: string;
    readonly route: {
      readonly gateway: "openrouter";
      readonly provider_slug: string;
      readonly model_slug: string;
    };
    readonly allowance: Readonly<Record<string, number>>;
  };
}

interface PublicRunBase {
  readonly version: "public_run.v1";
  readonly public_id: string;
  readonly activity: {
    readonly listing_key: string;
    readonly title: string;
    readonly description: string;
    readonly listing_revision: string;
    readonly pack: { readonly id: string; readonly version: string; readonly revision: string };
  };
  readonly started_at: string;
  readonly evidence: {
    readonly class: "unranked" | "exhibition_platform_house_agents";
    readonly label: string;
  };
  readonly participants: readonly PublicRunParticipant[];
}

export type PublicRun =
  | { readonly version: "public_run.v1"; readonly state: "unavailable" }
  | PublicRunBase & {
      readonly state: "live";
      readonly live: { readonly available: false };
    }
  | PublicRunBase & {
      readonly state: "result";
      readonly completed_at: string;
      readonly result: Readonly<Record<string, PublicJson>>;
    };

export interface RecentResults {
  readonly version: "recent_results.v1";
  readonly activity: "agent-heist";
  readonly order: "newest_first";
  readonly maximum: 20;
  readonly results: readonly Extract<PublicRun, { readonly state: "result" }>[];
}

export type PlatformSession =
  | { readonly state: "loading" | "guest" | "unavailable"; readonly csrf: null }
  | { readonly state: "authenticated"; readonly csrf: string };

export function usePlatformSession(): {
  readonly session: PlatformSession;
  readonly developmentSignInAvailable: boolean;
  readonly reload: () => Promise<void>;
} {
  const [session, setSession] = useState<PlatformSession>({ state: "loading", csrf: null });
  const [developmentSignInAvailable, setDevelopmentSignInAvailable] = useState(false);
  const reload = useCallback(async () => {
    setSession({ state: "loading", csrf: null });
    try {
      let response = await fetch("/api/auth/session", { credentials: "same-origin" });
      let value = await safeJson(response);
      if (
        response.status === 401 &&
        errorCode(value) === "session_refresh_required" &&
        typeof value.csrf === "string"
      ) {
        const refreshed = await fetch("/api/auth/session/refresh", {
          method: "POST",
          credentials: "same-origin",
          headers: mutationHeaders(value.csrf),
          body: "{}",
        });
        if (refreshed.ok) {
          response = await fetch("/api/auth/session", { credentials: "same-origin" });
          value = await safeJson(response);
        }
      }
      if (response.ok && value.authenticated === true && typeof value.csrf === "string") {
        setSession({ state: "authenticated", csrf: value.csrf });
      } else if (response.status === 401) {
        setSession({ state: "guest", csrf: null });
      } else {
        setSession({ state: "unavailable", csrf: null });
      }
    } catch {
      setSession({ state: "unavailable", csrf: null });
    }
  }, []);

  useEffect(() => {
    void reload();
    void fetch("/api/dev/status", { credentials: "same-origin" })
      .then((response) => setDevelopmentSignInAvailable(response.ok))
      .catch(() => setDevelopmentSignInAvailable(false));
  }, [reload]);
  return { session, developmentSignInAvailable, reload };
}

export async function readCatalog(): Promise<readonly HostedActivitySummary[]> {
  const response = await fetch("/api/catalog");
  const value = await safeJson(response);
  if (!response.ok || !Array.isArray(value.activities)) throw new Error("catalog_unavailable");
  return value.activities as unknown as readonly HostedActivitySummary[];
}

export async function readPublicRun(publicId: string): Promise<PublicRun> {
  const response = await fetch(`/api/runs/${encodeURIComponent(publicId)}`);
  const value = await safeJson(response);
  if (!response.ok || value.version !== "public_run.v1") {
    throw new Error("public_run_unavailable");
  }
  return value as unknown as PublicRun;
}

export async function readRecentResults(): Promise<RecentResults> {
  const response = await fetch("/api/results/agent-heist/recent");
  const value = await safeJson(response);
  if (
    !response.ok || value.version !== "recent_results.v1" ||
    value.activity !== "agent-heist" || !Array.isArray(value.results)
  ) {
    throw new Error("recent_results_unavailable");
  }
  return value as unknown as RecentResults;
}

export async function readLaunch(launchId: string): Promise<HostedLaunch> {
  return requestLaunch(`/api/launches/${encodeURIComponent(launchId)}`);
}

export async function createLaunch(
  csrf: string,
  input: {
    readonly listingSlug: string;
    readonly creatorAccess: "seat" | "spectator";
    readonly creatorSeat: string | null;
    readonly fillMode: "people_only" | "house_agents";
    readonly idempotencyKey: string;
  },
): Promise<HostedLaunch> {
  return mutateLaunch("/api/launches", csrf, {
    listing_slug: input.listingSlug,
    creator_access: input.creatorAccess,
    creator_seat: input.creatorSeat,
    fill_mode: input.fillMode,
    idempotency_key: input.idempotencyKey,
  });
}

export async function claimInvitation(
  csrf: string,
  token: string,
  participation: "human" | "external_agent",
): Promise<HostedLaunch> {
  return mutateLaunch("/api/invitations/claim", csrf, {
    invitation_token: token,
    participation,
  });
}

export async function launchMutation(
  csrf: string,
  launchId: string,
  action: "start" | "cancel",
): Promise<HostedLaunch | { readonly cancelled: true }> {
  return mutateLaunch(`/api/launches/${encodeURIComponent(launchId)}/${action}`, csrf, {});
}

export async function seatMutation(
  csrf: string,
  launchId: string,
  seatKey: string,
  action: "invitation" | "release" | "reset",
): Promise<HostedLaunch | { readonly invitation_token: string; readonly expires_at: string }> {
  return mutateLaunch(
    `/api/launches/${encodeURIComponent(launchId)}/seats/${encodeURIComponent(seatKey)}/${action}`,
    csrf,
    {},
  );
}

export async function enterRun(
  csrf: string,
  runId: string,
  entrySelector: string,
): Promise<string> {
  const response = await fetch("/api/runs/enter", {
    method: "POST",
    credentials: "same-origin",
    headers: mutationHeaders(csrf),
    body: JSON.stringify({ run_id: runId, entry_selector: entrySelector }),
  });
  const value = await safeJson(response);
  if (!response.ok || typeof value.client_url !== "string") {
    throw new Error(errorCode(value) ?? "run_entry_unavailable");
  }
  const target = new URL(value.client_url, window.location.origin);
  if (target.origin !== window.location.origin || !target.pathname.endsWith("/")) {
    throw new Error("run_entry_unavailable");
  }
  return target.toString();
}

export async function developmentSignIn(): Promise<void> {
  const response = await fetch("/api/dev/sign-in", {
    method: "POST",
    credentials: "same-origin",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ mode: "visible-local-only" }),
  });
  if (!response.ok) throw new Error("development_sign_in_unavailable");
}

export function githubSignIn(returnTarget: "/" | "/join"): void {
  window.location.assign(`/api/auth/github/start?return_to=${encodeURIComponent(returnTarget)}`);
}

async function requestLaunch(path: string): Promise<HostedLaunch> {
  const response = await fetch(path, { credentials: "same-origin" });
  const value = await safeJson(response);
  if (!response.ok || value.version !== "hosted_launch.v1") {
    throw new Error(errorCode(value) ?? "launch_unavailable");
  }
  return value as unknown as HostedLaunch;
}

async function mutateLaunch<T>(path: string, csrf: string, body: unknown): Promise<T> {
  const response = await fetch(path, {
    method: "POST",
    credentials: "same-origin",
    headers: mutationHeaders(csrf),
    body: JSON.stringify(body),
  });
  const value = await safeJson(response);
  if (!response.ok) throw new Error(errorCode(value) ?? "request_unavailable");
  return value as T;
}

function mutationHeaders(csrf: string): HeadersInit {
  return {
    "content-type": "application/json",
    "x-worldstream-csrf": csrf,
  };
}

async function safeJson(response: Response): Promise<Record<string, unknown>> {
  try {
    const value: unknown = await response.json();
    return value !== null && typeof value === "object" && !Array.isArray(value)
      ? value as Record<string, unknown>
      : {};
  } catch {
    return {};
  }
}

function errorCode(value: Record<string, unknown>): string | null {
  const error = value.error;
  if (error === null || typeof error !== "object" || Array.isArray(error)) return null;
  const code = (error as Record<string, unknown>).code;
  return typeof code === "string" ? code : null;
}
