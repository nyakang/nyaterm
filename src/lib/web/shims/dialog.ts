/**
 * Web shim for `@tauri-apps/plugin-dialog`.
 *
 * File selection uses `<input type="file">`; selected files are uploaded to
 * the server staging area and returned as `__web_upload__/<staged>` marker
 * paths that the backend web-file endpoints resolve. Save dialogs return
 * `__web_downloads__/<name>` markers — the transfer materializes the file
 * server-side and the RPC shim streams it to the browser as a download.
 *
 * Directory pickers are not supported in web mode (returns null, matching
 * the user cancelling).
 */

import { stageFileForUpload } from "../rpcClient";

export interface DialogFilter {
  name?: string;
  extensions?: string[];
}

export interface OpenDialogOptions {
  multiple?: boolean;
  directory?: boolean;
  filters?: DialogFilter[];
  defaultPath?: string;
  title?: string;
}

export interface SaveDialogOptions {
  defaultPath?: string;
  filters?: DialogFilter[];
  title?: string;
}

function extensionAccepted(file: File, filters?: DialogFilter[]): boolean {
  if (!filters || filters.length === 0) return true;
  const ext = file.name.includes(".") ? file.name.split(".").pop()?.toLowerCase() : "";
  const all = filters.flatMap((filter) => (filter.extensions ?? []).map((e) => e.toLowerCase()));
  if (all.length === 0) return true;
  return ext !== undefined && all.includes(ext);
}

function pickFiles(options: OpenDialogOptions): Promise<File[]> {
  return new Promise((resolve) => {
    const input = document.createElement("input");
    input.type = "file";
    input.style.display = "none";
    if (options.multiple) input.multiple = true;
    if (options.filters?.length) {
      const extensions = options.filters.flatMap((filter) => filter.extensions ?? []);
      if (extensions.length > 0 && !extensions.includes("*")) {
        input.accept = extensions.map((ext) => `.${ext}`).join(",");
      }
    }
    document.body.append(input);
    let settled = false;
    const finish = (files: File[]) => {
      if (settled) return;
      settled = true;
      input.remove();
      resolve(files);
    };
    input.addEventListener("change", () => {
      finish(Array.from(input.files ?? []));
    });
    // Cancel detection: focus returning without a change event.
    window.addEventListener(
      "focus",
      () => {
        setTimeout(() => finish([]), 300);
      },
      { once: true },
    );
    input.click();
  });
}

export async function open(
  options: OpenDialogOptions = {},
): Promise<string | string[] | null> {
  if (options.directory) {
    // Directory picking has no web equivalent in v1.
    return null;
  }
  const candidates = await pickFiles(options);
  const accepted = candidates.filter((file) => extensionAccepted(file, options.filters));
  if (accepted.length === 0) {
    return options.multiple ? [] : null;
  }
  const staged: string[] = [];
  for (const file of accepted) {
    try {
      staged.push(await stageFileForUpload(file));
    } catch (error) {
      console.error("[nyaterm-web] failed to stage file", file.name, error);
    }
  }
  return options.multiple ? staged : (staged[0] ?? null);
}

export async function save(options: SaveDialogOptions = {}): Promise<string | null> {
  const defaultName = options.defaultPath?.split(/[\\/]/).pop() ?? "download";
  return `__web_downloads__/${defaultName}`;
}

export const message = async (
  _message: string,
  _options?: Record<string, unknown>,
): Promise<unknown> => {
  window.alert(_message);
  return null;
};

export const confirm = async (
  _message: string,
  _options?: Record<string, unknown>,
): Promise<unknown> => window.confirm(_message);

export { message as ask };
