/**
 * Web shim for `@tauri-apps/plugin-process`.
 */

/** "Relaunch" in the browser reloads the page. */
export async function relaunch(): Promise<void> {
  window.location.reload();
}

export async function exit(_exitCode?: number): Promise<void> {
  window.close();
}
