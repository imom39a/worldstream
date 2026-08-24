import {
  ParticipantHandoffError,
  type ParticipantActionInput,
  type ParticipantBrowserObservation,
  type ParticipantConsoleStartup,
  type ParticipantHandoffClient,
  type ParticipantSessionStatus,
} from "./participantHandoff";

export type ParticipantConsoleSessionState =
  | { state: "connecting"; action: "wait"; message: string }
  | { state: "live"; action: "continue"; message: null; observation: ParticipantBrowserObservation }
  | { state: "disconnected"; action: "reconnect"; message: string }
  | { state: "setup_required"; action: "return_to_task_setup"; message: string };

export interface ParticipantConsoleAction {
  actionId: string;
  basedOnRoomSeq: number;
  offerId: string;
  schemaDigest: string;
  actionType: string;
  payload: unknown;
}

/** Small handed-off session state machine; the existing direct console remains separate. */
export class ParticipantConsoleSession {
  private current: ParticipantConsoleSessionState = {
    state: "connecting",
    action: "wait",
    message: "Opening Participant View…",
  };

  constructor(private readonly client: Pick<ParticipantHandoffClient, "redeem" | "resume" | "observe" | "act">) {}

  get state(): ParticipantConsoleSessionState {
    return this.current;
  }

  async start(startup: ParticipantConsoleStartup): Promise<ParticipantConsoleSessionState> {
    if (startup.kind === "invalid_handoff") {
      return this.setSetupRequired("This Participant View handoff is invalid or incomplete.");
    }
    if (startup.kind === "retained_error") return this.applyError(startup.error);
    try {
      const status = startup.kind === "handoff"
        ? await this.client.redeem(startup.handoff)
        : startup.kind === "retained"
          ? startup.status
          : await this.client.resume();
      return await this.applyStatus(status);
    } catch (error) {
      return this.applyError(error);
    }
  }

  async reconnect(): Promise<ParticipantConsoleSessionState> {
    this.current = { state: "connecting", action: "wait", message: "Reconnecting Participant View…" };
    try {
      const status = await this.client.resume();
      return await this.applyStatus(status);
    } catch (error) {
      return this.applyError(error);
    }
  }

  async act(action: ParticipantConsoleAction): Promise<unknown> {
    if (this.current.state !== "live") {
      throw new ParticipantHandoffError(
        "participant_session_not_live",
        "Participant View must reconnect before submitting an Action.",
        "reconnect",
        true,
      );
    }
    const request: ParticipantActionInput = {
      action_id: action.actionId,
      based_on_room_seq: action.basedOnRoomSeq,
      offer_id: action.offerId,
      schema_digest: action.schemaDigest,
      action_type: action.actionType,
      payload: action.payload,
    };
    return this.client.act(request);
  }

  private async applyStatus(status: ParticipantSessionStatus): Promise<ParticipantConsoleSessionState> {
    if (status.state === "disconnected") {
      this.current = { state: "disconnected", action: "reconnect", message: "Participant View is disconnected." };
      return this.current;
    }
    try {
      const observation = await this.client.observe(null);
      this.current = { state: "live", action: "continue", message: null, observation };
      return this.current;
    } catch (error) {
      return this.applyError(error);
    }
  }

  private applyError(error: unknown): ParticipantConsoleSessionState {
    if (error instanceof ParticipantHandoffError && error.nextAction === "return_to_task_setup") {
      return this.setSetupRequired(error.message);
    }
    const message = error instanceof ParticipantHandoffError
      ? error.message
      : "Participant View disconnected unexpectedly.";
    this.current = { state: "disconnected", action: "reconnect", message };
    return this.current;
  }

  private setSetupRequired(message: string): ParticipantConsoleSessionState {
    this.current = { state: "setup_required", action: "return_to_task_setup", message };
    return this.current;
  }
}
