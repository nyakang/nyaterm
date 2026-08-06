import { open as openFileDialog, save as saveFileDialog } from "@tauri-apps/plugin-dialog";
import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  MdCheck,
  MdContentCopy,
  MdDeleteOutline,
  MdDownload,
  MdPalette,
  MdUpload,
} from "react-icons/md";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { ScrollArea } from "@/components/ui/scroll-area";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import {
  ALL_THEME_COLOR_FIELDS,
  cloneThemeAsCustom,
  getThemeColor,
  isHexColor,
  normalizeImportedTheme,
  setThemeColor,
  structuredCloneTheme,
  UI_THEME_COLOR_FIELDS,
  validateTheme,
} from "@/lib/customThemes";
import { invoke } from "@/lib/invoke";
import { logger } from "@/lib/logger";
import { DEFAULT_THEME_ID, isBuiltinThemeId, resolveTheme, type Theme } from "@/lib/themes";
import type { AppearanceSettings } from "@/types/global";

interface ThemeDesignerDialogProps {
  open: boolean;
  onClose: () => void;
  appearance: AppearanceSettings;
  availableThemes: Theme[];
  updateAppearance: (patch: Partial<AppearanceSettings>) => void;
}

function hexToRgb(hex: string) {
  if (!isHexColor(hex)) return null;
  const value = hex.slice(1);
  return {
    r: parseInt(value.slice(0, 2), 16) / 255,
    g: parseInt(value.slice(2, 4), 16) / 255,
    b: parseInt(value.slice(4, 6), 16) / 255,
  };
}

function channelLuminance(value: number) {
  return value <= 0.03928 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
}

function contrastRatio(left: string, right: string) {
  const a = hexToRgb(left);
  const b = hexToRgb(right);
  if (!a || !b) return null;
  const leftLum =
    0.2126 * channelLuminance(a.r) +
    0.7152 * channelLuminance(a.g) +
    0.0722 * channelLuminance(a.b);
  const rightLum =
    0.2126 * channelLuminance(b.r) +
    0.7152 * channelLuminance(b.g) +
    0.0722 * channelLuminance(b.b);
  const lighter = Math.max(leftLum, rightLum);
  const darker = Math.min(leftLum, rightLum);
  return (lighter + 0.05) / (darker + 0.05);
}

function terminalAnsiFields() {
  return ALL_THEME_COLOR_FIELDS.filter((field) => field.path.startsWith("terminal.")).slice(7);
}

