import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { MdFolder, MdInsertDriveFile, MdRefresh } from "react-icons/md";
import { toast } from "sonner";
import {
  type FileExplorerBackendKind,
  getExplorerParentDirectory,
} from "@/components/panel/file-explorer/model";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { invoke } from "@/lib/invoke";
import type { FileProperties } from "@/types/global";

export interface PropertiesDialogData {
  sessionId: string;
  backend: FileExplorerBackendKind;
  fullPath: string;
  rawPathToken?: string;
  name: string;
  is_dir: boolean;
}

interface PropertiesDialogProps {
  data: PropertiesDialogData;
  onClose: () => void;
  onSuccess?: () => Promise<unknown> | unknown;
}

function formatSize(bytes: number): string {
  if (bytes === 0) return "0 Bytes";
  if (bytes < 1024) return `${bytes} Bytes`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function formatTime(unix: number): string {
  if (!unix) return "-";
  const d = new Date(unix * 1000);
  const pad = (n: number) => n.toString().padStart(2, "0");
  return `${d.getFullYear()}/${pad(d.getMonth() + 1)}/${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}

function parsePermissionsToOctal(perms: string): string {
  if (!perms || perms.length < 10) return "0644";
  let special = 0,
    u = 0,
    g = 0,
    o = 0;
  const p = perms.split("");

  if (p[1] === "r") u |= 4;
  if (p[2] === "w") u |= 2;
  if (p[3] === "x") u |= 1;
  else if (p[3] === "s") {
    u |= 1;
    special |= 4;
  } else if (p[3] === "S") {
    special |= 4;
  }

  if (p[4] === "r") g |= 4;
  if (p[5] === "w") g |= 2;
  if (p[6] === "x") g |= 1;
  else if (p[6] === "s") {
    g |= 1;
    special |= 2;
  } else if (p[6] === "S") {
    special |= 2;
  }

  if (p[7] === "r") o |= 4;
  if (p[8] === "w") o |= 2;
  if (p[9] === "x") o |= 1;
  else if (p[9] === "t") {
    o |= 1;
    special |= 1;
  } else if (p[9] === "T") {
    special |= 1;
  }

  return `${special}${u}${g}${o}`;
}

export default function PropertiesDialog({ data, onClose, onSuccess }: PropertiesDialogProps) {
  const { t } = useTranslation();
  const [properties, setProperties] = useState<FileProperties | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const [octal, setOctal] = useState<string>("0644");
  const [ownerInput, setOwnerInput] = useState("");
  const [groupInput, setGroupInput] = useState("");
  const [recursive, setRecursive] = useState(false);
  const [isSaving, setIsSaving] = useState(false);
  const initialOctal = properties ? parsePermissionsToOctal(properties.permissions) : "0644";
  const initialOwner = properties?.owner || properties?.uid || "";
  const initialGroup = properties?.group || properties?.gid || "";
  const canEditAttributes = data.backend === "remote";

  useEffect(() => {
    let isMounted = true;
    setLoading(true);
    const request =
      data.backend === "local"
        ? invoke<FileProperties>("get_local_file_properties", {
            sessionId: data.sessionId,
            path: data.fullPath,
          })
        : invoke<FileProperties>("get_file_properties", {
            sessionId: data.sessionId,
            path: data.fullPath,
            rawPathToken: data.rawPathToken,
          });

    request
      .then((props) => {
        if (isMounted) {
          setProperties(props);
          setOctal(parsePermissionsToOctal(props.permissions));
          setOwnerInput(props.owner || props.uid || "");
          setGroupInput(props.group || props.gid || "");
          setRecursive(false);
        }
      })
      .catch((e) => {
        if (isMounted) setError(String(e));
      })
      .finally(() => {
        if (isMounted) setLoading(false);
      });
    return () => {
      isMounted = false;
    };
  }, [data.backend, data.sessionId, data.fullPath, data.rawPathToken]);

  const handleSave = async () => {
    if (!properties || !canEditAttributes) return;

    const nextOwner = ownerInput.trim();
    const nextGroup = groupInput.trim();
    if (!nextOwner || !nextGroup) {
      toast.error(t("fileExplorer.ownerGroupRequired"));
      return;
    }

    const update = {
      mode: octal !== initialOctal ? octal : null,
      owner: nextOwner !== initialOwner ? nextOwner : null,
      group: nextGroup !== initialGroup ? nextGroup : null,
      recursive: data.is_dir && recursive,
    };

    if (!update.mode && !update.owner && !update.group) {
      onClose();
      return;
    }

    setIsSaving(true);
    try {
      await invoke("update_remote_file_attributes", {
        sessionId: data.sessionId,
        path: data.fullPath,
        rawPathToken: data.rawPathToken,
        update,
      });
      toast.success(t("fileExplorer.propertiesSaved"));
      await onSuccess?.();
      onClose();
    } catch (e) {
      toast.error(String(e));
    } finally {
      setIsSaving(false);
    }
  };

  const updateBit = (index: number, bit: number, checked: boolean) => {
    const chars = octal.split("");
    for (let i = 0; i < 4; i++) {
      if (!chars[i]) chars[i] = "0";
    }
    let val = parseInt(chars[index], 8);
    if (Number.isNaN(val)) val = 0;
    if (checked) val |= bit;
    else val &= ~bit;
    chars[index] = val.toString(8);
    setOctal(chars.join(""));
  };

  const hasBit = (index: number, bit: number) => {
    const val = parseInt(octal[index] || "0", 8);
    return (val & bit) === bit;
  };

  const getFileType = () => {
    if (data.is_dir) return t("fileExplorer.folder");
    const ext = data.name.split(".").pop()?.toLowerCase();
    if (ext === "sh" || ext === "bash") return t("fileExplorer.shellScript");
    return t("fileExplorer.file");
  };

  const getLocation = () => {
    return getExplorerParentDirectory(data.fullPath, data.backend);
  };

  return (
    <Dialog disablePointerDismissal open onOpenChange={(v) => !v && !isSaving && onClose()}>
      <DialogContent className="w-[min(460px,calc(100vw-2rem))] sm:max-w-[460px] p-0 gap-0">
        <DialogHeader className="pl-5 pr-12 py-3 border-b">
          <DialogTitle className="text-sm flex items-center gap-2 min-w-0">
            {data.is_dir ? (
              <MdFolder className="text-lg shrink-0" style={{ color: "#eab308" }} />
            ) : (
              <MdInsertDriveFile
                className="text-lg shrink-0"
                style={{ color: "var(--df-primary)" }}
              />
            )}
            <span className="truncate" title={t("fileExplorer.propertiesOf", { name: data.name })}>
              {t("fileExplorer.propertiesOf", { name: data.name })}
            </span>
          </DialogTitle>
          <DialogDescription className="sr-only">
            {t("fileExplorer.propertiesOf", { name: data.name })}
          </DialogDescription>
        </DialogHeader>

        {/* Body */}
        <div className="p-5 overflow-y-auto max-h-[75vh] space-y-5 relative min-h-[250px]">
          {loading ? (
            <div className="absolute inset-0 flex flex-col items-center justify-center space-y-2 text-muted-foreground">
              <MdRefresh className="animate-spin text-3xl" />
              <span className="text-xs">{t("fileExplorer.loading")}</span>
            </div>
          ) : error ? (
            <div className="absolute inset-0 flex items-center justify-center text-destructive text-xs px-5 text-center">
              {error}
            </div>
          ) : properties ? (
            <>
              {/* General Information */}
              <div>
                <h3 className="text-xs font-semibold mb-3 tracking-wider uppercase text-muted-foreground">
                  {t("fileExplorer.general")}
                </h3>
                <div className="space-y-2.5 text-xs text-left">
                  {[
                    { key: "type", label: t("fileExplorer.type"), value: getFileType() },
                    {
                      key: "location",
                      label: t("fileExplorer.location"),
                      value: (
                        <span className="break-all select-all font-mono" title={getLocation()}>
                          {getLocation()}
                        </span>
                      ),
                    },
                    {
                      key: "size",
                      label: t("fileExplorer.size"),
                      value: formatSize(properties.size),
                    },
                    {
                      key: "mtime",
                      label: t("fileExplorer.mtime"),
                      value: <span className="font-mono">{formatTime(properties.mtime)}</span>,
                    },
                    {
                      key: "atime",
                      label: t("fileExplorer.atime"),
                      value: <span className="font-mono">{formatTime(properties.atime)}</span>,
                    },
                    {
                      key: "owner",
                      label: t("fileExplorer.owner"),
                      value: (
                        <span>
                          {properties.owner || "-"}{" "}
                          {properties.uid && (
                            <span className="font-mono opacity-70">[{properties.uid}]</span>
                          )}
                        </span>
                      ),
                    },
                    {
                      key: "group",
                      label: t("fileExplorer.group"),
                      value: (
                        <span>
                          {properties.group || "-"}{" "}
                          {properties.gid && (
                            <span className="font-mono opacity-70">[{properties.gid}]</span>
                          )}
                        </span>
                      ),
                    },
                  ].map((row) => (
                    <div key={row.key} className="flex min-w-0 items-start">
                      <span className="w-24 shrink-0 text-muted-foreground">{row.label}:</span>
                      <span className="min-w-0 break-words">{row.value}</span>
                    </div>
                  ))}
                </div>
              </div>

              {canEditAttributes && <div className="border-t" />}

              {/* Ownership */}
              {canEditAttributes && (
                <div>
                  <h3 className="text-xs font-semibold mb-3 tracking-wider uppercase text-muted-foreground">
                    {t("fileExplorer.ownership")}
                  </h3>
                  <div className="space-y-3 text-xs">
                    <label className="grid grid-cols-[5.5rem_1fr] items-center gap-3">
                      <span className="text-muted-foreground">{t("fileExplorer.owner")}:</span>
                      <Input
                        className="h-8 text-xs"
                        value={ownerInput}
                        placeholder={properties.uid || t("fileExplorer.owner")}
                        disabled={isSaving}
                        onChange={(e) => setOwnerInput(e.target.value)}
                      />
                    </label>
                    <label className="grid grid-cols-[5.5rem_1fr] items-center gap-3">
                      <span className="text-muted-foreground">{t("fileExplorer.group")}:</span>
                      <Input
                        className="h-8 text-xs"
                        value={groupInput}
                        placeholder={properties.gid || t("fileExplorer.group")}
                        disabled={isSaving}
                        onChange={(e) => setGroupInput(e.target.value)}
                      />
                    </label>
                  </div>
                </div>
              )}

              {canEditAttributes && <div className="border-t" />}

              {/* Permissions */}
              {canEditAttributes && (
                <div>
                  <h3 className="text-xs font-semibold mb-3 tracking-wider uppercase text-muted-foreground">
                    {t("fileExplorer.permissions")}
                  </h3>
                  <div className="rounded-md border overflow-hidden bg-background">
                    <table className="w-full text-xs text-left select-none">
                      <thead className="bg-muted text-muted-foreground">
                        <tr>
                          <th className="font-normal px-3 py-2 w-16"></th>
                          <th className="font-normal px-2 py-2 text-center w-14">R</th>
                          <th className="font-normal px-2 py-2 text-center w-14">W</th>
                          <th className="font-normal px-2 py-2 text-center w-14">X</th>
                          <th className="font-normal px-2 py-2 text-center">
                            {t("fileExplorer.special")}
                          </th>
                        </tr>
                      </thead>
                      <tbody>
                        {[
                          {
                            label: t("fileExplorer.permUser"),
                            idx: 1,
                            sIdx: 0,
                            sBit: 4,
                            sLabel: "UID",
                            alt: true,
                          },
                          {
                            label: t("fileExplorer.permGroup"),
                            idx: 2,
                            sIdx: 0,
                            sBit: 2,
                            sLabel: "GID",
                            alt: false,
                          },
                          {
                            label: t("fileExplorer.permOther"),
                            idx: 3,
                            sIdx: 0,
                            sBit: 1,
                            sLabel: t("fileExplorer.permSticky"),
                            alt: true,
                          },
                        ].map((row) => (
                          <tr key={row.idx} className={`border-t ${row.alt ? "bg-muted/30" : ""}`}>
                            <td className="px-3 py-2 text-muted-foreground">{row.label}</td>
                            <td className="px-2 py-2 text-center">
                              <Checkbox
                                checked={hasBit(row.idx, 4)}
                                onCheckedChange={(checked) =>
                                  updateBit(row.idx, 4, checked === true)
                                }
                              />
                            </td>
                            <td className="px-2 py-2 text-center">
                              <Checkbox
                                checked={hasBit(row.idx, 2)}
                                onCheckedChange={(checked) =>
                                  updateBit(row.idx, 2, checked === true)
                                }
                              />
                            </td>
                            <td className="px-2 py-2 text-center">
                              <Checkbox
                                checked={hasBit(row.idx, 1)}
                                onCheckedChange={(checked) =>
                                  updateBit(row.idx, 1, checked === true)
                                }
                              />
                            </td>
                            <td className="px-2 py-2 text-center">
                              <label className="flex items-center justify-center gap-1.5 cursor-pointer text-[0.625rem]">
                                <Checkbox
                                  checked={hasBit(row.sIdx, row.sBit)}
                                  onCheckedChange={(checked) =>
                                    updateBit(row.sIdx, row.sBit, checked === true)
                                  }
                                />
                                {row.sLabel}
                              </label>
                            </td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </div>

                  <div className="flex items-center justify-between mt-4">
                    <span className="text-xs text-muted-foreground">
                      {t("fileExplorer.octal")}:
                    </span>
                    <div className="flex items-center">
                      <span className="text-xs font-mono mr-2 opacity-50">0</span>
                      <Input
                        className="w-[60px] text-center font-mono text-xs h-7"
                        style={{ letterSpacing: "2px" }}
                        value={octal.substring(1)}
                        onChange={(e) => {
                          let val = e.target.value.replace(/[^0-7]/g, "");
                          if (val.length > 3) val = val.substring(0, 3);
                          setOctal(octal[0] + val.padStart(3, "0"));
                        }}
                      />
                    </div>
                  </div>

                  {data.is_dir && (
                    <label className="mt-4 flex items-start gap-2 text-xs">
                      <Checkbox
                        className="mt-0.5"
                        checked={recursive}
                        disabled={isSaving}
                        onCheckedChange={(checked) => setRecursive(checked === true)}
                      />
                      <span className="leading-5 text-muted-foreground">
                        {t("fileExplorer.applyRecursively")}
                      </span>
                    </label>
                  )}
                </div>
              )}
            </>
          ) : null}
        </div>

        {/* Footer */}
        <DialogFooter className="px-5 py-3 border-t">
          <Button
            variant="outline"
            size="sm"
            className="text-xs"
            onClick={onClose}
            disabled={isSaving}
          >
            {canEditAttributes ? t("dialog.cancel") : t("common.close")}
          </Button>
          {canEditAttributes && (
            <Button
              size="sm"
              className="text-xs"
              onClick={handleSave}
              disabled={isSaving || loading || !!error}
            >
              {isSaving && <MdRefresh className="text-[0.875rem] animate-spin" />}
              {t("dialog.save")}
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
