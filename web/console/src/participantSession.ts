/** @deprecated New Activity Clients import canonical names from `@worldstream/client`. */
export { ActivityClientSession } from "@worldstream/client/retained-session";
export type {
  ActivityClientAction,
  ActivityClientSessionState,
} from "@worldstream/client/retained-session";

/** @deprecated Console-only source compatibility while the Inspector migrates. */
export { ActivityClientSession as ParticipantConsoleSession } from "@worldstream/client/retained-session";
export type {
  ActivityClientAction as ParticipantConsoleAction,
  ActivityClientSessionState as ParticipantConsoleSessionState,
} from "@worldstream/client/retained-session";
