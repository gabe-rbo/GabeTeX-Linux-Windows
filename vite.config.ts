import { defineConfig } from "vite";

// Tauri expects a fixed dev port and does not want the screen cleared.
export default defineConfig({
  clearScreen: false,
  server: { port: 1420, strictPort: true, watch: { ignored: ["**/src-tauri/**"] } },
  envPrefix: ["VITE_", "TAURI_ENV_*"],
  // pdf.js (~520 kB) is a lazily loaded chunk, so the default 500 kB warning does not apply to it.
  build: { target: "safari15", outDir: "dist", emptyOutDir: true, sourcemap: false, chunkSizeWarningLimit: 600 },
});
