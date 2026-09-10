import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";

export default defineConfig({
  base: "/midnight-archive-v6/",
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
    port: 5177,
  },
});
