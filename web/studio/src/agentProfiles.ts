import type {
  AgentProfileRevisionReference,
  RoomDraft,
} from "./roomDrafts";

export type AgentProfileSecretKind = "model_provider";
export type AgentProfileSecretAvailability = "configured" | "missing" | "unavailable";

export interface AgentProfileSecretSetting {
  key: string;
  kind: AgentProfileSecretKind;
  availability: AgentProfileSecretAvailability;
}

export type AgentHostContract =
  | { kind: "generic_mcp" }
  | {
      kind: "managed_reference";
      host_contract_revision: string;
      runner_template: { template_id: string; revision: string };
      provider: "open_ai_compatible";
      provider_address: string;
      model_id: string;
    };

export interface AgentProfileRevision {
  profile_id: string;
  revision: string;
  display_name: string;
  non_secret_configuration: Record<string, string>;
  secret_settings: AgentProfileSecretSetting[];
  host_contract: AgentHostContract;
}

export interface AgentProfileCatalog {
  schema: "worldstream/studio-agent-profile-catalog/v1";
  profiles: AgentProfileRevision[];
}
export interface ModelProviderCredentialCatalog { schema: "worldstream/studio-model-provider-credential-catalog/v1"; credentials: Array<{ credential_id: string; display_name: string; provider: "open_ai_compatible"; availability: "configured" | "missing" | "unavailable" }>; }

export interface AgentProfilePublishRequest {
  schema: "worldstream/studio-agent-profile-publish/v2";
  profile_id: string;
  revision: string;
  display_name: string;
  non_secret_configuration: Record<string, string>;
  host_contract: AgentHostContract;
  managed_provider_credential_id?: string;
}

export type AgentProfilePublishOutcome =
  | { kind: "published"; profile: AgentProfileRevision }
  | { kind: "revision_conflict" }
  | { kind: "rejected" }
  | { kind: "unavailable" };

export interface AgentProfileMembershipBinding {
  room_id: string;
  member_id: string;
  principal_id: string;
  role: string;
}

export type AgentExecutionBinding =
  | { kind: "external" }
  | { kind: "managed"; runner_id: string }
  | {
      kind: "managed_reference";
      runner_id: string;
      instance_id: string;
      template_id: string;
      template_revision: string;
    };

export interface AgentProfileSeatAssignment {
  schema: "worldstream/studio-agent-profile-assignment/v1";
  assignment_id: string;
  draft_id: string;
  seat_id: string;
  profile: AgentProfileRevisionReference;
  membership: AgentProfileMembershipBinding;
  execution: AgentExecutionBinding;
}

export interface AgentProfileAssignmentCatalog {
  schema: "worldstream/studio-agent-profile-assignment-catalog/v1";
  assignments: AgentProfileSeatAssignment[];
}

