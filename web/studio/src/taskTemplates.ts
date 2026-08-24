import {
  isRoomDraft,
  type RoomDraft,
  type RoomDraftSeat,
  type RoomDraftSeatReadinessPolicy,
} from "./roomDrafts";
import type { ActivityPackReference } from "./activityPacks";

export type TaskTemplateDependencyStatus = "ready" | "missing" | "incompatible" | "unavailable";

export interface TaskTemplateDependencyIssue {
  path: string;
  code: string;
  message: string;
}

export interface TaskTemplateDependencyReport {
  status: TaskTemplateDependencyStatus;
  issues: TaskTemplateDependencyIssue[];
}

export interface TaskTemplateRevision {
  schema: "worldstream/studio-task-template/v1";
  template_id: string;
  revision: string;
  display_name: string;
  source_draft_id: string;
  pack: ActivityPackReference;
  configuration: unknown;
  seats: RoomDraftSeat[];
  readiness: RoomDraftSeatReadinessPolicy[];
}

export interface TaskTemplateRevisionView {
  revision: TaskTemplateRevision;
  dependencies: TaskTemplateDependencyReport;
  used_by_draft_ids: string[];
}

export interface TaskTemplateCatalog {
  schema: "worldstream/studio-task-template-catalog/v1";
  revisions: TaskTemplateRevisionView[];
}

export interface TaskTemplatePublishRequest {
  template_id: string;
  revision: string;
  display_name: string;
  source_draft_id: string;
}

