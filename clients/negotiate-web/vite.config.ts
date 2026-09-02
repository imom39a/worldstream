import { defineConfig } from "vite";

export default defineConfig({
  base: "/negotiate/",
  server: {
    host: "127.0.0.1",
    port: 5176,
  },
});
