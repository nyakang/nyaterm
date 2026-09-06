/**
 * Web shim for `@tauri-apps/api/app`.
 */

import { fetchAppInfo } from "../rpcClient";

export async function getName(): Promise<string> {
  return (await fetchAppInfo()).name;
}

export async function getVersion(): Promise<string> {
  return (await fetchAppInfo()).version;
}

export async function getTauriVersion(): Promise<string> {
  return "web";
}
