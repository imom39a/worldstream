/** @deprecated New Activity Clients import canonical names from `@worldstream/client`. */
export {
  PARTICIPANT_HANDOFF_VERSION,
  PARTICIPANT_SESSION_VERSION,
  ActivityClientHandoffClient,
  ActivityClientHandoffError,
  consumeActivityClientHandoffFragment,
  resumeRetainedActivityClient,
  selectActivityClientStartup,
} from "@worldstream/client/browser-handoff";
export type {
  ActivityClientActionReceipt,
  ActivityClientDelivery,
  ActivityClientPack,
  ActivityClientRoomHead,
  ActivityClientSessionStatus,
  ActivityClientStartup,
  AuthorizedRoomDeliveryBatch,
  BrowserNavigationTarget,
  OfferedActionSubmission,
  VerifiedRoomReplay,
} from "@worldstream/client/browser-handoff";

/** @deprecated Console-only source compatibility while the Inspector migrates. */
export {
  ActivityClientHandoffClient as ParticipantHandoffClient,
  ActivityClientHandoffError as ParticipantHandoffError,
  consumeActivityClientHandoffFragment as consumeParticipantHandoffFragment,
  resumeRetainedActivityClient as resumeRetainedParticipantConsole,
  selectActivityClientStartup as selectParticipantConsoleStartup,
} from "@worldstream/client/browser-handoff";
export type {
  ActivityClientDelivery as ParticipantBrowserDelivery,
  ActivityClientPack as ParticipantBrowserPack,
  ActivityClientRoomHead as ParticipantBrowserRoomHead,
  ActivityClientSessionStatus as ParticipantSessionStatus,
  ActivityClientStartup as ParticipantConsoleStartup,
  AuthorizedRoomDeliveryBatch as ParticipantBrowserObservation,
  OfferedActionSubmission as ParticipantActionInput,
  VerifiedRoomReplay as ParticipantBrowserReplay,
} from "@worldstream/client/browser-handoff";
