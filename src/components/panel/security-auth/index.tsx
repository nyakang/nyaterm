import { useState } from "react";
import { useTranslation } from "react-i18next";
import PanelHeader from "@/components/layout/PanelHeader";
import { CredentialManagementTab } from "@/components/panel/security-auth/CredentialManagementTab";
import { KeyManagementTab } from "@/components/panel/security-auth/KeyManagementTab";
import { KnownHostsManagementTab } from "@/components/panel/security-auth/KnownHostsManagementTab";
import { OtpManagementTab } from "@/components/panel/security-auth/OtpManagementTab";
import { PasswordManagementTab } from "@/components/panel/security-auth/PasswordManagementTab";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { useApp } from "@/context/AppContext";

type SecurityAuthTab = "keys" | "passwords" | "credentials" | "otp" | "known-hosts";

function resolveSecurityAuthTab(value: string | undefined): SecurityAuthTab {
  return value === "passwords" ||
    value === "credentials" ||
    value === "otp" ||
    value === "known-hosts"
    ? value
    : "keys";
}

interface SecurityAuthPanelProps {
  activeSessionId?: string | null;
}

export default function SecurityAuthPanel({ activeSessionId = null }: SecurityAuthPanelProps) {
  const { t } = useTranslation();
  const { appSettings, updateUi } = useApp();
  const activeTab = resolveSecurityAuthTab(appSettings.ui.security_auth_panel_active_tab);
  const [keyCount, setKeyCount] = useState(0);
  const [passwordCount, setPasswordCount] = useState(0);
  const [credentialCount, setCredentialCount] = useState(0);
  const [otpCount, setOtpCount] = useState(0);
  const [knownHostCount, setKnownHostCount] = useState(0);
  const [secretsUnlocked, setSecretsUnlocked] = useState(false);

  const displayCount =
    activeTab === "keys"
      ? keyCount
      : activeTab === "passwords"
        ? passwordCount
        : activeTab === "credentials"
          ? credentialCount
          : activeTab === "otp"
            ? otpCount
            : knownHostCount;

  const handleTabChange = (value: string) => {
    updateUi({ security_auth_panel_active_tab: resolveSecurityAuthTab(value) });
  };

  return (
    <div
      className="nyaterm-wallpaper-transparent-surface h-full flex flex-col"
      style={{ backgroundColor: "var(--df-bg-panel)" }}
    >
      <PanelHeader
        title={t("securityAuth.title")}
        actions={
          <span className="text-[0.6875rem]" style={{ color: "var(--df-text-dimmed)" }}>
            {displayCount}
          </span>
        }
      />
      <div className="flex min-h-0 flex-1 flex-col">
        <Tabs
          value={activeTab}
          onValueChange={handleTabChange}
          className="min-h-0 w-full flex-1 gap-0"
        >
          <div className="px-3 pt-3">
            <TabsList className="grid h-8 w-full grid-cols-5">
              <TabsTrigger value="keys" className="min-w-0 px-1 text-xs">
                <span className="truncate">{t("securityAuth.keys")}</span>
              </TabsTrigger>
              <TabsTrigger value="passwords" className="min-w-0 px-1 text-xs">
                <span className="truncate">{t("securityAuth.passwords")}</span>
              </TabsTrigger>
              <TabsTrigger value="otp" className="min-w-0 px-1 text-xs">
                <span className="truncate">{t("securityAuth.otp")}</span>
              </TabsTrigger>
              <TabsTrigger value="credentials" className="min-w-0 px-1 text-xs">
                <span className="truncate">{t("securityAuth.credentials")}</span>
              </TabsTrigger>
              <TabsTrigger value="known-hosts" className="min-w-0 px-1 text-xs">
                <span className="truncate">{t("securityAuth.knownHosts")}</span>
              </TabsTrigger>
            </TabsList>
          </div>
          <TabsContent value="passwords" className="mt-3 flex min-h-0 flex-1 overflow-hidden">
            <PasswordManagementTab
              onCountChange={setPasswordCount}
              secretsUnlocked={secretsUnlocked}
              onLockSecrets={() => setSecretsUnlocked(false)}
              onUnlockSecrets={() => setSecretsUnlocked(true)}
              showSecretUnlockFooter
            />
          </TabsContent>
          <TabsContent value="credentials" className="mt-3 flex min-h-0 flex-1 overflow-hidden">
            <CredentialManagementTab
              onCountChange={setCredentialCount}
              secretsUnlocked={secretsUnlocked}
              onLockSecrets={() => setSecretsUnlocked(false)}
              onUnlockSecrets={() => setSecretsUnlocked(true)}
            />
          </TabsContent>
          <TabsContent value="keys" className="mt-3 flex min-h-0 flex-1 overflow-hidden">
            <KeyManagementTab
              onCountChange={setKeyCount}
              secretsUnlocked={secretsUnlocked}
              onLockSecrets={() => setSecretsUnlocked(false)}
              onUnlockSecrets={() => setSecretsUnlocked(true)}
            />
          </TabsContent>
          <TabsContent value="otp" className="mt-3 flex min-h-0 flex-1 overflow-hidden">
            <OtpManagementTab activeSessionId={activeSessionId} onCountChange={setOtpCount} />
          </TabsContent>
          <TabsContent value="known-hosts" className="mt-3 flex min-h-0 flex-1 overflow-hidden">
            <KnownHostsManagementTab onCountChange={setKnownHostCount} />
          </TabsContent>
        </Tabs>
      </div>
    </div>
  );
}
