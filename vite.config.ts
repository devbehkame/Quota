import { defineConfig } from "vite";

export default defineConfig({
  clearScreen: false,
  server: { port: 1420, strictPort: true, watch: { ignored: ["**/src-tauri/**", "**/crates/**"] } },
  build: {
    target: "chrome110",
    minify: "esbuild",
    sourcemap: false,
  },
});
