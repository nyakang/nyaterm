/**
 * Web shim for `@tauri-apps/plugin-updater`.
 *
 * Desktop auto-update does not apply to web deployments; report "no update".
 * Server upgrades happen by redeploying the server binary/image.
 */

export interface Update {
  version: string;
  currentVersion: string;
  body?: string;
  date?: string;
  downloadAndInstall: (onEvent?: (progress: unknown) => void) => Promise<void>;
  install: () => Promise<void>;
  close: () => Promise<void>;
}

export async function check(): Promise<Update | null> {
  return null;
}
