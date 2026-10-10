import { listen } from "@/lib/backend/api";
import { openUrl } from "@/lib/backend/platform/opener";
import { Copy } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { SelectItem } from "@/components/ui/select";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { useApp } from "@/context/AppContext";
import { useSettingsDraft } from "@/context/SettingsDraftContext";
import {
  type CloudSyncValidationCode,
  DEFAULT_CLOUD_SYNC_STATUS,
  formatCloudProvider,
  formatTimestamp,
  getCloudSyncValidationErrors,
  isRemoteInconsistentConflict,
  needsGistCapacityRecovery,
  secretInputValue,
  secretPlaceholder,
  shortValue,
} from "@/lib/cloudSync";
import { getErrorMessage } from "@/lib/errors";
import { invoke } from "@/lib/invoke";
import type {
  CloudConflictPreview,
  CloudSyncSettings,
  CloudSyncStatus,
  GithubGistDeviceFlowPoll,
  GithubGistDeviceFlowStart,
} from "@/types/global";
import {
  SettingFieldGrid,
  SettingInput,
  SettingNumberInput,
  SettingRow,
  SettingSection,
  SettingSelect,
  SettingSwitch,
} from "./SettingFormItems";

interface SyncBackupTabProps {
  onNavigateSecurity: () => void;
}

function getValidationMessage(
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
    case "giteeSnippetEndpointRequired":
      return t("settings.giteeSnippetEndpointRequired");
    case "giteeSnippetIdRequired":
      return t("settings.giteeSnippetIdRequired");
    case "giteeSnippetTokenRequired":
      return t("settings.giteeSnippetTokenRequired");
    case "driveRefreshTokenRequired":
      return t("settings.driveRefreshTokenRequired");
    case "driveClientIdRequired":
      return t("settings.driveClientIdRequired");
    case "driveClientSecretRequired":
      return t("settings.driveClientSecretRequired");
    case "githubGistRequired":
      return t("settings.githubGistRequired");
    case "githubGistTokenRequired":
      return t("settings.githubGistTokenRequired");
  }
}

function getGithubGistAuthErrorMessage(error: unknown, t: ReturnType<typeof useTranslation>["t"]) {
  const message = getErrorMessage(error);
  if (message.includes("GitHub Gist OAuth Client ID is not configured at build time")) {
    return t("settings.githubGistClientIdMissing");
  }
  return message;
}

type GithubGistAuthState = {
  flow: GithubGistDeviceFlowStart | null;
  login: string | null;
  message: string | null;
};

