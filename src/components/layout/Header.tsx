import { useState } from "react";
import { appLogDir } from "@tauri-apps/api/path";
import { openPath, openUrl } from "@tauri-apps/plugin-opener";
import { useTranslation } from "react-i18next";
import {
  MdAdd,
  MdArticle,
  MdComputer,
  MdContentCopy,
  MdContentPaste,
  MdDashboard,
  MdFileUpload,
  MdFullscreen,
  MdInfo,
  MdMenu,
  MdMenuBook,
  MdPalette,
  MdRestartAlt,
  MdSelectAll,
  MdSettings,
  MdTranslate,
  MdUpdate,
  MdViewSidebar,
  MdZoomIn,
  MdZoomOut,
} from "react-icons/md";
import packageJson from "../../../package.json";
import { useApp } from "../../context/AppContext";
import { useTheme } from "../../context/ThemeContext";
import { AVAILABLE_LANGUAGES } from "../../i18n";
import { openSettings } from "../../lib/windowManager";
import ImportDialog from "../dialog/saved-connections/ImportDialog";

import { MOD } from "../../hooks/useGlobalShortcuts";

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
  dashboard: MdDashboard,
  palette: MdPalette,
  translate: MdTranslate,
  zoom_in: MdZoomIn,
  zoom_out: MdZoomOut,
  restart_alt: MdRestartAlt,
  fullscreen: MdFullscreen,
  computer: MdComputer,
  menu_book: MdMenuBook,
  update: MdUpdate,
  article: MdArticle,
  info: MdInfo,
  menu: MdMenu,
  view_sidebar: MdViewSidebar,
  settings: MdSettings,
  file_upload: MdFileUpload,
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
}: HeaderProps) {
  const { themeName, setTheme, themeNames } = useTheme();
  const { appSettings, updateUi } = useApp();
  const [showImportDialog, setShowImportDialog] = useState(false);
  const uiConfig = appSettings.ui;
  const { t, i18n } = useTranslation();

  const changeLanguage = (lng: string) => {
    i18n.changeLanguage(lng);
    updateUi({ language: lng });
  };

  const handleZoom = (delta: number) => {
    const newZoom = Math.max(0.5, Math.min(2.0, uiConfig.zoom_level + delta));
    updateUi({ zoom_level: parseFloat(newZoom.toFixed(1)) });
  };

  const handleResetZoom = () => updateUi({ zoom_level: 1.0 });

  const toggleFullscreen = () => {
    if (!document.fullscreenElement) {
      document.documentElement.requestFullscreen().catch((err) => {
        console.error(`Error attempting to enable fullscreen: ${err.message}`);
      });
    } else {
      document.exitFullscreen();
    }
  };

  const menuKeys = [
    { key: "file", label: t("menu.file") },
    { key: "edit", label: t("menu.edit") },
    { key: "view", label: t("menu.view") },
    { key: "terminal", label: t("menu.terminal") },
    { key: "help", label: t("menu.help") },
  ];

  const menus: Record<string, MenuItem[]> = {
    file: [
      { label: t("menu.newSshConnection"), action: onNewSession, icon: "add", shortcut: `${MOD}+Shift+N` },
      { label: t("savedConnections.importSessions"), action: () => setShowImportDialog(true), icon: "file_upload" },
      { label: "separator", separator: true },
    ],
    edit: [
      { label: t("menu.copy"), icon: "content_copy", shortcut: `${MOD}+Shift+C` },
      { label: t("menu.paste"), icon: "content_paste", shortcut: `${MOD}+Shift+V` },
      { label: t("menu.selectAll"), icon: "select_all" },
    ],
    view: [
      {
        label: t("menu.layout"),
        icon: "dashboard",
        submenu: [
          {
            label: t("panel.fileExplorer"),
            checked: uiConfig.show_file_explorer,
            action: () => updateUi({ show_file_explorer: !uiConfig.show_file_explorer }),
          },
          {
            label: t("panel.fileTransfer"),
            checked: uiConfig.show_file_transfer,
            action: () => updateUi({ show_file_transfer: !uiConfig.show_file_transfer }),
          },
          {
            label: t("panel.savedConnections"),
            checked: uiConfig.show_saved_connections,
            action: () =>
              updateUi({ show_saved_connections: !uiConfig.show_saved_connections }),
          },
          {
            label: t("panel.activeSessions"),
            checked: uiConfig.show_active_sessions,
            action: () => updateUi({ show_active_sessions: !uiConfig.show_active_sessions }),
          },
          {
            label: t("panel.commandHistory"),
            checked: uiConfig.show_command_history,
            action: () => updateUi({ show_command_history: !uiConfig.show_command_history }),
          },
          {
            label: t("panel.quickCommands"),
            checked: uiConfig.show_quick_commands,
            action: () => updateUi({ show_quick_commands: !uiConfig.show_quick_commands }),
          },
          { label: "separator", separator: true },
          {
            label: t("menu.resetLayout"),
            action: () =>
              updateUi({
                panel_layout: {
                  left: ["fileExplorer", "fileTransfer"],
                  right: ["savedConnections", "activeSessions", "commandHistory"],
                },
              }),
          },
        ],
      },
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
      { label: t("menu.zoomIn"), action: () => handleZoom(0.1), icon: "zoom_in", shortcut: `${MOD}+=` },
      { label: t("menu.zoomOut"), action: () => handleZoom(-0.1), icon: "zoom_out", shortcut: `${MOD}+-` },
      { label: t("menu.resetZoom"), action: handleResetZoom, icon: "restart_alt", shortcut: `${MOD}+0` },
      { label: "separator", separator: true },
      { label: t("menu.fullscreen"), action: toggleFullscreen, icon: "fullscreen", shortcut: "F11" },
    ],
    terminal: [
      { label: t("menu.newSshConnection"), action: onNewSession, icon: "add", shortcut: `${MOD}+Shift+N` },
      { label: t("menu.newLocalTerminal"), action: onNewSession, icon: "computer", shortcut: `${MOD}+\`` },
    ],
    help: [
      {
        label: t("menu.documentation"),
        icon: "menu_book",
        action: () => openUrl(packageJson.homepage + "/docs"),
      },
      {
        label: t("menu.checkForUpdates"),
        icon: "update",
        action: () => openUrl(packageJson.homepage + "/releases"),
      },
      {
        label: t("menu.viewLogs"),
        icon: "article",
        action: async () => {
          try {
            const logDir = await appLogDir();
            await openPath(logDir);
          } catch (error) {
            console.error("Failed to open logs:", error);
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
          {item.label}
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
          <DynamicIcon name={item.icon} className="text-[1rem] mr-2 text-[var(--df-text-muted)]" />
        )}
        <span className="flex-1">{item.label}</span>
        {item.shortcut && <MenubarShortcut>{item.shortcut}</MenubarShortcut>}
      </MenubarItem>
    );
  };

  return (
    <header
      className="h-10 border-b flex items-center justify-between px-3 select-none shrink-0"
      style={{ backgroundColor: "var(--df-bg-panel)", borderColor: "var(--df-border)" }}
    >
      <div className="flex items-center gap-4">
        {/* Mobile Left Toggle */}
        <button
          className="lg:hidden flex items-center"
          style={{ color: "var(--df-text-muted)" }}
          onClick={onToggleLeft}
        >
          <MdMenu className="text-base" />
        </button>

        <Menubar className="border-none bg-transparent h-auto p-0 gap-1 shadow-none">
          {menuKeys.map(({ key, label }) => (
            <MenubarMenu key={key}>
              <MenubarTrigger className="cursor-default px-2.5 py-1 text-xs font-medium rounded-md transition-colors text-[var(--df-text-muted)] data-[state=open]:text-[var(--df-primary)] data-[state=open]:bg-[color-mix(in_srgb,var(--df-primary)_10%,transparent)] hover:bg-[color-mix(in_srgb,var(--df-text-muted)_10%,transparent)] focus:bg-[color-mix(in_srgb,var(--df-text-muted)_10%,transparent)] focus:text-[var(--df-text-muted)] data-[state=open]:focus:bg-[color-mix(in_srgb,var(--df-primary)_10%,transparent)] data-[state=open]:focus:text-[var(--df-primary)] outline-none">
                {label}
              </MenubarTrigger>
              <MenubarContent align="start" className="min-w-[180px]">
                {menus[key].map((item, idx) => renderMenuItem(item, idx))}
              </MenubarContent>
            </MenubarMenu>
          ))}
        </Menubar>
      </div>
      <div className="flex items-center gap-3" style={{ color: "var(--df-text-muted)" }}>
        {/* Mobile Right Toggle */}
        <button
          className="md:hidden flex items-center"
          style={{ color: "var(--df-text-muted)" }}
          onClick={onToggleRight}
        >
          <MdViewSidebar className="text-base" />
        </button>

        <MdSettings
          className="text-base cursor-pointer hover:opacity-80 transition-opacity hidden sm:block"
          onClick={() => openSettings()}
        />
      </div>
      <ImportDialog open={showImportDialog} onClose={() => setShowImportDialog(false)} />
    </header>
  );
}
