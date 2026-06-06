import { emit } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { FaServer } from "react-icons/fa6";
import { MdAdd, MdExpandMore } from "react-icons/md";
import {
  buildGroupPath,
  type ConnectionOption,
  sortLabel,
} from "@/components/dialog/network/shared";
import { SYSTEM_ICONS } from "@/components/icons";
import ChildWindowHeader from "@/components/layout/ChildWindowHeader";
import { LocalTerminal } from "@/components/sessions/LocalTerminal";
import { SerialForm } from "@/components/sessions/SerialForm";
import { type SshAuthMode, SshForm } from "@/components/sessions/SshForm";
import { TelnetForm } from "@/components/sessions/TelnetForm";
import { ActionButton, ActionFooter } from "@/components/ui/action-footer";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import { getErrorMessage } from "@/lib/errors";
import { invoke } from "@/lib/invoke";
import { isValidSerialBaudRate, MAX_SERIAL_BAUD_RATE, MIN_SERIAL_BAUD_RATE } from "@/lib/serial";
import type { Group, OtpEntry, ProxyConfig, SavedConnection } from "@/types/global";

const isValidPort = (value: number) => Number.isInteger(value) && value >= 1 && value <= 65535;
const DEFAULT_POST_LOGIN_DELAY_MS = 1000;
const MIN_POST_LOGIN_DELAY_MS = 0;
const MAX_POST_LOGIN_DELAY_MS = 60_000;

const isValidPostLoginDelay = (value: number) =>
  Number.isInteger(value) && value >= MIN_POST_LOGIN_DELAY_MS && value <= MAX_POST_LOGIN_DELAY_MS;

