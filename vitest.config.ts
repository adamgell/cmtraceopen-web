// Vitest config for the viewer.
//
// Kept separate from `vite.config.ts` so the dev-server proxy/build settings
// don't leak into the test runner. jsdom is used because the components
// under test (DevicesPanel, RoleGate) need DOM APIs (`document`, ARIA roles,
// click events).

import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: ["./src/test-setup.ts"],
    include: ["src/**/*.{test,spec}.{ts,tsx}"],
    // The wasm bridge imports from `../../pkg/cmtrace_wasm` which only exists
    // after `pnpm wasm:build`. Tests don't touch that bridge — exclude any
    // module path that would pull it in if a future test accidentally does.
    exclude: ["node_modules", "dist", ".idea", ".git", ".cache"],
  },
});
