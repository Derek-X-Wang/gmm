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
    // The Tauri IPC layer is mocked per-test; nothing here should ever
    // touch a real backend, so a short timeout catches accidental
    // real-invoke calls instead of hanging CI.
    testTimeout: 5000,
    coverage: {
      provider: "v8",
      include: ["src/**/*.{ts,tsx}"],
      exclude: ["src/**/*.test.{ts,tsx}", "src/test/**", "src/main.tsx"],
    },
  },
});
