export const AGENT_HEIST_ACTION_TYPES = [
  "inspect_clue",
  "publish_clue",
  "offer_exchange",
  "accept_exchange",
  "propose_plan",
  "endorse_plan",
  "challenge_plan",
  "commit_move",
  "acknowledge_result",
] as const;

export type AgentHeistActionType = typeof AGENT_HEIST_ACTION_TYPES[number];

const AGENT_HEIST_ACTION_TYPE_SET = new Set<string>(AGENT_HEIST_ACTION_TYPES);

export function isAgentHeistActionType(value: unknown): value is AgentHeistActionType {
  return typeof value === "string" && AGENT_HEIST_ACTION_TYPE_SET.has(value);
}
