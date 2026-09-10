export const MIDNIGHT_ARCHIVE_ACTION_TYPES = [
  "stage_move",
  "stage_inspect_records",
  "stage_inspect_conservation",
  "stage_use_verifier",
  "stage_accept_preservation_agreement",
  "stage_prepare_collection",
  "stage_energize_preservation_equipment",
  "stage_open_service_hatch",
  "stage_recover_candidate",
  "stage_protect_source_record",
  "stage_extract",
  "stage_wait",
  "commit_turn",
  "assign_mira_task",
  "cancel_mira_task",
  "set_mira_follow",
  "set_mira_hold",
  "set_mira_regroup",
  "request_mira_plan",
  "prepare_mira_contribution",
  "defer_mira_contribution",
] as const;

export type MidnightArchiveActionType = typeof MIDNIGHT_ARCHIVE_ACTION_TYPES[number];

const ACTION_TYPES = new Set<string>(MIDNIGHT_ARCHIVE_ACTION_TYPES);

export function isMidnightArchiveActionType(
  value: unknown,
): value is MidnightArchiveActionType {
  return typeof value === "string" && ACTION_TYPES.has(value);
}
