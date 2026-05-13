import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

const host = process.env.TAURI_DEV_HOST;

function manualChunks(id: string) {
  if (!id.includes("node_modules")) return undefined;
  if (/[\\/]node_modules[\\/](react|react-dom|scheduler)[\\/]/.test(id)) {
    return "vendor-react";
  }
  if (id.includes("framer-motion")) {
    return "vendor-motion";
  }
  if (id.includes("lucide-react")) {
    return "vendor-icons";
  }
  if (id.includes("@tauri-apps")) {
    return "vendor-tauri";
  }
  if (
    id.includes("react-markdown") ||
    id.includes("remark-") ||
    id.includes("rehype-") ||
    id.includes("micromark") ||
    id.includes("unified") ||
    id.includes("mdast") ||
    id.includes("hast") ||
    id.includes("unist") ||
    id.includes("vfile")
  ) {
    return "vendor-markdown";
  }
  if (id.includes("@dnd-kit") || id.includes("cmdk")) {
    return "vendor-interactions";
  }
  return undefined;
}

export default defineConfig(async () => ({
  plugins: [react(), tailwindcss()],

  clearScreen: false,

  build: {
    chunkSizeWarningLimit: 650,
    rollupOptions: {
      output: {
        manualChunks,
      },
    },
  },

  server: {
    port: 5173,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
}));
