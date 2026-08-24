export interface ActivityPackReference {
  id: string;
  version: string;
  digest: string;
}

export interface ActivityPackRevisionSummary {
  pack: ActivityPackReference;
  name: string;
  selectable_for_new_rooms: boolean;
  runnable_for_retained_rooms: boolean;
}

export interface ActivityPackCatalog {
  version: "activity_pack_catalog.v1";
  revisions: ActivityPackRevisionSummary[];
}

export interface ActivityPackSchema {
  schema_id: string;
  schema_digest: string;
  schema: unknown;
}

export interface ActivityPackRole {
  role: string;
  minimum: number;
  maximum: number;
}

export interface ActivityPackAction {
  action_type: string;
  payload_schema: ActivityPackSchema;
}

export interface ActivityPackLobbyCompatibility {
  contract: string;
  configuration_schema: ActivityPackSchema;
}

export interface ActivityPackRevisionDetail {
  summary: ActivityPackRevisionSummary;
  roles: ActivityPackRole[];
  configuration_schema: ActivityPackSchema;
  actions: ActivityPackAction[];
  lobby_compatibility?: ActivityPackLobbyCompatibility;
}

export interface ActivityPackDetailResponse {
  version: "activity_pack_catalog.v1";
  revision: ActivityPackRevisionDetail;
}

export interface StorageLike {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
}

type Fetcher = (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>;

const selectionKey = "worldstream.studio.activity-pack-selection.v1";
const maxCatalogRevisions = 256;
const maxDescriptorRows = 256;

export async function loadActivityPackCatalog(
  fetcher: Fetcher = fetch,
): Promise<ActivityPackCatalog | null> {
  try {
    const response = await fetcher("/api/v1/activity-packs", {
      headers: { accept: "application/json" },
    });
    if (!response.ok) return null;
    const value: unknown = await response.json();
    return isActivityPackCatalog(value) ? value : null;
  } catch {
    return null;
  }
}

export async function loadActivityPackDetail(
  digest: string,
  fetcher: Fetcher = fetch,
): Promise<ActivityPackDetailResponse | null> {
  if (!isDigest(digest)) return null;
  try {
    const response = await fetcher(`/api/v1/activity-packs/${digest}`, {
      headers: { accept: "application/json" },
    });
    if (!response.ok) return null;
    const value: unknown = await response.json();
    if (!isActivityPackDetailResponse(value)) return null;
    return value.revision.summary.pack.digest === digest ? value : null;
  } catch {
    return null;
  }
}

export function loadActivityPackSelection(
  storage: StorageLike = window.localStorage,
): ActivityPackReference | null {
  try {
    const encoded = storage.getItem(selectionKey);
    if (encoded === null) return null;
    const value: unknown = JSON.parse(encoded);
    return isPackReference(value) ? value : null;
  } catch {
    return null;
  }
}

export function saveActivityPackSelection(
  selection: ActivityPackReference | null,
  storage: StorageLike = window.localStorage,
): void {
  if (selection === null) {
    storage.removeItem(selectionKey);
    return;
  }
  if (!isPackReference(selection)) return;
  storage.setItem(selectionKey, JSON.stringify(selection));
}

function isActivityPackCatalog(value: unknown): value is ActivityPackCatalog {
  return (
    isRecordWithKeys(value, ["version", "revisions"]) &&
    value.version === "activity_pack_catalog.v1" &&
    Array.isArray(value.revisions) &&
    value.revisions.length <= maxCatalogRevisions &&
    value.revisions.every(isRevisionSummary) &&
    new Set(value.revisions.map((revision) => revision.pack.digest)).size ===
      value.revisions.length
  );
}

function isActivityPackDetailResponse(value: unknown): value is ActivityPackDetailResponse {
  return (
    isRecordWithKeys(value, ["version", "revision"]) &&
    value.version === "activity_pack_catalog.v1" &&
    isRevisionDetail(value.revision)
  );
}

function isRevisionDetail(value: unknown): value is ActivityPackRevisionDetail {
  if (!isRecord(value)) return false;
  const keys = Object.keys(value);
  if (
    !keys.every((key) =>
      ["summary", "roles", "configuration_schema", "actions", "lobby_compatibility"].includes(key),
    ) ||
    ![4, 5].includes(keys.length) ||
    !isRevisionSummary(value.summary) ||
    !Array.isArray(value.roles) ||
    value.roles.length > maxDescriptorRows ||
    !value.roles.every(isRole) ||
    !isSchema(value.configuration_schema) ||
    !Array.isArray(value.actions) ||
    value.actions.length > maxDescriptorRows ||
    !value.actions.every(isAction)
  ) {
    return false;
  }
  return value.lobby_compatibility === undefined || isLobbyCompatibility(value.lobby_compatibility);
}

function isRevisionSummary(value: unknown): value is ActivityPackRevisionSummary {
  return (
    isRecordWithKeys(value, [
      "pack",
      "name",
      "selectable_for_new_rooms",
      "runnable_for_retained_rooms",
    ]) &&
    isPackReference(value.pack) &&
    isBoundedString(value.name) &&
    typeof value.selectable_for_new_rooms === "boolean" &&
    typeof value.runnable_for_retained_rooms === "boolean"
  );
}

function isPackReference(value: unknown): value is ActivityPackReference {
  return (
    isRecordWithKeys(value, ["id", "version", "digest"]) &&
    isBoundedString(value.id) &&
    isBoundedString(value.version) &&
    isDigest(value.digest)
  );
}

function isSchema(value: unknown): value is ActivityPackSchema {
  return (
    isRecordWithKeys(value, ["schema_id", "schema_digest", "schema"]) &&
    isBoundedString(value.schema_id) &&
    isDigest(value.schema_digest)
  );
}

function isRole(value: unknown): value is ActivityPackRole {
  return (
    isRecordWithKeys(value, ["role", "minimum", "maximum"]) &&
    isBoundedString(value.role) &&
    isBoundedCount(value.minimum) &&
    isBoundedCount(value.maximum) &&
    value.minimum <= value.maximum
  );
}

function isAction(value: unknown): value is ActivityPackAction {
  return (
    isRecordWithKeys(value, ["action_type", "payload_schema"]) &&
    isBoundedString(value.action_type) &&
    isSchema(value.payload_schema)
  );
}

function isLobbyCompatibility(value: unknown): value is ActivityPackLobbyCompatibility {
  return (
    isRecordWithKeys(value, ["contract", "configuration_schema"]) &&
    isBoundedString(value.contract) &&
    isSchema(value.configuration_schema)
  );
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isRecordWithKeys(value: unknown, keys: readonly string[]): value is Record<string, unknown> {
  return isRecord(value) && Object.keys(value).length === keys.length && keys.every((key) => key in value);
}

function isBoundedString(value: unknown): value is string {
  return typeof value === "string" && value.length > 0 && value.length <= 256;
}

function isDigest(value: unknown): value is string {
  return typeof value === "string" && /^blake3:[0-9a-f]{64}$/.test(value);
}

function isBoundedCount(value: unknown): value is number {
  return Number.isSafeInteger(value) && Number(value) >= 0 && Number(value) <= 1_000_000;
}
