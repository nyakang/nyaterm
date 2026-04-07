import { getCurrentWindow } from "@tauri-apps/api/window";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import {
  MdMouse,
  MdPalette,
  MdRouter,
  MdSearch,
  MdSecurity,
  MdSettings,
  MdTerminal,
  MdTranslate,
  MdVpnKey,
} from "react-icons/md";
import ChildWindowHeader from "@/components/layout/ChildWindowHeader";
import { AppearanceTab } from "@/components/settings/AppearanceTab";
import { GeneralTab } from "@/components/settings/GeneralTab";
import { InteractionTab } from "@/components/settings/InteractionTab";
import { ProxyTab } from "@/components/settings/ProxyTab";
import { SearchTab } from "@/components/settings/SearchTab";
import { SecurityTab } from "@/components/settings/SecurityTab";
import { TerminalTab } from "@/components/settings/TerminalTab";
import { TranslationTab } from "@/components/settings/TranslationTab";
import { Button } from "@/components/ui/button";
import { useApp } from "@/context/AppContext";


export default function SettingsPage() {
  const { t } = useTranslation();
  const { appSettings } = useApp();

  const params = new URLSearchParams(window.location.search);
  const initialTab = params.get("tab") || "general";
  const [activeTab, setActiveTab] = useState(initialTab);

  const tabs = [
    { id: "general", label: t("settings.general"), icon: "settings", Component: GeneralTab },
    {
      id: "appearance",
      label: t("settings.appearance"),
      icon: "palette",
      Component: AppearanceTab,
    },
    { id: "proxy", label: t("settings.proxy"), icon: "router", Component: ProxyTab },
    { id: "search", label: t("settings.search"), icon: "search", Component: SearchTab },
    {
      id: "translation",
      label: t("settings.translation"),
      icon: "translate",
      Component: TranslationTab,
    },
    { id: "security", label: t("settings.security"), icon: "security", Component: SecurityTab },
    { id: "terminal", label: t("settings.terminal"), icon: "terminal", Component: TerminalTab },
    {
      id: "interaction",
      label: t("settings.interaction"),
      icon: "mouse",
      Component: InteractionTab,
    },
  ];

  const ActiveComponent = tabs.find((t) => t.id === activeTab)?.Component;

  const iconMap: Record<string, React.ElementType> = {
    settings: MdSettings,
    palette: MdPalette,
    router: MdRouter,
    search: MdSearch,
    translate: MdTranslate,
    security: MdSecurity,
    vpnKey: MdVpnKey,
    terminal: MdTerminal,
    mouse: MdMouse,
  };

  function DynamicIcon({ name, className }: { name: string; className?: string }) {
    const Icon = iconMap[name];
    if (!Icon) return null;
    return <Icon className={className} />;
  }

  const handleClose = () => getCurrentWindow().close();

  return (
    <div
      className="h-screen flex flex-col overflow-hidden"
      style={{ fontFamily: appSettings.appearance.font_family }}
    >
      <ChildWindowHeader
        title={t("settings.title")}
        icon={<MdSettings className="text-base" />}
        onClose={handleClose}
      />

      <div className="flex-1 flex flex-col sm:flex-row overflow-hidden">
        {/* Sidebar */}
        <div className="w-full sm:w-52 shrink-0 flex flex-col border-r bg-background overflow-y-auto">
          <div className="p-6 border-b shrink-0 flex items-center gap-3" data-tauri-drag-region>
            <MdSettings className="text-2xl text-primary" />
            <h1 className="text-xl font-semibold">{t("settings.title")}</h1>
          </div>
          <div className="flex-1 py-3 px-3 space-y-1">
            {tabs.map((tab) => (
              <Button
                key={tab.id}
                variant="ghost"
                onClick={() => setActiveTab(tab.id)}
                className={`w-full flex items-center justify-start gap-3 px-3 py-2.5 h-auto rounded-lg text-sm font-medium transition-colors ${activeTab === tab.id ? "bg-primary/15 text-primary hover:bg-primary/20 hover:text-primary" : "text-muted-foreground hover:bg-accent hover:text-accent-foreground"}`}
              >
                <DynamicIcon
                  name={tab.icon}
                  className={`text-[1.125rem] ${activeTab === tab.id ? "text-primary" : ""}`}
                />
                {tab.label}
              </Button>
            ))}
          </div>
        </div>

        {/* Content Area */}
        <div className="flex-1 flex flex-col min-w-0">
          <div
            className="p-6 border-b shrink-0 flex items-center justify-between"
            data-tauri-drag-region
          >
            <h3 className="text-2xl font-semibold">
              {tabs.find((t) => t.id === activeTab)?.label}
            </h3>
          </div>
          <div className="flex-1 overflow-y-auto p-6 md:p-8">
            <div className="max-w-2xl text-base space-y-6">
              {ActiveComponent && <ActiveComponent />}
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
