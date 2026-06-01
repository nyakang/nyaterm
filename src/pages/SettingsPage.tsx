import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  type ComponentType,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import { FiBook } from "react-icons/fi";
import {
  MdAutoAwesome,
  MdBackup,
  MdDashboard,
  MdDns,
  MdKeyboard,
  MdKeyboardArrowRight,
  MdMouse,
  MdPalette,
  MdSearch,
  MdSecurity,
  MdSettings,
  MdSwapHoriz,
  MdTerminal,
  MdTranslate,
} from "react-icons/md";
import { TbCubeSpark } from "react-icons/tb";
import { toast } from "sonner";
import ChildWindowHeader from "@/components/layout/ChildWindowHeader";
import { AiGeneralTab, AiModelsTab, AiRulesTab } from "@/components/settings/AiTab";
import { AppearanceTab } from "@/components/settings/AppearanceTab";
import { GeneralTab } from "@/components/settings/GeneralTab";
import { InteractionTab } from "@/components/settings/InteractionTab";
import { KeyboardShortcutsTab } from "@/components/settings/KeyboardShortcutsTab";
import { SearchTab } from "@/components/settings/SearchTab";
import { SecurityTab } from "@/components/settings/SecurityTab";
import { SyncBackupTab } from "@/components/settings/SyncBackupTab";
import { TerminalTab } from "@/components/settings/TerminalTab";
import { TransferTab } from "@/components/settings/TransferTab";
import { TranslationTab } from "@/components/settings/TranslationTab";
import { ActionButton, ActionFooter } from "@/components/ui/action-footer";
import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { AppContext, useApp } from "@/context/AppContext";
import { SettingsDraftContext } from "@/context/SettingsDraftContext";
import { type CloudSyncValidationCode, getCloudSyncValidationErrors } from "@/lib/cloudSync";
import { getErrorMessage } from "@/lib/errors";
import { invoke } from "@/lib/invoke";
import type { AppSettings, UiConfig } from "@/types/global";

type SettingsTabConfig = {
  id: string;
  label: string;
  icon: string;
  Component?: ComponentType;
};

function getCloudSyncValidationMessage(
  code: CloudSyncValidationCode,
  t: ReturnType<typeof useTranslation>["t"],
) {
  switch (code) {
    case "webdavEndpointRequired":
      return t("settings.webdavEndpointRequired");
    case "s3EndpointRequired":
      return t("settings.s3EndpointRequired");
    case "s3BucketRequired":
      return t("settings.s3BucketRequired");
    case "s3CredentialsIncomplete":
      return t("settings.s3CredentialsIncomplete");
  }
}

