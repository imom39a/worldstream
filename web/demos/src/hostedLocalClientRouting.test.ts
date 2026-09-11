import { describe, expect, it } from "vitest";

import bindings from "../../../config/activity-clients/hosted-local-bindings.json";
import {
  hostedLocalClientPaths,
  hostedLocalClientProxy,
} from "./hostedLocalClientRouting";

describe("hosted local Activity Client routing", () => {
  it("routes the current reviewed hosted client and retained local clients through the exact client host", () => {
    const paths = hostedLocalClientPaths(bindings);
    expect(paths).toEqual(expect.arrayContaining([
      "/agent-heist-v2",
      "/agent-heist-v3",
      "/agent-heist-v4",
      "/agent-heist-v5",
      "/agent-heist-v6",
      "/agent-heist-v11",
      "/agent-heist-v12",
    ]));
    expect(hostedLocalClientProxy("http://127.0.0.1:5173", bindings)).toMatchObject({
      "/agent-heist-v6": { target: "http://127.0.0.1:5173", changeOrigin: true },
      "/agent-heist-v11": { target: "http://127.0.0.1:5173", changeOrigin: true },
      "/agent-heist-v12": { target: "http://127.0.0.1:5173", changeOrigin: true },
    });
  });

  it("does not create a proxy route from a malformed or non-local reviewed declaration", () => {
    const paths = hostedLocalClientPaths({
      deployments: [{
        surfaces: [
          { launch_url: "https://client.example/agent-heist-v9/hosted/" },
          { launch_url: "http://127.0.0.1:5180/agent-heist-v7/hosted/?not=allowed" },
          { launch_url: "not a URL" },
        ],
      }],
    });
    expect(paths).not.toContain("/agent-heist-v7");
    expect(paths).not.toContain("/agent-heist-v9");
    expect(hostedLocalClientProxy(undefined, bindings)).toEqual({});
  });
});
