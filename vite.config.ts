import { readFileSync } from "node:fs";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// Tauri expects a fixed port and must not have its output cleared.
export default defineConfig({
  plugins: [react(), tailwindcss()],
  clearScreen: false,
  // host/allowedHosts: the sandbox preview proxy serves this dev server from its own domain.
  server: { port: 1420, strictPort: true, host: true, allowedHosts: true, watch: { ignored: ["**/src-tauri/**"] } },
  build: { target: "es2022" },
  // One version, from package.json, for the About row and the footer.
  define: { "import.meta.env.APP_VERSION": JSON.stringify(JSON.parse(readFileSync("package.json", "utf8")).version) },
});