export default function SettingsPage() {
  const { t } = useTranslation();
  const app = useApp();
  const committedSettings = app.appSettings;

  const params = new URLSearchParams(window.location.search);
  const requestedInitialTab = params.get("tab") || "general";
  const initialTab = requestedInitialTab === "ai" ? "ai-general" : requestedInitialTab;
  const [activeTab, setActiveTab] = useState(initialTab);
  const [draftSettings, setDraftSettings] = useState<AppSettings>(committedSettings);
  const [isSaving, setIsSaving] = useState(false);

  const scrollContainerRef = useRef<HTMLDivElement>(null);
  const scrollStates = useRef<Record<string, number>>({});

  useLayoutEffect(() => {
    if (scrollContainerRef.current) {
      scrollContainerRef.current.scrollTop = scrollStates.current[activeTab] || 0;
    }
  }, [activeTab]);

  useEffect(() => {
    const unlisten = listen<{ tab: string }>("settings-open-tab", ({ payload }) => {
      setActiveTab(payload.tab === "ai" ? "ai-general" : payload.tab);
    });

    return () => {
      unlisten.then((dispose) => dispose());
    };
  }, []);

  type SettingsCategory = {
    id: string;
    label: string;
    icon: string;
    items: string[];
  };

  const categories: SettingsCategory[] = useMemo(
    () => [
      {
        id: "workspace",
        label: t("settings.groupWorkspace"),
        icon: "dashboard",
        items: ["general", "appearance", "interaction", "keybindings"],
      },
      {
        id: "terminal_session",
        label: t("settings.groupTerminalSession"),
        icon: "terminal",
        items: ["terminal-general", "search", "translation"],
      },
      {
        id: "ai_group",
        label: t("ai.title"),
        icon: "ai",
        items: ["ai-general", "ai-models", "ai-rules"],
      },
      {
        id: "transfer_group",
        label: t("settings.groupTransfer"),
        icon: "swap_horiz",
        items: ["transfer"],
      },
      {
        id: "security_group",
        label: t("settings.groupSecurity"),
        icon: "security",
        items: ["security"],
      },
      {
        id: "syncBackup_group",
        label: t("settings.groupSyncBackup"),
        icon: "backup",
        items: ["syncBackup"],
      },
    ],
    [t],
  );

  const [expandedGroups, setExpandedGroups] = useState<Record<string, boolean>>(() => {
    const initialGroup = categories.find((c) => c.items.includes(initialTab))?.id;
    return initialGroup ? { [initialGroup]: true } : {};
  });

  const toggleGroup = useCallback((groupId: string) => {
    setExpandedGroups((prev) => ({ ...prev, [groupId]: !prev[groupId] }));
  }, []);

  const committedSerialized = useMemo(() => JSON.stringify(committedSettings), [committedSettings]);
  const draftSerialized = useMemo(() => JSON.stringify(draftSettings), [draftSettings]);
  const isDirty = committedSerialized !== draftSerialized;

  useEffect(() => {
    if (!isDirty) {
      setDraftSettings(committedSettings);
    }
  }, [committedSettings, isDirty]);

  const updateDraftSettings = useCallback(
    (updates: Partial<AppSettings> | ((prev: AppSettings) => Partial<AppSettings>)) => {
      setDraftSettings((prev) => {
        const nextUpdates = typeof updates === "function" ? updates(prev) : updates;
        return { ...prev, ...nextUpdates };
      });
    },
    [],
  );

  const updateDraftUi = useCallback(
    (updates: Partial<UiConfig> | ((prev: UiConfig) => Partial<UiConfig>)) => {
      updateDraftSettings((prev) => {
        const nextUpdates = typeof updates === "function" ? updates(prev.ui) : updates;
        return { ui: { ...prev.ui, ...nextUpdates } };
      });
    },
    [updateDraftSettings],
  );

  const nestedAppContextValue = useMemo(
    () => ({
      ...app,
      appSettings: draftSettings,
      updateAppSettings: updateDraftSettings,
      updateUi: updateDraftUi,
      replaceAppSettings: app.replaceAppSettings,
    }),
    [app, draftSettings, updateDraftSettings, updateDraftUi],
  );

  const tabs: SettingsTabConfig[] = [
    { id: "general", label: t("settings.general"), icon: "settings", Component: GeneralTab },
    {
      id: "appearance",
      label: t("settings.appearance"),
      icon: "palette",
      Component: AppearanceTab,
    },
    { id: "transfer", label: t("settings.transfer"), icon: "swap_horiz", Component: TransferTab },
    { id: "search", label: t("settings.search"), icon: "search", Component: SearchTab },
    {
      id: "translation",
      label: t("settings.translation"),
      icon: "translate",
      Component: TranslationTab,
    },
    { id: "security", label: t("settings.security"), icon: "security", Component: SecurityTab },
    { id: "syncBackup", label: t("settings.syncBackup"), icon: "backup" },
    {
      id: "terminal-general",
      label: t("settings.general"),
      icon: "settings",
      Component: TerminalTab,
    },
    {
      id: "interaction",
      label: t("settings.interaction"),
      icon: "mouse",
      Component: InteractionTab,
    },
    {
      id: "keybindings",
      label: t("settings.keybindings"),
      icon: "keyboard",
      Component: KeyboardShortcutsTab,
    },
    {
      id: "ai-general",
      label: t("ai.general"),
      icon: "settings",
      Component: AiGeneralTab,
    },
    {
      id: "ai-models",
      label: t("ai.models"),
      icon: "model",
      Component: AiModelsTab,
    },
    {
      id: "ai-rules",
      label: t("ai.rules"),
      icon: "rules",
      Component: AiRulesTab,
    },
  ];

  const activeTabConfig = tabs.find((tab) => tab.id === activeTab);
  const ActiveComponent = activeTabConfig?.Component;

  const iconMap: Record<string, React.ElementType> = {
    backup: MdBackup,
    dashboard: MdDashboard,
    dns: MdDns,
    settings: MdSettings,
    palette: MdPalette,
    swap_horiz: MdSwapHoriz,
    search: MdSearch,
    translate: MdTranslate,
    security: MdSecurity,
    terminal: MdTerminal,
    mouse: MdMouse,
    keyboard: MdKeyboard,
    ai: MdAutoAwesome,
    model: TbCubeSpark,
    rules: FiBook,
  };

  function DynamicIcon({ name, className }: { name: string; className?: string }) {
    const Icon = iconMap[name];
    if (!Icon) return null;
    return <Icon className={className} />;
  }

  const getDraftSaveBlockState = useCallback(() => {
    if (draftSettings.security.master_password === "") {
      return {
        message: t("settings.masterPasswordEmptyDesc"),
        targetTab: "security" as const,
      };
    }

    if (!draftSettings.cloud_sync.enabled) {
      return null;
    }

    if (!draftSettings.security.master_password) {
      return {
        message: t("settings.masterPasswordRequiredDesc"),
        targetTab: "security" as const,
      };
    }

    const errors = getCloudSyncValidationErrors(draftSettings.cloud_sync);
    if (errors.length === 0) {
      return null;
    }

    return {
      message: getCloudSyncValidationMessage(errors[0], t),
      targetTab: "syncBackup" as const,
    };
  }, [draftSettings.cloud_sync, draftSettings.security.master_password, t]);

  const saveBlockState = useMemo(
    () => (isDirty ? getDraftSaveBlockState() : null),
    [getDraftSaveBlockState, isDirty],
  );
  const saveBlockedMessage = saveBlockState?.message ?? null;

  const saveDraftSettings = useCallback(
    async (closeAfterSave: boolean) => {
      const validationState = getDraftSaveBlockState();
      if (validationState) {
        toast.error(validationState.message);
        return;
      }

      setIsSaving(true);
      try {
        await invoke("save_app_settings", { settings: draftSettings });
        const nextSettings = await invoke<AppSettings>("get_app_settings");
        app.replaceAppSettings(nextSettings);
        setDraftSettings(nextSettings);

        if (closeAfterSave) {
          setIsSaving(false);
          void getCurrentWindow().close();
          return;
        } else {
          toast.success(t("settings.saveSuccess"));
        }
      } catch (error) {
        toast.error(getErrorMessage(error));
      } finally {
        setIsSaving(false);
      }
    },
    [app, draftSettings, getDraftSaveBlockState, t],
  );

  const handleCancel = useCallback(async () => {
    setDraftSettings(committedSettings);
    await getCurrentWindow().close();
  }, [committedSettings]);

  const handleConfirm = useCallback(async () => {
    if (!isDirty) {
      await getCurrentWindow().close();
      return;
    }
    await saveDraftSettings(true);
  }, [isDirty, saveDraftSettings]);

  const handleApply = useCallback(async () => {
    if (!isDirty || isSaving) {
      return;
    }
    await saveDraftSettings(false);
  }, [isDirty, isSaving, saveDraftSettings]);

  return (
    <div
      className="h-full min-h-0 flex flex-col overflow-hidden"
      style={{ fontFamily: committedSettings.appearance.ui_font_family }}
    >
      <ChildWindowHeader
        title={t("settings.title")}
        icon={<MdSettings className="text-base" />}
        onClose={() => {
          void handleCancel();
        }}
      />

      <SettingsDraftContext.Provider
        value={{
          committedSettings,
          isDirty,
          isSaving,
        }}
      >
        <AppContext.Provider value={nestedAppContextValue}>
          <div className="flex min-h-0 flex-1 overflow-hidden bg-background">
            <div className="flex w-14 shrink-0 flex-col border-r border-border/70 bg-muted/20 sm:w-48 lg:w-56">
              <div
                className="flex items-center justify-center gap-3 border-b border-border/70 px-3 py-4 sm:justify-start sm:px-4 sm:py-5"
                data-tauri-drag-region
              >
                <MdSettings className="shrink-0 text-2xl text-primary" />
                <h1 className="hidden text-lg font-semibold sm:block lg:text-xl">
                  {t("settings.title")}
                </h1>
              </div>
              <div className="min-h-0 flex-1 overflow-y-auto px-2 py-3 sm:px-3 sm:py-4">
                <div className="flex flex-col gap-2">
                  {categories.map((category) => {
                    const isExpanded = expandedGroups[category.id];
                    const hasMultiple = category.items.length > 1;

                    if (!hasMultiple) {
                      const tabId = category.items[0];
                      const tabItem = tabs.find((t) => t.id === tabId);
                      if (!tabItem) return null;
                      const isActive = activeTab === tabId;
                      return (
                        <Button
                          key={tabId}
                          variant="ghost"
                          onClick={() => setActiveTab(tabId)}
                          title={category.label}
                          className={`h-auto w-full justify-center gap-3 rounded-xl border px-2 py-2.5 text-sm font-semibold transition-colors sm:justify-start sm:px-3 ${
                            isActive
                              ? "border-primary/20 bg-primary/12 text-foreground shadow-xs hover:bg-primary/16"
                              : "border-transparent text-muted-foreground hover:border-border/70 hover:bg-background hover:text-foreground"
                          }`}
                        >
                          <DynamicIcon
                            name={tabItem.icon}
                            className={`shrink-0 text-[1.125rem] ${isActive ? "text-primary" : ""}`}
                          />
                          <span className="hidden truncate sm:inline">{category.label}</span>
                        </Button>
                      );
                    }

                    const activeChildCount = category.items.filter(
                      (item) => activeTab === item,
                    ).length;
                    const isGroupActive = activeChildCount > 0;

                    return (
                      <div key={category.id} className="flex flex-col">
                        <Button
                          variant="ghost"
                          onClick={() => toggleGroup(category.id)}
                          title={category.label}
                          className={`h-auto w-full justify-center sm:justify-between rounded-xl px-2 py-2.5 text-sm font-semibold transition-colors sm:px-3 ${
                            isGroupActive && !isExpanded
                              ? "border border-primary/20 bg-primary/5 text-foreground hover:bg-primary/10"
                              : "border border-transparent text-muted-foreground hover:border-border/70 hover:bg-background hover:text-foreground"
                          }`}
                        >
                          <div className="flex items-center justify-center sm:justify-start gap-3">
                            <DynamicIcon
                              name={category.icon}
                              className={`shrink-0 text-[1.125rem] ${isGroupActive && !isExpanded ? "text-primary" : ""}`}
                            />
                            <span className="hidden truncate sm:inline">{category.label}</span>
                          </div>
                          <MdKeyboardArrowRight
                            className={`hidden sm:block shrink-0 text-[1.125rem] transition-transform duration-200 ${
                              isExpanded ? "rotate-90" : ""
                            }`}
                          />
                        </Button>

                        <div
                          className={`relative flex flex-col gap-1 overflow-hidden transition-all duration-200 ${
                            isExpanded ? "max-h-64 opacity-100 mt-1" : "max-h-0 opacity-0"
                          } sm:ml-[1.3125rem] sm:pl-3 sm:border-l-2 sm:border-border/40`}
                        >
                          {category.items.map((tabId) => {
                            const tabItem = tabs.find((t) => t.id === tabId);
                            if (!tabItem) return null;
                            const isActive = activeTab === tabId;
                            return (
                              <Button
                                key={tabId}
                                variant="ghost"
                                onClick={() => setActiveTab(tabId)}
                                title={tabItem.label}
                                className={`h-auto w-full justify-center gap-3 rounded-lg border px-2 py-2 text-[0.85rem] font-medium transition-colors sm:justify-start sm:px-3 ${
                                  isActive
                                    ? "border-primary/20 bg-primary/12 text-foreground shadow-xs hover:bg-primary/16"
                                    : "border-transparent text-muted-foreground hover:border-border/70 hover:bg-background/50 hover:text-foreground"
                                }`}
                              >
                                <DynamicIcon
                                  name={tabItem.icon}
                                  className={`shrink-0 text-[1rem] ${isActive ? "text-primary" : "text-muted-foreground/70"}`}
                                />
                                <span className="hidden truncate sm:inline">{tabItem.label}</span>
                              </Button>
                            );
                          })}
                        </div>
                      </div>
                    );
                  })}
                </div>
              </div>
            </div>

            <div className="flex flex-1 min-h-0 min-w-0 flex-col">
              <div
                className="flex shrink-0 items-center justify-between border-b border-border/70 bg-background/90 px-4 py-4 backdrop-blur sm:px-6 sm:py-5"
                data-tauri-drag-region
              >
                <h3 className="text-lg font-semibold sm:text-2xl">{activeTabConfig?.label}</h3>
              </div>

              <div
                ref={scrollContainerRef}
                onScroll={(e) => {
                  scrollStates.current[activeTab] = e.currentTarget.scrollTop;
                }}
                className="flex-1 overflow-y-auto bg-gradient-to-b from-background via-background to-muted/10 px-4 py-4 sm:px-6 sm:py-6 lg:px-8 lg:py-8"
              >
                <div className="mx-auto w-full max-w-5xl space-y-5 text-base sm:space-y-6">
                  {activeTab === "syncBackup" ? (
                    <SyncBackupTab onNavigateSecurity={() => setActiveTab("security")} />
                  ) : ActiveComponent ? (
                    <ActiveComponent />
                  ) : null}
                </div>
              </div>

              <ActionFooter
                className="border-border/70 px-4 sm:px-6"
                leading={
                  saveBlockState ? (
                    <div className="flex flex-wrap items-center gap-2 text-sm text-amber-700 dark:text-amber-300">
                      <span className="min-w-0 flex-1">{saveBlockedMessage}</span>
                      {activeTab !== saveBlockState.targetTab ? (
                        <Button
                          variant="ghost"
                          size="sm"
                          className="shrink-0"
                          onClick={() => setActiveTab(saveBlockState.targetTab)}
                        >
                          {saveBlockState.targetTab === "security"
                            ? t("settings.security")
                            : t("settings.syncBackup")}
                        </Button>
                      ) : null}
                    </div>
                  ) : null
                }
              >
                <ActionButton
                  variant="outline"
                  onClick={() => void handleCancel()}
                  disabled={isSaving}
                >
                  {t("common.cancel")}
                </ActionButton>
                {saveBlockedMessage ? (
                  <>
                    <Tooltip>
                      <TooltipTrigger asChild>
                        <div className="inline-flex">
                          <ActionButton variant="outline" disabled>
                            {t("common.apply")}
                          </ActionButton>
                        </div>
                      </TooltipTrigger>
                      <TooltipContent side="top">{saveBlockedMessage}</TooltipContent>
                    </Tooltip>
                    <Tooltip>
                      <TooltipTrigger asChild>
                        <div className="inline-flex">
                          <ActionButton disabled>
                            {isSaving ? t("common.saving") : t("common.confirm")}
                          </ActionButton>
                        </div>
                      </TooltipTrigger>
                      <TooltipContent side="top">{saveBlockedMessage}</TooltipContent>
                    </Tooltip>
                  </>
                ) : (
                  <>
                    <ActionButton
                      variant="outline"
                      onClick={() => void handleApply()}
                      disabled={!isDirty || isSaving}
                    >
                      {t("common.apply")}
                    </ActionButton>
                    <ActionButton onClick={() => void handleConfirm()} disabled={isSaving}>
                      {isSaving ? t("common.saving") : t("common.confirm")}
                    </ActionButton>
                  </>
                )}
              </ActionFooter>
            </div>
          </div>
        </AppContext.Provider>
      </SettingsDraftContext.Provider>
    </div>
  );
}
