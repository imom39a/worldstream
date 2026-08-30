import { fileURLToPath } from "node:url";

import { defineConfig } from "vite";

export default defineConfig({
  build: {
    rollupOptions: {
      input: {
        catalog: fileURLToPath(new URL("./index.html", import.meta.url)),
        agentHeist: fileURLToPath(new URL("./demos/agent-heist/index.html", import.meta.url)),
      },
    },
  },
});
