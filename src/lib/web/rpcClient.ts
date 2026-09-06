/**
 * Web-mode transport: token-authenticated HTTP RPC + WebSocket event stream.
 *
 * This module is only bundled in web builds (see `vite.config.ts`, which
 * aliases `@tauri-apps/api/*` to `src/lib/web/shims/*` when
 * `NYATERM_WEB_BUILD=1`). Desktop builds keep the real Tauri APIs.
 */

const TOKEN_STORAGE_KEY = "nyaterm_web_token";
const API_BASE_STORAGE_KEY = "nyaterm_api_base";

/** Optional cross-origin API base (defaults to same-origin `/api`). */
export function apiBase(): string {
  if (typeof window === "undefined") return "";
  const injected = (window as unknown as Record<string, unknown>).NYATERM_API_BASE;
  if (typeof injected === "string" && injected.trim()) {
    return injected.trim().replace(/\/$/, "");
  }
  const stored = window.localStorage.getItem(API_BASE_STORAGE_KEY);
  if (stored?.trim()) {
    return stored.trim().replace(/\/$/, "");
  }
  return "";
}

export function apiUrl(path: string): string {
  return `${apiBase()}${path}`;
}

export function getStoredToken(): string | null {
  if (typeof window === "undefined") return null;
  const injected = (window as unknown as Record<string, unknown>).NYATERM_WEB_TOKEN;
  if (typeof injected === "string" && injected.trim()) {
    return injected.trim();
  }
  return window.localStorage.getItem(TOKEN_STORAGE_KEY);
}

export function setStoredToken(token: string): void {
  window.localStorage.setItem(TOKEN_STORAGE_KEY, token.trim());
}

export function clearStoredToken(): void {
  window.localStorage.removeItem(TOKEN_STORAGE_KEY);
}

let pendingAuthPrompt: Promise<string> | null = null;

/**
 * Resolves the access token, showing a minimal login overlay when missing.
 * `?token=...` in the URL is consumed automatically on first load.
 */
export function ensureWebToken(): Promise<string> {
  bootstrapUrlToken();
  const existing = getStoredToken();
  if (existing) return Promise.resolve(existing);
  pendingAuthPrompt ??= showTokenPrompt().finally(() => {
    pendingAuthPrompt = null;
  });
  return pendingAuthPrompt;
}

function bootstrapUrlToken(): void {
  if (typeof window === "undefined") return;
  const params = new URLSearchParams(window.location.search);
  const urlToken = params.get("token");
  if (urlToken) {
    setStoredToken(urlToken);
    params.delete("token");
    const rest = params.toString();
    window.history.replaceState(
      null,
      "",
      `${window.location.pathname}${rest ? `?${rest}` : ""}${window.location.hash}`,
    );
  }
}

function showTokenPrompt(): Promise<string> {
  return new Promise((resolve, reject) => {
    const overlay = document.createElement("div");
    overlay.setAttribute("data-nyaterm-web-login", "");
    Object.assign(overlay.style, {
      position: "fixed",
      inset: "0",
      zIndex: "2147483647",
      display: "flex",
      alignItems: "center",
      justifyContent: "center",
      background: "rgba(8, 10, 14, 0.86)",
      fontFamily: "system-ui, sans-serif",
      color: "#e6edf3",
    });

    const card = document.createElement("div");
    Object.assign(card.style, {
      background: "#14181f",
      border: "1px solid #2b3240",
      borderRadius: "12px",
      padding: "28px",
      width: "340px",
      boxShadow: "0 18px 60px rgba(0,0,0,0.5)",
    });
    card.innerHTML = `
      <div style="font-size: 15px; font-weight: 600; margin-bottom: 6px;">NyaTerm Web</div>
      <div style="font-size: 12.5px; opacity: 0.72; line-height: 1.5; margin-bottom: 14px;">
        Paste the access token printed by the NyaTerm web server
        (<code style="opacity:.85">web-token</code> file or
        <code style="opacity:.85">NYATERM_WEB_TOKEN</code>).
      </div>
    `;
    const input = document.createElement("input");
    input.type = "password";
    input.autofocus = true;
    input.placeholder = "Access token";
    Object.assign(input.style, {
      width: "100%",
      boxSizing: "border-box",
      padding: "9px 11px",
      borderRadius: "8px",
      border: "1px solid #2b3240",
      background: "#0d1117",
      color: "#e6edf3",
      fontSize: "13px",
      outline: "none",
      marginBottom: "12px",
    });

    const errorLine = document.createElement("div");
    errorLine.style.cssText = "color:#ff7b72; font-size:12px; min-height:16px; margin-bottom:8px;";

    const button = document.createElement("button");
    button.textContent = "Connect";
    Object.assign(button.style, {
      width: "100%",
      padding: "9px 0",
      borderRadius: "8px",
      border: "none",
      background: "#2f81f7",
      color: "#fff",
      fontSize: "13px",
      fontWeight: "600",
      cursor: "pointer",
    });

    const submit = () => {
      const value = input.value.trim();
      if (!value) {
        errorLine.textContent = "Token is required.";
        return;
      }
      setStoredToken(value);
      overlay.remove();
      resolve(value);
    };
    button.addEventListener("click", submit);
    input.addEventListener("keydown", (event) => {
      if (event.key === "Enter") submit();
      if (event.key === "Escape") {
        overlay.remove();
        reject(new Error("cancelled"));
      }
    });

    card.append(input, errorLine, button);
    overlay.append(card);
    document.body.append(overlay);
    input.focus();
  });
}

