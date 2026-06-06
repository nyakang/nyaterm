import { getCurrentWindow } from "@tauri-apps/api/window";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { BiExport, BiImport, BiServer } from "react-icons/bi";
import { GrUpgrade } from "react-icons/gr";
import {
  MdAdd,
  MdArticle,
  MdCellTower,
  MdContentCopy,
  MdContentPaste,
  MdDashboard,
  MdDeleteSweep,
  MdFitScreen,
  MdInfo,
  MdMenu,
  MdMenuBook,
  MdPalette,
  MdRestartAlt,
  MdSelectAll,
  MdSettings,
  MdSplitscreen,
  MdSwapHoriz,
  MdSwapVert,
  MdSync,
  MdTerminal,
  MdTranslate,
  MdUpdate,
  MdViewSidebar,
  MdZoomIn,
  MdZoomOut,
} from "react-icons/md";
import {
  VscChromeClose,
  VscChromeMaximize,
  VscChromeMinimize,
  VscChromeRestore,
} from "react-icons/vsc";
import packageJson from "@/../package.json";
import QuitConfirmDialog from "@/components/dialog/app/QuitConfirmDialog";
import { Button } from "@/components/ui/button";
import { useApp } from "@/context/AppContext";
import { useTheme } from "@/context/ThemeContext";
import { useConfigTransfer } from "@/hooks/useConfigTransfer";
import { resolveDisplayKeys } from "@/hooks/useShortcutMap";
import { AVAILABLE_LANGUAGES } from "@/i18n";
import { invoke } from "@/lib/invoke";
import { logger } from "@/lib/logger";
import { isMacOS } from "@/lib/platform";
import {
  DEFAULT_TERMINAL_FONT_SIZE,
  decreaseTerminalFontSize,
  increaseTerminalFontSize,
} from "@/lib/terminalFontSize";
import { getActivePane, getTabDisplayName } from "@/lib/workspaceTabs";
import type { SavedConnection, Tab } from "@/types/global";
import ImportDialog from "../dialog/connections/ImportDialog";
import { SYSTEM_ICONS } from "../icons";
import NyaTermLogo from "../NyaTermLogo";
import {
  Menubar,
  MenubarCheckboxItem,
  MenubarContent,
  MenubarItem,
  MenubarMenu,
  MenubarPortal,
  MenubarSeparator,
  MenubarShortcut,
  MenubarSub,
  MenubarSubContent,
  MenubarSubTrigger,
  MenubarTrigger,
} from "../ui/menubar";

const iconMap: Record<string, React.ElementType> = {
  add: MdAdd,
  content_copy: MdContentCopy,
  content_paste: MdContentPaste,
  select_all: MdSelectAll,
  palette: MdPalette,
  translate: MdTranslate,
  zoom_in: MdZoomIn,
  zoom_out: MdZoomOut,
  restart_alt: MdRestartAlt,
  menu_book: MdMenuBook,
  update: MdUpdate,
  upgrade: GrUpgrade,
  article: MdArticle,
  info: MdInfo,
  menu: MdMenu,
  view_sidebar: MdViewSidebar,
  settings: MdSettings,
  file_export: BiExport,
  file_import: BiImport,
  splitscreen: MdSplitscreen,
  dashboard: MdDashboard,
  swap_horiz: MdSwapHoriz,
  swap_vert: MdSwapVert,
  sync: MdSync,
  cell_tower: MdCellTower,
  delete_sweep: MdDeleteSweep,
  fit_screen: MdFitScreen,
  terminal: MdTerminal,
};

function DynamicIcon({ name, className }: { name: string; className?: string }) {
  const Icon = iconMap[name];
  if (!Icon) return null;
  return <Icon className={className} />;
}

