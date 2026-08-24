import type {
  ActivityPackReference,
  ActivityPackRole,
} from "./activityPacks";

export type RoomDraftStep =
  | "activity"
  | "configuration"
  | "seats"
  | "readiness"
  | "review";

export interface RoomDraftSeat {
  seat_id: string;
  role: string;
  required: boolean;
  display_name: string;
}

export interface RoomDraftSeatReadinessPolicy {
  seat_id: string;
  role: string;
  required: boolean;
}

export interface RoomDraft {
  schema: "worldstream/studio-room-draft/v1";
  draft_id: string;
  pack: ActivityPackReference | null;
  configuration: unknown;
  seats: RoomDraftSeat[];
  readiness: RoomDraftSeatReadinessPolicy[];
  last_valid_step: RoomDraftStep | null;
}

export interface RoomDraftReview {
  pack: ActivityPackReference | null;
  configuration: unknown;
  seats: RoomDraftSeat[];
  readiness: RoomDraftSeatReadinessPolicy[];
}

export interface RoomDraftResponse {
  version: "studio_room_draft.v1";
  draft: RoomDraft;
  review: RoomDraftReview;
}

export interface RoomDraftFieldError {
  path: string;
  code: string;
  message: string;
}

export interface RoomDraftValidationFailure {
  version: "studio_room_draft_error.v1";
  field_errors: RoomDraftFieldError[];
}

