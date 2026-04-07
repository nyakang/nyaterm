import { useTranslation } from "react-i18next";
import PanelHeader from "@/components/layout/PanelHeader";
import { KeyManagementTab } from "@/components/settings/KeyManagementTab";

export default function KeyManagementPanel() {
  const { t } = useTranslation();

  return (
    <div className="h-full flex flex-col" style={{ backgroundColor: "var(--df-bg-panel)" }}>
      <PanelHeader title={t("settings.keyManagement")} />
      <div className="flex-1 overflow-y-auto p-3 terminal-scroll">
        <KeyManagementTab />
      </div>
    </div>
  );
}
