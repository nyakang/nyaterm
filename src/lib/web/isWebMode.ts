/**
 * True when the frontend runs in a plain browser rather than the Tauri
 * webview. Drives small UI conditionals (e.g. hiding desktop window
 * controls); the transport-level swap lives in src/lib/web/shims and is
 * applied only to web builds via the vite alias.
 */
export const isWebMode =
  typeof window !== "undefined" && !("__TAURI_INTERNALS__" in window);