type Fetcher = (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>;

const maximumErrors = 64;
const maximumSeats = 64;

export function createRoomDraft(draftId: string): RoomDraft {
  return {
    schema: "worldstream/studio-room-draft/v1",
    draft_id: draftId,
    pack: null,
    configuration: {},
    seats: [],
    readiness: [],
    last_valid_step: null,
  };
}

export async function loadRoomDraft(
  draftId: string,
  fetcher: Fetcher = fetch,
): Promise<RoomDraftResponse | null> {
  if (!isIdentifier(draftId)) return null;
  try {
    const response = await fetcher(`/api/v1/room-drafts/${draftId}`, {
      headers: { accept: "application/json" },
    });
    if (!response.ok) return null;
    const value: unknown = await response.json();
    return isRoomDraftResponse(value) && value.draft.draft_id === draftId ? value : null;
  } catch {
    return null;
  }
}

export async function saveRoomDraft(
  draft: RoomDraft,
  fetcher: Fetcher = fetch,
): Promise<RoomDraftResponse | RoomDraftValidationFailure | null> {
  if (!isRoomDraft(draft)) return null;
  try {
    const response = await fetcher(`/api/v1/room-drafts/${draft.draft_id}`, {
      method: "PUT",
      headers: { accept: "application/json", "content-type": "application/json" },
      body: JSON.stringify(draft),
    });
    const value: unknown = await response.json();
    if (response.ok && isRoomDraftResponse(value)) return value;
    const errors = extractFieldErrors(value);
    return errors.length > 0
      ? { version: "studio_room_draft_error.v1", field_errors: errors }
      : null;
  } catch {
    return null;
  }
}

export function buildSeatPolicy(roles: ActivityPackRole[]): {
  seats: RoomDraftSeat[];
  readiness: RoomDraftSeatReadinessPolicy[];
} {
  const seats: RoomDraftSeat[] = [];
  for (const [roleIndex, role] of roles.entries()) {
    const maximum = Math.min(role.maximum, maximumSeats - seats.length);
    for (let seatIndex = 0; seatIndex < maximum; seatIndex += 1) {
      seats.push({
        seat_id: `role-${roleIndex + 1}-seat-${seatIndex + 1}`,
        role: role.role,
        required: seatIndex < role.minimum,
        display_name: `${role.role} ${seatIndex + 1}`,
      });
    }
    if (seats.length === maximumSeats) break;
  }
  return {
    seats,
    readiness: seats.map(({ seat_id, role, required }) => ({ seat_id, role, required })),
  };
}

export function validateConfiguration(
  schema: unknown,
  configuration: unknown,
): RoomDraftFieldError[] {
  const errors: RoomDraftFieldError[] = [];
  validateSchema(schema, configuration, "/configuration", errors);
  return errors.slice(0, maximumErrors);
}

function validateSchema(
  schema: unknown,
  value: unknown,
  path: string,
  errors: RoomDraftFieldError[],
): void {
  if (errors.length >= maximumErrors) return;
  if (!isRecord(schema) || typeof schema.type !== "string") {
    errors.push(error(path, "schema_unsupported", "The installed configuration schema is unsupported."));
    return;
  }
  if ("const" in schema && !jsonEquals(schema.const, value)) {
    errors.push(error(path, "const", "The field must use the declared fixed value."));
  }
  if (Array.isArray(schema.enum) && !schema.enum.some((choice) => jsonEquals(choice, value))) {
    errors.push(error(path, "enum", "The field must use one of the declared values."));
  }
  if (!matchesType(value, schema.type)) {
    errors.push(error(path, "type", "The field has the wrong value type."));
    return;
  }
  if (schema.type === "object" && isRecord(value)) {
    const properties = isRecord(schema.properties) ? schema.properties : {};
    const required = Array.isArray(schema.required)
      ? schema.required.filter((field): field is string => typeof field === "string")
      : [];
    for (const field of required) {
      if (!(field in value)) {
        errors.push(error(childPointer(path, field), "required", "The field is required."));
      }
    }
    if (schema.additionalProperties === false) {
      for (const field of Object.keys(value)) {
        if (!(field in properties)) {
          errors.push(error(childPointer(path, field), "additional_property", "The field is not declared by this exact revision."));
        }
      }
    }
    for (const [field, fieldSchema] of Object.entries(properties)) {
      if (field in value) validateSchema(fieldSchema, value[field], childPointer(path, field), errors);
    }
  } else if (schema.type === "array" && Array.isArray(value)) {
    if (typeof schema.minItems === "number" && value.length < schema.minItems) {
      errors.push(error(path, "min_items", "The field has too few items."));
    }
    if (typeof schema.maxItems === "number" && value.length > schema.maxItems) {
      errors.push(error(path, "max_items", "The field has too many items."));
    }
    if (schema.items !== undefined) {
      value.forEach((item, index) => validateSchema(schema.items, item, `${path}/${index}`, errors));
    }
  } else if (schema.type === "string" && typeof value === "string") {
    if (typeof schema.minLength === "number" && [...value].length < schema.minLength) {
      errors.push(error(path, "min_length", "The field is shorter than allowed."));
    }
    if (typeof schema.maxLength === "number" && [...value].length > schema.maxLength) {
      errors.push(error(path, "max_length", "The field is longer than allowed."));
    }
  } else if ((schema.type === "number" || schema.type === "integer") && typeof value === "number") {
    if (typeof schema.minimum === "number" && value < schema.minimum) {
      errors.push(error(path, "minimum", "The field is below the declared minimum."));
    }
    if (typeof schema.maximum === "number" && value > schema.maximum) {
      errors.push(error(path, "maximum", "The field is above the declared maximum."));
    }
  }
}

function isRoomDraftResponse(value: unknown): value is RoomDraftResponse {
  return (
    isRecordWithKeys(value, ["version", "draft", "review"]) &&
    value.version === "studio_room_draft.v1" &&
    isRoomDraft(value.draft) &&
    isReview(value.review) &&
    jsonEquals(value.review.pack, value.draft.pack) &&
    jsonEquals(value.review.configuration, value.draft.configuration) &&
    jsonEquals(value.review.seats, value.draft.seats) &&
    jsonEquals(value.review.readiness, value.draft.readiness)
  );
}

function isRoomDraft(value: unknown): value is RoomDraft {
  if (!isRecordWithKeys(value, [
    "schema", "draft_id", "pack", "configuration", "seats", "readiness", "last_valid_step",
  ])) return false;
  if (
    value.schema !== "worldstream/studio-room-draft/v1" ||
    !isIdentifier(value.draft_id) ||
    !(value.pack === null || isPackReference(value.pack)) ||
    !Array.isArray(value.seats) || value.seats.length > maximumSeats ||
    !value.seats.every(isSeat) ||
    !Array.isArray(value.readiness) || value.readiness.length !== value.seats.length ||
    !value.readiness.every(isReadiness) ||
    !(value.last_valid_step === null || isStep(value.last_valid_step))
  ) return false;
  const seats = new Map(value.seats.map((seat) => [seat.seat_id, seat]));
  return new Set(value.seats.map((seat) => seat.seat_id)).size === value.seats.length &&
    value.readiness.every((policy) => {
      const seat = seats.get(policy.seat_id);
      return seat !== undefined && seat.role === policy.role && seat.required === policy.required;
    });
}

function isReview(value: unknown): value is RoomDraftReview {
  return isRecordWithKeys(value, ["pack", "configuration", "seats", "readiness"]);
}

function isSeat(value: unknown): value is RoomDraftSeat {
  return isRecordWithKeys(value, ["seat_id", "role", "required", "display_name"]) &&
    isIdentifier(value.seat_id) && isText(value.role) && typeof value.required === "boolean" &&
    isText(value.display_name);
}

function isReadiness(value: unknown): value is RoomDraftSeatReadinessPolicy {
  return isRecordWithKeys(value, ["seat_id", "role", "required"]) &&
    isIdentifier(value.seat_id) && isText(value.role) && typeof value.required === "boolean";
}

function isPackReference(value: unknown): value is ActivityPackReference {
  return isRecordWithKeys(value, ["id", "version", "digest"]) && isText(value.id) &&
    isText(value.version) && typeof value.digest === "string" && /^blake3:[0-9a-f]{64}$/.test(value.digest);
}

function extractFieldErrors(value: unknown): RoomDraftFieldError[] {
  if (!isRecord(value) || !isRecord(value.error) || !Array.isArray(value.error.field_errors)) return [];
  return value.error.field_errors.filter(isFieldError).slice(0, maximumErrors);
}

function isFieldError(value: unknown): value is RoomDraftFieldError {
  return isRecordWithKeys(value, ["path", "code", "message"]) &&
    typeof value.path === "string" && value.path.startsWith("/") && value.path.length <= 512 &&
    isText(value.code) && typeof value.message === "string" && value.message.length > 0 && value.message.length <= 256;
}

function matchesType(value: unknown, type: string): boolean {
  if (type === "object") return isRecord(value);
  if (type === "array") return Array.isArray(value);
  if (type === "string") return typeof value === "string";
  if (type === "number") return typeof value === "number" && Number.isFinite(value);
  if (type === "integer") return typeof value === "number" && Number.isSafeInteger(value);
  if (type === "boolean") return typeof value === "boolean";
  if (type === "null") return value === null;
  return false;
}

function error(path: string, code: string, message: string): RoomDraftFieldError {
  return { path, code, message };
}

function childPointer(parent: string, field: string): string {
  return `${parent}/${field.replaceAll("~", "~0").replaceAll("/", "~1")}`;
}

function jsonEquals(left: unknown, right: unknown): boolean {
  return JSON.stringify(left) === JSON.stringify(right);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isRecordWithKeys(value: unknown, keys: readonly string[]): value is Record<string, unknown> {
  return isRecord(value) && Object.keys(value).length === keys.length && keys.every((key) => key in value);
}

function isIdentifier(value: unknown): value is string {
  return typeof value === "string" && /^[a-z0-9][a-z0-9-]{0,63}$/.test(value);
}

function isText(value: unknown): value is string {
  return typeof value === "string" && value.length > 0 && value.length <= 256;
}

function isStep(value: unknown): value is RoomDraftStep {
  return ["activity", "configuration", "seats", "readiness", "review"].includes(String(value));
}
