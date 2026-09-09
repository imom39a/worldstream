import { defineConfig } from "vite";
import { fileURLToPath } from "node:url";

export default defineConfig({
  base: "/agent-heist-v6/",
  build: {
    rollupOptions: {
      input: {
        standalone: fileURLToPath(new URL("./index.html", import.meta.url)),
        hosted: fileURLToPath(new URL("./hosted/index.html", import.meta.url)),
      },
    },
  },
  server: {
    host: "127.0.0.1",
    port: 5175,
  },
});