type Fetcher = (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>;

const maximumRows = 256;

export async function loadAgentProfiles(
  fetcher: Fetcher = fetch,
): Promise<AgentProfileCatalog | null> {
  return loadJson("/api/v1/agent-profiles", isAgentProfileCatalog, fetcher);
}
export async function loadModelProviderCredentials(fetcher: Fetcher = fetch): Promise<ModelProviderCredentialCatalog | null> { return loadJson("/api/v1/model-provider-credentials", isModelProviderCredentialCatalog, fetcher); }
function isModelProviderCredentialCatalog(value: unknown): value is ModelProviderCredentialCatalog { return isExactRecord(value, ["schema", "credentials"]) && value.schema === "worldstream/studio-model-provider-credential-catalog/v1" && isBoundedArray(value.credentials, (item): item is ModelProviderCredentialCatalog["credentials"][number] => isExactRecord(item, ["credential_id", "display_name", "provider", "availability"]) && isProfileId(item.credential_id) && isText(item.display_name) && item.provider === "open_ai_compatible" && ["configured", "missing", "unavailable"].includes(String(item.availability))); }

export async function loadAgentProfileRevision(
  profileId: string,
  revision: string,
  fetcher: Fetcher = fetch,
): Promise<AgentProfileRevision | null> {
  if (!isProfileId(profileId) || !isRevision(revision)) return null;
  return loadJson(
    `/api/v1/agent-profiles/${encodeURIComponent(profileId)}/revisions/${encodeURIComponent(revision)}`,
    isAgentProfileRevision,
    fetcher,
  );
}

export async function loadAgentProfileAssignments(
  fetcher: Fetcher = fetch,
): Promise<AgentProfileAssignmentCatalog | null> {
  return loadJson(
    "/api/v1/agent-profile-assignments",
    isAgentProfileAssignmentCatalog,
    fetcher,
  );
}

export async function publishAgentProfile(
  request: AgentProfilePublishRequest,
  fetcher: Fetcher = fetch,
): Promise<AgentProfilePublishOutcome> {
  if (!isAgentProfilePublishRequest(request)) return { kind: "rejected" };
  try {
    const response = await fetcher("/api/v1/agent-profiles", {
      method: "POST",
      headers: { accept: "application/json", "content-type": "application/json" },
      body: JSON.stringify(request),
    });
    if (response.status === 409) return { kind: "revision_conflict" };
    if (response.status === 400) return { kind: "rejected" };
    if (!response.ok) return { kind: "unavailable" };
    const value: unknown = await response.json();
    return isAgentProfileRevision(value) && publishedRevisionMatches(request, value)
      ? { kind: "published", profile: value }
      : { kind: "unavailable" };
  } catch {
    return { kind: "unavailable" };
  }
}

export function createAgentProfileRevisionDraft(
  source?: AgentProfileRevision,
): AgentProfilePublishRequest {
  return {
    schema: "worldstream/studio-agent-profile-publish/v2",
    profile_id: source?.profile_id ?? "",
    revision: "",
    display_name: source?.display_name ?? "",
    non_secret_configuration: { ...(source?.non_secret_configuration ?? {}) },
    host_contract: source?.host_contract ?? { kind: "generic_mcp" },
  };
}

/// Selects one exact immutable profile revision in Room planning only.
/// Membership and Runner authority identities remain owned by setup workflows.
export function assignAgentProfileRevision(
  draft: RoomDraft,
  seatId: string,
  profile: AgentProfileRevision,
): RoomDraft | null {
  if (!isProfileId(seatId) || !isAgentProfileRevision(profile)) return null;
  const seat = draft.seats.find((candidate) => candidate.seat_id === seatId);
  if (seat?.principal_kind !== "agent" || seat.agent_assignment === undefined) return null;
  const reference: AgentProfileRevisionReference = {
    profile_id: profile.profile_id,
    revision: profile.revision,
  };
  return {
    ...draft,
    seats: draft.seats.map((candidate) => candidate.seat_id === seatId
      ? { ...candidate, agent_profile: reference }
      : candidate),
  };
}

async function loadJson<T>(
  path: string,
  validate: (value: unknown) => value is T,
  fetcher: Fetcher,
): Promise<T | null> {
  try {
    const response = await fetcher(path, { headers: { accept: "application/json" } });
    if (!response.ok) return null;
    const value: unknown = await response.json();
    return validate(value) ? value : null;
  } catch {
    return null;
  }
}

function isAgentProfileCatalog(value: unknown): value is AgentProfileCatalog {
  return isExactRecord(value, ["schema", "profiles"])
    && value.schema === "worldstream/studio-agent-profile-catalog/v1"
    && isBoundedArray(value.profiles, isAgentProfileRevision)
    && unique(value.profiles.map((profile) => `${profile.profile_id}\0${profile.revision}`));
}

function isAgentProfilePublishRequest(value: unknown): value is AgentProfilePublishRequest {
  if (!isRecord(value)
    || !isExactRecord(value, value.host_contract && isRecord(value.host_contract)
      && value.host_contract.kind === "managed_reference"
      ? ["schema", "profile_id", "revision", "display_name", "non_secret_configuration", "host_contract", "managed_provider_credential_id"]
      : ["schema", "profile_id", "revision", "display_name", "non_secret_configuration", "host_contract"])) return false;
  return value.schema === "worldstream/studio-agent-profile-publish/v2"
    && isProfileId(value.profile_id)
    && isRevision(value.revision)
    && isText(value.display_name)
    && isConfiguration(value.non_secret_configuration)
    && isAgentHostContract(value.host_contract)
    && (value.host_contract.kind === "generic_mcp"
      ? value.managed_provider_credential_id === undefined
      : isProfileId(value.managed_provider_credential_id));
}

function isAgentProfileRevision(value: unknown): value is AgentProfileRevision {
  return isExactRecord(value, [
    "profile_id", "revision", "display_name", "non_secret_configuration", "secret_settings", "host_contract",
  ])
    && isProfileId(value.profile_id)
    && isRevision(value.revision)
    && isText(value.display_name)
    && isConfiguration(value.non_secret_configuration)
    && isBoundedArray(value.secret_settings, isSecretSetting)
    && unique(value.secret_settings.map((setting) => setting.key))
    && isAgentHostContract(value.host_contract, value.secret_settings);
}

function isAgentHostContract(
  value: unknown,
  settings?: AgentProfileSecretSetting[],
): value is AgentHostContract {
  if (!isRecord(value)) return false;
  if (value.kind === "generic_mcp") return isExactRecord(value, ["kind"]);
  if (value.kind !== "managed_reference" || !isExactRecord(value, [
    "kind", "host_contract_revision", "runner_template", "provider", "provider_address", "model_id",
  ])) return false;
  return isRevision(value.host_contract_revision)
    && isRunnerTemplateReference(value.runner_template)
    && value.provider === "open_ai_compatible"
    && isLoopbackSocket(value.provider_address)
    && isText(value.model_id)
    && (settings === undefined || (settings.length === 1
      && settings[0]?.kind === "model_provider"
      && settings[0]?.key === "MODEL_PROVIDER_TOKEN"));
}

function isSecretSetting(value: unknown): value is AgentProfileSecretSetting {
  return isExactRecord(value, ["key", "kind", "availability"])
    && isSecretKey(value.key)
    && value.kind === "model_provider"
    && ["configured", "missing", "unavailable"].includes(String(value.availability));
}

function isAgentProfileAssignmentCatalog(value: unknown): value is AgentProfileAssignmentCatalog {
  return isExactRecord(value, ["schema", "assignments"])
    && value.schema === "worldstream/studio-agent-profile-assignment-catalog/v1"
    && isBoundedArray(value.assignments, isAgentProfileAssignment)
    && unique(value.assignments.map((assignment) => assignment.assignment_id))
    && unique(value.assignments.map((assignment) => `${assignment.draft_id}\0${assignment.seat_id}`))
    && unique(value.assignments.map((assignment) =>
      `${assignment.membership.room_id}\0${assignment.membership.member_id}`));
}

function isAgentProfileAssignment(value: unknown): value is AgentProfileSeatAssignment {
  return isExactRecord(value, [
    "schema", "assignment_id", "draft_id", "seat_id", "profile", "membership", "execution",
  ])
    && value.schema === "worldstream/studio-agent-profile-assignment/v1"
    && isUlid(value.assignment_id)
    && isProfileId(value.draft_id)
    && isProfileId(value.seat_id)
    && isProfileReference(value.profile)
    && isMembership(value.membership)
    && isExecution(value.execution);
}

function isProfileReference(value: unknown): value is AgentProfileRevisionReference {
  return isExactRecord(value, ["profile_id", "revision"])
    && isProfileId(value.profile_id)
    && isRevision(value.revision);
}

function isRunnerTemplateReference(
  value: unknown,
): value is { template_id: string; revision: string } {
  return isExactRecord(value, ["template_id", "revision"])
    && isProfileId(value.template_id)
    && isRevision(value.revision);
}

function isMembership(value: unknown): value is AgentProfileMembershipBinding {
  return isExactRecord(value, ["room_id", "member_id", "principal_id", "role"])
    && isUlid(value.room_id)
    && isUlid(value.member_id)
    && isUlid(value.principal_id)
    && isText(value.role);
}

function isExecution(value: unknown): value is AgentExecutionBinding {
  if (!isRecord(value)) return false;
  if (value.kind === "external") return isExactRecord(value, ["kind"]);
  if (value.kind === "managed") {
    return isExactRecord(value, ["kind", "runner_id"]) && isUlid(value.runner_id);
  }
  return value.kind === "managed_reference"
    && isExactRecord(value, [
      "kind", "runner_id", "instance_id", "template_id", "template_revision",
    ])
    && isUlid(value.runner_id)
    && isProfileId(value.instance_id)
    && isProfileId(value.template_id)
    && isRevision(value.template_revision);
}

function isConfiguration(value: unknown): value is Record<string, string> {
  return isRecord(value)
    && Object.keys(value).length <= 64
    && Object.entries(value).every(([key, setting]) =>
      /^[a-z0-9][a-z0-9_.-]{0,63}$/.test(key)
      && !/(secret|token|password|credential|key)/i.test(key)
      && typeof setting === "string"
      && setting.length > 0
      && setting.length <= 4096
      && !/[\0\r\n]/.test(setting)
      && !isCredentialShapedValue(setting));
}

function isSensitiveKey(value: string): boolean {
  return /(secret|token|password|credential|key)/i.test(value);
}

function publishedRevisionMatches(
  request: AgentProfilePublishRequest,
  response: AgentProfileRevision,
): boolean {
  return response.profile_id === request.profile_id
    && response.revision === request.revision
    && response.display_name === request.display_name
    && exactStringRecord(response.non_secret_configuration, request.non_secret_configuration)
    && JSON.stringify(response.host_contract) === JSON.stringify(request.host_contract)
    && (request.host_contract.kind === "generic_mcp"
      ? response.secret_settings.length === 0
      : response.secret_settings.length === 1
        && response.secret_settings[0]?.key === "MODEL_PROVIDER_TOKEN"
        && response.secret_settings[0]?.kind === "model_provider");
}

function isLoopbackSocket(value: unknown): value is string {
  if (typeof value !== "string") return false;
  const match = /^(?:127(?:\.\d{1,3}){3}|\[::1\]):([0-9]{1,5})$/.exec(value);
  if (match === null) return false;
  const port = Number(match[1]);
  return Number.isInteger(port) && port > 0 && port <= 65535;
}

function exactStringRecord(left: Record<string, string>, right: Record<string, string>): boolean {
  const leftKeys = Object.keys(left).sort();
  const rightKeys = Object.keys(right).sort();
  return leftKeys.length === rightKeys.length
    && leftKeys.every((key, index) => key === rightKeys[index] && left[key] === right[key]);
}

function isCredentialShapedValue(value: string): boolean {
  return /^(bearer(?:\s|=)|basic\s|sk[-_]|api_?key=|token=|secret=|password=|credential=)/i
    .test(value.trim());
}

function isBoundedArray<T>(
  value: unknown,
  validate: (item: unknown) => item is T,
): value is T[] {
  return Array.isArray(value) && value.length <= maximumRows && value.every(validate);
}

function isExactRecord(value: unknown, keys: readonly string[]): value is Record<string, unknown> {
  return isRecord(value)
    && Object.keys(value).length === keys.length
    && keys.every((key) => key in value);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isProfileId(value: unknown): value is string {
  return typeof value === "string" && /^[a-z0-9][a-z0-9_-]{0,63}$/.test(value);
}

function isRevision(value: unknown): value is string {
  return typeof value === "string" && /^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/.test(value);
}

function isSecretKey(value: unknown): value is string {
  return typeof value === "string" && /^[A-Z_][A-Z0-9_]{0,63}$/.test(value);
}

function isUlid(value: unknown): value is string {
  return typeof value === "string" && /^[0-7][0-9A-HJKMNP-TV-Z]{25}$/.test(value);
}

function isText(value: unknown): value is string {
  return typeof value === "string" && value.length > 0 && value.length <= 256 && !/[\0\r\n]/.test(value);
}

function unique(values: string[]): boolean {
  return new Set(values).size === values.length;
}