export interface RpcEnvelope<T> {
  ok: boolean;
  data?: T;
  error?: string;
}

/**
 * Invokes a backend command over HTTP. Rejects with the same string the
 * desktop `invoke()` rejects with (Tauri serializes `AppError` to its
 * display string), so error handling code paths behave identically.
 */
export async function webInvoke<T>(
  cmd: string,
  args?: Record<string, unknown>,
): Promise<T> {
  const token = await ensureWebToken();
  let response: Response;
  try {
    response = await fetch(apiUrl("/api/rpc"), {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Authorization: `Bearer ${token}`,
      },
      body: JSON.stringify({ cmd, args: args ?? {} }),
    });
  } catch (error) {
    throw new Error(`NyaTerm web server unreachable: ${String(error)}`);
  }

  if (response.status === 401) {
    clearStoredToken();
    const retry = await ensureWebToken().catch(() => null);
    if (retry) return webInvoke<T>(cmd, args);
    throw new Error("Unauthorized");
  }
  if (!response.ok) {
    throw new Error(`NyaTerm web server error (HTTP ${response.status})`);
  }

  const envelope = (await response.json()) as RpcEnvelope<T>;
  if (!envelope.ok) {
    throw (envelope.error ?? "Unknown command failure");
  }
  const data = envelope.data as T;
  return maybeTriggerWebDownload(data);
}

/**
 * The server answers `download_remote_file` with `{ __webDownload: name }`
 * when the file has been materialized on the server side. Intercept it here
 * and hand the bytes to the browser as a download, so frontend flows that
 * expect a void result keep working unchanged.
 */
async function maybeTriggerWebDownload<T>(data: T): Promise<T> {
  if (
    data
    && typeof data === "object"
    && "__webDownload" in (data as Record<string, unknown>)
  ) {
    const name = (data as Record<string, unknown>).__webDownload;
    if (typeof name === "string" && name) {
      await triggerBrowserDownload(apiUrl(`/api/sftp/temp/${encodeURIComponent(name)}`), name);
      return undefined as T;
    }
  }
  return data;
}

export async function triggerBrowserDownload(url: string, filename: string): Promise<void> {
  const response = await fetch(url, {
    headers: { Authorization: `Bearer ${getStoredToken() ?? ""}` },
  });
  if (!response.ok) {
    throw new Error(`Download failed (HTTP ${response.status})`);
  }
  const blob = await response.blob();
  const objectUrl = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = objectUrl;
  anchor.download = filename || "download";
  document.body.append(anchor);
  anchor.click();
  anchor.remove();
  setTimeout(() => URL.revokeObjectURL(objectUrl), 30_000);
}

/** Uploads a browser File to the server staging area; returns the marker path. */
export async function stageFileForUpload(file: File): Promise<string> {
  const token = await ensureWebToken();
  const response = await fetch(
    apiUrl(`/api/sftp/staging?name=${encodeURIComponent(file.name)}`),
    {
      method: "POST",
      headers: { Authorization: `Bearer ${token}` },
      body: file,
    },
  );
  if (!response.ok) {
    throw new Error(`Upload staging failed (HTTP ${response.status})`);
  }
  const payload = (await response.json()) as { name: string };
  return `__web_upload__/${payload.name}`;
}

/** App information for the `@tauri-apps/api/app` shim. */
export async function fetchAppInfo(): Promise<{ name: string; version: string }> {
  try {
    const response = await fetch(apiUrl("/api/health"));
    if (response.ok) {
      const payload = (await response.json()) as { name?: string; version?: string };
      return { name: payload.name ?? "NyaTerm", version: payload.version ?? "" };
    }
  } catch {
    // fall through
  }
  return { name: "NyaTerm", version: "" };
}