interface HeaderProps {
  onNewSession: () => void;
  onToggleLeft?: () => void;
  onToggleRight?: () => void;
  onAbout: () => void;
  onCheckForUpdates: () => void;
  hasUpdate?: boolean;
  showUpdateDot?: boolean;
  onHelpMenuOpen?: () => void;
  activeTab?: Tab | null;
  savedConnections?: SavedConnection[];
  onSmartSplit?: (mode: "auto" | "horizontal" | "vertical") => void;
  onManageSyncGroups?: () => void;
  onBroadcastToAll?: () => void;
  broadcastToAll?: boolean;
  onClearTerminal?: () => void;
  onResetTerminalSize?: () => void;
}

interface MenuItem {
  label: string;
  action?: () => void;
  separator?: boolean;
  submenu?: MenuItem[];
  checked?: boolean;
  icon?: string;
  shortcut?: string;
}

/** Top bar with File/Edit/View/Terminal/Help menus, theme picker, and mobile toggles. */
export default function Header({
  onNewSession,
  onToggleLeft,
  onToggleRight,
  onAbout,
  onCheckForUpdates,
  hasUpdate,
  showUpdateDot,
  onHelpMenuOpen,
  activeTab,
  savedConnections,
  onSmartSplit,
  onManageSyncGroups,
  onBroadcastToAll,
  broadcastToAll,
  onClearTerminal,
  onResetTerminalSize,
}: HeaderProps) {
  const [appWindow] = useState(() => getCurrentWindow());
  const { themeName, setTheme, themeNames } = useTheme();
  const { updateAppSettings, updateUi, appSettings, tabs } = useApp();
  const [showImportDialog, setShowImportDialog] = useState(false);
  const [isMaximized, setIsMaximized] = useState(false);
  const [showCloseConfirm, setShowCloseConfirm] = useState(false);
  const { t, i18n } = useTranslation();
  const { handleExport, passwordAlert } = useConfigTransfer();

  const activePane = activeTab ? getActivePane(activeTab) : null;
  const activeConnection = activePane?.connectionId
    ? savedConnections?.find((c) => c.id === activePane.connectionId)
    : undefined;
  const activeDisplayName = activeTab ? getTabDisplayName(activeTab) : "NyaTerm";

  useEffect(() => {
    let mounted = true;

    const syncMaximizedState = async () => {
      const maximized = await appWindow.isMaximized().catch(() => false);
      if (mounted) {
        setIsMaximized(maximized);
      }
    };

    void syncMaximizedState();

    let unlistenResized: (() => void) | undefined;
    appWindow
      .onResized(() => {
        void syncMaximizedState();
      })
      .then((unlisten) => {
        unlistenResized = unlisten;
      })
      .catch(() => {});

    return () => {
      mounted = false;
      unlistenResized?.();
    };
  }, [appWindow]);

  const changeLanguage = (lng: string) => {
    i18n.changeLanguage(lng);
    updateUi({ language: lng });
  };

  const handleZoom = (delta: number) => {
    updateAppSettings((prev) => ({
      appearance: {
        ...prev.appearance,
        font_size:
          delta > 0
            ? increaseTerminalFontSize(prev.appearance.font_size)
            : decreaseTerminalFontSize(prev.appearance.font_size),
      },
    }));
  };

  const handleResetZoom = () =>
    updateAppSettings((prev) => ({
      appearance: { ...prev.appearance, font_size: DEFAULT_TERMINAL_FONT_SIZE },
    }));

  const menuKeys = [
    { key: "file", label: t("menu.file") },
    { key: "view", label: t("menu.view") },
    { key: "terminal", label: t("menu.terminal") },
    { key: "help", label: t("menu.help") },
  ];

  const dk = (id: string) => resolveDisplayKeys(id, appSettings.keybindings);

  const menus: Record<string, MenuItem[]> = {
    file: [
      {
        label: t("menu.newSession"),
        action: onNewSession,
        icon: "add",
        shortcut: dk("tab.newSession"),
      },
      { label: "separator", separator: true },
      {
        label: t("settings.importConfig"),
        action: () => setShowImportDialog(true),
        icon: "file_import",
      },
      {
        label: t("settings.exportConfig"),
        action: handleExport,
        icon: "file_export",
      },
    ],
    view: [
      {
        label: t("menu.theme"),
        icon: "palette",
        submenu: themeNames.map((th) => ({
          label: th.name,
          checked: themeName === th.id,
          action: () => setTheme(th.id),
        })),
      },
      {
        label: t("menu.language"),
        icon: "translate",
        submenu: AVAILABLE_LANGUAGES.map((l) => ({
          label: l.name,
          checked: i18n.language === l.id,
          action: () => changeLanguage(l.id),
        })),
      },
      { label: "separator", separator: true },
      {
        label: t("menu.zoomIn"),
        action: () => handleZoom(0.1),
        icon: "zoom_in",
        shortcut: dk("view.zoomIn"),
      },
      {
        label: t("menu.zoomOut"),
        action: () => handleZoom(-0.1),
        icon: "zoom_out",
        shortcut: dk("view.zoomOut"),
      },
      {
        label: t("menu.resetZoom"),
        action: handleResetZoom,
        icon: "restart_alt",
        shortcut: dk("view.resetZoom"),
      },
    ],
    terminal: [
      {
        label: t("menu.smartSplit"),
        icon: "splitscreen",
        submenu: [
          {
            label: t("menu.autoTile"),
            icon: "dashboard",
            action: () => onSmartSplit?.("auto"),
          },
          {
            label: t("menu.tileHorizontally"),
            icon: "swap_horiz",
            action: () => onSmartSplit?.("horizontal"),
          },
          {
            label: t("menu.tileVertically"),
            icon: "swap_vert",
            action: () => onSmartSplit?.("vertical"),
          },
        ],
      },
      { label: "separator", separator: true },
      {
        label: t("menu.syncInput"),
        icon: "sync",
        submenu: [
          {
            label: t("menu.manageGroups"),
            icon: "settings",
            action: () => onManageSyncGroups?.(),
            shortcut: dk("terminal.manageSyncGroups"),
          },
        ],
      },
      { label: "separator", separator: true },
      {
        label: t("menu.broadcastToAll"),
        icon: "cell_tower",
        action: () => onBroadcastToAll?.(),
        checked: broadcastToAll,
      },
      { label: "separator", separator: true },
      {
        label: t("menu.clearTerminal"),
        icon: "delete_sweep",
        action: () => onClearTerminal?.(),
        shortcut: dk("terminal.clear"),
      },
      {
        label: t("menu.resetTerminalSize"),
        icon: "fit_screen",
        action: () => onResetTerminalSize?.(),
      },
    ],
    help: [
      {
        label: t("menu.documentation"),
        icon: "menu_book",
        action: () => openUrl(`${packageJson.docspage}`),
      },
      {
        label: t("menu.checkForUpdates"),
        icon: hasUpdate ? "upgrade" : "update",
        action: onCheckForUpdates,
      },
      {
        label: t("menu.viewLogs"),
        icon: "article",
        action: async () => {
          try {
            await invoke("open_log_dir");
          } catch (error) {
            logger.error({
              domain: "ui.error",
              event: "logs.open_failed",
              message: "Failed to open logs",
              error,
            });
          }
        },
      },
      { label: "separator", separator: true },
      { label: t("menu.about"), action: onAbout, icon: "info" },
    ],
  };

  const renderMenuItem = (item: MenuItem, idx: number) => {
    if (item.separator) {
      return <MenubarSeparator key={`sep-${idx}`} />;
    }

    if (item.submenu) {
      return (
        <MenubarSub key={item.label}>
          <MenubarSubTrigger>
            {item.icon && (
              <DynamicIcon
                name={item.icon}
                className="text-[1rem] mr-2 text-[var(--df-text-muted)]"
              />
            )}
            <span className="flex-1">{item.label}</span>
          </MenubarSubTrigger>
          <MenubarPortal>
            <MenubarSubContent>
              {item.submenu.map((sub, i) => renderMenuItem(sub, i))}
            </MenubarSubContent>
          </MenubarPortal>
        </MenubarSub>
      );
    }

    if (item.checked !== undefined) {
      return (
        <MenubarCheckboxItem
          key={item.label}
          checked={item.checked}
          onCheckedChange={() => {
            item.action?.();
          }}
        >
          <span className="flex-1">{item.label}</span>
          {item.shortcut && <MenubarShortcut>{item.shortcut}</MenubarShortcut>}
        </MenubarCheckboxItem>
      );
    }

    return (
      <MenubarItem
        key={item.label}
        onClick={() => {
          item.action?.();
        }}
      >
        {item.icon && (
          <DynamicIcon
            name={item.icon}
            className={`text-[1rem] mr-2 ${item.icon === "upgrade" ? "text-green-500" : "text-[var(--df-text-muted)]"}`}
          />
        )}
        <span className="flex-1">{item.label}</span>
        {item.icon === "upgrade" && (
          <span className="ml-2 text-[10px] font-medium text-green-500">
            {t("updater.hasNewVersion")}
          </span>
        )}
        {item.shortcut && <MenubarShortcut>{item.shortcut}</MenubarShortcut>}
      </MenubarItem>
    );
  };

  const handleMinimizeWindow = () => {
    appWindow.minimize().catch(() => {});
  };

  const handleToggleMaximizeWindow = () => {
    appWindow.toggleMaximize().catch(() => {});
  };

  const handleCloseWindow = () => {
    if (
      !appSettings.general.minimize_to_tray &&
      tabs.length > 0 &&
      appSettings.general.confirm_on_close !== false
    ) {
      setShowCloseConfirm(true);
    } else {
      appWindow.close().catch(() => {});
    }
  };

  const handleConfirmClose = () => {
    setShowCloseConfirm(false);
    appWindow.close().catch(() => {});
  };

  return (
    <header
      className="h-10 border-b flex items-center gap-2 px-2 select-none shrink-0"
      style={{ backgroundColor: "var(--df-bg-panel)", borderColor: "var(--df-border)" }}
    >
      <div className={`flex items-center gap-2 shrink-0${isMacOS ? " pl-[70px]" : ""}`}>
        {!isMacOS && (
          <NyaTermLogo className="h-5 w-5 shrink-0" onDoubleClick={handleToggleMaximizeWindow} />
        )}

        {!isMacOS && (
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            className="lg:hidden text-[var(--df-text-muted)] hover:bg-[color-mix(in_srgb,var(--df-text-muted)_10%,transparent)] hover:text-[var(--df-text-muted)]"
            onClick={onToggleLeft}
          >
            <MdMenu className="text-base" />
          </Button>
        )}

        <Menubar className="border-none bg-transparent h-auto p-0 gap-1 shadow-none">
          {menuKeys.map(({ key, label }) => (
            <MenubarMenu key={key}>
              <MenubarTrigger
                className="relative cursor-default px-2.5 py-1 text-xs font-medium rounded-md transition-colors text-[var(--df-text-muted)] data-[state=open]:text-[var(--df-primary)] data-[state=open]:bg-[color-mix(in_srgb,var(--df-primary)_10%,transparent)] hover:bg-[color-mix(in_srgb,var(--df-text-muted)_10%,transparent)] focus:bg-[color-mix(in_srgb,var(--df-text-muted)_10%,transparent)] focus:text-[var(--df-text-muted)] data-[state=open]:focus:bg-[color-mix(in_srgb,var(--df-primary)_10%,transparent)] data-[state=open]:focus:text-[var(--df-primary)] outline-none"
                {...(key === "help" && showUpdateDot ? { onClick: onHelpMenuOpen } : {})}
              >
                {label}
                {key === "help" && showUpdateDot && (
                  <span className="absolute -top-0.5 -right-0.5 flex h-2 w-2">
                    <span className="animate-ping absolute inline-flex h-full w-full rounded-full bg-green-400 opacity-75" />
                    <span className="relative inline-flex rounded-full h-2 w-2 bg-green-500" />
                  </span>
                )}
              </MenubarTrigger>
              <MenubarContent align="start" className="min-w-[180px]">
                {menus[key].map((item, idx) => renderMenuItem(item, idx))}
              </MenubarContent>
            </MenubarMenu>
          ))}
        </Menubar>
      </div>

      <div
        className="flex-1 min-w-0 h-full flex items-center justify-center gap-2 px-2"
        data-tauri-drag-region
      >
        <div
          className="flex items-center gap-2 min-w-0 pointer-events-none"
          style={{ color: "var(--df-text-muted)" }}
        >
          {activeTab && activePane ? (
            activePane.type === "SSH" && activeConnection && !activeTab.customName ? (
              <>
                {activeConnection.icon && SYSTEM_ICONS[activeConnection.icon] && (
                  <span className="text-sm shrink-0">
                    {SYSTEM_ICONS[activeConnection.icon].icon({ className: "text-sm shrink-0" })}
                  </span>
                )}
                <span className="text-xs font-medium truncate">
                  {activeConnection.name} — {activeConnection.username}@{activeConnection.host}:
                  {activeConnection.port}
                </span>
              </>
            ) : activePane.type === "SSH" ? (
              <>
                <BiServer className="text-sm shrink-0" />
                <span className="text-xs font-medium truncate">{activeDisplayName}</span>
              </>
            ) : (
              <>
                <MdTerminal className="text-sm shrink-0" />
                <span className="text-xs font-medium truncate">{activeDisplayName}</span>
              </>
            )
          ) : (
            <span className="text-xs font-medium truncate">NyaTerm</span>
          )}
        </div>
      </div>

      <div className="flex items-center gap-1 shrink-0" style={{ color: "var(--df-text-muted)" }}>
        {!isMacOS && (
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            className="md:hidden text-[var(--df-text-muted)] hover:bg-[color-mix(in_srgb,var(--df-text-muted)_10%,transparent)] hover:text-[var(--df-text-muted)]"
            onClick={onToggleRight}
          >
            <MdViewSidebar className="text-base" />
          </Button>
        )}

        {!isMacOS && (
          <div className="flex items-center h-full -mr-2 ml-1">
            <Button
              type="button"
              variant="ghost"
              className="rounded-none h-10 w-[46px] px-0 text-[var(--df-text-muted)] transition-colors hover:!bg-[color-mix(in_srgb,var(--df-text)_10%,transparent)] hover:!text-[var(--df-text)]"
              aria-label={t("menu.minimize")}
              onClick={handleMinimizeWindow}
            >
              <VscChromeMinimize className="text-base" />
            </Button>

            <Button
              type="button"
              variant="ghost"
              className="rounded-none h-10 w-[46px] px-0 text-[var(--df-text-muted)] transition-colors hover:!bg-[color-mix(in_srgb,var(--df-text)_10%,transparent)] hover:!text-[var(--df-text)]"
              aria-label={isMaximized ? t("menu.restore") : t("menu.maximize")}
              onClick={handleToggleMaximizeWindow}
            >
              {isMaximized ? (
                <VscChromeRestore className="text-base" />
              ) : (
                <VscChromeMaximize className="text-base" />
              )}
            </Button>

            <Button
              type="button"
              variant="ghost"
              className="rounded-none h-10 w-[46px] px-0 text-[var(--df-text-muted)] transition-colors hover:!bg-[#e81123] hover:!text-white"
              aria-label={t("common.close")}
              onClick={handleCloseWindow}
            >
              <VscChromeClose className="text-base" />
            </Button>
          </div>
        )}
      </div>
      <ImportDialog open={showImportDialog} onClose={() => setShowImportDialog(false)} />
      {passwordAlert}

      <QuitConfirmDialog
        open={showCloseConfirm}
        onOpenChange={setShowCloseConfirm}
        onConfirm={handleConfirmClose}
      />
    </header>
  );
}
