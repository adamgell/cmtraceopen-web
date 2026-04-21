import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  server: {
    port: 5173,
    strictPort: false,
    // Dev-only reverse proxy so the viewer can call the api-server without
    // CORS headers on the server side. Once the api-server grows a proper
    // CORS layer (follow-up) this can drop — prod deployments are expected
    // to be co-located anyway, so same-origin remains the default.
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
