import { defineConfig } from "vite";

// Tauri expects a fixed dev port and a WebView2-compatible build target.
export default defineConfig({
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**", "**/crates/**", "**/target/**"] },
  },
  build: { target: "chrome105", minify: "esbuild", sourcemap: false },
});