export function SyncBackupTab({ onNavigateSecurity }: SyncBackupTabProps) {
  const { t } = useTranslation();
  const { appSettings, updateAppSettings } = useApp();
  const { committedSettings, isDirty, isSaving } = useSettingsDraft();

  const settings = appSettings.cloud_sync;
  const committedCloudSync = committedSettings.cloud_sync;
  const hasDraftMasterPassword = Boolean(appSettings.security.master_password);
  const hasCommittedMasterPassword = Boolean(committedSettings.security.master_password);

  const [status, setStatus] = useState<CloudSyncStatus>(DEFAULT_CLOUD_SYNC_STATUS);
  const [loading, setLoading] = useState(true);
  const [runningAction, setRunningAction] = useState<string | null>(null);
  const [githubAuth, setGithubAuth] = useState<GithubGistAuthState>({
    flow: null,
    login: null,
    message: null,
  });

  const validationErrors = useMemo(() => getCloudSyncValidationErrors(settings), [settings]);
  const committedValidationErrors = useMemo(
    () => getCloudSyncValidationErrors(committedCloudSync),
    [committedCloudSync],
  );

  const formDisabled = !hasDraftMasterPassword || isSaving;
  const autoSyncSectionDisabled = formDisabled || !settings.enabled;
  const canUseCommittedProvider =
    hasCommittedMasterPassword && committedValidationErrors.length === 0;
  const canRunConfigDependentActions = canUseCommittedProvider && !isDirty && !isSaving;
  const canRunEnabledActions = canRunConfigDependentActions && committedCloudSync.enabled;
  const isBusy = loading || isSaving || runningAction !== null;
  const isRemoteInconsistent = isRemoteInconsistentConflict(status.conflict);
  const showGistCapacityRecovery =
    status.state === "failed" &&
    needsGistCapacityRecovery(status.provider, status.message, status.error_code);

  const updateCloudSync = useCallback(
    (patch: Partial<CloudSyncSettings>) => {
      updateAppSettings({
        cloud_sync: {
          ...settings,
          ...patch,
        },
      });
    },
    [settings, updateAppSettings],
  );

  const handleEnabledChange = useCallback(
    (enabled: boolean) => {
      if (enabled && !hasDraftMasterPassword) {
        toast.error(t("settings.masterPasswordRequiredDesc"));
        onNavigateSecurity();
        return;
      }

      updateCloudSync({ enabled });
    },
    [hasDraftMasterPassword, onNavigateSecurity, t, updateCloudSync],
  );

  const refreshStatus = useCallback(async () => {
    const nextStatus = await invoke<CloudSyncStatus>("get_cloud_sync_status");
    setStatus(nextStatus);
    return nextStatus;
  }, []);

  const loadRuntimeData = useCallback(async () => {
    setLoading(true);
    try {
      await refreshStatus();
    } catch (error) {
      toast.error(getErrorMessage(error));
    } finally {
      setLoading(false);
    }
  }, [refreshStatus]);

  useEffect(() => {
    void loadRuntimeData();
  }, [loadRuntimeData]);

  useEffect(() => {
    const unsubs = [
      listen<CloudSyncStatus>("cloud-sync-status-changed", (event) => {
        setStatus(event.payload);
      }),
      listen<CloudConflictPreview | null>("cloud-sync-conflict", (event) => {
        const conflict = event.payload;
        if (!conflict) {
          return;
        }
        setStatus((current) => ({
          ...current,
          state: "conflict",
          message: conflict.message,
          conflict,
        }));
      }),
    ];

    return () => {
      unsubs.forEach((promise) => {
        promise.then((unlisten) => unlisten());
      });
    };
  }, []);

  const runAction = useCallback(
    async (
      actionKey: string,
      successMessage: string,
      task: () => Promise<void>,
      options?: { allowWhenDisabled?: boolean },
    ) => {
      if (!hasCommittedMasterPassword) {
        toast.error(t("settings.masterPasswordRequiredDesc"));
        onNavigateSecurity();
        return;
      }

      if (isDirty) {
        toast.error(t("settings.applySettingsFirst"));
        return;
      }

      if (committedValidationErrors.length > 0) {
        toast.error(getValidationMessage(committedValidationErrors[0], t));
        return;
      }

      if (!options?.allowWhenDisabled && !committedCloudSync.enabled) {
        toast.error(t("settings.syncEnableFirst"));
        return;
      }

      setRunningAction(actionKey);
      try {
        await task();
        await refreshStatus();
        toast.success(successMessage);
      } catch (error) {
        toast.error(getErrorMessage(error));
      } finally {
        setRunningAction(null);
      }
    },
    [
      committedCloudSync.enabled,
      committedValidationErrors,
      hasCommittedMasterPassword,
      isDirty,
      onNavigateSecurity,
      refreshStatus,
      t,
    ],
  );

  const handleStartGithubGistAuth = useCallback(async () => {
    setRunningAction("github-gist-auth");
    try {
      const flow = await invoke<GithubGistDeviceFlowStart>("begin_github_gist_device_flow");
      setGithubAuth({
        flow,
        login: null,
        message: t("settings.githubGistWaitingForAuth"),
      });
      await openUrl(flow.verification_uri);
    } catch (error) {
      toast.error(getGithubGistAuthErrorMessage(error, t));
    } finally {
      setRunningAction(null);
    }
  }, [t]);

  const handleCancelGithubGistAuth = useCallback(async () => {
    const flowId = githubAuth.flow?.flow_id;
    if (flowId) {
      await invoke("cancel_github_gist_device_flow", { flowId }).catch(() => {});
    }
    setGithubAuth({ flow: null, login: null, message: null });
  }, [githubAuth.flow?.flow_id]);

  const handleCopyGithubGistUserCode = useCallback(async () => {
    const userCode = githubAuth.flow?.user_code;
    if (!userCode) return;

    try {
      await navigator.clipboard.writeText(userCode);
      toast.success(t("settings.githubGistUserCodeCopied"));
    } catch (error) {
      toast.error(getErrorMessage(error));
    }
  }, [githubAuth.flow?.user_code, t]);

  useEffect(() => {
    const flow = githubAuth.flow;
    if (!flow) return;

    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | null = null;

    const poll = async (delaySeconds: number) => {
      timer = setTimeout(async () => {
        if (cancelled) return;
        try {
          const result = await invoke<GithubGistDeviceFlowPoll>("poll_github_gist_device_flow", {
            flowId: flow.flow_id,
            existingGistId: settings.github_gist.gist_id || null,
          });

          if (cancelled) return;

          if (result.state === "success" && result.access_token && result.gist_id) {
            updateCloudSync({
              github_gist: {
                ...settings.github_gist,
                access_token: result.access_token,
                gist_id: result.gist_id,
              },
            });
            setGithubAuth({
              flow: null,
              login: result.login ?? null,
              message: t("settings.githubGistConnected"),
            });
            toast.success(t("settings.githubGistConnected"));
            return;
          }

          if (result.state === "expired" || result.state === "denied" || result.state === "error") {
            setGithubAuth({
              flow: null,
              login: null,
              message: result.message ?? t("settings.githubGistAuthFailed"),
            });
            toast.error(result.message ?? t("settings.githubGistAuthFailed"));
            return;
          }

          setGithubAuth((current) => ({
            ...current,
            message:
              result.state === "slow_down"
                ? t("settings.githubGistSlowDown")
                : t("settings.githubGistWaitingForAuth"),
          }));
          void poll(result.interval ?? flow.interval);
        } catch (error) {
          const message = getGithubGistAuthErrorMessage(error, t);
          setGithubAuth({
            flow: null,
            login: null,
            message,
          });
          toast.error(message);
        }
      }, delaySeconds * 1000);
    };

    void poll(flow.interval);

    return () => {
      cancelled = true;
      if (timer) clearTimeout(timer);
    };
  }, [githubAuth.flow, settings.github_gist, t, updateCloudSync]);

  const statusItems = useMemo(
    () => [
      {
        label: t("settings.syncStatus"),
        value: t(`settings.syncState.${status.state}`, status.state),
      },
      {
        label: t("settings.syncProvider"),
        value: formatCloudProvider(status.provider),
      },
      {
        label: t("settings.lastSyncCheck"),
        value: formatTimestamp(status.last_checked_at_ms) ?? t("settings.never"),
      },
      {
        label: t("settings.lastSyncAt"),
        value: formatTimestamp(status.last_synced_at_ms) ?? t("settings.never"),
      },
      {
        label: t("settings.currentOperation"),
        value:
          status.current_operation && status.current_operation.length > 0
            ? status.current_operation
            : t("settings.none"),
      },
    ],
    [status, t],
  );

  const actionBlockMessage = useMemo(() => {
    if (!hasCommittedMasterPassword) {
      return t("settings.masterPasswordRequiredDesc");
    }
    if (isDirty) {
      return t("settings.applySettingsFirst");
    }
    if (committedValidationErrors.length > 0) {
      return getValidationMessage(committedValidationErrors[0], t);
    }
    return null;
  }, [committedValidationErrors, hasCommittedMasterPassword, isDirty, t]);

  const renderOAuthDriveFields = (
    provider: "google_drive" | "onedrive",
    label: string,
    desc: string,
  ) => {
    const providerSettings = settings[provider];

    return (
      <SettingFieldGrid>
        <SettingInput
          label={t("settings.providerRoot")}
          desc={t("settings.providerRootDesc")}
          value={providerSettings.root}
          placeholder="/apps/nyaterm"
          disabled={formDisabled}
          onChange={(event) =>
            updateCloudSync({
              [provider]: { ...providerSettings, root: event.target.value },
            } as Partial<CloudSyncSettings>)
          }
        />
        <SettingInput
          label={t("settings.driveAccessToken")}
          desc={t("settings.driveAccessTokenDesc")}
          type="password"
          value={secretInputValue(providerSettings.access_token)}
          placeholder={secretPlaceholder(
            providerSettings.access_token,
            t("settings.optionalField"),
          )}
          disabled={formDisabled}
          onChange={(event) =>
            updateCloudSync({
              [provider]: { ...providerSettings, access_token: event.target.value },
            } as Partial<CloudSyncSettings>)
          }
        />
        <SettingInput
          label={t("settings.driveRefreshToken")}
          desc={desc}
          type="password"
          value={secretInputValue(providerSettings.refresh_token)}
          placeholder={secretPlaceholder(
            providerSettings.refresh_token,
            t("settings.driveRefreshToken"),
          )}
          disabled={formDisabled}
          onChange={(event) =>
            updateCloudSync({
              [provider]: { ...providerSettings, refresh_token: event.target.value },
            } as Partial<CloudSyncSettings>)
          }
        />
        <SettingInput
          label={t("settings.driveClientId")}
          desc={label}
          value={providerSettings.client_id ?? ""}
          disabled={formDisabled}
          onChange={(event) =>
            updateCloudSync({
              [provider]: { ...providerSettings, client_id: event.target.value },
            } as Partial<CloudSyncSettings>)
          }
        />
        <SettingInput
          label={t("settings.driveClientSecret")}
          type="password"
          value={secretInputValue(providerSettings.client_secret)}
          placeholder={secretPlaceholder(
            providerSettings.client_secret,
            t("settings.driveClientSecret"),
          )}
          disabled={formDisabled}
          onChange={(event) =>
            updateCloudSync({
              [provider]: { ...providerSettings, client_secret: event.target.value },
            } as Partial<CloudSyncSettings>)
          }
        />
      </SettingFieldGrid>
    );
  };

  if (loading) {
    return (
      <div className="py-10 text-sm text-muted-foreground">
        {t("common.loading")}
      </div>
    );
  }

  return (
    <div className="min-w-0 space-y-5">
      <SettingSection
        title={t("settings.syncProviderConfig")}
        desc={t("settings.syncProviderConfigDesc")}
        contentClassName="space-y-5"
      >
        <SettingRow label={t("settings.enableCloudSync")} desc={t("settings.enableCloudSyncDesc")}>
          {hasDraftMasterPassword ? (
            <SettingSwitch
              checked={settings.enabled}
              disabled={isSaving}
              onChange={handleEnabledChange}
            />
          ) : (
            <Tooltip>
              <TooltipTrigger asChild>
                <div className="inline-flex">
                  <SettingSwitch
                    checked={settings.enabled}
                    disabled
                    onChange={handleEnabledChange}
                  />
                </div>
              </TooltipTrigger>
              <TooltipContent side="top">{t("settings.masterPasswordRequiredDesc")}</TooltipContent>
            </Tooltip>
          )}
        </SettingRow>

        {!hasDraftMasterPassword ? (
          <div className="rounded-xl border border-amber-500/30 bg-amber-500/10 px-4 py-4">
            <div className="text-sm text-muted-foreground">
              {t("settings.syncMasterPasswordMissingDesc")}
            </div>
            <div className="mt-3">
              <Button size="sm" onClick={onNavigateSecurity}>
                {t("settings.openSecuritySettings")}
              </Button>
            </div>
          </div>
        ) : null}

        {settings.enabled && validationErrors.length > 0 ? (
          <div className="rounded-xl border border-amber-500/30 bg-amber-500/10 px-4 py-3 text-sm text-muted-foreground">
            {getValidationMessage(validationErrors[0], t)}
          </div>
        ) : null}

        <SettingFieldGrid>
          <SettingSelect
            label={t("settings.syncProvider")}
            desc={t("settings.syncProviderDesc")}
            value={settings.provider}
            disabled={formDisabled}
            onValueChange={(provider) => updateCloudSync({ provider })}
          >
            <SelectItem value="webdav">WebDAV</SelectItem>
            <SelectItem value="s3">S3 Compatible</SelectItem>
            <SelectItem value="gitee_snippet">Gitee Snippet</SelectItem>
            <SelectItem value="github_gist">GitHub Gist</SelectItem>
            <SelectItem value="google_drive">Google Drive</SelectItem>
            <SelectItem value="onedrive">OneDrive</SelectItem>
            <SelectItem value="aliyun_drive">AliyunDrive</SelectItem>
          </SettingSelect>

          <SettingInput
            label={t("settings.deviceName")}
            desc={t("settings.deviceNameDesc")}
            value={settings.device_name}
            disabled={formDisabled}
            onChange={(event) => updateCloudSync({ device_name: event.target.value })}
          />

          <SettingInput
            label={t("settings.remoteNamespace")}
            desc={t("settings.remoteNamespaceDesc")}
            value={settings.remote_root}
            disabled={formDisabled}
            onChange={(event) => updateCloudSync({ remote_root: event.target.value })}
          />
        </SettingFieldGrid>

        {settings.provider === "webdav" ? (
          <SettingFieldGrid>
            <SettingInput
              label={t("settings.webdavEndpoint")}
              value={settings.webdav.endpoint}
              placeholder="https://dav.example.com"
              disabled={formDisabled}
              onChange={(event) =>
                updateCloudSync({
                  webdav: { ...settings.webdav, endpoint: event.target.value },
                })
              }
            />
            <SettingInput
              label={t("settings.providerRoot")}
              desc={t("settings.providerRootDesc")}
              value={settings.webdav.root}
              placeholder="/apps/nyaterm"
              disabled={formDisabled}
              onChange={(event) =>
                updateCloudSync({
                  webdav: { ...settings.webdav, root: event.target.value },
                })
              }
            />
            <SettingInput
              label={t("dialog.username")}
              value={settings.webdav.username}
              disabled={formDisabled}
              onChange={(event) =>
                updateCloudSync({
                  webdav: { ...settings.webdav, username: event.target.value },
                })
              }
            />
            <SettingInput
              label={t("dialog.password")}
              type="password"
              value={secretInputValue(settings.webdav.password)}
              placeholder={secretPlaceholder(settings.webdav.password, t("dialog.password"))}
              disabled={formDisabled}
              onChange={(event) =>
                updateCloudSync({
                  webdav: { ...settings.webdav, password: event.target.value },
                })
              }
            />
          </SettingFieldGrid>
        ) : settings.provider === "s3" ? (
          <SettingFieldGrid>
            <SettingInput
              label={t("settings.s3Endpoint")}
              value={settings.s3.endpoint}
              placeholder="https://s3.example.com"
              disabled={formDisabled}
              onChange={(event) =>
                updateCloudSync({
                  s3: { ...settings.s3, endpoint: event.target.value },
                })
              }
            />
            <SettingInput
              label={t("settings.s3Bucket")}
              value={settings.s3.bucket}
              disabled={formDisabled}
              onChange={(event) =>
                updateCloudSync({
                  s3: { ...settings.s3, bucket: event.target.value },
                })
              }
            />
            <SettingInput
              label={t("settings.s3Region")}
              value={settings.s3.region}
              placeholder="auto"
              disabled={formDisabled}
              onChange={(event) =>
                updateCloudSync({
                  s3: { ...settings.s3, region: event.target.value },
                })
              }
            />
            <SettingInput
              label={t("settings.providerRoot")}
              desc={t("settings.providerRootDesc")}
              value={settings.s3.root}
              placeholder="/apps/nyaterm"
              disabled={formDisabled}
              onChange={(event) =>
                updateCloudSync({
                  s3: { ...settings.s3, root: event.target.value },
                })
              }
            />
            <SettingInput
              label={t("settings.s3AccessKeyId")}
              value={secretInputValue(settings.s3.access_key_id)}
              placeholder={secretPlaceholder(settings.s3.access_key_id, "AKIA...")}
              disabled={formDisabled}
              onChange={(event) =>
                updateCloudSync({
                  s3: { ...settings.s3, access_key_id: event.target.value },
                })
              }
            />
            <SettingInput
              label={t("settings.s3SecretAccessKey")}
              type="password"
              value={secretInputValue(settings.s3.secret_access_key)}
              placeholder={secretPlaceholder(
                settings.s3.secret_access_key,
                t("settings.s3SecretAccessKey"),
              )}
              disabled={formDisabled}
              onChange={(event) =>
                updateCloudSync({
                  s3: { ...settings.s3, secret_access_key: event.target.value },
                })
              }
            />
            <SettingInput
              label={t("settings.s3SessionToken")}
              type="password"
              value={secretInputValue(settings.s3.session_token)}
              placeholder={secretPlaceholder(
                settings.s3.session_token,
                t("settings.optionalField"),
              )}
              disabled={formDisabled}
              onChange={(event) =>
                updateCloudSync({
                  s3: { ...settings.s3, session_token: event.target.value },
                })
              }
            />
            <SettingRow
              label={t("settings.s3VirtualHostStyle")}
              desc={t("settings.s3VirtualHostStyleDesc")}
            >
              <SettingSwitch
                checked={settings.s3.virtual_host_style}
                disabled={formDisabled}
                onChange={(virtual_host_style) =>
                  updateCloudSync({
                    s3: { ...settings.s3, virtual_host_style },
                  })
                }
              />
            </SettingRow>
          </SettingFieldGrid>
        ) : settings.provider === "gitee_snippet" ? (
          <SettingFieldGrid>
            <SettingInput
              label={t("settings.giteeSnippetApiEndpoint")}
              desc={t("settings.giteeSnippetApiEndpointDesc")}
              value={settings.gitee_snippet.api_endpoint}
              placeholder="https://gitee.com/api/v5"
              disabled={formDisabled}
              onChange={(event) =>
                updateCloudSync({
                  gitee_snippet: {
                    ...settings.gitee_snippet,
                    api_endpoint: event.target.value,
                  },
                })
              }
            />
            <SettingInput
              label={t("settings.giteeSnippetId")}
              desc={t("settings.giteeSnippetIdDesc")}
              value={settings.gitee_snippet.gist_id}
              disabled={formDisabled}
              onChange={(event) =>
                updateCloudSync({
                  gitee_snippet: { ...settings.gitee_snippet, gist_id: event.target.value },
                })
              }
            />
            <SettingInput
              label={t("settings.giteeSnippetAccessToken")}
              desc={t("settings.giteeSnippetAccessTokenDesc")}
              type="password"
              value={secretInputValue(settings.gitee_snippet.access_token)}
              placeholder={secretPlaceholder(
                settings.gitee_snippet.access_token,
                t("settings.giteeSnippetAccessToken"),
              )}
              disabled={formDisabled}
              onChange={(event) =>
                updateCloudSync({
                  gitee_snippet: {
                    ...settings.gitee_snippet,
                    access_token: event.target.value,
                  },
                })
              }
            />
          </SettingFieldGrid>
        ) : settings.provider === "github_gist" ? (
          <SettingFieldGrid>
            <SettingInput
              label={t("settings.githubGistId")}
              desc={t("settings.githubGistIdDesc")}
              value={settings.github_gist.gist_id}
              disabled={formDisabled || githubAuth.flow !== null}
              onChange={(event) =>
                updateCloudSync({
                  github_gist: { ...settings.github_gist, gist_id: event.target.value },
                })
              }
            />
            <SettingRow
              label={t("settings.githubGistAuth")}
              desc={t("settings.githubGistAuthDesc")}
            >
              <div className="flex flex-wrap items-center gap-2">
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  disabled={formDisabled || runningAction !== null || githubAuth.flow !== null}
                  onClick={() => void handleStartGithubGistAuth()}
                >
                  {settings.github_gist.access_token
                    ? t("settings.githubGistReconnect")
                    : t("settings.githubGistConnect")}
                </Button>
                {githubAuth.flow ? (
                  <Button
                    type="button"
                    variant="ghost"
                    size="sm"
                    disabled={runningAction !== null}
                    onClick={() => void handleCancelGithubGistAuth()}
                  >
                    {t("common.cancel")}
                  </Button>
                ) : null}
              </div>
            </SettingRow>
            {githubAuth.flow ? (
              <div className="rounded-lg border border-border/70 bg-muted/15 px-4 py-3">
                <div className="text-xs text-muted-foreground">
                  {t("settings.githubGistUserCode")}
                </div>
                <div className="mt-1 flex flex-wrap items-center gap-2">
                  <div className="font-mono text-lg font-semibold tracking-[0.2em]">
                    {githubAuth.flow.user_code}
                  </div>
                  <Button
                    type="button"
                    variant="outline"
                    size="xs"
                    onClick={() => void handleCopyGithubGistUserCode()}
                  >
                    <Copy />
                    {t("settings.copyGithubGistUserCode")}
                  </Button>
                </div>
                <Button
                  type="button"
                  variant="link"
                  className="mt-1 h-auto p-0"
                  onClick={() => void openUrl(githubAuth.flow?.verification_uri ?? "")}
                >
                  {githubAuth.flow.verification_uri}
                </Button>
              </div>
            ) : null}
            {githubAuth.message || githubAuth.login || settings.github_gist.gist_id ? (
              <div className="min-w-0 rounded-lg border border-border/70 bg-card/60 px-4 py-3 text-sm text-muted-foreground break-words [overflow-wrap:anywhere]">
                {githubAuth.login ? (
                  <div>{t("settings.githubGistConnectedAs", { login: githubAuth.login })}</div>
                ) : null}
                {settings.github_gist.gist_id ? (
                  <div>
                    {t("settings.githubGistCurrentId", {
                      gistId: shortValue(settings.github_gist.gist_id, 10),
                    })}
                  </div>
                ) : null}
                {githubAuth.message ? <div>{githubAuth.message}</div> : null}
              </div>
            ) : null}
          </SettingFieldGrid>
        ) : settings.provider === "google_drive" ? (
          renderOAuthDriveFields(
            "google_drive",
            t("settings.googleDriveClientIdDesc"),
            t("settings.googleDriveRefreshTokenDesc"),
          )
        ) : settings.provider === "onedrive" ? (
          renderOAuthDriveFields(
            "onedrive",
            t("settings.onedriveClientIdDesc"),
            t("settings.onedriveRefreshTokenDesc"),
          )
        ) : settings.provider === "aliyun_drive" ? (
          <SettingFieldGrid>
            <SettingInput
              label={t("settings.providerRoot")}
              desc={t("settings.providerRootDesc")}
              value={settings.aliyun_drive.root}
              placeholder="/apps/nyaterm"
              disabled={formDisabled}
              onChange={(event) =>
                updateCloudSync({
                  aliyun_drive: { ...settings.aliyun_drive, root: event.target.value },
                })
              }
            />
            <SettingInput
              label={t("settings.aliyunDriveType")}
              desc={t("settings.aliyunDriveTypeDesc")}
              value={settings.aliyun_drive.drive_type}
              placeholder="resource"
              disabled={formDisabled}
              onChange={(event) =>
                updateCloudSync({
                  aliyun_drive: { ...settings.aliyun_drive, drive_type: event.target.value },
                })
              }
            />
            <SettingInput
              label={t("settings.driveAccessToken")}
              desc={t("settings.driveAccessTokenDesc")}
              type="password"
              value={secretInputValue(settings.aliyun_drive.access_token)}
              placeholder={secretPlaceholder(
                settings.aliyun_drive.access_token,
                t("settings.optionalField"),
              )}
              disabled={formDisabled}
              onChange={(event) =>
                updateCloudSync({
                  aliyun_drive: {
                    ...settings.aliyun_drive,
                    access_token: event.target.value,
                  },
                })
              }
            />
            <SettingInput
              label={t("settings.driveRefreshToken")}
              desc={t("settings.aliyunDriveRefreshTokenDesc")}
              type="password"
              value={secretInputValue(settings.aliyun_drive.refresh_token)}
              placeholder={secretPlaceholder(
                settings.aliyun_drive.refresh_token,
                t("settings.driveRefreshToken"),
              )}
              disabled={formDisabled}
              onChange={(event) =>
                updateCloudSync({
                  aliyun_drive: {
                    ...settings.aliyun_drive,
                    refresh_token: event.target.value,
                  },
                })
              }
            />
            <SettingInput
              label={t("settings.driveClientId")}
              desc={t("settings.aliyunDriveClientIdDesc")}
              value={settings.aliyun_drive.client_id ?? ""}
              disabled={formDisabled}
              onChange={(event) =>
                updateCloudSync({
                  aliyun_drive: { ...settings.aliyun_drive, client_id: event.target.value },
                })
              }
            />
            <SettingInput
              label={t("settings.driveClientSecret")}
              type="password"
              value={secretInputValue(settings.aliyun_drive.client_secret)}
              placeholder={secretPlaceholder(
                settings.aliyun_drive.client_secret,
                t("settings.driveClientSecret"),
              )}
              disabled={formDisabled}
              onChange={(event) =>
                updateCloudSync({
                  aliyun_drive: {
                    ...settings.aliyun_drive,
                    client_secret: event.target.value,
                  },
                })
              }
            />
          </SettingFieldGrid>
        ) : null}
      </SettingSection>

      <SettingSection
        title={t("settings.autoSyncStrategy")}
        desc={t("settings.autoSyncStrategyDesc")}
        contentClassName="space-y-5"
      >
        <SettingRow
          label={t("settings.autoCheckOnStartup")}
          desc={t("settings.autoCheckOnStartupDesc")}
        >
          <SettingSwitch
            checked={settings.auto_check_on_startup}
            disabled={autoSyncSectionDisabled}
            onChange={(auto_check_on_startup) => updateCloudSync({ auto_check_on_startup })}
          />
        </SettingRow>
        <SettingRow
          label={t("settings.autoPushOnChange")}
          desc={t("settings.autoPushOnChangeDesc")}
        >
          <SettingSwitch
            checked={settings.auto_push_on_change}
            disabled={autoSyncSectionDisabled}
            onChange={(auto_push_on_change) => updateCloudSync({ auto_push_on_change })}
          />
        </SettingRow>
        <SettingRow
          label={t("settings.autoPullRemoteChanges")}
          desc={t("settings.autoPullRemoteChangesDesc")}
        >
          <SettingSwitch
            checked={settings.auto_pull_remote_changes}
            disabled={autoSyncSectionDisabled}
            onChange={(auto_pull_remote_changes) => updateCloudSync({ auto_pull_remote_changes })}
          />
        </SettingRow>
        <SettingNumberInput
          label={t("settings.syncDebounceSeconds")}
          desc={t("settings.syncDebounceSecondsDesc")}
          value={settings.sync_debounce_seconds}
          disabled={autoSyncSectionDisabled || !settings.auto_push_on_change}
          min={1}
          max={3600}
          onChange={(sync_debounce_seconds) => updateCloudSync({ sync_debounce_seconds })}
        />
      </SettingSection>

      <SettingSection title={t("settings.manualSyncActions")} contentClassName="space-y-5">
        {actionBlockMessage ? (
          <div className="rounded-xl border border-amber-500/30 bg-amber-500/10 px-4 py-3 text-sm text-muted-foreground">
            {actionBlockMessage}
          </div>
        ) : null}

        <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-3">
          {statusItems.map((item) => (
            <div
              key={item.label}
              className="rounded-lg border border-border/70 bg-muted/15 px-3 py-3"
            >
              <div className="text-[0.6875rem] font-semibold uppercase tracking-[0.14em] text-muted-foreground">
                {item.label}
              </div>
              <div className="mt-2 min-w-0 break-words text-sm font-medium [overflow-wrap:anywhere]">
                {item.value}
              </div>
            </div>
          ))}
        </div>

        {status.message ? (
          <div className="min-w-0 rounded-lg border border-border/70 bg-card/60 px-4 py-3 text-sm text-muted-foreground break-words [overflow-wrap:anywhere]">
            {status.message}
          </div>
        ) : null}

        <div className="flex flex-wrap gap-2">
          <Button
            variant="outline"
            onClick={() =>
              void runAction(
                "test",
                t("settings.syncTestSuccess"),
                () => invoke("test_cloud_sync_connection"),
                { allowWhenDisabled: true },
              )
            }
            disabled={isBusy || !canRunConfigDependentActions}
          >
            {t("settings.testConnection")}
          </Button>
          <Button
            variant="outline"
            onClick={() =>
              void runAction("push", t("settings.syncPushSuccess"), () => invoke("sync_push_now"))
            }
            disabled={isBusy || !canRunEnabledActions}
          >
            {t("settings.syncPushNow")}
          </Button>
          <Button
            variant="outline"
            onClick={() =>
              void runAction("pull", t("settings.syncPullSuccess"), () => invoke("sync_pull_now"))
            }
            disabled={isBusy || !canRunEnabledActions}
          >
            {t("settings.syncPullNow")}
          </Button>
          {showGistCapacityRecovery ? (
            <Button
              onClick={() =>
                void runAction("prune-retry-push", t("settings.syncPruneAndRetrySuccess"), () =>
                  invoke("sync_push_now"),
                )
              }
              disabled={isBusy || !canRunEnabledActions}
            >
              {t("settings.syncPruneAndRetry")}
            </Button>
          ) : null}
        </div>
        {showGistCapacityRecovery ? (
          <p className="text-xs leading-5 text-muted-foreground">
            {t("settings.syncGistCapacityHint")}
          </p>
        ) : null}
      </SettingSection>

      <SettingSection
        title={t("settings.syncConflictSection")}
        desc={t("settings.syncConflictSectionDesc")}
        contentClassName="space-y-4"
      >
        {status.conflict ? (
          <div className="rounded-lg border border-amber-500/30 bg-amber-500/10 px-4 py-4">
            <div className="text-sm font-semibold">
              {isRemoteInconsistent
                ? t("settings.syncRemoteIncompleteTitle")
                : t("settings.syncConflictTitle")}
            </div>
            <p className="mt-1 text-xs leading-5 text-muted-foreground">
              <span className="break-words [overflow-wrap:anywhere]">
                {status.conflict.message}
              </span>
            </p>
            <div className="mt-3 grid gap-2 md:grid-cols-2">
              <div className="rounded-md border border-border/70 bg-background/70 px-3 py-3">
                <div className="text-[0.6875rem] uppercase tracking-[0.14em] text-muted-foreground">
                  {t("settings.localSnapshot")}
                </div>
                <div className="mt-2 text-xs font-medium">
                  {shortValue(status.conflict.local_payload_hash, 10)}
                </div>
              </div>
              <div className="rounded-md border border-border/70 bg-background/70 px-3 py-3">
                <div className="text-[0.6875rem] uppercase tracking-[0.14em] text-muted-foreground">
                  {t("settings.remoteSnapshot")}
                </div>
                <div className="mt-2 text-xs font-medium">
                  {shortValue(status.conflict.remote_revision, 10)}
                </div>
                <div className="mt-1 text-[0.6875rem] text-muted-foreground">
                  {formatTimestamp(status.conflict.remote_created_at_ms)}
                </div>
              </div>
              {isRemoteInconsistent ? (
                <div className="rounded-md border border-border/70 bg-background/70 px-3 py-3 md:col-span-2">
                  <div className="text-[0.6875rem] uppercase tracking-[0.14em] text-muted-foreground">
                    {t("settings.currentRemoteSnapshot")}
                  </div>
                  <div className="mt-2 text-xs font-medium">
                    {shortValue(status.conflict.recovery_revision, 10)}
                  </div>
                  <div className="mt-1 text-[0.6875rem] text-muted-foreground">
                    {formatTimestamp(status.conflict.recovery_created_at_ms)}
                  </div>
                </div>
              ) : null}
            </div>
            {status.conflict.kind === "content_conflict" ? (
              <p className="mt-3 text-xs leading-5 text-muted-foreground">
                {t("settings.syncMergeConnectionsHint")}
              </p>
            ) : null}
            <div className="mt-3 flex flex-wrap gap-2">
              {isRemoteInconsistent ? (
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() =>
                    void runAction(
                      "resolve-recover-current",
                      t("settings.syncRecoverCurrentSuccess"),
                      () =>
                        invoke("resolve_cloud_sync_conflict", {
                          action: "recover_current_remote",
                        }),
                      { allowWhenDisabled: true },
                    )
                  }
                  disabled={isBusy || !canRunConfigDependentActions}
                >
                  {t("settings.useCurrentRemoteSnapshot")}
                </Button>
              ) : (
                <>
                  {status.conflict.kind === "content_conflict" ? (
                    <Button
                      variant="outline"
                      size="sm"
                      onClick={() =>
                        void runAction(
                          "resolve-merge-connections",
                          t("settings.syncResolveMergeSuccess"),
                          () =>
                            invoke("resolve_cloud_sync_conflict", {
                              action: "merge_connections",
                            }),
                        )
                      }
                      disabled={isBusy || !canRunEnabledActions}
                    >
                      {t("settings.syncMergeConnections")}
                    </Button>
                  ) : null}
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() =>
                      void runAction(
                        "resolve-download",
                        t("settings.syncResolveDownloadSuccess"),
                        () =>
                          invoke("resolve_cloud_sync_conflict", {
                            action: "download_remote",
                          }),
                        { allowWhenDisabled: true },
                      )
                    }
                    disabled={isBusy || !canRunConfigDependentActions}
                  >
                    {t("settings.downloadRemoteVersion")}
                  </Button>
                </>
              )}
              <Button
                size="sm"
                onClick={() =>
                  void runAction(
                    "resolve-upload",
                    t("settings.syncResolveUploadSuccess"),
                    () =>
                      invoke("resolve_cloud_sync_conflict", {
                        action: "upload_local",
                      }),
                    { allowWhenDisabled: true },
                  )
                }
                disabled={isBusy || !canRunConfigDependentActions}
              >
                {t("settings.uploadLocalVersion")}
              </Button>
            </div>
          </div>
        ) : null}

        {!status.conflict ? (
          <div className="rounded-lg border border-dashed border-border/70 px-4 py-8 text-center text-sm text-muted-foreground">
            {t("settings.noSyncConflict")}
          </div>
        ) : null}
      </SettingSection>
    </div>
  );
}
