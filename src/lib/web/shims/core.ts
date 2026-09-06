/**
 * Web shim for `@tauri-apps/api/core`.
 *
 * Routes every command through the HTTP RPC transport. A small set of
 * window-management commands is intercepted locally because web mode has no
 * OS windows: "child windows" are in-document modals rendered by
 * `modalHost.tsx`.
 */

import "../webOverrides";
import { webInvoke } from "../rpcClient";

/** Tauri-compatible stub: channels only matter for RDP/VNC/updater flows,
 * which are not exposed in web mode. */
export class Channel<T = unknown> {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  onmessage?: (message: T) => void;

  toJSON(): null {
    return null;
  }
}

const MODAL_WINDOW_COMMANDS = new Set(["open_child_window", "close_child_window"]);
const DESKTOP_ONLY_COMMANDS = new Set([
  "quit_application",
  "hide_main_window",
  "open_download_dir",
  "open_log_dir",
  "open_transfer_target_directory",
  "open_recording_file",
  "show_recording_in_folder",
  "zmodem_pick_download_dir",
  "zmodem_pick_upload_files",
  // External-MCP and OS deep-link flows require desktop windows/integration.
  "notify_mcp_session_restore_complete",
  "report_mcp_active_session",
]);
// Commands whose desktop result shape must be preserved in web mode.
const EMPTY_RESULT_COMMANDS = new Set([
  // Deep-link/open-on-login queue: no OS open requests can exist in a browser.
  "claim_external_open_requests",
]);

async function dispatchWebCommand<T>(
  cmd: string,
  args?: Record<string, unknown>,
): Promise<T> {
  if (cmd === "open_child_window" || cmd === "close_child_window") {
    const { handleWebWindowCommand } = await import("../modalHost");
    return handleWebWindowCommand(cmd, args) as T;
  }
  return webInvoke<T>(cmd, args);
}

export async function invoke<T>(
  cmd: string,
  args?: Record<string, unknown>,
  // Tauri's `InvokeOptions` — ignored in web mode.
  // eslint-disable-next-line @typescript-eslint/no-unused-vars
  _options?: unknown,
): Promise<T> {
  if (MODAL_WINDOW_COMMANDS.has(cmd)) {
    return dispatchWebCommand<T>(cmd, args);
  }
  if (DESKTOP_ONLY_COMMANDS.has(cmd)) {
    // These commands only make sense with OS integration (process exit,
    // native folders, native dialogs). Web mode degrades them to no-ops so
    // peripheral UI keeps working without errors.
    return undefined as T;
  }
  if (EMPTY_RESULT_COMMANDS.has(cmd)) {
    return [] as unknown as T;
  }
  return webInvoke<T>(cmd, args);
}

export function transformCallback(callback?: (response: unknown) => void): number {
  // Legacy Tauri plumbing; unused by web shims.
  void callback;
  return -1;
}

export function convertFileSrc(filePath: string, _protocol = "asset"): string {
  // Web builds never use the asset protocol; surface the path unchanged.
  return filePath;
}
