// The page `keepane web` serves, built into one file (scripts and styles
// inline) that the binary carries: `npm run build` writes dist/index.html,
// which src/web.rs includes. `npm run dev` serves it here with /api going to
// a running `keepane web` (KEEPANE_WEB, default http://127.0.0.1:7681).
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { viteSingleFile } from "vite-plugin-singlefile";

export default defineConfig({
  // No lightningcss on the CSS (Tailwind's optimize, Vite's CSS minify): it
  // turns oklch colours into lab ones and rounds them differently on Windows
  // and Linux, and the same source must build the same bytes everywhere (CI
  // checks it). Every browser the page is for takes oklch; gzip makes up for
  // the unminified CSS (the page is smaller this way, without the fallbacks).
  plugins: [react(), tailwindcss({ optimize: false }), viteSingleFile({ removeViteModuleLoader: true })],
  build: { outDir: "dist", emptyOutDir: true, target: "es2020", cssMinify: false, reportCompressedSize: true },
  server: {
    proxy: { "/api": { target: process.env.KEEPANE_WEB || "http://127.0.0.1:7681", changeOrigin: true } },
  },
});
