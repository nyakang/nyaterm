import { invoke } from "./invoke";
import { isWindows } from "./platform";

export type ClipboardPathPayload =
  | { kind: "file_paths"; paths: string[] }
  | { kind: "image_file"; path: string };

export interface RemoteClipboardImagePayload {
  remote_path: string;
}

export async function readClipboardText(): Promise<string> {
  const text = await invoke<string | null>("read_clipboard_text");
  return text ?? "";
}

export async function writeClipboardText(text: string): Promise<void> {
  if (isWindows) {
    try {
      await invoke<void>("write_clipboard_text", { text });
      return;
    } catch {
      /* fall back to the WebView clipboard API */
    }
  }

  if (navigator.clipboard?.writeText) {
    try {
      await navigator.clipboard.writeText(text);
      return;
    } catch {
      /* fall back to the Tauri clipboard command */
    }
  }

  await invoke<void>("write_clipboard_text", { text });
}

export async function readClipboardPathPayload(): Promise<ClipboardPathPayload | null> {
  return invoke<ClipboardPathPayload | null>("read_clipboard_path_payload");
}

export async function readClipboardFilePaths(): Promise<string[]> {
  return invoke<string[]>("read_clipboard_file_paths");
}

export async function uploadClipboardImageToSsh(
  sessionId: string,
): Promise<RemoteClipboardImagePayload | null> {
  return invoke<RemoteClipboardImagePayload | null>("upload_clipboard_image_to_ssh", {
    sessionId,
    remoteDir: null,
  });
}
