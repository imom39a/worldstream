import {
  ActivityClientHandoffError,
  type ActivityClientActionReceipt,
  type ActivityClientHandoffClient,
  type ActivityClientSessionStatus,
  type ActivityClientStartup,
  type AuthorizedRoomDeliveryBatch,
  type OfferedActionSubmission,
  type VerifiedRoomReplay,
} from "./browserHandoff";

export type ActivityClientSessionState =
  | { state: "connecting"; action: "wait"; message: string }
  | { state: "live"; action: "continue"; message: null; deliveryBatch: AuthorizedRoomDeliveryBatch }
  | { state: "disconnected"; action: "reconnect"; message: string }
  | { state: "setup_required"; action: "return_to_task_setup"; message: string };

export interface ActivityClientAction {
  actionId: string;
  basedOnRoomSeq: number;
  offerId: string;
  schemaDigest: string;
  actionType: string;
  payload: unknown;
}

/** Small handed-off Activity Client session state machine. */
export class ActivityClientSession {
  private current: ActivityClientSessionState = {
    state: "connecting",
    action: "wait",
    message: "Opening Activity Client…",
  };
  private installedActionOffers: readonly unknown[] = EMPTY_ACTION_OFFERS;
  private startTask: Promise<ActivityClientSessionState> | null = null;

  constructor(private readonly client: Pick<ActivityClientHandoffClient, "redeem" | "resume" | "observe" | "act" | "replay">) {}

  get state(): ActivityClientSessionState {
    return this.current;
  }

  /** The exact current Action Offers reduced across ordered delivery batches. */
  get actionOffers(): readonly unknown[] {
    return this.installedActionOffers;
  }

  async start(startup: ActivityClientStartup): Promise<ActivityClientSessionState> {
    this.startTask ??= this.startOnce(startup);
    return this.startTask;
  }

  private async startOnce(startup: ActivityClientStartup): Promise<ActivityClientSessionState> {
    if (startup.kind === "invalid_handoff") {
      return this.setSetupRequired("This Activity Client handoff is invalid or incomplete.");
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

  async reconnect(): Promise<ActivityClientSessionState> {
    this.current = { state: "connecting", action: "wait", message: "Reconnecting Activity Client…" };
    try {
      const status = await this.client.resume();
      return await this.applyStatus(status);
    } catch (error) {
      return this.applyError(error);
    }
  }

  async act(action: ActivityClientAction): Promise<ActivityClientActionReceipt> {
    if (this.current.state !== "live") {
      throw new ActivityClientHandoffError(
        "participant_session_not_live",
        "Activity Client must reconnect before submitting an Action.",
        "reconnect",
        true,
      );
    }
    const request: OfferedActionSubmission = {
      action_id: action.actionId,
      based_on_room_seq: action.basedOnRoomSeq,
      offer_id: action.offerId,
      schema_digest: action.schemaDigest,
      action_type: action.actionType,
      payload: action.payload,
    };
    return this.client.act(request);
  }

  /** Refreshes from the last received frame without advancing the Membership Cursor. */
  async refresh(): Promise<ActivityClientSessionState> {
    if (this.current.state !== "live") return this.current;
    try {
      const previous = this.current.deliveryBatch;
      const next = await this.client.observe(previous.frame_head);
      this.installedActionOffers = reduceActionOffers(this.installedActionOffers, next.delivery);
      this.current = {
        state: "live",
        action: "continue",
        message: null,
        deliveryBatch: {
          ...next,
          // An empty retained range does not discard the currently installed
          // authorized Projection or its Action Offers.
          delivery: next.delivery.length === 0 ? previous.delivery : next.delivery,
        },
      };
      return this.current;
    } catch (error) {
      return this.applyError(error);
    }
  }

  /** Reads verified Canonical History only; it never invokes the Runner path. */
  async replay(): Promise<VerifiedRoomReplay> {
    if (this.current.state !== "live") {
      throw new ActivityClientHandoffError(
        "participant_session_not_live",
        "Activity Client must reconnect before reading historical Replay.",
        "reconnect",
        true,
      );
    }
    return this.client.replay(this.current.deliveryBatch.room_head.room_seq);
  }

  /** Converts one bounded client failure into the same actionable session state. */
  fail(error: unknown): ActivityClientSessionState {
    return this.applyError(error);
  }

  private async applyStatus(status: ActivityClientSessionStatus): Promise<ActivityClientSessionState> {
    if (status.state === "disconnected") {
      this.installedActionOffers = EMPTY_ACTION_OFFERS;
      this.current = { state: "disconnected", action: "reconnect", message: "Activity Client is disconnected." };
      return this.current;
    }
    try {
      const deliveryBatch = await this.client.observe(null);
      this.installedActionOffers = reduceActionOffers(EMPTY_ACTION_OFFERS, deliveryBatch.delivery);
      this.current = { state: "live", action: "continue", message: null, deliveryBatch };
      return this.current;
    } catch (error) {
      return this.applyError(error);
    }
  }

  private applyError(error: unknown): ActivityClientSessionState {
    this.installedActionOffers = EMPTY_ACTION_OFFERS;
    if (error instanceof ActivityClientHandoffError && error.nextAction === "return_to_task_setup") {
      return this.setSetupRequired(error.message);
    }
    const message = error instanceof ActivityClientHandoffError
      ? error.message
      : "Activity Client disconnected unexpectedly.";
    this.current = { state: "disconnected", action: "reconnect", message };
    return this.current;
  }

  private setSetupRequired(message: string): ActivityClientSessionState {
    this.installedActionOffers = EMPTY_ACTION_OFFERS;
    this.current = { state: "setup_required", action: "return_to_task_setup", message };
    return this.current;
  }
}

const EMPTY_ACTION_OFFERS: readonly unknown[] = Object.freeze([]);

function reduceActionOffers(
  current: readonly unknown[],
  delivery: AuthorizedRoomDeliveryBatch["delivery"],
): readonly unknown[] {
  let installed = current;
  for (const item of delivery) {
    const envelope = item.kind === "projection_reset"
      ? record(item.body.projection)
      : record(item.body.observation);
    const projection = Object.hasOwn(envelope, "projection")
      ? record(envelope.projection)
      : envelope;
    if (Object.hasOwn(projection, "action_offers")) {
      installed = Array.isArray(projection.action_offers)
        ? Object.freeze([...projection.action_offers])
        : EMPTY_ACTION_OFFERS;
    }
  }
  return installed;
}

function record(value: unknown): Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? value as Record<string, unknown>
    : {};
}
