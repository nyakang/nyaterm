import { toast } from "sonner";
import type { ResolvedLocalDropPathEntry } from "@/components/panel/file-explorer/model";
import { invoke } from "@/lib/invoke";
import { sendSessionInput } from "@/lib/sessionInput";
import {
  isZmodemUploadErrorHandled,
  uploadFilesViaZmodem,
} from "@/lib/terminalZmodemUpload";
import type { SessionType } from "@/types/global";

function quoteLocalPath(path: string): string {
  if (!/[\s'"\\]/.test(path)) {
    return path;
  }
  if (path.includes("\\")) {
    return `"${path.replace(/"/g, '\\"')}"`;
  }
  return `'${path.replace(/'/g, `'\\''`)}'`;
}

export function formatLocalTerminalDropInput(paths: string[]): string {
  return paths.map(quoteLocalPath).join(" ");
}

export function getTerminalDropOverlayCopy(
  sessionType: SessionType,
  t: (key: string) => string,
): { title: string; hint: string } {
  switch (sessionType) {
    case "Local":
      return {
        title: t("terminal.dropOverlayTitleLocal"),
        hint: t("terminal.dropOverlayHintLocal"),
      };
    case "SSH":
    case "Telnet":
      return {
        title: t("terminal.dropOverlayTitleUpload"),
        hint: t("terminal.dropOverlayHintZmodem"),
      };
    case "Serial":
      return {
        title: t("terminal.dropOverlayTitleUpload"),
        hint: t("terminal.dropOverlayHintSerialModem"),
      };
  }
}

export async function handleTerminalFileDrop(params: {
  sessionId: string;
  sessionType: SessionType;
  entries: ResolvedLocalDropPathEntry[];
  t: (key: string) => string;
  duplicateStrategy?: string;
}): Promise<void> {
  const { sessionId, sessionType, entries, t, duplicateStrategy = "ask" } = params;
  if (entries.length === 0) {
    return;
  }

  const fileEntries = entries.filter((entry) => !entry.isDir);
  const hasDirectories = entries.some((entry) => entry.isDir);

  if (sessionType === "Local") {
    if (fileEntries.length === 0) {
      toast.message(t("terminal.dropFoldersNotSupportedLocal"));
      return;
    }
    await sendSessionInput(
      sessionId,
      formatLocalTerminalDropInput(fileEntries.map((entry) => entry.path)),
    );
    return;
  }

  if (sessionType === "Serial") {
    if (hasDirectories) {
      toast.error(t("terminal.dropFoldersSerialModemOnly"));
      return;
    }
    if (fileEntries.length === 0) {
      return;
    }

    await invoke("serial_modem_upload", {
      sessionId,
      filePaths: fileEntries.map((entry) => entry.path),
    }).catch((error) => {
      const message = String(error);
      toast.error(
        message.includes("XMODEM supports exactly one file")
          ? t("terminal.xmodemSingleFileOnly")
          : t("terminal.serialModemUploadFailed"),
      );
      throw error;
    });
    return;
  }

  // SSH / Telnet: ZMODEM only, no folder support.
  if (hasDirectories) {
    toast.error(t("terminal.dropFoldersZmodemOnly"));
    return;
  }

  if (fileEntries.length === 0) {
    return;
  }

  await uploadFilesViaZmodem(
    sessionId,
    fileEntries.map((entry) => entry.path),
    duplicateStrategy,
  ).catch((error) => {
    if (!isZmodemUploadErrorHandled(error)) {
      toast.error(t("terminal.dropUploadFailed"));
    }
    throw error;
  });
}
