/// <reference types="vitest" />
import { resolve, dirname } from "path";
import { fileURLToPath } from "url";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

const __dirname = dirname(fileURLToPath(import.meta.url));
const backendPort = process.env.RALPH_BACKEND_PORT || "3000";
const backendTarget = `http://localhost:${backendPort}`;

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      "@": resolve(__dirname, "./src"),
    },
  },
  server: {
    port: 5173,
    host: true, // Listen on all interfaces (0.0.0.0)
    allowedHosts: ["studio", "localhost"],
    proxy: {
      "/rpc": {
        target: backendTarget,
        ws: true,
        changeOrigin: true,
      },
      "/health": {
        target: backendTarget,
        changeOrigin: true,
      },
      // Monitor service (backend/ralph-monitor, default :3200).
      // Same-origin proxy keeps the session cookie + CSRF dance inside
      // the dev origin — no CORS, no cross-site writes.
      "/api/monitor": {
        target: process.env.RALPH_MONITOR_TARGET || "http://127.0.0.1:3200",
        changeOrigin: true,
      },
    },
  },
  test: {
    globals: true,
    environment: "jsdom",
    setupFiles: ["./src/test/setup.ts"],
    include: ["src/**/*.{test,spec}.{ts,tsx}"],
  },
});
