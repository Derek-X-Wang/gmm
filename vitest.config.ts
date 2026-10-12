import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// Frontend unit/component tests. Kept in a separate config from
// vite.config.ts so the Tauri dev-server settings (fixed port 1420,
// src-tauri watch ignores) don't leak into the test runner.
export default defineConfig({
  plugins: [react()],
  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: ["./src/test/setup.ts"],
    include: ["src/**/*.test.{ts,tsx}"],
    // App interaction tests can exceed 5s under Windows runner contention.
    // Allow bounded headroom while still failing hung tests and accidental
    // real-invoke calls; the Tauri IPC layer is mocked per-test.
    testTimeout: 30000,
    coverage: {
      provider: "v8",
      include: ["src/**/*.{ts,tsx}"],
      exclude: ["src/**/*.test.{ts,tsx}", "src/test/**", "src/main.tsx"],
    },
  },
});
