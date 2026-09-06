import path from "path";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import browserslist from "browserslist";
import { browserslistToTargets } from "lightningcss";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;
// @ts-expect-error process is a nodejs global
const WEB_BUILD = process.env.NYATERM_WEB_BUILD === "1";

// Web builds swap the Tauri IPC/event stack for the HTTP/WebSocket transport
// in src/lib/web/shims. Desktop builds resolve the real packages and are
// completely unaffected.
const webShimAliases = WEB_BUILD
  ? {
      "@tauri-apps/api/core": "src/lib/web/shims/core.ts",
      "@tauri-apps/api/event": "src/lib/web/shims/event.ts",
      "@tauri-apps/api/window": "src/lib/web/shims/window.ts",
      "@tauri-apps/api/webview": "src/lib/web/shims/webview.ts",
      "@tauri-apps/api/webviewWindow": "src/lib/web/shims/webviewWindow.ts",
      "@tauri-apps/api/path": "src/lib/web/shims/path.ts",
      "@tauri-apps/api/app": "src/lib/web/shims/app.ts",
      "@tauri-apps/plugin-dialog": "src/lib/web/shims/dialog.ts",
      "@tauri-apps/plugin-opener": "src/lib/web/shims/opener.ts",
      "@tauri-apps/plugin-updater": "src/lib/web/shims/updater.ts",
      "@tauri-apps/plugin-process": "src/lib/web/shims/process.ts",
    }
  : {};

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [react(), tailwindcss()],
  css: {
    transformer: "lightningcss",
    lightningcss: {
      targets: browserslistToTargets(browserslist("safari >= 14, chrome >= 105")),
    },
  },
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
      ...webShimAliases,
    },
  },
  test: {
    environment: "jsdom",
    setupFiles: "./src/test/setup.ts",
  },

  clearScreen: false,
  server: {
    port: 1420,
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

  build: {
    cssMinify: "lightningcss",
    rollupOptions: {
      output: {
        manualChunks: {
          react: ["react", "react-dom"],
          xterm: [
            "@xterm/xterm",
            "@xterm/addon-fit",
            "@xterm/addon-image",
            "@xterm/addon-web-links",
            "@xterm/addon-webgl",
            "@xterm/addon-search",
          ],
          // Web builds alias @tauri-apps/api to local shims, so the package
          // chunk would be empty there.
          ...(!WEB_BUILD ? { tauri: ["@tauri-apps/api"] } : {}),
        },
      },
    },
  },
}));
