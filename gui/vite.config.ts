import { defineConfig } from "vite";

// Dev: `npm run dev` proxies the API to a running `moodcut serve` on :7878.
export default defineConfig({
  esbuild: { jsx: "automatic", jsxImportSource: "preact" },
  build: { outDir: "dist", emptyOutDir: true, target: "es2022", chunkSizeWarningLimit: 800 },
  server: {
    proxy: {
      "/api": { target: "http://127.0.0.1:7878", ws: true },
      "/mcp": "http://127.0.0.1:7878",
      "/fonts": "http://127.0.0.1:7878",
    },
  },
});
