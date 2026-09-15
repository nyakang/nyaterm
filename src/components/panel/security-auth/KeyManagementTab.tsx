import { open as openFileDialog } from "@tauri-apps/plugin-dialog";
import { Copy, KeyRound } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { MdAdd, MdDelete, MdEdit } from "react-icons/md";
import { toast } from "sonner";
import { KeyDeleteDialog } from "@/components/dialog/security-auth/KeyDeleteDialog";
import {
  KeyEditorDialog,
  type KeyMaterialMode,
} from "@/components/dialog/security-auth/KeyEditorDialog";
import { PrivateKeyViewDialog } from "@/components/dialog/security-auth/PrivateKeyViewDialog";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { writeClipboardText } from "@/lib/clipboard";
import { getErrorMessage } from "@/lib/errors";
import { invoke } from "@/lib/invoke";
import type { SshKey } from "@/types/global";
import { SecretUnlockFooter } from "./SecretUnlockFooter";

interface KeyManagementTabProps {
  onCountChange?: (count: number) => void;
  secretsUnlocked?: boolean;
  onLockSecrets?: () => void;
  onUnlockSecrets?: () => void;
}

function getPathFileName(path: string) {
  const normalized = path.replace(/\\/g, "/").trim();
  if (!normalized) return "";
  const parts = normalized.split("/");
  return parts[parts.length - 1] || normalized;
}

