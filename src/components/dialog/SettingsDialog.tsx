import { useState } from "react";
import { useTranslation } from "react-i18next";
import {
  MdClose,
  MdMouse,
  MdPalette,
  MdRouter,
  MdSearch,
  MdSecurity,
  MdSettings,
  MdTerminal,
  MdTranslate,
} from "react-icons/md";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
import { useApp } from "@/context/AppContext";
import { AppearanceTab } from "./settings/AppearanceTab";
import { GeneralTab } from "./settings/GeneralTab";
import { InteractionTab } from "./settings/InteractionTab";
import { ProxyTab } from "./settings/ProxyTab";
import { SearchTab } from "./settings/SearchTab";
import { SecurityTab } from "./settings/SecurityTab";
import { TerminalTab } from "./settings/TerminalTab";
import { TranslationTab } from "./settings/TranslationTab";

export default function SettingsDialog() {
  const { t } = useTranslation();
  const { showSettingsDialog, setShowSettingsDialog, appSettings } = useApp();
  const [activeTab, setActiveTab] = useState("general");

  const tabs = [
    {
      id: "general",
      label: t("settings.general", "General"),
      icon: "settings",
      Component: GeneralTab,
    },
    {
      id: "appearance",
      label: t("settings.appearance", "Appearance"),
      icon: "palette",
      Component: AppearanceTab,
    },
    { id: "proxy", label: t("settings.proxy", "Proxy"), icon: "router", Component: ProxyTab },
    { id: "search", label: t("settings.search", "Search"), icon: "search", Component: SearchTab },
    {
      id: "translation",
      label: t("settings.translation", "Translation"),
      icon: "translate",
      Component: TranslationTab,
    },
    {
      id: "security",
      label: t("settings.security", "Security"),
      icon: "security",
      Component: SecurityTab,
    },
    {
      id: "terminal",
      label: t("settings.terminal", "Terminal Core"),
      icon: "terminal",
      Component: TerminalTab,
    },
    {
      id: "interaction",
      label: t("settings.interaction", "Interaction"),
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
    terminal: MdTerminal,
    mouse: MdMouse,
    close: MdClose,
  };

  function DynamicIcon({ name, className }: { name: string; className?: string }) {
    const Icon = iconMap[name];
    if (!Icon) return null;
    return <Icon className={className} />;
  }

  return (
    <Dialog open={showSettingsDialog} onOpenChange={(v) => !v && setShowSettingsDialog(false)}>
      <DialogContent
        aria-describedby={undefined}
        className="w-full max-w-3xl sm:max-w-3xl h-[80vh] p-0 gap-0 flex flex-col sm:flex-row overflow-hidden"
        showCloseButton={false}
        style={{ fontFamily: appSettings.appearance.font_family }}
      >
        {/* Sidebar */}
        <div className="w-full sm:w-52 shrink-0 flex flex-col border-r bg-background overflow-y-auto">
          <div className="p-6 border-b shrink-0 flex items-center gap-3">
            <MdSettings className="text-2xl text-primary" />
            <DialogTitle className="text-xl font-semibold">{t("settings.title", "Settings")}</DialogTitle>
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
                  className={`text-[18px] ${activeTab === tab.id ? "text-primary" : ""}`}
                />
                {tab.label}
              </Button>
            ))}
          </div>
        </div>

        {/* Content Area */}
        <div className="flex-1 flex flex-col min-w-0">
          <div className="p-6 border-b shrink-0 flex items-center justify-between">
            <h3 className="text-2xl font-semibold">
              {tabs.find((t) => t.id === activeTab)?.label}
            </h3>
            <Button variant="ghost" size="icon-sm" onClick={() => setShowSettingsDialog(false)}>
              <MdClose className="text-xl" />
            </Button>
          </div>

          <div className="flex-1 overflow-y-auto p-6 md:p-8">
            <div className="max-w-2xl text-base space-y-6">
              {ActiveComponent && <ActiveComponent />}
            </div>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
