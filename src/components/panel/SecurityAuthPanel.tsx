import { useTranslation } from "react-i18next";
import PanelHeader from "@/components/layout/PanelHeader";
import { KeyManagementTab } from "@/components/settings/KeyManagementTab";
import { PasswordManagementTab } from "@/components/settings/PasswordManagementTab";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";

export default function SecurityAuthPanel() {
  const { t } = useTranslation();

  return (
    <div className="h-full flex flex-col" style={{ backgroundColor: "var(--df-bg-panel)" }}>
      <PanelHeader title={t("securityAuth.title")} />
      <div className="flex-1 overflow-y-auto p-3 terminal-scroll">
        <Tabs defaultValue="passwords" className="w-full">
          <TabsList className="grid w-full grid-cols-2 h-8">
            <TabsTrigger value="passwords" className="text-xs">
              {t("securityAuth.passwords")}
            </TabsTrigger>
            <TabsTrigger value="keys" className="text-xs">
              {t("securityAuth.keys")}
            </TabsTrigger>
          </TabsList>
          <TabsContent value="passwords" className="mt-3">
            <PasswordManagementTab />
          </TabsContent>
          <TabsContent value="keys" className="mt-3">
            <KeyManagementTab />
          </TabsContent>
        </Tabs>
      </div>
    </div>
  );
}
