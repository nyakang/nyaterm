import { open as openFileDialog } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { ComponentType } from "react";
import { useTranslation } from "react-i18next";
import { MdDataObject, MdOpenInNew, MdTerminal } from "react-icons/md";
import { toast } from "sonner";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { useConfigTransfer } from "@/hooks/useConfigTransfer";
import { useApp } from "../../../context/AppContext";
import { invoke } from "../../../lib/invoke";
import { logger } from "../../../lib/logger";

interface ImportDialogProps {
  open: boolean;
  onClose: () => void;
}

interface ImportSource {
  id: string;
  name: string;
  icon: string | ComponentType<{ className?: string }>;
  extensions: string[];
  hint?: string;
  type: "backup" | "sessions";
}

const IMPORT_SOURCES: ImportSource[] = [
  {
    id: "nyaterm",
    name: "NyaTerm",
    icon: "/nyaterm.svg",
    extensions: ["nya"],
    hint: ".nya",
    type: "backup",
  },
  {
    id: "xshell",
    name: "Xshell",
    icon: "/Xshell.svg",
    extensions: ["xts"],
    hint: ".xts",
    type: "sessions",
  },
  {
    id: "mobaxterm",
    name: "MobaXterm",
    icon: "/MobaXterm.svg",
    extensions: ["mxtsessions"],
    hint: ".mxtsessions",
    type: "sessions",
  },
  {
    id: "windterm",
    name: "WindTerm",
    icon: "/WindTerm.svg",
    extensions: ["sessions"],
    hint: ".sessions",
    type: "sessions",
  },
  {
    id: "nyaterm_json",
    name: "JSON",
    icon: MdDataObject,
    extensions: ["json"],
    hint: ".json",
    type: "sessions",
  },
];

const SESSION_IMPORT_DOC_URLS = {
  zh: "https://nyaterm.app/docs/guide/ssh-connection#导入其他客户端的会话",
  en: "https://nyaterm.app/docs/guide/ssh-connection#import-sessions-from-other-clients",
};

export default function ImportDialog({ open, onClose }: ImportDialogProps) {
  const { i18n, t } = useTranslation();
  const { refreshConnections } = useApp();
  const { handleImport, passwordAlert } = useConfigTransfer();
  const docsUrl = i18n.language.toLowerCase().startsWith("zh")
    ? SESSION_IMPORT_DOC_URLS.zh
    : SESSION_IMPORT_DOC_URLS.en;

  const renderSourceIcon = (source: ImportSource) => {
    if (typeof source.icon === "string") {
      return <img src={source.icon} alt={source.name} className="h-10 w-10" draggable={false} />;
    }

    const Icon = source.icon;
    return <Icon className="h-10 w-10 text-[var(--df-primary)]" />;
  };

  const handleSelect = async (source: ImportSource) => {
    onClose();

    if (source.type === "backup") {
      await handleImport();
      return;
    }

    const selected = await openFileDialog({
      multiple: false,
      filters: [{ name: source.name, extensions: source.extensions }],
    });
    if (!selected) return;
    try {
      const count = await invoke<number>("import_sessions", { filePath: selected });
      if (count > 0) {
        toast.success(t("savedConnections.importSuccess", { count }));
        refreshConnections();
      } else {
        toast.info(t("savedConnections.importSuccess", { count: 0 }));
      }
    } catch (e) {
      logger.error({
        domain: "settings.persistence",
        event: "sessions.import_failed",
        message: "Import sessions failed",
        error: e,
      });
      toast.error(t("savedConnections.importFailed", { error: e }));
    }
  };

  return (
    <>
      <Dialog open={open} onOpenChange={(v) => !v && onClose()}>
        <DialogContent className="w-[min(480px,calc(100vw-2rem))] sm:max-w-[480px] p-6">
          <DialogHeader>
            <DialogTitle className="text-sm">{t("settings.importConfig")}</DialogTitle>
            <DialogDescription className="text-xs">
              {t("savedConnections.importSelectSource")}
            </DialogDescription>
          </DialogHeader>
          <div className="grid grid-cols-2 gap-3 pt-2 sm:grid-cols-3">
            {IMPORT_SOURCES.map((source) => (
              <button
                key={source.id}
                type="button"
                className="flex min-h-32 flex-col items-center justify-center gap-2 rounded-lg border p-3 text-center transition-colors hover:border-[var(--df-primary)] hover:bg-[color-mix(in_srgb,var(--df-primary)_8%,transparent)] cursor-pointer"
                style={{ borderColor: "var(--df-border)" }}
                onClick={() => handleSelect(source)}
              >
                {renderSourceIcon(source)}
                <span className="text-xs font-medium" style={{ color: "var(--df-text)" }}>
                  {source.name}
                </span>
                {source.hint && (
                  <span
                    className="text-[0.6rem] leading-tight text-center break-all"
                    style={{ color: "var(--df-text-dimmed)" }}
                  >
                    {source.hint}
                  </span>
                )}
              </button>
            ))}
          </div>
          <div
            className="flex items-center justify-between gap-3 pt-1 text-[0.6875rem]"
            style={{ color: "var(--df-text-dimmed)" }}
          >
            <div className="flex min-w-0 flex-1 items-center gap-1.5">
              <MdTerminal className="shrink-0 text-[0.85rem]" />
              <span className="leading-tight">{t("savedConnections.importMergeHint")}</span>
            </div>
            <button
              type="button"
              className="inline-flex shrink-0 items-center gap-1 rounded px-1.5 py-1 text-[0.6875rem] transition-colors hover:bg-[var(--df-bg-hover)]"
              style={{ color: "var(--df-primary)" }}
              onClick={() => void openUrl(encodeURI(docsUrl))}
            >
              {t("savedConnections.importDocs")}
              <MdOpenInNew className="text-[0.75rem]" />
            </button>
          </div>
        </DialogContent>
      </Dialog>
      {passwordAlert}
    </>
  );
}
