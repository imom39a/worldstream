export const MIDNIGHT_ARCHIVE_ACTION_TYPES = [
  "stage_move",
  "stage_inspect_records",
  "stage_inspect_conservation",
  "stage_use_verifier",
  "stage_open_service_hatch",
  "stage_recover_candidate",
  "stage_extract",
  "stage_wait",
  "commit_turn",
] as const;

export type MidnightArchiveActionType = typeof MIDNIGHT_ARCHIVE_ACTION_TYPES[number];

const ACTION_TYPES = new Set<string>(MIDNIGHT_ARCHIVE_ACTION_TYPES);

export function isMidnightArchiveActionType(
  value: unknown,
): value is MidnightArchiveActionType {
  return typeof value === "string" && ACTION_TYPES.has(value);
}
