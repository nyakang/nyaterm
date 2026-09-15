import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { MdDelete } from "react-icons/md";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Label } from "@/components/ui/label";
import { getErrorMessage } from "@/lib/errors";
import { invoke } from "@/lib/invoke";
import type { KnownHostEntry } from "@/types/global";

interface KnownHostsManagementTabProps {
  onCountChange?: (count: number) => void;
}

export function KnownHostsManagementTab({ onCountChange }: KnownHostsManagementTabProps) {
  const { t } = useTranslation();
  const [entries, setEntries] = useState<KnownHostEntry[]>([]);
  const [deletingEntry, setDeletingEntry] = useState<KnownHostEntry | null>(null);
  const [clearOpen, setClearOpen] = useState(false);

  const loadKnownHosts = useCallback(async () => {
    try {
      const result = await invoke<KnownHostEntry[]>("get_known_hosts");
      setEntries(result);
      onCountChange?.(result.length);
    } catch (error) {
      toast.error(t("knownHosts.loadFailed", { error: getErrorMessage(error) }));
    }
  }, [onCountChange, t]);

  useEffect(() => {
    void loadKnownHosts();
  }, [loadKnownHosts]);

  const handleDeleteConfirm = async () => {
    if (!deletingEntry) return;
    try {
      await invoke("delete_known_host", { id: deletingEntry.id });
      setDeletingEntry(null);
      await loadKnownHosts();
    } catch (error) {
      toast.error(t("knownHosts.deleteFailed", { error: getErrorMessage(error) }));
    }
  };

  const handleClearConfirm = async () => {
    try {
      await invoke("clear_known_hosts");
      setClearOpen(false);
      await loadKnownHosts();
    } catch (error) {
      toast.error(t("knownHosts.clearFailed", { error: getErrorMessage(error) }));
    }
  };

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="min-h-0 flex-1 overflow-y-auto px-3 pb-3 terminal-scroll">
        <div className="space-y-2">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <Label className="min-w-0 text-sm font-medium">{t("knownHosts.title")}</Label>
            <Button
              variant="ghost"
              size="sm"
              className="h-7 shrink-0 px-2 text-xs text-destructive hover:bg-destructive/10"
              onClick={() => setClearOpen(true)}
            >
              {t("knownHosts.clearAll")}
            </Button>
          </div>

          <div className="overflow-hidden rounded-md border">
            {entries.map((entry) => {
              const host = entry.hostPatterns.length
                ? entry.hostPatterns.join(",")
                : entry.hostIdentifier;
              const displayHost = entry.marker ? `${entry.marker} ${host}` : host;
              return (
                <div
                  key={entry.id}
                  className="security-auth-action-row flex items-start gap-2 border-b px-3 py-2.5 transition-colors last:border-0 hover:bg-accent"
                >
                  <div className="min-w-0 flex-1 space-y-1">
                    <div className="break-all text-xs">
                      <span className="text-muted-foreground">{t("knownHosts.host")}: </span>
                      {displayHost}
                    </div>
                    <div className="break-all text-[0.6875rem] text-muted-foreground">
                      {t("knownHosts.keyType")}: {entry.keyType}
                    </div>
                    <div className="break-all font-mono text-[0.6875rem] text-muted-foreground">
                      {t("knownHosts.fingerprint")}:{" "}
                      {entry.fingerprint ?? t("knownHosts.fingerprintUnavailable")}
                    </div>
                  </div>
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    className="shrink-0 text-destructive hover:bg-destructive/10"
                    onClick={() => setDeletingEntry(entry)}
                    aria-label={t("knownHosts.deleteEntry")}
                  >
                    <MdDelete className="text-base" />
                  </Button>
                </div>
              );
            })}

            {entries.length === 0 ? (
              <div className="py-6 text-center text-xs text-muted-foreground">
                {t("knownHosts.noEntries")}
              </div>
            ) : null}
          </div>
        </div>
      </div>

      <Dialog
        open={deletingEntry !== null}
        onOpenChange={(open) => !open && setDeletingEntry(null)}
      >
        <DialogContent showCloseButton={false} className="max-w-xs">
          <DialogHeader>
            <DialogTitle>{t("knownHosts.deleteTitle")}</DialogTitle>
            <DialogDescription>
              {t("knownHosts.deleteConfirm", {
                host: deletingEntry?.hostPatterns.join(",") || deletingEntry?.hostIdentifier,
              })}
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="outline" onClick={() => setDeletingEntry(null)}>
              {t("common.cancel")}
            </Button>
            <Button variant="destructive" onClick={handleDeleteConfirm}>
              {t("common.delete")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog open={clearOpen} onOpenChange={setClearOpen}>
        <DialogContent showCloseButton={false} className="max-w-xs">
          <DialogHeader>
            <DialogTitle>{t("knownHosts.clearTitle")}</DialogTitle>
            <DialogDescription>{t("knownHosts.clearConfirm")}</DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="outline" onClick={() => setClearOpen(false)}>
              {t("common.cancel")}
            </Button>
            <Button variant="destructive" onClick={handleClearConfirm}>
              {t("knownHosts.clearAll")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
