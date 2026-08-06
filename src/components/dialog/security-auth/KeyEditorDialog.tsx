import { ClipboardPaste, Eye, EyeOff, FileText } from "lucide-react";
import { useTranslation } from "react-i18next";
import { MdFolderOpen } from "react-icons/md";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";

export type KeyMaterialMode = "content" | "file";

interface KeyMaterialInputProps {
  contentValue: string;
  fileName: string;
  filePath: string;
  hasStoredData: boolean;
  isEditing: boolean;
  mode: KeyMaterialMode;
  onContentChange: (value: string) => void;
  onModeChange: (value: KeyMaterialMode) => void;
  onPathChange: (value: string) => void;
  onPickFile: () => Promise<void>;
  pathPlaceholder: string;
  placeholder: string;
  storedLabel: string;
  title: string;
}

interface KeyEditorDialogProps {
  certExpanded: boolean;
  editCertData: string;
  editCertFileName: string;
  editCertFilePath: string;
  editHasCertData: boolean;
  editHasKeyData: boolean;
  editKeyData: string;
  editKeyFileName: string;
  editKeyFilePath: string;
  editCertInputMode: KeyMaterialMode;
  editKeyInputMode: KeyMaterialMode;
  editName: string;
  editPassphrase: string;
  editShowPassphrase: boolean;
  isEditing: boolean;
  open: boolean;
  passphraseLoading: boolean;
  onCancel: () => void;
  onCertDataChange: (value: string) => void;
  onCertInputModeChange: (value: KeyMaterialMode) => void;
  onCertPathChange: (value: string) => void;
  onKeyDataChange: (value: string) => void;
  onKeyInputModeChange: (value: KeyMaterialMode) => void;
  onKeyPathChange: (value: string) => void;
  onNameChange: (value: string) => void;
  onPassphraseChange: (value: string) => void;
  onPickCertFile: () => Promise<void>;
  onPickFile: () => Promise<void>;
  onSave: () => void;
  onToggleCertExpanded: () => void;
  onTogglePassphrase: () => void;
  saveDisabled: boolean;
}

function getPathFileName(path: string) {
  const normalized = path.replace(/\\/g, "/").trim();
  if (!normalized) return "";
  const parts = normalized.split("/");
  return parts[parts.length - 1] || normalized;
}

function KeyMaterialInput({
  contentValue,
  fileName,
  filePath,
  hasStoredData,
  isEditing,
  mode,
  onContentChange,
  onModeChange,
  onPathChange,
  onPickFile,
  pathPlaceholder,
  placeholder,
  storedLabel,
  title,
}: KeyMaterialInputProps) {
  const { t } = useTranslation();
  const hasContent = contentValue.trim().length > 0;
  const hasPath = filePath.trim().length > 0;
  const status = hasContent
    ? t("settings.keyContentPasted")
    : hasPath
      ? fileName || getPathFileName(filePath)
      : isEditing && hasStoredData
        ? storedLabel
        : t("settings.notConfigured");

  return (
    <div className="space-y-1.5 rounded-md border bg-background/40 p-2">
      <div className="flex items-center justify-between gap-2">
        <Label className="min-w-0 truncate text-xs font-medium">{title}</Label>
        <Tabs
          value={mode}
          onValueChange={(value) => onModeChange(value as KeyMaterialMode)}
          className="shrink-0"
        >
          <TabsList className="grid h-7 w-32 grid-cols-2">
            <TabsTrigger value="content" className="h-6 px-1.5 text-[0.6875rem]">
              <ClipboardPaste className="h-3 w-3" />
              {t("settings.keyInputContentMode")}
            </TabsTrigger>
            <TabsTrigger value="file" className="h-6 px-1.5 text-[0.6875rem]">
              <FileText className="h-3 w-3" />
              {t("settings.keyInputFileMode")}
            </TabsTrigger>
          </TabsList>
        </Tabs>
      </div>
      {mode === "content" ? (
        <Textarea
          value={contentValue}
          onChange={(event) => onContentChange(event.target.value)}
          placeholder={placeholder}
          className="terminal-scroll min-h-24 max-h-40 resize-y font-mono text-[0.6875rem] leading-4"
        />
      ) : (
        <div className="flex items-center overflow-hidden rounded-md border bg-transparent">
          <Input
            value={filePath}
            onChange={(event) => onPathChange(event.target.value)}
            placeholder={pathPlaceholder}
            className="h-8 min-w-0 flex-1 border-0 text-xs shadow-none focus-visible:ring-0"
          />
          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="h-8 rounded-none border-l px-3"
            onClick={() => {
              void onPickFile();
            }}
            aria-label={pathPlaceholder}
          >
            <MdFolderOpen className="text-base" />
          </Button>
        </div>
      )}
      <div className="truncate text-[0.6875rem] text-muted-foreground">{status}</div>
    </div>
  );
}

