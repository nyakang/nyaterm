import { emit } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { MdAdd, MdExpandMore } from "react-icons/md";
import {
  DEFAULT_CONNECTION_ICON,
  LINUX_ICONS,
  resolveConnectionIcon,
  SERVER_ICONS,
  SYSTEM_ICONS,
} from "@/components/icons";
import ChildWindowHeader from "@/components/layout/ChildWindowHeader";
import { buildGroupPath, type ConnectionOption, sortLabel } from "@/components/network/shared";
import { LocalTerminal } from "@/components/sessions/LocalTerminal";
import { SerialForm } from "@/components/sessions/SerialForm";
import { type SshAuthMode, SshForm } from "@/components/sessions/SshForm";
import { TelnetForm } from "@/components/sessions/TelnetForm";
import { ActionButton, ActionFooter } from "@/components/ui/action-footer";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Switch } from "@/components/ui/switch";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { useApp } from "@/context/AppContext";
import { getErrorMessage } from "@/lib/errors";
import { invoke } from "@/lib/invoke";
import { isValidSerialBaudRate, MAX_SERIAL_BAUD_RATE, MIN_SERIAL_BAUD_RATE } from "@/lib/serial";
import type {
  Group,
  OtpEntry,
  ProxyConfig,
  SavedConnection,
  SftpSettings,
  SshAlgorithmPreferences,
} from "@/types/global";

const isValidPort = (value: number) => Number.isInteger(value) && value >= 1 && value <= 65535;
const DEFAULT_POST_LOGIN_DELAY_MS = 1000;
const MIN_POST_LOGIN_DELAY_MS = 0;
const MAX_POST_LOGIN_DELAY_MS = 60_000;
const DEFAULT_SFTP_SHELL_DETECTION_TIMEOUT_MS = 3000;
const MIN_SFTP_SHELL_DETECTION_TIMEOUT_MS = 100;
const MAX_SFTP_SHELL_DETECTION_TIMEOUT_MS = 60_000;
const DEFAULT_SSH_ALGORITHMS: SshAlgorithmPreferences = {
  mode: "compatible",
  kex: [],
  ciphers: [],
  macs: [],
  host_keys: [],
};
const DEFAULT_SFTP_SETTINGS: SftpSettings = {
  enabled: true,
  cwd_follow_mode: "shell_integration",
  shell_detection_timeout_ms: DEFAULT_SFTP_SHELL_DETECTION_TIMEOUT_MS,
  filename_encoding: "",
};

function normalizeSshAlgorithms(
  value: SavedConnection["ssh_algorithms"] | undefined,
): SshAlgorithmPreferences {
  if (!value) {
    return { ...DEFAULT_SSH_ALGORITHMS };
  }

  return {
    mode: value.mode || "compatible",
    kex: value.kex || [],
    ciphers: value.ciphers || [],
    macs: value.macs || [],
    host_keys: value.host_keys || [],
  };
}

function normalizeSftpSettings(value: SavedConnection["sftp"] | undefined): SftpSettings {
  return {
    enabled: value?.enabled ?? true,
    cwd_follow_mode: value?.cwd_follow_mode || "shell_integration",
    shell_detection_timeout_ms:
      value?.shell_detection_timeout_ms ?? DEFAULT_SFTP_SHELL_DETECTION_TIMEOUT_MS,
    filename_encoding: value?.filename_encoding || "",
  };
}

const isValidPostLoginDelay = (value: number) =>
  Number.isInteger(value) && value >= MIN_POST_LOGIN_DELAY_MS && value <= MAX_POST_LOGIN_DELAY_MS;

const isValidSftpShellDetectionTimeout = (value: number) =>
  Number.isInteger(value) &&
  value >= MIN_SFTP_SHELL_DETECTION_TIMEOUT_MS &&
  value <= MAX_SFTP_SHELL_DETECTION_TIMEOUT_MS;

