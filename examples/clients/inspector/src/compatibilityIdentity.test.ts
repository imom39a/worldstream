import { describe, expect, it } from "vitest";

import {
  CLIENT_CONTRACT_IDENTITY,
  CLIENT_CONTRACT_IDENTITY_JSON,
  CLIENT_CONTRACT_IDENTITY_PATH,
} from "./compatibilityIdentity";

describe("client contract identity", () => {
  it("exposes the manifest-derived wire/config/core and exact pack contract", () => {
    expect(CLIENT_CONTRACT_IDENTITY.schema).toBe(
      "worldstream/client-contract-identity/v1",
    );
    expect(CLIENT_CONTRACT_IDENTITY.wire).toBe("0.1");
    expect(CLIENT_CONTRACT_IDENTITY.config).toBe(1);
    expect(CLIENT_CONTRACT_IDENTITY.core_schema_version).toBe(
      "worldstream.core-room-state.v1",
    );
    expect(CLIENT_CONTRACT_IDENTITY.hash_suite).toBe(
      "blake3-canonical-json-v1",
    );
    expect(CLIENT_CONTRACT_IDENTITY.pack_executors).toHaveLength(4);
    expect(CLIENT_CONTRACT_IDENTITY.pack_executors).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          pack_id: "worldstream.agent-heist",
          explanatory_version: "0.1.0",
          revision_digest:
            "blake3:b05a682f0923001914a800072ee68348e68b979453c93e033cba7916b99a4407",
          selectable_for_new_rooms: true,
          runnable_for_retained_rooms: true,
        }),
      ]),
    );
  });

  it("identifies the packaged runtime artifact path", () => {
    expect(CLIENT_CONTRACT_IDENTITY_PATH).toBe("/compatibility-identity.json");
    expect(JSON.parse(CLIENT_CONTRACT_IDENTITY_JSON)).toEqual(
      CLIENT_CONTRACT_IDENTITY,
    );
  });
});
