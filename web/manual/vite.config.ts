import { defineConfig, loadEnv } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig(({ mode }) => {
  const environment = loadEnv(mode, ".", "");
  const siteUrl = environment.DOCS_SITE_URL?.replace(/\/$/u, "");
  return {
    base: environment.DOCS_BASE || "/",
    plugins: [
      react(),
      {
        name: "worldstream-manual-metadata",
        transformIndexHtml(html) {
          const socialImage = siteUrl ? `${siteUrl}/og.png` : "/og.png";
          return html.replace("%DOCS_SOCIAL_IMAGE%", socialImage);
        },
      },
    ],
    build: {
      outDir: "dist",
      emptyOutDir: true,
    },
  };
});