export function KeyManagementTab({
  onCountChange,
  secretsUnlocked = false,
  onLockSecrets,
  onUnlockSecrets,
}: KeyManagementTabProps) {
  const { t } = useTranslation();
  const [keys, setKeys] = useState<SshKey[]>([]);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editName, setEditName] = useState("");
  const [editCertData, setEditCertData] = useState("");
  const [editCertFilePath, setEditCertFilePath] = useState("");
  const [editCertFileName, setEditCertFileName] = useState("");
  const [editKeyData, setEditKeyData] = useState("");
  const [editKeyFilePath, setEditKeyFilePath] = useState("");
  const [editKeyFileName, setEditKeyFileName] = useState("");
  const [editCertInputMode, setEditCertInputMode] = useState<KeyMaterialMode>("file");
  const [editKeyInputMode, setEditKeyInputMode] = useState<KeyMaterialMode>("content");
  const [editCertExpanded, setEditCertExpanded] = useState(false);
  const [editPassphrase, setEditPassphrase] = useState("");
  const [editPassphraseLoaded, setEditPassphraseLoaded] = useState(false);
  const [editShowPassphrase, setEditShowPassphrase] = useState(false);
  const [editHasCertData, setEditHasCertData] = useState(false);
  const [editHasKeyData, setEditHasKeyData] = useState(false);
  const [passphraseLoading, setPassphraseLoading] = useState(false);
  const [isNew, setIsNew] = useState(false);
  const [deletingKey, setDeletingKey] = useState<SshKey | null>(null);
  const [privateKeyEntry, setPrivateKeyEntry] = useState<SshKey | null>(null);
  const [privateKeyValue, setPrivateKeyValue] = useState("");
  const [privateKeyLoading, setPrivateKeyLoading] = useState(false);
  const [privateKeyError, setPrivateKeyError] = useState(false);
  const [publicKeyLoadingId, setPublicKeyLoadingId] = useState<string | null>(null);
  const [unlockRequestNonce, setUnlockRequestNonce] = useState(0);
  const editRequestRef = useRef(0);
  const pendingUnlockedActionRef = useRef<(() => void | Promise<void>) | null>(null);

  const loadKeys = useCallback(async () => {
    try {
      const result = await invoke<SshKey[]>("get_ssh_keys");
      setKeys(result);
      onCountChange?.(result.length);
    } catch {
      /* ignore */
    }
  }, [onCountChange]);

  useEffect(() => {
    loadKeys();
  }, [loadKeys]);

  useEffect(() => {
    if (!secretsUnlocked) {
      if (editingId !== "__new__" && editPassphraseLoaded) {
        setEditPassphrase("");
        setEditPassphraseLoaded(false);
      }
      setEditShowPassphrase(false);
      setPrivateKeyEntry(null);
      setPrivateKeyValue("");
      setPrivateKeyError(false);
      setPrivateKeyLoading(false);
    }
  }, [editPassphraseLoaded, editingId, secretsUnlocked]);

  const resetEdit = () => {
    editRequestRef.current += 1;
    setEditingId(null);
    setEditName("");
    setEditCertData("");
    setEditCertFilePath("");
    setEditCertFileName("");
    setEditKeyData("");
    setEditKeyFilePath("");
    setEditKeyFileName("");
    setEditCertInputMode("file");
    setEditKeyInputMode("content");
    setEditCertExpanded(false);
    setEditPassphrase("");
    setEditPassphraseLoaded(false);
    setEditShowPassphrase(false);
    setEditHasCertData(false);
    setEditHasKeyData(false);
    setPassphraseLoading(false);
    setIsNew(false);
  };

  const handleAdd = () => {
    resetEdit();
    setEditingId("__new__");
    setIsNew(true);
  };

  const loadEditPassphrase = useCallback(async (id: string, requestId = editRequestRef.current) => {
    setPassphraseLoading(true);
    try {
      const passphrase = await invoke<string | null>("get_ssh_key_passphrase", { id });
      if (editRequestRef.current !== requestId) return;
      setEditPassphrase(passphrase ?? "");
      setEditPassphraseLoaded(true);
    } catch {
      if (editRequestRef.current !== requestId) return;
      setEditPassphrase("");
      setEditPassphraseLoaded(true);
    } finally {
      if (editRequestRef.current === requestId) {
        setPassphraseLoading(false);
      }
    }
  }, []);

  const handleEdit = async (key: SshKey) => {
    const requestId = ++editRequestRef.current;
    setEditingId(key.id);
    setEditName(key.name);
    setEditCertData("");
    setEditCertFilePath("");
    setEditCertFileName("");
    setEditKeyData("");
    setEditKeyFilePath("");
    setEditKeyFileName("");
    setEditCertInputMode("file");
    setEditKeyInputMode("file");
    setEditCertExpanded(key.has_cert_data || false);
    setEditPassphrase("");
    setEditPassphraseLoaded(false);
    setEditShowPassphrase(false);
    setEditHasCertData(key.has_cert_data || false);
    setEditHasKeyData(key.has_key_data || false);
    setPassphraseLoading(false);
    setIsNew(false);

    if (secretsUnlocked) {
      await loadEditPassphrase(key.id, requestId);
    }
  };

  const handleSave = async () => {
    if (!editName.trim()) return;
    const keyData = editKeyData.trim();
    const keyPath = editKeyFilePath.trim();
    const certData = editCertData.trim();
    const certPath = editCertFilePath.trim();
    if (isNew && !keyData && !keyPath) return;
    try {
      await invoke("save_ssh_key", {
        key: {
          id: isNew ? "" : editingId,
          name: editName.trim(),
          cert_data: certData || undefined,
          cert_file_path: certPath || undefined,
          key_data: keyData || undefined,
          key_file_path: keyPath || undefined,
          passphrase: editPassphrase || undefined,
        },
      });
      resetEdit();
      await loadKeys();
    } catch (error) {
      toast.error(getErrorMessage(error));
    }
  };

  const handleDeleteConfirm = async () => {
    if (!deletingKey) return;
    try {
      await invoke("delete_ssh_key", { id: deletingKey.id });
      await loadKeys();
    } catch {
      /* ignore */
    }
    setDeletingKey(null);
  };

  const runUnlockedAction = useCallback(
    (action: () => void | Promise<void>) => {
      if (secretsUnlocked) {
        void action();
        return;
      }

      pendingUnlockedActionRef.current = action;
      setUnlockRequestNonce((value) => value + 1);
    },
    [secretsUnlocked],
  );

  const handleSecretsUnlocked = useCallback(() => {
    onUnlockSecrets?.();
    const pendingAction = pendingUnlockedActionRef.current;
    pendingUnlockedActionRef.current = null;
    if (pendingAction) {
      window.setTimeout(() => {
        void pendingAction();
      }, 0);
    }
  }, [onUnlockSecrets]);

  const handleTogglePassphrase = useCallback(() => {
    if (!isNew && editingId && !secretsUnlocked && !editPassphrase) {
      const targetId = editingId;
      const requestId = editRequestRef.current;
      runUnlockedAction(async () => {
        await loadEditPassphrase(targetId, requestId);
        if (editRequestRef.current === requestId) {
          setEditShowPassphrase(true);
        }
      });
      return;
    }

    setEditShowPassphrase((value) => !value);
  }, [editPassphrase, editingId, isNew, loadEditPassphrase, runUnlockedAction, secretsUnlocked]);

  const handleViewPrivateKey = useCallback(async (key: SshKey) => {
    setPrivateKeyEntry(key);
    setPrivateKeyValue("");
    setPrivateKeyError(false);
    setPrivateKeyLoading(true);
    try {
      const value = await invoke<string | null>("get_ssh_key_private_key", { id: key.id });
      setPrivateKeyValue(value ?? "");
    } catch {
      setPrivateKeyError(true);
    } finally {
      setPrivateKeyLoading(false);
    }
  }, []);

  const handleCopyPublicKey = useCallback(
    async (key: SshKey) => {
      setPublicKeyLoadingId(key.id);
      try {
        const value = await invoke<string>("get_ssh_key_public_key", { id: key.id });
        await writeClipboardText(value);
        toast.success(t("settings.copyPublicKeySuccess"));
      } catch (error) {
        toast.error(getErrorMessage(error));
      } finally {
        setPublicKeyLoadingId((current) => (current === key.id ? null : current));
      }
    },
    [t],
  );

  const handlePickFile = async () => {
    const selected = await openFileDialog({
      multiple: false,
      title: t("settings.selectKeyFileTitle"),
    });
    if (selected) {
      setEditKeyInputMode("file");
      setEditKeyData("");
      setEditKeyFilePath(selected);
      setEditKeyFileName(getPathFileName(selected));
    }
  };

  const handlePickCertFile = async () => {
    const selected = await openFileDialog({
      multiple: false,
      title: t("settings.selectCertFileTitle"),
    });
    if (selected) {
      setEditCertExpanded(true);
      setEditCertInputMode("file");
      setEditCertData("");
      setEditCertFilePath(selected);
      setEditCertFileName(getPathFileName(selected));
    }
  };

  const handleKeyInputModeChange = (mode: KeyMaterialMode) => {
    setEditKeyInputMode(mode);
    if (mode === "content") {
      setEditKeyFilePath("");
      setEditKeyFileName("");
    } else {
      setEditKeyData("");
    }
  };

  const handleCertInputModeChange = (mode: KeyMaterialMode) => {
    setEditCertInputMode(mode);
    if (mode === "content") {
      setEditCertFilePath("");
      setEditCertFileName("");
    } else {
      setEditCertData("");
    }
  };

  const handleKeyPathChange = (value: string) => {
    setEditKeyFilePath(value);
    setEditKeyFileName(getPathFileName(value));
    if (value.trim()) setEditKeyData("");
  };

  const handleCertPathChange = (value: string) => {
    setEditCertFilePath(value);
    setEditCertFileName(getPathFileName(value));
    if (value.trim()) setEditCertData("");
  };

  const handleKeyDataChange = (value: string) => {
    setEditKeyData(value);
    if (value.trim()) {
      setEditKeyFilePath("");
      setEditKeyFileName("");
    }
  };

  const handleCertDataChange = (value: string) => {
    setEditCertData(value);
    if (value.trim()) {
      setEditCertFilePath("");
      setEditCertFileName("");
    }
  };

  const lockedHint = !secretsUnlocked ? t("secretUnlock.lockedActionHint") : undefined;
  const hasResolvedKeySource =
    editKeyData.trim().length > 0 ||
    editKeyFilePath.trim().length > 0 ||
    (!isNew && editHasKeyData);

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="min-h-0 flex-1 overflow-y-auto px-3 pb-3 terminal-scroll">
        <div className="space-y-2">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <Label className="min-w-0 text-sm font-medium">{t("settings.keyManagement")}</Label>
            <Button
              variant="ghost"
              size="sm"
              className="h-7 shrink-0 px-2 text-xs text-primary"
              onClick={handleAdd}
              disabled={editingId !== null}
            >
              <MdAdd className="text-base mr-1" /> {t("settings.addKey")}
            </Button>
          </div>

          <div className="border rounded-md overflow-hidden">
            {keys.map((key) => (
              <div
                key={key.id}
                className="security-auth-action-row flex flex-wrap items-start gap-2 border-b px-3 py-2.5 transition-colors last:border-0 hover:bg-accent"
              >
                <span className="min-w-24 flex-1 truncate text-xs leading-8">{key.name}</span>
                <div className="security-auth-row-actions flex shrink-0 items-center">
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <span className="inline-flex">
                        <Button
                          variant="ghost"
                          size="icon-sm"
                          onClick={() => {
                            void handleCopyPublicKey(key);
                          }}
                          disabled={editingId !== null || publicKeyLoadingId === key.id}
                          aria-label={t("settings.copyPublicKey")}
                        >
                          <Copy className="h-4 w-4" />
                        </Button>
                      </span>
                    </TooltipTrigger>
                    <TooltipContent side="top">{t("settings.copyPublicKey")}</TooltipContent>
                  </Tooltip>
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <span className="inline-flex">
                        <Button
                          variant="ghost"
                          size="icon-sm"
                          onClick={() => {
                            runUnlockedAction(() => handleViewPrivateKey(key));
                          }}
                          disabled={editingId !== null || privateKeyLoading}
                          aria-label={t("settings.viewPrivateKey")}
                        >
                          <KeyRound className="h-4 w-4" />
                        </Button>
                      </span>
                    </TooltipTrigger>
                    <TooltipContent side="top">
                      {lockedHint ?? t("settings.viewPrivateKey")}
                    </TooltipContent>
                  </Tooltip>
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    onClick={() => {
                      void handleEdit(key);
                    }}
                    disabled={editingId !== null}
                  >
                    <MdEdit className="text-base" />
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    className="text-destructive hover:bg-destructive/10"
                    onClick={() => setDeletingKey(key)}
                    disabled={editingId !== null}
                  >
                    <MdDelete className="text-base" />
                  </Button>
                </div>
              </div>
            ))}

            {keys.length === 0 && (
              <div className="text-center py-6 text-xs text-muted-foreground">
                {t("settings.noKeys")}
              </div>
            )}
          </div>
        </div>
      </div>

      <SecretUnlockFooter
        unlocked={secretsUnlocked}
        onLock={onLockSecrets ?? (() => {})}
        onUnlocked={handleSecretsUnlocked}
        unlockRequestNonce={unlockRequestNonce}
      />

      <KeyEditorDialog
        open={editingId !== null}
        certExpanded={editCertExpanded}
        editCertData={editCertData}
        editCertFileName={editCertFileName}
        editCertFilePath={editCertFilePath}
        editHasCertData={editHasCertData}
        editHasKeyData={editHasKeyData}
        editKeyData={editKeyData}
        editKeyFileName={editKeyFileName}
        editKeyFilePath={editKeyFilePath}
        editCertInputMode={editCertInputMode}
        editKeyInputMode={editKeyInputMode}
        editName={editName}
        editPassphrase={editPassphrase}
        editShowPassphrase={editShowPassphrase}
        isEditing={!isNew}
        passphraseLoading={passphraseLoading}
        onCancel={resetEdit}
        onCertDataChange={handleCertDataChange}
        onCertInputModeChange={handleCertInputModeChange}
        onCertPathChange={handleCertPathChange}
        onKeyDataChange={handleKeyDataChange}
        onKeyInputModeChange={handleKeyInputModeChange}
        onKeyPathChange={handleKeyPathChange}
        onNameChange={setEditName}
        onPassphraseChange={(value) => {
          setEditPassphrase(value);
          if (!isNew) setEditPassphraseLoaded(false);
        }}
        onPickCertFile={handlePickCertFile}
        onPickFile={handlePickFile}
        onSave={handleSave}
        onToggleCertExpanded={() => setEditCertExpanded(true)}
        onTogglePassphrase={handleTogglePassphrase}
        saveDisabled={passphraseLoading || !editName.trim() || !hasResolvedKeySource}
      />

      <PrivateKeyViewDialog
        entry={privateKeyEntry}
        value={privateKeyValue}
        loading={privateKeyLoading}
        loadError={privateKeyError}
        onOpenChange={(open) => {
          if (!open) {
            setPrivateKeyEntry(null);
            setPrivateKeyValue("");
            setPrivateKeyError(false);
            setPrivateKeyLoading(false);
          }
        }}
      />

      <KeyDeleteDialog
        entry={deletingKey}
        onOpenChange={(open) => !open && setDeletingKey(null)}
        onCancel={() => setDeletingKey(null)}
        onConfirm={handleDeleteConfirm}
      />
    </div>
  );
}