export function KeyEditorDialog({
  certExpanded,
  editCertData,
  editCertFileName,
  editCertFilePath,
  editHasCertData,
  editHasKeyData,
  editKeyData,
  editKeyFileName,
  editKeyFilePath,
  editCertInputMode,
  editKeyInputMode,
  editName,
  editPassphrase,
  editShowPassphrase,
  isEditing,
  open,
  passphraseLoading,
  onCancel,
  onCertDataChange,
  onCertInputModeChange,
  onCertPathChange,
  onKeyDataChange,
  onKeyInputModeChange,
  onKeyPathChange,
  onNameChange,
  onPassphraseChange,
  onPickCertFile,
  onPickFile,
  onSave,
  onToggleCertExpanded,
  onTogglePassphrase,
  saveDisabled,
}: KeyEditorDialogProps) {
  const { t } = useTranslation();

  return (
    <Dialog disablePointerDismissal open={open} onOpenChange={(nextOpen) => !nextOpen && onCancel()}>
      <DialogContent className="w-[min(720px,calc(100vw-2rem))] max-w-none gap-0 overflow-hidden p-0">
        <DialogHeader className="border-b px-5 py-3 pr-12">
          <DialogTitle className="text-sm">
            {isEditing ? t("settings.keyEditorEditTitle") : t("settings.keyEditorAddTitle")}
          </DialogTitle>
          <DialogDescription>{t("settings.keyEditorDescription")}</DialogDescription>
        </DialogHeader>
        <div className="terminal-scroll max-h-[calc(100vh-12rem)] overflow-y-auto px-5 py-4">
          <div className="space-y-2.5">
            <Input
              placeholder={t("settings.keyNamePlaceholder")}
              className="h-8 text-xs"
              value={editName}
              onChange={(event) => onNameChange(event.target.value)}
              autoFocus
            />
            <KeyMaterialInput
              contentValue={editKeyData}
              fileName={editKeyFileName}
              filePath={editKeyFilePath}
              hasStoredData={editHasKeyData}
              isEditing={isEditing}
              mode={editKeyInputMode}
              onContentChange={onKeyDataChange}
              onModeChange={onKeyInputModeChange}
              onPathChange={onKeyPathChange}
              onPickFile={onPickFile}
              pathPlaceholder={t("settings.keyFilePathPlaceholder")}
              placeholder={t("settings.keyContentPlaceholder")}
              storedLabel={t("settings.storedPrivateKey")}
              title={t("settings.privateKey")}
            />
            {certExpanded ? (
              <KeyMaterialInput
                contentValue={editCertData}
                fileName={editCertFileName}
                filePath={editCertFilePath}
                hasStoredData={editHasCertData}
                isEditing={isEditing}
                mode={editCertInputMode}
                onContentChange={onCertDataChange}
                onModeChange={onCertInputModeChange}
                onPathChange={onCertPathChange}
                onPickFile={onPickCertFile}
                pathPlaceholder={t("settings.certFilePathPlaceholder")}
                placeholder={t("settings.certContentPlaceholder")}
                storedLabel={t("settings.storedCertificate")}
                title={t("settings.openSshUserCertificate")}
              />
            ) : (
              <Button
                type="button"
                variant="outline"
                size="sm"
                className="h-8 w-full justify-start px-2 text-xs text-muted-foreground"
                onClick={onToggleCertExpanded}
              >
                <FileText className="h-3.5 w-3.5" />
                {t("settings.addCertificate")}
              </Button>
            )}
            <div className="relative">
              <Input
                type={editShowPassphrase ? "text" : "password"}
                placeholder={passphraseLoading ? t("common.loading") : t("settings.passphrase")}
                className="h-8 pr-8 text-xs"
                value={editPassphrase}
                onChange={(event) => onPassphraseChange(event.target.value)}
                disabled={passphraseLoading}
              />
              <Button
                type="button"
                variant="ghost"
                size="icon"
                className="absolute top-0.5 right-0.5 h-7 w-7 text-muted-foreground hover:text-foreground"
                onClick={onTogglePassphrase}
                disabled={passphraseLoading}
                aria-label={
                  editShowPassphrase ? t("settings.hidePassphrase") : t("settings.showPassphrase")
                }
              >
                {editShowPassphrase ? (
                  <EyeOff className="h-3.5 w-3.5" />
                ) : (
                  <Eye className="h-3.5 w-3.5" />
                )}
              </Button>
            </div>
          </div>
        </div>
        <DialogFooter className="border-t px-5 py-3">
          <Button variant="outline" size="sm" className="h-7 px-3 text-xs" onClick={onCancel}>
            {t("common.cancel")}
          </Button>
          <Button
            size="sm"
            className="h-7 px-3 text-xs"
            onClick={onSave}
            disabled={passphraseLoading || saveDisabled}
          >
            {t("common.save")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