export default function NewSessionPage() {
  const { t } = useTranslation();
  const { appSettings } = useApp();
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
  const [passwordSecretsUnlocked, setPasswordSecretsUnlocked] = useState(false);
  const [keyId, setKeyId] = useState("");
  const [iconKey, setIconKey] = useState("");
  const [iconAutoDetect, setIconAutoDetect] = useState(true);
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
  const [sshBackspaceMode, setSshBackspaceMode] = useState("del");
  const [x11Forwarding, setX11Forwarding] = useState(false);
  const [sshAlgorithms, setSshAlgorithms] =
    useState<SshAlgorithmPreferences>(DEFAULT_SSH_ALGORITHMS);
  const [sftpSettings, setSftpSettings] = useState<SftpSettings>(DEFAULT_SFTP_SETTINGS);

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
  const [telnetRawTcpCli, setTelnetRawTcpCli] = useState(false);
  const [telnetEnterMode, setTelnetEnterMode] = useState<"crlf" | "cr" | "lf">("cr");
  const [telnetLocalEcho, setTelnetLocalEcho] = useState(false);
  const [telnetLocalLineEdit, setTelnetLocalLineEdit] = useState(false);
  const [telnetForceCharacterAtATime, setTelnetForceCharacterAtATime] = useState(false);
  const [telnetSendNaws, setTelnetSendNaws] = useState(true);
  const [telnetSendSga, setTelnetSendSga] = useState(true);

  // Per-connection encoding ("global" = follow global setting)
  const [encoding, setEncoding] = useState("global");

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
        setIconAutoDetect(found.icon_auto_detect ?? !found.icon);

        const tabMap: Record<string, string> = {
          ssh: "ssh",
          local_terminal: "local",
          telnet: "telnet",
          serial: "serial",
        };
        setCurrentTab(tabMap[found.type] || "ssh");
        setEncoding(found.encoding || "global");

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
          setSshBackspaceMode(found.backspace_mode || "del");
          setX11Forwarding(found.x11_forwarding ?? false);
          setSshAlgorithms(normalizeSshAlgorithms(found.ssh_algorithms));
          setSftpSettings(normalizeSftpSettings(found.sftp));
        } else if (found.type === "telnet") {
          setHost(found.host || "");
          setTelnetPort(found.port || 23);
          setUsername(found.username || "");
          setAuthType((found.auth?.mode === "none" ? "none" : "password") as SshAuthMode);
          setPasswordId(found.auth?.password_id || "");
          setHasPassword(found.auth?.has_password || false);
          setTelnetBackspaceMode(found.backspace_mode || "del");
          setTelnetRawTcpCli(found.raw_tcp_cli ?? false);
          setTelnetEnterMode(found.enter_mode || "cr");
          setTelnetLocalEcho(found.local_echo ?? false);
          setTelnetLocalLineEdit(found.local_line_edit ?? false);
          setTelnetForceCharacterAtATime(found.force_character_at_a_time ?? false);
          setTelnetSendNaws(found.send_naws ?? true);
          setTelnetSendSga(found.send_sga ?? true);
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
    setIconAutoDetect(true);
    setProxyId("");
    setJumpHostId("");
    setOtpId("");
    setAutoFillOtp(false);
    setPostLoginEnabled(false);
    setPostLoginCommand("");
    setPostLoginDelayMs(DEFAULT_POST_LOGIN_DELAY_MS);
    setSshBackspaceMode("del");
    setX11Forwarding(false);
    setSshAlgorithms({ ...DEFAULT_SSH_ALGORITHMS });
    setSftpSettings({ ...DEFAULT_SFTP_SETTINGS });
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
    setTelnetRawTcpCli(false);
    setTelnetEnterMode("cr");
    setTelnetLocalEcho(false);
    setTelnetLocalLineEdit(false);
    setTelnetForceCharacterAtATime(false);
    setTelnetSendNaws(true);
    setTelnetSendSga(true);
    setEncoding("global");
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
  const remoteStatsEnabled = appSettings.ui.show_remote_stats ?? true;
  const iconAutoDetectDisabled = !remoteStatsEnabled;
  const iconAutoDetectTooltip = !remoteStatsEnabled
    ? t("dialog.iconAutoDetectRemoteStatsDisabledTooltip")
    : t("dialog.iconAutoDetectTooltip");

  const newGroupParentLabel = useMemo(() => {
    if (!groupId || groupId === "new") {
      return "";
    }
    return buildGroupPath(groupId, groupsById);
  }, [groupId, groupsById]);

  const savedConnectionsById = useMemo(
    () => new Map(savedConnections.map((connection) => [connection.id, connection])),
    [savedConnections],
  );

  const getJumpHostChain = useCallback(
    (connection: SavedConnection) => {
      const chain: SavedConnection[] = [];
      const seen = new Set<string>([connection.id]);
      let current = connection;

      while (current.network?.proxy_jump_id) {
        const next = savedConnectionsById.get(current.network.proxy_jump_id);
        if (!next || seen.has(next.id)) {
          break;
        }
        chain.push(next);
        seen.add(next.id);
        current = next;
      }

      return chain;
    },
    [savedConnectionsById],
  );

  const wouldCreateJumpHostCycle = useCallback(
    (candidate: SavedConnection) => {
      if (!editId) {
        return false;
      }
      if (candidate.id === editId) {
        return true;
      }

      return getJumpHostChain(candidate).some((jump) => jump.id === editId);
    },
    [editId, getJumpHostChain],
  );

  const jumpHostOptions = useMemo<ConnectionOption[]>(() => {
    return savedConnections
      .filter((connection) => connection.type === "ssh" && connection.id !== editId)
      .map((connection) => {
        const groupPath = buildGroupPath(connection.group_id, groupsById);
        const jumpChain = getJumpHostChain(connection);
        const chainLabel =
          jumpChain.length > 0
            ? [connection.name, ...jumpChain.map((jump) => jump.name)].join(" -> ")
            : "";
        const subtitle = [
          groupPath,
          connection.host && `${connection.host}:${connection.port ?? 22}`,
          connection.username,
          chainLabel,
        ]
          .filter(Boolean)
          .join(" · ");
        const disabled = wouldCreateJumpHostCycle(connection);

        return {
          connection,
          groupPath,
          subtitle,
          searchText: [connection.name, connection.host, connection.username, groupPath, chainLabel]
            .filter(Boolean)
            .join(" "),
          disabled,
          disabledReason: disabled ? t("dialog.proxyJumpCycleDetected") : undefined,
        };
      })
      .sort((left, right) => {
        const pathSort = sortLabel(left.groupPath, right.groupPath);
        return pathSort !== 0 ? pathSort : sortLabel(left.connection.name, right.connection.name);
      });
  }, [editId, getJumpHostChain, groupsById, savedConnections, t, wouldCreateJumpHostCycle]);

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
      if (!isValidSftpShellDetectionTimeout(sftpSettings.shell_detection_timeout_ms)) {
        return t("dialog.sftpShellDetectionTimeoutInvalid", {
          min: MIN_SFTP_SHELL_DETECTION_TIMEOUT_MS,
          max: MAX_SFTP_SHELL_DETECTION_TIMEOUT_MS,
          defaultValue: "Shell detection timeout must be between {{min}} and {{max}} ms",
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
    baudRate,
    currentTab,
    host,
    postLoginCommand,
    postLoginDelayMs,
    postLoginEnabled,
    serialPortName,
    shellPath,
    sshPort,
    sftpSettings.shell_detection_timeout_ms,
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
              : normalizedHost;

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
        currentTab === "ssh" || currentTab === "telnet"
          ? (() => {
              const resolvedAuthMode: SshAuthMode =
                currentTab === "telnet"
                  ? authType === "none"
                    ? "none"
                    : "password"
                  : authType === "password"
                    ? "password"
                    : authType === "key" && keyId
                      ? "key"
                      : "none";
              const nextAuth: NonNullable<SavedConnection["auth"]> = {
                mode: resolvedAuthMode,
                password_id: resolvedAuthMode === "password" ? passwordId || "" : "",
                key_id: currentTab === "ssh" && resolvedAuthMode === "key" ? keyId : undefined,
                otp_id: currentTab === "ssh" ? otpId || undefined : undefined,
                auto_fill_otp: currentTab === "ssh" && otpId ? autoFillOtp : undefined,
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
      const finalGroupKey = finalGroupId || "";
      const initialGroupKey = initialData?.group_id || "";
      const siblingConnections = savedConnections.filter(
        (connection) =>
          connection.id !== initialData?.id && (connection.group_id || "") === finalGroupKey,
      );
      const nextSortOrder =
        siblingConnections.reduce(
          (max, connection) => Math.max(max, connection.sort_order ?? 0),
          -1,
        ) + 1;
      const sortOrder =
        initialData && initialGroupKey === finalGroupKey
          ? (initialData.sort_order ?? nextSortOrder)
          : nextSortOrder;

      const connection: SavedConnection = {
        id: initialData?.id || "",
        name: normalizedName || defaultName,
        type: typeTag as SavedConnection["type"],
        group_id: finalGroupId || undefined,
        description: normalizedDescription || undefined,
        sort_order: sortOrder,
        icon: iconKey || undefined,
        icon_auto_detect: currentTab === "ssh" ? iconAutoDetect : false,
        encoding: encoding === "global" ? undefined : encoding,
        ...(currentTab === "ssh"
          ? {
              host: normalizedHost,
              port: sshPort,
              username: normalizedUsername,
              auth,
              network,
              post_login: postLogin,
              ssh_algorithms: sshAlgorithms,
              sftp: sftpSettings,
              backspace_mode: sshBackspaceMode,
              x11_forwarding: x11Forwarding,
            }
          : {}),
        ...(currentTab === "telnet"
          ? {
              host: normalizedHost,
              port: telnetPort,
              username: normalizedUsername,
              auth,
              backspace_mode: telnetBackspaceMode,
              raw_tcp_cli: telnetRawTcpCli,
              enter_mode: telnetEnterMode,
              local_echo: telnetLocalEcho,
              local_line_edit: telnetLocalLineEdit,
              force_character_at_a_time: telnetForceCharacterAtATime,
              send_naws: telnetSendNaws,
              send_sga: telnetSendSga,
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
                    {(() => {
                      const def = resolveConnectionIcon(iconKey);
                      const IconComp = def.icon;
                      return <IconComp style={{ color: def.color }} className="text-sm" />;
                    })()}
                  </Button>
                </PopoverTrigger>
                <PopoverContent
                  align="start"
                  side="bottom"
                  collisionPadding={16}
                  className="w-56 max-w-[calc(100vw-2rem)] p-2"
                >
                  <div className="grid grid-cols-7 gap-0.5">
                    {Object.entries(SERVER_ICONS).map(([key, def]) => {
                      const IconComp = def.icon;
                      const activeKey = iconKey || DEFAULT_CONNECTION_ICON;
                      return (
                        <button
                          key={key}
                          type="button"
                          className={`flex h-7 w-7 items-center justify-center rounded transition-colors hover:bg-accent ${activeKey === key ? "bg-primary/15 ring-1 ring-primary/40" : ""}`}
                          title={key === DEFAULT_CONNECTION_ICON ? t("dialog.none") : key}
                          onClick={() => {
                            setIconKey(key);
                            setIconAutoDetect(false);
                            setShowIconPicker(false);
                          }}
                        >
                          <IconComp style={{ color: def.color }} className="text-sm" />
                        </button>
                      );
                    })}
                    {Object.entries(LINUX_ICONS).map(([key, def]) => {
                      const IconComp = def.icon;
                      return (
                        <button
                          key={key}
                          type="button"
                          className={`flex h-7 w-7 items-center justify-center rounded transition-colors hover:bg-accent ${iconKey === key ? "bg-primary/15 ring-1 ring-primary/40" : ""}`}
                          title={key}
                          onClick={() => {
                            setIconKey(key);
                            setIconAutoDetect(false);
                            setShowIconPicker(false);
                          }}
                        >
                          <IconComp style={{ color: def.color }} className="text-sm" />
                        </button>
                      );
                    })}
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
                            setIconAutoDetect(false);
                            setShowIconPicker(false);
                          }}
                        >
                          <IconComp style={{ color: def.color }} className="text-sm" />
                        </button>
                      );
                    })}
                  </div>
                  {currentTab === "ssh" && (
                    <div className="mt-2 border-t pt-2">
                      <Tooltip>
                        <TooltipTrigger asChild>
                          <div
                            className={`flex items-center justify-between gap-3 rounded px-1.5 py-1 ${
                              iconAutoDetectDisabled ? "cursor-not-allowed opacity-70" : ""
                            }`}
                          >
                            <div className="min-w-0">
                              <div className="truncate text-xs font-medium">
                                {t("dialog.iconAutoDetect")}
                              </div>
                            </div>
                            <Switch
                              size="sm"
                              checked={iconAutoDetect}
                              disabled={iconAutoDetectDisabled}
                              onCheckedChange={setIconAutoDetect}
                              aria-label={t("dialog.iconAutoDetect")}
                            />
                          </div>
                        </TooltipTrigger>
                        <TooltipContent side="bottom">{iconAutoDetectTooltip}</TooltipContent>
                      </Tooltip>
                    </div>
                  )}
                </PopoverContent>
              </Popover>
            </div>
            <div className="min-w-48 flex-1">
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
            <div className="min-w-48 flex-1 sm:max-w-[18rem]">
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
                  className="w-(--radix-popover-trigger-width) min-w-48 overflow-hidden p-0"
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
              backspaceMode={sshBackspaceMode}
              setBackspaceMode={setSshBackspaceMode}
              x11Forwarding={x11Forwarding}
              setX11Forwarding={setX11Forwarding}
              sshAlgorithms={sshAlgorithms}
              setSshAlgorithms={setSshAlgorithms}
              sftpSettings={sftpSettings}
              setSftpSettings={setSftpSettings}
              connectionId={initialData?.id || editId}
              encoding={encoding}
              setEncoding={setEncoding}
              passwordSecretsUnlocked={passwordSecretsUnlocked}
              onUnlockPasswordSecrets={() => setPasswordSecretsUnlocked(true)}
              onLockPasswordSecrets={() => setPasswordSecretsUnlocked(false)}
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
              encoding={encoding}
              setEncoding={setEncoding}
            />
          </TabsContent>

          <TabsContent value="telnet" className="space-y-3 m-0 border-0 outline-none w-full">
            <TelnetForm
              host={host}
              setHost={setHost}
              port={telnetPort}
              setPort={setTelnetPort}
              username={username}
              setUsername={setUsername}
              authType={authType === "none" ? "none" : "password"}
              setAuthType={(value) => setAuthType(value)}
              passwordId={passwordId}
              setPasswordId={setPasswordId}
              password={password}
              setPassword={setPassword}
              hasPassword={hasPassword}
              setHasPassword={setHasPassword}
              backspaceMode={telnetBackspaceMode}
              setBackspaceMode={setTelnetBackspaceMode}
              rawTcpCli={telnetRawTcpCli}
              setRawTcpCli={setTelnetRawTcpCli}
              enterMode={telnetEnterMode}
              setEnterMode={setTelnetEnterMode}
              localEcho={telnetLocalEcho}
              setLocalEcho={setTelnetLocalEcho}
              localLineEdit={telnetLocalLineEdit}
              setLocalLineEdit={setTelnetLocalLineEdit}
              forceCharacterAtATime={telnetForceCharacterAtATime}
              setForceCharacterAtATime={setTelnetForceCharacterAtATime}
              sendNaws={telnetSendNaws}
              setSendNaws={setTelnetSendNaws}
              sendSga={telnetSendSga}
              setSendSga={setTelnetSendSga}
              connectionId={initialData?.id || editId}
              encoding={encoding}
              setEncoding={setEncoding}
              passwordSecretsUnlocked={passwordSecretsUnlocked}
              onUnlockPasswordSecrets={() => setPasswordSecretsUnlocked(true)}
              onLockPasswordSecrets={() => setPasswordSecretsUnlocked(false)}
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
              encoding={encoding}
              setEncoding={setEncoding}
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
