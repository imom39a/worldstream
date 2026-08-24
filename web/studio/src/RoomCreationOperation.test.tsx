import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { RoomCreationOperation } from "./RoomCreationOperation";
import type { RoomCreationStatus } from "./roomCreation";

const retrying: RoomCreationStatus = {
  version: "studio_room_creation.v1",
  draft_id: "new-room",
  operation_id: "room-create-operation-0123456789abcdef",
  idempotency_key: "studio-room-create-0123456789abcdef",
  review_hash: `blake3:${"a".repeat(64)}`,
  intent_hash: `blake3:${"b".repeat(64)}`,
  state: "retrying",
  attempts: 2,
  room_id: null,
  attention: {
    code: "daemon_result_ambiguous",
    message: "The original Room creation result is not resolved yet; retrying will reuse the exact persisted intent.",
    retryable: true,
  },
};

describe("durable Room creation operation", () => {
  it("starts only from a reviewed persisted draft", () => {
    const ready = renderToStaticMarkup(
      <RoomCreationOperation status={null} reviewed loading={false} />,
    );
    const notReady = renderToStaticMarkup(
      <RoomCreationOperation status={null} reviewed={false} loading={false} />,
    );

    expect(ready).toContain("Create from reviewed draft");
    expect(ready).toContain("persisted before the daemon call");
    expect(notReady).toContain("Complete and save Review");
    expect(notReady).not.toContain("Create from reviewed draft");
  });

  it("keeps ambiguous state visible and retries only the original intent", () => {
    const dom = renderToStaticMarkup(
      <RoomCreationOperation status={retrying} reviewed loading={false} />,
    );

    expect(dom).toContain("Original Room result unresolved");
    expect(dom).toContain("Retry original operation");
    expect(dom).toContain(retrying.operation_id);
    expect(dom).toContain(retrying.idempotency_key);
    expect(dom).toContain("No replacement Room");
    expect(dom).not.toContain("Create from reviewed draft");
  });

  it("renders a permanent draft-to-Room binding without another action", () => {
    const dom = renderToStaticMarkup(
      <RoomCreationOperation
        status={{ ...retrying, state: "succeeded", room_id: "01ARZ3NDEKTSV4RRFFQ69G5FAW", attention: null }}
        reviewed
        loading={false}
      />,
    );

    expect(dom).toContain("Room creation succeeded");
    expect(dom).toContain("01ARZ3NDEKTSV4RRFFQ69G5FAW");
    expect(dom).toContain("permanently bound");
    expect(dom).not.toContain("Retry original operation");
    expect(dom).not.toContain("Create from reviewed draft");
  });

  it("shows permanent rejection as needs-attention without substitution", () => {
    const dom = renderToStaticMarkup(
      <RoomCreationOperation
        status={{
          ...retrying,
          state: "needs_attention",
          attention: { code: "reviewed_intent_rejected", message: "The reviewed Room creation intent needs operator attention.", retryable: false },
        }}
        reviewed
        loading={false}
      />,
    );

    expect(dom).toContain("Needs operator attention");
    expect(dom).toContain("reviewed intent rejected");
    expect(dom).toContain("No replacement operation");
    expect(dom).not.toContain("Retry original operation");
  });

  it("clears a prior success claim when current operation status is unavailable", () => {
    const dom = renderToStaticMarkup(
      <RoomCreationOperation
        status={null}
        statusAvailable={false}
        reviewed
        loading={false}
      />,
    );

    expect(dom).toContain("Room creation status is unavailable");
    expect(dom).not.toContain("Room creation succeeded");
    expect(dom).not.toContain("Create from reviewed draft");
  });

  it("offers safe retry after an operator fixes authorization", () => {
    const dom = renderToStaticMarkup(
      <RoomCreationOperation
        status={{
          ...retrying,
          state: "needs_attention",
          attention: { code: "daemon_authorization_unavailable", message: "Repair authorization.", retryable: true },
        }}
        reviewed
        loading={false}
      />,
    );

    expect(dom).toContain("Retry original operation");
    expect(dom).not.toContain("Create from reviewed draft");
  });
});
