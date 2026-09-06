/**
 * Web shim for `@tauri-apps/api/path`.
 *
 * The browser has no filesystem paths; the shims return the same marker
 * prefixes the backend web-file endpoints understand:
 * - `__web_downloads__` — download targets (files materialized server-side,
 *   then streamed to the browser)
 * - `__web_temp__` — scratch space (staged uploads live server-side)
 */

export const WEB_DOWNLOADS_MARKER = "__web_downloads__";
export const WEB_TEMP_MARKER = "__web_temp__";

export async function downloadDir(): Promise<string> {
  return WEB_DOWNLOADS_MARKER;
}

export async function tempDir(): Promise<string> {
  return WEB_TEMP_MARKER;
}

export function join(...parts: string[]): string {
  const cleaned = parts
    .filter((part) => part !== undefined && part !== null && part !== "")
    .map((part, index) =>
      index === 0 ? part.replace(/[\\/]+$/, "") : part.replace(/^[\\/]+|[\\/]+$/g, ""),
    );
  return cleaned.join("/");
}

export async function appConfigDir(): Promise<string> {
  return WEB_TEMP_MARKER;
}

export async function appDataDir(): Promise<string> {
  return WEB_TEMP_MARKER;
}

export async function appLogDir(): Promise<string> {
  return WEB_TEMP_MARKER;
}

export async function audioDir(): Promise<string> {
  return WEB_DOWNLOADS_MARKER;
}

export async function documentDir(): Promise<string> {
  return WEB_DOWNLOADS_MARKER;
}

export { join as resolve };
