import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// Standalone vitest config so unit tests do not load the tauri-specific
// vite.config.ts dev-server section (which assumes the Tauri host env).
export default defineConfig({
  plugins: [react()],
  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: ["./src/test/setup.ts"],
    include: ["src/**/*.test.{ts,tsx}"],
    css: false,
  },
});
