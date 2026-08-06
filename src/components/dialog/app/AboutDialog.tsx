import { getName, getVersion } from "@tauri-apps/api/app";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Check, Copy } from "lucide-react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { writeClipboardText } from "@/lib/clipboard";
import { invoke } from "@/lib/invoke";
import type { AppSupportInfo } from "@/types/global";
import pkg from "../../../../package.json";
import NyaTermLogo from "../../NyaTermLogo";

interface AboutDialogProps {
  open: boolean;
  onClose: () => void;
}

export default function AboutDialog({ open, onClose }: AboutDialogProps) {
  const { t } = useTranslation();
  const [appName, setAppName] = useState("NyaTerm");
  const [appVersion, setAppVersion] = useState<string | null>(null);
  const [appVersionFailed, setAppVersionFailed] = useState(false);
  const [supportInfo, setSupportInfo] = useState<AppSupportInfo | null>(null);
  const [supportInfoFailed, setSupportInfoFailed] = useState(false);
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    if (!open) {
      return;
    }

    let cancelled = false;
    setCopied(false);
    setAppVersion(null);
    setAppVersionFailed(false);
    setSupportInfo(null);
    setSupportInfoFailed(false);

    getName()
      .then((name) => {
        if (!cancelled) setAppName(name);
      })
      .catch(() => undefined);

    getVersion()
      .then((version) => {
        if (!cancelled) setAppVersion(version);
      })
      .catch(() => {
        if (!cancelled) setAppVersionFailed(true);
      });

    invoke<AppSupportInfo>("get_support_info")
      .then((info) => {
        if (!cancelled) setSupportInfo(info);
      })
      .catch(() => {
        if (!cancelled) setSupportInfoFailed(true);
      });

    return () => {
      cancelled = true;
    };
  }, [open]);

  const copySupportInfo = async () => {
    if (!appVersion || !supportInfo) {
      return;
    }

    const text = [
      "NyaTerm Support Information",
      `Version: ${appVersion}`,
      `Operating System: ${supportInfo.os}`,
      `Application Architecture: ${supportInfo.architecture}`,
      `Runtime: ${supportInfo.runtime === "portable" ? t("about.portable") : t("about.installed")}`,
    ].join("\n");
    try {
      await writeClipboardText(text);
      setCopied(true);
      toast.success(t("about.supportInfoCopied"));
      window.setTimeout(() => setCopied(false), 1800);
    } catch {
      toast.error(t("about.supportInfoCopyFailed"));
    }
  };

  const supportInfoReady = Boolean(appVersion && supportInfo);
  const versionDisplay =
    appVersion ?? (appVersionFailed ? t("about.unknown") : t("common.loading"));
  const osDisplay =
    supportInfo?.os ?? (supportInfoFailed ? t("about.unknown") : t("common.loading"));
  const architectureDisplay =
    supportInfo?.architecture ?? (supportInfoFailed ? t("about.unknown") : t("common.loading"));
  const runtimeDisplay = supportInfo
    ? supportInfo.runtime === "portable"
      ? t("about.portable")
      : t("about.installed")
    : supportInfoFailed
      ? t("about.unknown")
      : t("common.loading");

  const supportRows = [
    [t("about.version"), versionDisplay],
    [t("about.operatingSystem"), osDisplay],
    [t("about.architecture"), architectureDisplay],
    [t("about.runtime"), runtimeDisplay],
  ];

  return (
    <Dialog open={open} onOpenChange={(v) => !v && onClose()}>
      <DialogContent className="w-[min(440px,calc(100vw-2rem))] sm:max-w-[440px] flex flex-col items-center p-6 gap-4">
        <DialogHeader className="items-center">
          <NyaTermLogo className="w-24 h-24 object-contain" />
          <DialogTitle className="text-lg">{appName}</DialogTitle>
          <DialogDescription className="text-xs">v{versionDisplay}</DialogDescription>
        </DialogHeader>

        <p className="text-xs text-center px-4 leading-relaxed text-muted-foreground">
          {t("about.description")}
        </p>

        <section
          className="w-full rounded-md border bg-muted/20 p-3"
          aria-labelledby="support-info-title"
        >
          <div className="flex items-center justify-between gap-3 mb-2">
            <h3 id="support-info-title" className="text-sm font-medium">
              {t("about.supportInfo")}
            </h3>
            <Button
              variant="ghost"
              size="sm"
              className="h-8 gap-1.5 px-2 text-xs"
              onClick={() => void copySupportInfo()}
              title={t("about.copySupportInfo")}
              aria-label={t("about.copySupportInfo")}
              disabled={!supportInfoReady}
            >
              {copied ? <Check className="size-3.5" /> : <Copy className="size-3.5" />}
              {copied ? t("about.supportInfoCopiedShort") : t("about.copySupportInfo")}
            </Button>
          </div>
          <dl className="grid grid-cols-[minmax(0,auto)_1fr] gap-x-4 gap-y-1.5 text-xs">
            {supportRows.map(([label, value]) => (
              <div className="contents" key={label}>
                <dt className="text-muted-foreground">{label}</dt>
                <dd className="min-w-0 break-words text-right font-mono">{value}</dd>
              </div>
            ))}
          </dl>
        </section>

        <div className="flex gap-3 w-full pt-2">
          <Button
            variant="outline"
            size="sm"
            className="flex-1 text-xs"
            onClick={() => openUrl(pkg.homepage)}
          >
            {t("about.website")}
          </Button>
          <Button
            variant="outline"
            size="sm"
            className="flex-1 text-xs"
            onClick={() => openUrl(pkg.bugs.url)}
          >
            {t("about.issues")}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
