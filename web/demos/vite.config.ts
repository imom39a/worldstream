import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";

import { defineConfig } from "vite";

const repositoryRoot = fileURLToPath(new URL("../..", import.meta.url));
const sourceRevision = execFileSync("git", ["rev-parse", "--short=12", "HEAD"], {
  cwd: repositoryRoot,
  encoding: "utf8",
}).trim();

export default defineConfig({
  define: {
    __BUILD_REVISION__: JSON.stringify(sourceRevision),
  },
  build: {
    rollupOptions: {
      input: {
        catalog: fileURLToPath(new URL("./index.html", import.meta.url)),
        agentHeist: fileURLToPath(new URL("./demos/agent-heist/index.html", import.meta.url)),
      },
    },
  },
});
