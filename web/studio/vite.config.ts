import { defineConfig, loadEnv } from "vite";

export default defineConfig(({ mode }) => {
  const supervisor = loadEnv(mode, process.cwd(), "VITE_").VITE_WORLDSTREAM_SUPERVISOR_URL
    ?? "http://127.0.0.1:9420";
  return {
    server: {
      proxy: {
        "/api": supervisor,
      },
    },
  };
});
