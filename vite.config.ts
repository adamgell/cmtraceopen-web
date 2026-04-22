import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  server: {
    port: 5173,
    strictPort: false,
    // Dev-only reverse proxy. The api-server now has a proper CORS layer
    // (env: CMTRACE_CORS_ORIGINS), so this proxy is no longer *required* —
    // but it keeps the dev loop on a single origin and skips the preflight
    // round-trip, which is still convenient. Prod deployments should either
    // serve the viewer same-origin OR set CMTRACE_CORS_ORIGINS to the
    // viewer's public origin.
    proxy: {
      "/v1": { target: "http://localhost:8080", changeOrigin: true },
      "/healthz": { target: "http://localhost:8080", changeOrigin: true },
    },
  },
  build: {
    target: "es2022",
    sourcemap: true,
  },
});
