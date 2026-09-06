/**
 * Web shim for `@tauri-apps/plugin-opener`.
 */

import { apiUrl, getStoredToken } from "../rpcClient";

/** Opens a URL in a new browser tab. */
export async function openUrl(url: string): Promise<void> {
  window.open(url, "_blank", "noopener,noreferrer");
}

/**
 * "Opens" a server-side file. Web mode cannot launch external applications,
 * so files are served as downloads:
 * - `__web_upload__/<staged>` — a file previously staged by the browser
 * - `__web_downloads__/<name>` — a completed server-side download
 * - anything else — ignored (there is no server-local path to open)
 */
export async function openPath(path: string): Promise<void> {
  let name: string | null = null;
  let endpoint: string | null = null;
  if (path.startsWith("__web_upload__/")) {
    name = path.slice("__web_upload__/".length);
    endpoint = "/api/sftp/staging";
  } else if (path.startsWith("__web_downloads__/")) {
    name = path.slice("__web_downloads__/".length);
    endpoint = "/api/sftp/temp";
  }
  if (!name || !endpoint) return;

  const url = apiUrl(`${endpoint}/${encodeURIComponent(name)}`);
  try {
    const response = await fetch(url, {
      headers: { Authorization: `Bearer ${getStoredToken() ?? ""}` },
    });
    if (!response.ok) return;
    const blob = await response.blob();
    const objectUrl = URL.createObjectURL(blob);
    const anchor = document.createElement("a");
    anchor.href = objectUrl;
    anchor.download = name;
    document.body.append(anchor);
    anchor.click();
    anchor.remove();
    setTimeout(() => URL.revokeObjectURL(objectUrl), 30_000);
  } catch {
    // Opening is best-effort in web mode.
  }
}

export async function revealItemInDir(_path: string): Promise<void> {
  // No OS file manager in the browser.
}
