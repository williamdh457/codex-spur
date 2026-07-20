import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

const siteRoot = fileURLToPath(new URL(".", import.meta.url));

export default defineConfig({
  root: siteRoot,
  plugins: [react()],
  build: {
    outDir: "dist",
    emptyOutDir: true,
    rollupOptions: {
      input: {
        zh: fileURLToPath(new URL("./index.html", import.meta.url)),
        en: fileURLToPath(new URL("./en/index.html", import.meta.url)),
      },
    },
  },
  server: { port: 4174, strictPort: true },
});
