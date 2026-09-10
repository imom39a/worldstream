import { defineConfig } from "vite";
import { fileURLToPath } from "node:url";

export default defineConfig(({ command }) => ({
  // Preserve the design-review URL while publishing new immutable release paths.
  base: command === "serve" ? "/agent-heist-v8/" : "/agent-heist-v9/",
  build: {
    rollupOptions: {
      input: {
        standalone: fileURLToPath(new URL("./index.html", import.meta.url)),
        hosted: fileURLToPath(new URL("./hosted/index.html", import.meta.url)),
        practice: fileURLToPath(new URL("./practice/index.html", import.meta.url)),
      },
    },
  },
  server: {
    host: "127.0.0.1",
    port: 5175,
  },
}));