export function ThemeDesignerDialog({
  open,
  onClose,
  appearance,
  availableThemes,
  updateAppearance,
}: ThemeDesignerDialogProps) {
  const { t } = useTranslation();
  const customThemes = appearance.custom_themes ?? [];
  const [sourceThemeId, setSourceThemeId] = useState(appearance.theme || DEFAULT_THEME_ID);
  const [selectedThemeId, setSelectedThemeId] = useState<string | null>(null);
  const [draft, setDraft] = useState<Theme | null>(null);
  const [activeTab, setActiveTab] = useState("ui");

  const sourceTheme = resolveTheme(sourceThemeId, customThemes);
  const validationErrors = useMemo(() => (draft ? validateTheme(draft) : []), [draft]);
  const uiContrast = draft ? contrastRatio(draft.colors.text, draft.colors.bg) : null;
  const terminalContrast = draft
    ? contrastRatio(draft.colors.terminal.foreground, draft.colors.terminal.background)
    : null;

  useEffect(() => {
    if (!open) return;
    const initial = customThemes[0];
    if (initial) {
      setSelectedThemeId(initial.id);
      setDraft(structuredCloneTheme(initial));
    } else {
      const next = cloneThemeAsCustom(resolveTheme(appearance.theme, customThemes));
      setSelectedThemeId(next.id);
      setDraft(next);
    }
    setSourceThemeId(appearance.theme || DEFAULT_THEME_ID);
  }, [appearance.theme, customThemes, open]);

  function patchDraft(patch: Partial<Theme>) {
    setDraft((current) => (current ? { ...current, ...patch } : current));
  }

  function patchColor(path: Parameters<typeof setThemeColor>[1], value: string) {
    setDraft((current) => (current ? setThemeColor(current, path, value) : current));
  }

  function saveDraft() {
    if (!draft) return false;
    const errors = validateTheme(draft);
    if (errors.length > 0) {
      toast.error(t("settings.themeDesignerValidationFailed"));
      return false;
    }
    const nextTheme = structuredCloneTheme(draft);
    const nextThemes = customThemes.some((theme) => theme.id === nextTheme.id)
      ? customThemes.map((theme) => (theme.id === nextTheme.id ? nextTheme : theme))
      : [...customThemes, nextTheme];
    updateAppearance({ custom_themes: nextThemes });
    setSelectedThemeId(nextTheme.id);
    toast.success(t("settings.themeDesignerSaved"));
    return true;
  }

  function createFromSource() {
    const next = cloneThemeAsCustom(sourceTheme);
    setSelectedThemeId(next.id);
    setDraft(next);
    setActiveTab("ui");
  }

  function selectCustomTheme(id: string) {
    const next = customThemes.find((theme) => theme.id === id);
    if (!next) return;
    setSelectedThemeId(id);
    setDraft(structuredCloneTheme(next));
  }

  function deleteSelected() {
    if (!draft || !customThemes.some((theme) => theme.id === draft.id)) return;
    const nextThemes = customThemes.filter((theme) => theme.id !== draft.id);
    const patch: Partial<AppearanceSettings> = { custom_themes: nextThemes };
    if (appearance.theme === draft.id) patch.theme = DEFAULT_THEME_ID;
    if (appearance.terminal_theme === draft.id) patch.terminal_theme = null;
    updateAppearance(patch);
    const next = nextThemes[0]
      ? structuredCloneTheme(nextThemes[0])
      : cloneThemeAsCustom(sourceTheme);
    setSelectedThemeId(next.id);
    setDraft(next);
    toast.success(t("settings.themeDesignerDeleted"));
  }

  async function exportDraft() {
    if (!draft || !saveDraft()) return;
    const outputPath = await saveFileDialog({
      defaultPath: "nyaterm-theme.json",
      filters: [{ name: "NyaTerm Theme", extensions: ["json"] }],
    });
    if (!outputPath) return;

    try {
      await invoke("write_theme_file", { outputPath, theme: draft });
      toast.success(t("settings.themeDesignerExportSuccess"));
    } catch (error) {
      logger.error({
        domain: "settings.persistence",
        event: "theme.export_failed",
        message: "Export theme failed",
        error,
      });
      toast.error(t("settings.themeDesignerExportFailed", { error: String(error) }));
    }
  }

  async function importTheme() {
    const filePath = await openFileDialog({
      multiple: false,
      filters: [{ name: "NyaTerm Theme", extensions: ["json"] }],
    });
    if (!filePath || Array.isArray(filePath)) return;

    try {
      const imported = await invoke<Theme>("read_theme_file", { filePath });
      const existingIds = new Set([...availableThemes.map((theme) => theme.id)]);
      const next = normalizeImportedTheme(imported, existingIds);
      const errors = validateTheme(next);
      if (errors.length > 0 || isBuiltinThemeId(next.id)) {
        toast.error(t("settings.themeDesignerImportInvalid"));
        return;
      }
      updateAppearance({ custom_themes: [...customThemes, next] });
      setSelectedThemeId(next.id);
      setDraft(structuredCloneTheme(next));
      toast.success(t("settings.themeDesignerImportSuccess"));
    } catch (error) {
      logger.error({
        domain: "settings.persistence",
        event: "theme.import_failed",
        message: "Import theme failed",
        error,
      });
      toast.error(t("settings.themeDesignerImportFailed", { error: String(error) }));
    }
  }

  const canDelete = !!draft && customThemes.some((theme) => theme.id === draft.id);
  const hasValidationErrors = validationErrors.length > 0;

  return (
    <Dialog disablePointerDismissal open={open} onOpenChange={(value) => !value && onClose()}>
      <DialogContent className="flex h-[min(760px,calc(100vh-2rem))] w-[calc(100vw-2rem)] max-w-[calc(100vw-2rem)] flex-col gap-0 overflow-hidden p-0 sm:max-w-[calc(100vw-2rem)] xl:w-[1120px] xl:max-w-[1120px]">
        <DialogHeader className="border-b px-5 py-4">
          <DialogTitle className="flex items-center gap-2 text-sm">
            <MdPalette className="text-base text-[var(--df-primary)]" />
            {t("settings.themeDesignerTitle")}
          </DialogTitle>
          <DialogDescription className="text-xs">
            {t("settings.themeDesignerDesc")}
          </DialogDescription>
        </DialogHeader>

        <div className="grid min-h-0 flex-1 grid-rows-[auto_minmax(0,1fr)] md:grid-cols-[280px_minmax(0,1fr)] md:grid-rows-1">
          <aside className="flex min-h-0 flex-col border-b bg-muted/20 md:border-r md:border-b-0">
            <div className="space-y-3 border-b p-4">
              <Label className="text-xs">{t("settings.themeDesignerCopyFrom")}</Label>
              <Select value={sourceThemeId} onValueChange={setSourceThemeId}>
                <SelectTrigger className="w-full text-xs">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {availableThemes.map((theme) => (
                    <SelectItem key={theme.id} value={theme.id}>
                      {theme.name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              <Button size="sm" className="w-full" onClick={createFromSource}>
                <MdContentCopy />
                {t("settings.themeDesignerCopyTheme")}
              </Button>
              <div className="grid grid-cols-2 gap-2">
                <Button size="sm" variant="outline" onClick={importTheme}>
                  <MdUpload />
                  {t("settings.themeDesignerImport")}
                </Button>
                <Button size="sm" variant="outline" onClick={exportDraft} disabled={!draft}>
                  <MdDownload />
                  {t("settings.themeDesignerExport")}
                </Button>
              </div>
            </div>

            <ScrollArea className="min-h-0 flex-1">
              <div className="space-y-1 p-3">
                {customThemes.length === 0 && (
                  <div className="rounded-md border border-dashed p-3 text-xs text-muted-foreground">
                    {t("settings.themeDesignerEmpty")}
                  </div>
                )}
                {customThemes.map((theme) => (
                  <button
                    key={theme.id}
                    type="button"
                    className="flex w-full items-center gap-2 rounded-md px-2 py-2 text-left text-xs transition-colors hover:bg-accent"
                    style={{
                      backgroundColor:
                        selectedThemeId === theme.id ? "var(--df-bg-hover)" : undefined,
                    }}
                    onClick={() => selectCustomTheme(theme.id)}
                  >
                    <span
                      className="h-4 w-4 shrink-0 rounded-sm border"
                      style={{ backgroundColor: theme.swatch, borderColor: "var(--df-border)" }}
                    />
                    <span className="min-w-0 flex-1 truncate">{theme.name}</span>
                    {(appearance.theme === theme.id || appearance.terminal_theme === theme.id) && (
                      <MdCheck className="shrink-0 text-[var(--df-success)]" />
                    )}
                  </button>
                ))}
              </div>
            </ScrollArea>
          </aside>

          <section className="flex min-h-0 flex-col">
            <ScrollArea className="min-h-0 flex-1">
              <div className="space-y-5 p-5">
                {draft && (
                  <>
                    <div className="grid gap-3 md:grid-cols-[minmax(0,1fr)_120px_96px]">
                      <div className="space-y-1.5">
                        <Label className="text-xs">{t("settings.themeDesignerName")}</Label>
                        <Input
                          value={draft.name}
                          className="text-sm"
                          onChange={(event) => patchDraft({ name: event.target.value })}
                        />
                      </div>
                      <div className="space-y-1.5">
                        <Label className="text-xs">{t("settings.themeDesignerLabel")}</Label>
                        <Input
                          value={draft.label}
                          className="text-sm"
                          maxLength={14}
                          onChange={(event) => patchDraft({ label: event.target.value })}
                        />
                      </div>
                      <div className="space-y-1.5">
                        <Label className="text-xs">{t("settings.themeDesignerSwatch")}</Label>
                        <Input
                          type="color"
                          value={isHexColor(draft.swatch) ? draft.swatch : "#000000"}
                          className="h-9 p-1"
                          onChange={(event) => patchDraft({ swatch: event.target.value })}
                        />
                      </div>
                    </div>

                    <ThemePreview theme={draft} />

                    <div className="space-y-2 text-xs">
                      {uiContrast !== null && uiContrast < 4.5 && (
                        <div className="rounded-md border border-[var(--df-warning)]/50 p-2 text-[var(--df-warning)]">
                          {t("settings.themeDesignerUiContrastWarning")}
                        </div>
                      )}
                      {terminalContrast !== null && terminalContrast < 4.5 && (
                        <div className="rounded-md border border-[var(--df-warning)]/50 p-2 text-[var(--df-warning)]">
                          {t("settings.themeDesignerTerminalContrastWarning")}
                        </div>
                      )}
                      {hasValidationErrors && (
                        <div className="rounded-md border border-[var(--df-danger)]/50 p-2 text-[var(--df-danger)]">
                          {t("settings.themeDesignerValidationHint", {
                            count: validationErrors.length,
                          })}
                        </div>
                      )}
                    </div>

                    <Tabs value={activeTab} onValueChange={setActiveTab}>
                      <TabsList>
                        <TabsTrigger value="ui">{t("settings.themeDesignerUiColors")}</TabsTrigger>
                        <TabsTrigger value="terminal">
                          {t("settings.themeDesignerTerminalColors")}
                        </TabsTrigger>
                      </TabsList>
                      <TabsContent value="ui" className="mt-4">
                        <ColorFieldGrid
                          fields={UI_THEME_COLOR_FIELDS}
                          draft={draft}
                          onChange={patchColor}
                        />
                      </TabsContent>
                      <TabsContent value="terminal" className="mt-4">
                        <ColorFieldGrid
                          fields={ALL_THEME_COLOR_FIELDS.filter((field) =>
                            field.path.startsWith("terminal."),
                          )}
                          draft={draft}
                          onChange={patchColor}
                        />
                      </TabsContent>
                    </Tabs>
                  </>
                )}
              </div>
            </ScrollArea>

            <div className="flex flex-col-reverse gap-2 border-t bg-background/95 p-4 sm:flex-row sm:items-center sm:justify-between">
              <Button variant="destructive" onClick={deleteSelected} disabled={!canDelete}>
                <MdDeleteOutline />
                {t("settings.themeDesignerDelete")}
              </Button>
              <Button onClick={() => saveDraft()} disabled={!draft || hasValidationErrors}>
                {t("settings.themeDesignerSave")}
              </Button>
            </div>
          </section>
        </div>
      </DialogContent>
    </Dialog>
  );
}

function ColorFieldGrid({
  fields,
  draft,
  onChange,
}: {
  fields: readonly (typeof ALL_THEME_COLOR_FIELDS)[number][];
  draft: Theme;
  onChange: (path: Parameters<typeof setThemeColor>[1], value: string) => void;
}) {
  const { t } = useTranslation();
  return (
    <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-3">
      {fields.map((field) => {
        const value = getThemeColor(draft, field.path);
        return (
          <div key={field.path} className="space-y-1.5">
            <Label className="text-xs">{t(field.labelKey)}</Label>
            <div className="flex items-center gap-2">
              {!field.cssColor && (
                <Input
                  type="color"
                  value={isHexColor(value) ? value : "#000000"}
                  className="h-9 w-12 shrink-0 p-1"
                  onChange={(event) => onChange(field.path, event.target.value)}
                />
              )}
              <Input
                value={value}
                className="min-w-0 flex-1 font-mono text-xs"
                onChange={(event) => onChange(field.path, event.target.value)}
              />
            </div>
          </div>
        );
      })}
    </div>
  );
}

function ThemePreview({ theme }: { theme: Theme }) {
  const { t } = useTranslation();
  return (
    <div className="space-y-3">
      <div
        className="rounded-md border p-3"
        style={{
          backgroundColor: theme.colors.bg,
          borderColor: theme.colors.border,
          color: theme.colors.text,
        }}
      >
        <div className="mb-3 flex items-center justify-between">
          <span className="text-xs font-semibold">{theme.name}</span>
          <span
            className="rounded-sm px-2 py-1 text-[0.65rem]"
            style={{ backgroundColor: theme.colors.primary, color: theme.colors.onPrimary }}
          >
            {theme.label}
          </span>
        </div>
        <div className="flex gap-1">
          <span
            className="h-2 flex-1 rounded-sm"
            style={{ backgroundColor: theme.colors.danger }}
          />
          <span
            className="h-2 flex-1 rounded-sm"
            style={{ backgroundColor: theme.colors.warning }}
          />
          <span
            className="h-2 flex-1 rounded-sm"
            style={{ backgroundColor: theme.colors.success }}
          />
          <span className="h-2 flex-1 rounded-sm" style={{ backgroundColor: theme.colors.link }} />
        </div>
      </div>

      <div
        className="rounded-md border p-3 font-mono text-xs"
        style={{
          backgroundColor: theme.colors.terminal.background,
          borderColor: theme.colors.border,
          color: theme.colors.terminal.foreground,
        }}
      >
        <div style={{ color: theme.colors.terminal.brightGreen }}>$ pnpm build</div>
        <div>
          <span style={{ color: theme.colors.terminal.brightBlue }}>info</span>{" "}
          {t("settings.themeDesignerPreviewBuild")}
        </div>
        <div>
          <span style={{ color: theme.colors.terminal.brightYellow }}>warn</span>{" "}
          {t("settings.themeDesignerPreviewWarn")}
        </div>
      </div>

      <div className="grid grid-cols-8 gap-1">
        {terminalAnsiFields().map((field) => (
          <span
            key={field.path}
            className="h-5 rounded-sm border"
            title={field.path}
            style={{
              backgroundColor: getThemeColor(theme, field.path),
              borderColor: theme.colors.border,
            }}
          />
        ))}
      </div>
    </div>
  );
}