export default function NewSessionPage() {
  const { t } = useTranslation();
  const params = new URLSearchParams(window.location.search);
  const editId = params.get("edit") ?? undefined;
  const autoConnect = params.get("autoConnect") === "1";
  const ownerWindowLabel = params.get("owner") ?? undefined;
  const initialGroupId = editId ? "" : (params.get("groupId") ?? "");
  const targetLeafId = params.get("targetLeafId") ?? undefined;
  const anchorTabId = params.get("anchorTabId") ?? undefined;
  const sourceTabId = params.get("sourceTabId") ?? undefined;
  const sourcePaneId = params.get("sourcePaneId") ?? undefined;

  const [initialData, setInitialData] = useState<SavedConnection | undefined>();
  const [name, setName] = useState("");
  const [groupId, setGroupId] = useState(initialGroupId);
  const [newGroupNamePending, setNewGroupNamePending] = useState("");
  const [description, setDescription] = useState("");
  const [host, setHost] = useState("");
  const [sshPort, setSshPort] = useState(22);
  const [telnetPort, setTelnetPort] = useState(23);
  const [username, setUsername] = useState("root");
  const [authType, setAuthType] = useState<SshAuthMode>("password");
  const [passwordId, setPasswordId] = useState("");
  const [password, setPassword] = useState("");
  const [hasPassword, setHasPassword] = useState(false);
  const [keyId, setKeyId] = useState("");
  const [iconKey, setIconKey] = useState("");
  const [showIconPicker, setShowIconPicker] = useState(false);
  const [connecting, setConnecting] = useState(false);
  const [error, setError] = useState("");
  const [groups, setGroups] = useState<Group[]>([]);
  const [savedConnections, setSavedConnections] = useState<SavedConnection[]>([]);
  const [showGroupDropdown, setShowGroupDropdown] = useState(false);
  const [newGroupName, setNewGroupName] = useState("");
  const [newGroupParentId, setNewGroupParentId] = useState("");
  const [currentTab, setCurrentTab] = useState("ssh");

  // Proxy
  const [proxyId, setProxyId] = useState("");
  const [jumpHostId, setJumpHostId] = useState("");
  const [proxies, setProxies] = useState<ProxyConfig[]>([]);

  // OTP / 2FA
  const [otpId, setOtpId] = useState("");
  const [autoFillOtp, setAutoFillOtp] = useState(false);
  const [otpEntries, setOtpEntries] = useState<OtpEntry[]>([]);

  // SSH post-login command
  const [postLoginEnabled, setPostLoginEnabled] = useState(false);
  const [postLoginCommand, setPostLoginCommand] = useState("");
  const [postLoginDelayMs, setPostLoginDelayMs] = useState(DEFAULT_POST_LOGIN_DELAY_MS);

  // Serial Settings States
  const [serialPortName, setSerialPortName] = useState("");
  const [serialPorts, setSerialPorts] = useState<string[]>([]);
  const [serialPortsLoading, setSerialPortsLoading] = useState(false);
  const [serialPortsError, setSerialPortsError] = useState("");
  const [baudRate, setBaudRate] = useState("115200");
  const [dataBits, setDataBits] = useState("8");
  const [parity, setParity] = useState("none");
  const [stopBits, setStopBits] = useState("1");

  // Local Terminal States
  const [shellPath, setShellPath] = useState("powershell.exe");
  const [shellArgs, setShellArgs] = useState("");
  const [workingDir, setWorkingDir] = useState("");
  const [serialBackspaceMode, setSerialBackspaceMode] = useState("ctrl_h");
  const [telnetBackspaceMode, setTelnetBackspaceMode] = useState("del");

  useEffect(() => {
    invoke<Group[]>("get_groups")
      .then(setGroups)
      .catch((e) => setError(getErrorMessage(e)));
    invoke<ProxyConfig[]>("get_proxies")
      .then(setProxies)
      .catch((e) => setError(getErrorMessage(e)));
    invoke<OtpEntry[]>("get_otp_entries")
      .then(setOtpEntries)
      .catch((e) => setError(getErrorMessage(e)));
    invoke<SavedConnection[]>("get_saved_connections")
      .then((conns) => {
        setSavedConnections(conns);
        if (!editId) {
          return;
        }

        const found = conns.find((connection) => connection.id === editId);
        if (!found) {
          setError(t("dialog.connectionNotFound"));
          return;
        }

        setInitialData(found);
        setName(found.name);
        setGroupId(found.group_id || "");
        setDescription(found.description || "");
        setIconKey(found.icon || "");

        const tabMap: Record<string, string> = {
          ssh: "ssh",
          local_terminal: "local",
          telnet: "telnet",
          serial: "serial",
        };
        setCurrentTab(tabMap[found.type] || "ssh");

        if (found.type === "ssh") {
          setHost(found.host || "");
          setSshPort(found.port || 22);
          setUsername(found.username || "root");
          setAuthType((found.auth?.mode as SshAuthMode) || "password");
          setPasswordId(found.auth?.password_id || "");
          setHasPassword(found.auth?.has_password || false);
          setKeyId(found.auth?.key_id || "");
          setProxyId(found.network?.proxy_id || "");
          setJumpHostId(found.network?.proxy_jump_id || "");
          setOtpId(found.auth?.otp_id || "");
          setAutoFillOtp(found.auth?.auto_fill_otp || false);
          setPostLoginEnabled(found.post_login?.enabled ?? false);
          setPostLoginCommand(found.post_login?.command ?? "");
          setPostLoginDelayMs(found.post_login?.delay_ms ?? DEFAULT_POST_LOGIN_DELAY_MS);
        } else if (found.type === "telnet") {
          setHost(found.host || "");
          setTelnetPort(found.port || 23);
          setTelnetBackspaceMode(found.backspace_mode || "del");
        } else if (found.type === "local_terminal") {
          setShellPath(found.shell_path || "powershell.exe");
          setShellArgs(found.shell_args || "");
          setWorkingDir(found.working_dir || "");
        } else if (found.type === "serial") {
          setSerialPortName(found.port_name || "");
          setBaudRate(String(found.baud_rate || 115200));
          setDataBits(String(found.data_bits || 8));
          setParity(found.parity || "none");
          setStopBits(found.stop_bits || "1");
          setSerialBackspaceMode(found.backspace_mode || "ctrl_h");
        }
      })
      .catch((e) => setError(getErrorMessage(e)));
  }, [editId, t]);

  const loadSerialPorts = useCallback(async () => {
    setSerialPortsLoading(true);
    setSerialPortsError("");

    try {
      const ports = await invoke<string[]>("list_serial_ports");
      setSerialPorts(ports);
    } catch (e) {
      setSerialPortsError(
        `${t("dialog.serialPortsLoadFailed", "Failed to load serial ports")}: ${String(e)}`,
      );
    } finally {
      setSerialPortsLoading(false);
    }
  }, [t]);

  useEffect(() => {
    if (currentTab === "serial") {
      void loadSerialPorts();
    }
  }, [currentTab, loadSerialPorts]);

  const resetForm = useCallback(() => {
    setName("");
    setGroupId("");
    setNewGroupNamePending("");
    setDescription("");
    setHost("");
    setSshPort(22);
    setTelnetPort(23);
    setUsername("root");
    setAuthType("password");
    setPasswordId("");
    setPassword("");
    setHasPassword(false);
    setKeyId("");
    setIconKey("");
    setProxyId("");
    setJumpHostId("");
    setOtpId("");
    setAutoFillOtp(false);
    setPostLoginEnabled(false);
    setPostLoginCommand("");
    setPostLoginDelayMs(DEFAULT_POST_LOGIN_DELAY_MS);
    setSerialPortName("");
    setSerialPorts([]);
    setSerialPortsLoading(false);
    setSerialPortsError("");
    setBaudRate("115200");
    setDataBits("8");
    setParity("none");
    setStopBits("1");
    setShellPath("powershell.exe");
    setShellArgs("");
    setWorkingDir("");
    setSerialBackspaceMode("ctrl_h");
    setTelnetBackspaceMode("del");
    setShowIconPicker(false);
    setError("");
    setConnecting(false);
  }, []);

  const serialPortOptions: { unavailable?: boolean; value: string }[] = serialPorts.map((port) => ({
    value: port,
  }));
  if (serialPortName && !serialPorts.includes(serialPortName)) {
    serialPortOptions.unshift({
      value: serialPortName,
      unavailable: true,
    });
  }

  const groupsById = useMemo(() => new Map(groups.map((group) => [group.id, group])), [groups]);

  const selectedGroupLabel = useMemo(() => {
    if (groupId === "new") {
      return newGroupNamePending || t("dialog.newGroupPlaceholder");
    }
    if (!groupId) {
      return t("dialog.none");
    }
    return buildGroupPath(groupId, groupsById) || t("dialog.none");
  }, [groupId, groupsById, newGroupNamePending, t]);

  const newGroupParentLabel = useMemo(() => {
    if (!groupId || groupId === "new") {
      return "";
    }
    return buildGroupPath(groupId, groupsById);
  }, [groupId, groupsById]);

  const jumpHostOptions = useMemo<ConnectionOption[]>(() => {
    return savedConnections
      .filter((connection) => connection.type === "ssh" && connection.id !== editId)
      .map((connection) => {
        const groupPath = buildGroupPath(connection.group_id, groupsById);
        const subtitle = [
          groupPath,
          connection.host && `${connection.host}:${connection.port ?? 22}`,
          connection.username,
        ]
          .filter(Boolean)
          .join(" · ");

        return {
          connection,
          groupPath,
          subtitle,
          searchText: [connection.name, connection.host, connection.username, groupPath]
            .filter(Boolean)
            .join(" "),
          disabled: !!connection.network?.proxy_jump_id,
          disabledReason: connection.network?.proxy_jump_id
            ? t("dialog.proxyJumpAlreadyConfigured")
            : undefined,
        };
      })
      .sort((left, right) => {
        const pathSort = sortLabel(left.groupPath, right.groupPath);
        return pathSort !== 0 ? pathSort : sortLabel(left.connection.name, right.connection.name);
      });
  }, [editId, groupsById, savedConnections, t]);

  const handleClose = () => {
    if (connecting) return;
    getCurrentWindow().close();
  };

  const getValidationError = useCallback(() => {
    if (currentTab === "ssh") {
      if (!host.trim()) {
        return t("dialog.hostRequired");
      }
      if (!isValidPort(sshPort)) {
        return t("dialog.portInvalid", "Port must be between 1 and 65535");
      }
      if (!username.trim()) {
        return t("dialog.usernameRequired", "Username is required");
      }
      if (authType === "password" && !passwordId && !password && !hasPassword) {
        return t("dialog.passwordRequired");
      }
      if (postLoginEnabled && !postLoginCommand.trim()) {
        return t("dialog.postLoginCommandRequired");
      }
      if (!isValidPostLoginDelay(postLoginDelayMs)) {
        return t("dialog.postLoginDelayInvalid", {
          min: MIN_POST_LOGIN_DELAY_MS,
          max: MAX_POST_LOGIN_DELAY_MS,
          defaultValue: "Delay must be between {{min}} and {{max}} ms",
        });
      }
    }

    if (currentTab === "telnet") {
      if (!host.trim()) {
        return t("dialog.hostRequired");
      }
      if (!isValidPort(telnetPort)) {
        return t("dialog.portInvalid", "Port must be between 1 and 65535");
      }
    }

    if (currentTab === "serial") {
      if (!serialPortName.trim()) {
        return t("dialog.serialPortRequired", "Serial port is required");
      }
      if (!isValidSerialBaudRate(baudRate)) {
        return t("dialog.baudRateInvalid", {
          min: MIN_SERIAL_BAUD_RATE,
          max: MAX_SERIAL_BAUD_RATE,
          defaultValue: "Baud rate must be between {{min}} and {{max}}",
        });
      }
    }

    if (currentTab === "local" && !shellPath.trim()) {
      return t("dialog.shellPathRequired", "Shell path is required");
    }

    return "";
  }, [
    authType,
    baudRate,
    currentTab,
    hasPassword,
    host,
    password,
    passwordId,
    postLoginCommand,
    postLoginDelayMs,
    postLoginEnabled,
    serialPortName,
    shellPath,
    sshPort,
    telnetPort,
    t,
    username,
  ]);

  const validationError = getValidationError();
  const saveDisabled = connecting || !!validationError;

  const handleSave = async () => {
    const nextValidationError = getValidationError();
    if (nextValidationError) {
      setError(nextValidationError);
      return;
    }

    setError("");
    setConnecting(true);

    try {
      const normalizedName = name.trim();
      const normalizedDescription = description.trim();
      const normalizedHost = host.trim();
      const normalizedUsername = username.trim();
      const normalizedSerialPortName = serialPortName.trim();
      const normalizedShellPath = shellPath.trim();
      const normalizedShellArgs = shellArgs.trim();
      const normalizedWorkingDir = workingDir.trim();
      let finalGroupId = groupId;
      if (groupId === "new" && newGroupNamePending) {
        finalGroupId = await invoke<string>("save_group", {
          group: {
            id: "",
            name: newGroupNamePending,
            parent_id: newGroupParentId || null,
            sort_order: groups.length,
          },
        });
      }

      const defaultName =
        currentTab === "local"
          ? t("dialog.localTerminal")
          : currentTab === "serial"
            ? normalizedSerialPortName
            : currentTab === "telnet"
              ? `${normalizedHost}:${telnetPort}`
              : `${normalizedHost}:${sshPort}`;

      const typeTag =
        currentTab === "ssh"
          ? "ssh"
          : currentTab === "local"
            ? "local_terminal"
            : currentTab === "telnet"
              ? "telnet"
              : "serial";
      const network =
        currentTab === "ssh"
          ? (() => {
              const nextNetwork: NonNullable<SavedConnection["network"]> = {};
              if (proxyId) {
                nextNetwork.proxy_id = proxyId;
              }
              if (jumpHostId) {
                nextNetwork.proxy_jump_id = jumpHostId;
              }
              return Object.keys(nextNetwork).length > 0 ? nextNetwork : undefined;
            })()
          : undefined;
      const auth =
        currentTab === "ssh"
          ? (() => {
              const resolvedAuthMode: SshAuthMode =
                authType === "password" ? "password" : authType === "key" && keyId ? "key" : "none";
              const nextAuth: NonNullable<SavedConnection["auth"]> = {
                mode: resolvedAuthMode,
                password_id: resolvedAuthMode === "password" ? passwordId || "" : "",
                key_id: resolvedAuthMode === "key" ? keyId : undefined,
                otp_id: otpId || undefined,
                auto_fill_otp: otpId ? autoFillOtp : undefined,
              };

              if (resolvedAuthMode !== "password" || passwordId) {
                nextAuth.password = "";
              } else if (password) {
                nextAuth.password = password;
              } else if (!hasPassword) {
                nextAuth.password = "";
              }

              return nextAuth;
            })()
          : undefined;
      const postLogin =
        currentTab === "ssh"
          ? (() => {
              const normalizedCommand = postLoginCommand.trim();
              if (
                !postLoginEnabled &&
                !normalizedCommand &&
                postLoginDelayMs === DEFAULT_POST_LOGIN_DELAY_MS
              ) {
                return undefined;
              }

              return {
                enabled: postLoginEnabled,
                command: postLoginCommand,
                delay_ms: postLoginDelayMs,
              };
            })()
          : undefined;

      const connection: SavedConnection = {
        id: initialData?.id || "",
        name: normalizedName || defaultName,
        type: typeTag as SavedConnection["type"],
        group_id: finalGroupId || undefined,
        description: normalizedDescription || undefined,
        icon: iconKey || undefined,
        ...(currentTab === "ssh"
          ? {
              host: normalizedHost,
              port: sshPort,
              username: normalizedUsername,
              auth,
              network,
              post_login: postLogin,
            }
          : {}),
        ...(currentTab === "telnet"
          ? {
              host: normalizedHost,
              port: telnetPort,
              backspace_mode: telnetBackspaceMode,
            }
          : {}),
        ...(currentTab === "local"
          ? {
              shell_path: normalizedShellPath,
              shell_args: normalizedShellArgs,
              working_dir: normalizedWorkingDir || undefined,
            }
          : {}),
        ...(currentTab === "serial"
          ? {
              port_name: normalizedSerialPortName,
              baud_rate: Number(baudRate),
              data_bits: Number(dataBits),
              parity,
              stop_bits: stopBits,
              backspace_mode: serialBackspaceMode,
            }
          : {}),
      };

      const savedId = await invoke<string>("save_connection", { connection });
      await emit("session-saved");
      if (autoConnect && (initialData?.id || savedId)) {
        await emit("session-connect-after-edit", {
          connectionId: initialData?.id || savedId,
          targetLeafId,
          anchorTabId,
          sourceTabId,
          sourcePaneId,
          targetWindowLabel: ownerWindowLabel,
        });
      }
      resetForm();
      getCurrentWindow().close();
    } catch (e) {
      setError(getErrorMessage(e));
    } finally {
      setConnecting(false);
    }
  };

  return (
    <div className="h-full min-h-0 flex flex-col overflow-hidden bg-background text-foreground">
      <ChildWindowHeader
        title={t(editId ? "dialog.editConnection" : "dialog.newConnection")}
        onClose={handleClose}
      />

      {/* Body */}
      <Tabs
        value={currentTab}
        onValueChange={setCurrentTab}
        className="flex-1 min-h-0 flex flex-col overflow-hidden"
      >
        <div className="shrink-0 px-4 pt-3 sm:px-5">
          <TabsList className="grid h-8 w-full grid-cols-4 pointer-events-auto">
            <TabsTrigger value="ssh" className="text-xs">
              SSH
            </TabsTrigger>
            <TabsTrigger value="local" className="text-xs">
              {t("dialog.localTerminal")}
            </TabsTrigger>
            <TabsTrigger value="telnet" className="text-xs">
              Telnet
            </TabsTrigger>
            <TabsTrigger value="serial" className="text-xs">
              {t("dialog.serial")}
            </TabsTrigger>
          </TabsList>
        </div>

        <div className="flex-1 min-h-0 w-full space-y-3 overflow-y-auto p-4 pb-20 sm:p-5 sm:pb-20">
          <div className="flex flex-wrap items-end gap-3">
            {/* Name + Group */}
            <div className="shrink-0">
              <Label className="mb-1 block text-xs font-medium text-foreground/80">
                {t("dialog.icon")}
              </Label>
              <Popover open={showIconPicker} onOpenChange={setShowIconPicker}>
                <PopoverTrigger asChild>
                  <Button
                    type="button"
                    variant="outline"
                    className="flex h-8 w-8 items-center justify-center p-0"
                    title={iconKey || t("dialog.none")}
                  >
                    {iconKey && SYSTEM_ICONS[iconKey] ? (
                      (() => {
                        const IconComp = SYSTEM_ICONS[iconKey].icon;
                        return (
                          <IconComp
                            style={{ color: SYSTEM_ICONS[iconKey].color }}
                            className="text-sm"
                          />
                        );
                      })()
                    ) : (
                      <FaServer className="text-sm text-muted-foreground" />
                    )}
                  </Button>
                </PopoverTrigger>
                <PopoverContent
                  align="start"
                  side="bottom"
                  collisionPadding={16}
                  className="w-56 max-w-[calc(100vw-2rem)] p-2"
                >
                  <div className="grid grid-cols-7 gap-0.5">
                    <button
                      type="button"
                      className={`flex h-7 w-7 items-center justify-center rounded transition-colors hover:bg-accent ${!iconKey ? "bg-primary/15 ring-1 ring-primary/40" : ""}`}
                      title={t("dialog.none")}
                      onClick={() => {
                        setIconKey("");
                        setShowIconPicker(false);
                      }}
                    >
                      <FaServer className="text-sm text-muted-foreground" />
                    </button>
                    {Object.entries(SYSTEM_ICONS).map(([key, def]) => {
                      const IconComp = def.icon;
                      return (
                        <button
                          key={key}
                          type="button"
                          className={`flex h-7 w-7 items-center justify-center rounded transition-colors hover:bg-accent ${iconKey === key ? "bg-primary/15 ring-1 ring-primary/40" : ""}`}
                          title={key}
                          onClick={() => {
                            setIconKey(key);
                            setShowIconPicker(false);
                          }}
                        >
                          <IconComp style={{ color: def.color }} className="text-sm" />
                        </button>
                      );
                    })}
                  </div>
                </PopoverContent>
              </Popover>
            </div>
            <div className="min-w-[12rem] flex-1">
              <Label className="text-xs font-medium text-foreground/80">
                {t("dialog.connectionName")}
              </Label>
              <Input
                className="mt-1 text-xs h-8"
                placeholder={t("dialog.serverPlaceholder")}
                value={name}
                onChange={(e) => setName(e.target.value)}
              />
            </div>
            <div className="min-w-[12rem] flex-1 sm:max-w-[18rem]">
              <Label className="text-xs font-medium text-foreground/80">{t("dialog.group")}</Label>
              <Popover
                open={showGroupDropdown}
                onOpenChange={(open) => {
                  setShowGroupDropdown(open);
                  if (!open) {
                    setNewGroupName("");
                  }
                }}
              >
                <PopoverTrigger asChild>
                  <Button
                    type="button"
                    variant="outline"
                    className="mt-1 h-8 w-full justify-between text-xs font-normal"
                  >
                    <span className={`truncate ${groupId ? "" : "text-muted-foreground"}`}>
                      {selectedGroupLabel}
                    </span>
                    <MdExpandMore className="shrink-0 text-xs text-muted-foreground" />
                  </Button>
                </PopoverTrigger>
                <PopoverContent
                  align="start"
                  side="bottom"
                  sideOffset={4}
                  collisionPadding={16}
                  className="w-[var(--radix-popover-trigger-width)] min-w-[12rem] overflow-hidden p-0"
                >
                  <div className="max-h-48 overflow-y-auto">
                    <button
                      type="button"
                      className={`w-full px-3 py-1.5 text-left text-xs transition-colors hover:bg-accent ${!groupId ? "bg-primary/15 text-primary" : "text-muted-foreground"}`}
                      onClick={() => {
                        setGroupId("");
                        setNewGroupNamePending("");
                        setNewGroupParentId("");
                        setShowGroupDropdown(false);
                      }}
                    >
                      {t("dialog.none")}
                    </button>
                    {(() => {
                      const getDepth = (g: Group): number => {
                        let d = 0;
                        let cur: string | undefined = g.parent_id;
                        while (cur) {
                          d++;
                          const parent = groups.find((x) => x.id === cur);
                          cur = parent?.parent_id;
                        }
                        return d;
                      };
                      const sorted = [...groups].sort((a, b) => a.sort_order - b.sort_order);
                      const buildTree = (parentId: string | undefined): Group[] => {
                        const children = sorted.filter(
                          (g) => (g.parent_id || undefined) === parentId,
                        );
                        return children.flatMap((g) => [g, ...buildTree(g.id)]);
                      };
                      const ordered = buildTree(undefined);
                      return ordered.map((g) => {
                        const depth = getDepth(g);
                        return (
                          <button
                            key={g.id}
                            type="button"
                            className={`w-full py-1.5 text-left text-xs transition-colors hover:bg-accent ${groupId === g.id ? "bg-primary/15 text-primary" : ""}`}
                            style={{ paddingLeft: `${12 + depth * 16}px`, paddingRight: "12px" }}
                            onClick={() => {
                              setGroupId(g.id);
                              setNewGroupNamePending("");
                              setNewGroupParentId("");
                              setShowGroupDropdown(false);
                            }}
                          >
                            {g.name}
                          </button>
                        );
                      });
                    })()}
                  </div>
                  <div className="border-t p-1.5">
                    <div className="flex items-center gap-1.5">
                      <Input
                        className="flex-1 min-w-0 h-7 text-xs"
                        placeholder={t("dialog.newGroupPlaceholder")}
                        value={newGroupName}
                        onChange={(e) => setNewGroupName(e.target.value)}
                        onKeyDown={(e) => {
                          if (e.key === "Enter" && newGroupName.trim()) {
                            setGroupId("new");
                            setNewGroupNamePending(newGroupName.trim());
                            setNewGroupParentId(groupId && groupId !== "new" ? groupId : "");
                            setNewGroupName("");
                            setShowGroupDropdown(false);
                          }
                        }}
                      />
                      <Button
                        variant="ghost"
                        size="icon-xs"
                        disabled={!newGroupName.trim()}
                        onClick={() => {
                          if (newGroupName.trim()) {
                            setGroupId("new");
                            setNewGroupNamePending(newGroupName.trim());
                            setNewGroupParentId(groupId && groupId !== "new" ? groupId : "");
                            setNewGroupName("");
                            setShowGroupDropdown(false);
                          }
                        }}
                      >
                        <MdAdd className="text-sm" />
                      </Button>
                    </div>
                    <p className="px-1 pt-1 text-[0.6875rem] leading-snug text-muted-foreground">
                      {newGroupParentLabel
                        ? t("dialog.newGroupParentHint", { group: newGroupParentLabel })
                        : t("dialog.newGroupRootHint")}
                    </p>
                  </div>
                </PopoverContent>
              </Popover>
            </div>
          </div>

          <TabsContent value="ssh" className="space-y-3 m-0 border-0 outline-none w-full">
            <SshForm
              host={host}
              setHost={setHost}
              port={sshPort}
              setPort={setSshPort}
              username={username}
              setUsername={setUsername}
              authType={authType}
              setAuthType={(value) => setAuthType(value)}
              passwordId={passwordId}
              setPasswordId={setPasswordId}
              password={password}
              setPassword={setPassword}
              hasPassword={hasPassword}
              setHasPassword={setHasPassword}
              keyId={keyId}
              setKeyId={setKeyId}
              proxyId={proxyId}
              setProxyId={setProxyId}
              proxies={proxies}
              jumpHostId={jumpHostId}
              setJumpHostId={setJumpHostId}
              jumpHostOptions={jumpHostOptions}
              otpId={otpId}
              setOtpId={setOtpId}
              autoFillOtp={autoFillOtp}
              setAutoFillOtp={setAutoFillOtp}
              otpEntries={otpEntries}
              postLoginEnabled={postLoginEnabled}
              setPostLoginEnabled={setPostLoginEnabled}
              postLoginCommand={postLoginCommand}
              setPostLoginCommand={setPostLoginCommand}
              postLoginDelayMs={postLoginDelayMs}
              setPostLoginDelayMs={setPostLoginDelayMs}
              minPostLoginDelayMs={MIN_POST_LOGIN_DELAY_MS}
              maxPostLoginDelayMs={MAX_POST_LOGIN_DELAY_MS}
            />
          </TabsContent>

          <TabsContent value="local" className="space-y-3 m-0 border-0 outline-none w-full">
            <LocalTerminal
              shellPath={shellPath}
              setShellPath={setShellPath}
              shellArgs={shellArgs}
              setShellArgs={setShellArgs}
              workingDir={workingDir}
              setWorkingDir={setWorkingDir}
            />
          </TabsContent>

          <TabsContent value="telnet" className="space-y-3 m-0 border-0 outline-none w-full">
            <TelnetForm
              host={host}
              setHost={setHost}
              port={telnetPort}
              setPort={setTelnetPort}
              backspaceMode={telnetBackspaceMode}
              setBackspaceMode={setTelnetBackspaceMode}
            />
          </TabsContent>

          <TabsContent value="serial" className="space-y-3 m-0 border-0 outline-none w-full">
            <SerialForm
              serialPortName={serialPortName}
              setSerialPortName={setSerialPortName}
              serialPortOptions={serialPortOptions}
              serialPortsLoading={serialPortsLoading}
              serialPortsError={serialPortsError}
              onSerialPortDropdownOpen={() => {
                void loadSerialPorts();
              }}
              baudRate={baudRate}
              setBaudRate={setBaudRate}
              dataBits={dataBits}
              setDataBits={setDataBits}
              parity={parity}
              setParity={setParity}
              stopBits={stopBits}
              setStopBits={setStopBits}
              backspaceMode={serialBackspaceMode}
              setBackspaceMode={setSerialBackspaceMode}
            />
          </TabsContent>

          <div className="mt-5 space-y-3">
            {/* Description */}
            <div>
              <Label className="text-xs font-medium text-foreground/80">
                {t("dialog.description")}
              </Label>
              <Textarea
                rows={2}
                placeholder={t("dialog.descriptionPlaceholder")}
                className="mt-1 text-xs resize-none"
                value={description}
                onChange={(e) => setDescription(e.target.value)}
              />
            </div>
            {/* Messages */}
            {error && (
              <div className="p-2 bg-destructive/10 border border-destructive/30 rounded text-xs text-red-400">
                {error}
              </div>
            )}
          </div>
        </div>
      </Tabs>

      {/* Footer */}
      <ActionFooter>
        <ActionButton variant="outline" onClick={handleClose}>
          {t("dialog.cancel")}
        </ActionButton>
        <ActionButton
          onClick={handleSave}
          disabled={saveDisabled}
          title={validationError || undefined}
        >
          {connecting ? t("dialog.saving") : t("dialog.save")}
        </ActionButton>
      </ActionFooter>
    </div>
  );
}