type Fetcher = (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>;

const maximumRows = 256;

export async function loadTaskTemplates(
  fetcher: Fetcher = fetch,
): Promise<TaskTemplateCatalog | null> {
  try {
    const response = await fetcher("/api/v1/task-templates", { headers: { accept: "application/json" } });
    if (!response.ok) return null;
    const value: unknown = await response.json();
    return isCatalog(value) ? value : null;
  } catch {
    return null;
  }
}

export async function publishTaskTemplate(
  request: TaskTemplatePublishRequest,
  fetcher: Fetcher = fetch,
): Promise<TaskTemplateRevisionView | null> {
  if (!isPublishRequest(request)) return null;
  try {
    const response = await fetcher("/api/v1/task-templates", {
      method: "POST",
      headers: { accept: "application/json", "content-type": "application/json" },
      body: JSON.stringify(request),
    });
    if (!response.ok) return null;
    const value: unknown = await response.json();
    return isRevisionView(value) ? value : null;
  } catch {
    return null;
  }
}

export async function instantiateTaskTemplate(
  templateId: string,
  revision: string,
  draftId: string,
  fetcher: Fetcher = fetch,
): Promise<RoomDraft | null> {
  if (!isId(templateId) || !isRevisionId(revision) || !isId(draftId)) return null;
  try {
    const response = await fetcher(
      `/api/v1/task-templates/${encodeURIComponent(templateId)}/revisions/${encodeURIComponent(revision)}/instantiate`,
      {
        method: "POST",
        headers: { accept: "application/json", "content-type": "application/json" },
        body: JSON.stringify({ draft_id: draftId }),
      },
    );
    if (!response.ok) return null;
    const value: unknown = await response.json();
    return isRoomDraft(value) && value.draft_id === draftId ? value : null;
  } catch {
    return null;
  }
}

function isCatalog(value: unknown): value is TaskTemplateCatalog {
  return isRecordWithKeys(value, ["schema", "revisions"]) &&
    value.schema === "worldstream/studio-task-template-catalog/v1" &&
    Array.isArray(value.revisions) && value.revisions.length <= maximumRows &&
    value.revisions.every(isRevisionView) &&
    new Set(value.revisions.map((view) =>
      `${view.revision.template_id}\0${view.revision.revision}`)).size === value.revisions.length;
}

function isRevisionView(value: unknown): value is TaskTemplateRevisionView {
  if (containsCredentialField(value) || !isRecordWithKeys(value, [
    "revision", "dependencies", "used_by_draft_ids",
  ]) || !isRevision(value.revision) || !isDependencies(value.dependencies) ||
    !Array.isArray(value.used_by_draft_ids) || value.used_by_draft_ids.length > maximumRows ||
    !value.used_by_draft_ids.every(isId)) return false;
  return new Set(value.used_by_draft_ids).size === value.used_by_draft_ids.length;
}

function isRevision(value: unknown): value is TaskTemplateRevision {
  if (!isRecordWithKeys(value, [
    "schema", "template_id", "revision", "display_name", "source_draft_id", "pack",
    "configuration", "seats", "readiness",
  ]) || value.schema !== "worldstream/studio-task-template/v1" || !isId(value.template_id) ||
    !isRevisionId(value.revision) || !isText(value.display_name) || !isId(value.source_draft_id)) {
    return false;
  }
  return isRoomDraft({
    schema: "worldstream/studio-room-draft/v1",
    draft_id: value.source_draft_id,
    pack: value.pack,
    configuration: value.configuration,
    seats: value.seats,
    readiness: value.readiness,
    last_valid_step: "review",
  }) && (value.seats as RoomDraftSeat[]).every((seat) =>
    seat.principal_kind !== "agent" || seat.agent_profile !== undefined);
}

function isDependencies(value: unknown): value is TaskTemplateDependencyReport {
  if (!isRecordWithKeys(value, ["status", "issues"]) ||
    !["ready", "missing", "incompatible", "unavailable"].includes(String(value.status)) ||
    !Array.isArray(value.issues) || value.issues.length > 64 || !value.issues.every(isIssue)) return false;
  if (value.status === "ready") return value.issues.length === 0;
  if (value.issues.length === 0) return false;
  const codes = (value.issues as TaskTemplateDependencyIssue[]).map((issue) => issue.code);
  const unavailable = codes.includes("dependency_check_unavailable");
  const missing = codes.some((code) => code.endsWith("_missing") || code === "revision_unavailable");
  if (value.status === "unavailable") return unavailable;
  if (value.status === "missing") return !unavailable && missing;
  return !unavailable && !missing;
}

function isIssue(value: unknown): value is TaskTemplateDependencyIssue {
  return isRecordWithKeys(value, ["path", "code", "message"]) &&
    typeof value.path === "string" && value.path.startsWith("/") && value.path.length <= 512 &&
    typeof value.code === "string" && /^[a-z0-9][a-z0-9_]{0,63}$/.test(value.code) &&
    typeof value.message === "string" && value.message.length > 0 && value.message.length <= 256;
}

function isPublishRequest(value: unknown): value is TaskTemplatePublishRequest {
  return isRecordWithKeys(value, ["template_id", "revision", "display_name", "source_draft_id"]) &&
    isId(value.template_id) && isRevisionId(value.revision) && isText(value.display_name) &&
    isId(value.source_draft_id);
}

function containsCredentialField(value: unknown, seen = new Set<object>()): boolean {
  if (Array.isArray(value)) return value.some((item) => containsCredentialField(item, seen));
  if (!isRecord(value) || seen.has(value)) return false;
  seen.add(value);
  return Object.entries(value).some(([key, child]) => {
    const normalized = key.replaceAll("-", "_").toLowerCase();
    return normalized.includes("bearer") || normalized.includes("password") ||
      normalized.includes("api_key") || normalized.includes("apikey") ||
      normalized.includes("token") || normalized.includes("secret_reference") ||
      containsCredentialField(child, seen);
  });
}

function isId(value: unknown): value is string {
  return typeof value === "string" && /^[a-z0-9][a-z0-9_-]{0,63}$/.test(value);
}

function isRevisionId(value: unknown): value is string {
  return typeof value === "string" && /^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/.test(value);
}

function isText(value: unknown): value is string {
  return typeof value === "string" && value.length > 0 && value.length <= 256 &&
    !/[\0\r\n]/.test(value);
}

function isRecordWithKeys(value: unknown, keys: readonly string[]): value is Record<string, unknown> {
  return isRecord(value) && Object.keys(value).length === keys.length && keys.every((key) => key in value);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
