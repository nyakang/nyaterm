import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { BiExport, BiImport } from "react-icons/bi";
import { GrUpgrade } from "react-icons/gr";
import {
  MdAccessTime,
  MdAdd,
  MdArticle,
  MdCellTower,
  MdComputer,
  MdContentCopy,
  MdContentPaste,
  MdDashboard,
  MdDeleteSweep,
  MdDns,
  MdDownload,
  MdFitScreen,
  MdInfo,
  MdKeyboardArrowDown,
  MdKeyboardArrowUp,
  MdListAlt,
  MdMemory,
  MdMenu,
  MdMenuBook,
  MdMerge,
  MdOutlineMonitorHeart,
  MdOutlineStickyNote2,
  MdPalette,
  MdRestartAlt,
  MdSearch,
  MdSelectAll,
  MdSettings,
  MdSpeed,
  MdSplitscreen,
  MdSwapHoriz,
  MdSwapVert,
  MdSync,
  MdTerminal,
  MdTranslate,
  MdUpdate,
  MdUpload,
  MdViewSidebar,
  MdVisibility,
  MdVisibilityOff,
  MdZoomIn,
  MdZoomOut,
} from "react-icons/md";
import { SiDocker, SiNvidia } from "react-icons/si";
import {
  VscChromeClose,
  VscChromeMaximize,
  VscChromeMinimize,
  VscChromeRestore,
} from "react-icons/vsc";
import packageJson from "@/../package.json";
import HeaderStatusHideConfirmDialog from "@/components/dialog/app/HeaderStatusHideConfirmDialog";
import QuitConfirmDialog from "@/components/dialog/app/QuitConfirmDialog";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { useApp } from "@/context/AppContext";
import { useTheme } from "@/context/ThemeContext";
import { useConfigTransfer } from "@/hooks/useConfigTransfer";
import type { RemoteGpuOverviewState } from "@/hooks/useRemoteGpuOverview";
import type { RemoteNpuOverviewState } from "@/hooks/useRemoteNpuOverview";
import type { RemoteStatsState } from "@/hooks/useRemoteStats";
import { resolveDisplayKeys, resolveShortcutKeys } from "@/hooks/useShortcutMap";
import { AVAILABLE_LANGUAGES } from "@/i18n";
import { HEADER_STATUS_MODES, normalizeHeaderStatusMode } from "@/lib/headerStatus";
import {
  ACTIVITY_BAR_PANEL_ITEM_IDS,
  getActivityBarItemIdsForSide,
  isActivityItemAvailable,
  normalizePanelOpenMode,
  type PanelOpenMode,
} from "@/lib/appWorkspace";
import { invoke } from "@/lib/invoke";
import { logger } from "@/lib/logger";
import { isMacOS } from "@/lib/platform";
import {
  decreaseTerminalFontSizeDelta,
  increaseTerminalFontSizeDelta,
  resetTerminalFontSizeDelta,
} from "@/lib/terminalFontSize";
import {
  getActivePane,
  getActiveSessionTabDisplayName,
} from "@/lib/workspaceTabs";
import { getDynamicTitle, useDynamicTitles } from "@/lib/dynamicTabTitles";
import type {
  AppearanceSettings,
  RemoteGpuOverview,
  RemoteNpuOverview,
  SavedConnection,
  Tab,
} from "@/types/global";
import ImportDialog from "../dialog/connections/ImportDialog";
import { resolveConnectionIcon } from "../icons";
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

function AscendIcon({ className }: { className?: string }) {
  return (
    <span
      aria-hidden="true"
      className={`inline-block h-[1em] w-[1em] bg-current ${className ?? ""}`}
      style={{
        WebkitMask: "url('/icons/brands/ascend.svg') center / contain no-repeat",
        mask: "url('/icons/brands/ascend.svg') center / contain no-repeat",
      }}
    />
  );
}

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
  visibility: MdVisibility,
  file_export: BiExport,
  file_import: BiImport,
  splitscreen: MdSplitscreen,
  merge: MdMerge,
  dashboard: MdDashboard,
  swap_horiz: MdSwapHoriz,
  swap_vert: MdSwapVert,
  sync: MdSync,
  upload: MdUpload,
  download: MdDownload,
  cell_tower: MdCellTower,
  delete_sweep: MdDeleteSweep,
  fit_screen: MdFitScreen,
  terminal: MdTerminal,
  computer: MdComputer,
  search: MdSearch,
  memory: MdMemory,
  speed: MdSpeed,
  monitor_heart: MdOutlineMonitorHeart,
  sticky_note: MdOutlineStickyNote2,
  nvidia: SiNvidia,
  ascend: AscendIcon,
  list_alt: MdListAlt,
  docker: SiDocker,
};

function DynamicIcon({ name, className }: { name: string; className?: string }) {
  const Icon = iconMap[name];
  if (!Icon) return null;
  return <Icon className={className} />;
}

function HeaderStatusPart({
  icon,
  children,
  color = "var(--df-text-muted)",
  iconColor = "var(--df-text-dimmed)",
  className,
}: {
  icon: React.ReactNode;
  children: React.ReactNode;
  color?: string;
  iconColor?: string;
  className?: string;
}) {
  return (
    <span
      className={`inline-flex min-w-0 items-center gap-1 whitespace-nowrap ${className ?? ""}`}
      style={{ color }}
    >
      <span
        className="inline-flex shrink-0 text-[0.875rem]"
        style={{ color: iconColor, opacity: 0.78 }}
      >
        {icon}
      </span>
      <span className="min-w-0 truncate">{children}</span>
    </span>
  );
}

function HeaderStatusDivider() {
  return (
    <span className="px-0.5 font-sans" style={{ color: "var(--df-text-dimmed)" }}>
      -
    </span>
  );
}

function formatPct(value: number | null): string {
  if (value == null) return "--";
  return `${Math.round(Math.min(100, Math.max(0, value)))}%`;
}

function formatBytes(bytes: number): string {
  if (bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  const val = bytes / 1024 ** i;
  return `${val < 10 ? val.toFixed(1) : val < 100 ? val.toFixed(1) : val.toFixed(0)} ${units[i]}`;
}

function formatRate(bytesPerSec: number): string {
  return `${formatBytes(bytesPerSec)}/s`;
}

function formatMemoryMbCompact(value: number): string {
  if (value >= 1024) {
    const gib = value / 1024;
    const rounded = Math.round(gib * 10) / 10;
    if (rounded < 10 && !Number.isInteger(rounded)) {
      return `${rounded.toFixed(1)}G`;
    }
    return `${Math.round(gib)}G`;
  }
  return `${Math.round(value)}M`;
}

function getPressureColor(usagePercent: number | null): string | undefined {
  if (usagePercent == null) return undefined;
  if (usagePercent >= 90) return "#f87171";
  if (usagePercent >= 75) return "#f59e0b";
  return undefined;
}

function getHardwareCardLimit(width: number): number {
  if (width >= 1180) return 4;
  if (width >= 920) return 3;
  if (width >= 700) return 2;
  return 1;
}

function getHardwareStatusCompact(visibleCardCount: number, hiddenCount: number, cardLimit: number): boolean {
  return cardLimit <= 1 || (cardLimit <= 2 && visibleCardCount >= 2) || visibleCardCount >= 3 || hiddenCount > 0;
}

function formatUptimeShort(
  seconds: number,
  t: (key: string, opts?: Record<string, unknown>) => string,
): string {
  if (seconds >= 86400) {
    return t("headerStatus.uptimeDays", { count: Math.floor(seconds / 86400) });
  }
  if (seconds >= 3600) {
    return t("headerStatus.uptimeHours", { count: Math.floor(seconds / 3600) });
  }
  return t("headerStatus.uptimeMinutes", { count: Math.max(1, Math.floor(seconds / 60)) });
}

interface HeaderHardwareCard {
  id: string;
  indexLabel: string;
  title: string;
  utilizationPercent: number | null;
  memoryPercent: number | null;
  memoryText: string;
  temperatureText: string;
  powerText: string;
}

function clampPercent(value: number): number {
  return Math.min(100, Math.max(0, value));
}

function formatOptionalPct(value: number | null): string {
  return value == null ? "--" : `${Math.round(clampPercent(value))}%`;
}

function formatOptionalTemperature(value?: number | null): string {
  return value == null ? "-" : `${Math.round(value)} C`;
}

function formatOptionalWatts(value?: number | null): string {
  if (value == null) return "-";
  return `${value < 100 ? value.toFixed(1) : Math.round(value)} W`;
}

function sortHardwareCardsByIndex(cards: HeaderHardwareCard[]): HeaderHardwareCard[] {
  return [...cards].sort(
    (left, right) =>
      Number(left.indexLabel.split(":")[0]) - Number(right.indexLabel.split(":")[0]) ||
      left.indexLabel.localeCompare(right.indexLabel),
  );
}

function buildGpuHardwareCards(overview: RemoteGpuOverview): HeaderHardwareCard[] {
  return overview.gpus.map((gpu) => {
    const memoryPercent =
      gpu.memory_total_mb > 0 ? (gpu.memory_used_mb / gpu.memory_total_mb) * 100 : null;
    const memoryText =
      gpu.memory_total_mb > 0
        ? `${formatMemoryMbCompact(gpu.memory_used_mb)}/${formatMemoryMbCompact(gpu.memory_total_mb)}`
        : "-";

    return {
      id: gpu.uuid || `gpu-${gpu.index}`,
      indexLabel: gpu.index.toString(),
      title: gpu.name || `GPU ${gpu.index}`,
      utilizationPercent: gpu.utilization_gpu_percent ?? null,
      memoryPercent,
      memoryText,
      temperatureText: formatOptionalTemperature(gpu.temperature_c),
      powerText:
        gpu.power_draw_w == null && gpu.power_limit_w == null
          ? "-"
          : gpu.power_limit_w == null
            ? formatOptionalWatts(gpu.power_draw_w)
            : `${formatOptionalWatts(gpu.power_draw_w)} / ${formatOptionalWatts(gpu.power_limit_w)}`,
    };
  });
}

function buildNpuHardwareCards(overview: RemoteNpuOverview): HeaderHardwareCard[] {
  return overview.npus.map((npu) => {
    const totalMb =
      npu.hbm_total_mb != null && npu.hbm_total_mb > 0 ? npu.hbm_total_mb : npu.memory_total_mb;
    const usedMb =
      npu.hbm_total_mb != null && npu.hbm_total_mb > 0
        ? (npu.hbm_used_mb ?? 0)
        : npu.memory_used_mb;
    const memoryPercent = totalMb > 0 ? (usedMb / totalMb) * 100 : null;
    const memoryText =
      totalMb > 0 ? `${formatMemoryMbCompact(usedMb)}/${formatMemoryMbCompact(totalMb)}` : "-";

    return {
      id: npu.device_key || `npu-${npu.index}-${npu.chip_id}`,
      indexLabel: npu.index.toString(),
      title: npu.name || `NPU ${npu.index}`,
      utilizationPercent: npu.utilization_aicore_percent ?? null,
      memoryPercent,
      memoryText,
      temperatureText: formatOptionalTemperature(npu.temperature_c),
      powerText: formatOptionalWatts(npu.power_draw_w),
    };
  });
}

function buildHardwareTitle(
  label: "GPU" | "NPU",
  cards: HeaderHardwareCard[],
  utilizationLabel: string,
): string {
  if (cards.length === 0) return label;
  return cards
    .map(
      (card) =>
        `${label} ${card.indexLabel} ${card.title} - ${utilizationLabel} ${formatOptionalPct(
          card.utilizationPercent,
        )} - MEM ${card.memoryText} - TEMP ${card.temperatureText} - POWER ${card.powerText}`,
    )
    .join("\n");
}

function HeaderHardwareStatus({
  cards,
  compact,
  hiddenCount,
  icon,
  label,
  onNextPage,
  onPreviousPage,
}: {
  cards: HeaderHardwareCard[];
  compact: boolean;
  hiddenCount: number;
  icon: React.ReactNode;
  label: "GPU" | "NPU";
  onNextPage: () => void;
  onPreviousPage: () => void;
}) {
  return (
    <span className="inline-flex min-w-0 items-center gap-2 font-mono tabular-nums leading-none">
      <span className="inline-flex shrink-0 items-center gap-1 text-[0.6875rem] font-semibold text-[var(--df-text-muted)]">
        <span className="inline-flex text-[0.875rem] text-[var(--df-text-dimmed)]">{icon}</span>
        {label}
      </span>
      <span className="flex min-w-0 items-center">
        {cards.map((card, index) => (
          <span key={card.id} className="inline-flex shrink-0 items-center">
            {index > 0 && <HeaderHardwareSeparator />}
            <HeaderHardwareCardCell card={card} compact={compact} />
          </span>
        ))}
        {hiddenCount > 0 && (
          <span className="inline-flex shrink-0 items-center">
            {cards.length > 0 && <HeaderHardwareSeparator />}
            <HeaderHardwarePager
              hiddenCount={hiddenCount}
              label={label}
              onNextPage={onNextPage}
              onPreviousPage={onPreviousPage}
            />
          </span>
        )}
      </span>
    </span>
  );
}

function HeaderHardwarePager({
  hiddenCount,
  label,
  onNextPage,
  onPreviousPage,
}: {
  hiddenCount: number;
  label: "GPU" | "NPU";
  onNextPage: () => void;
  onPreviousPage: () => void;
}) {
  const stopDrag = (event: React.MouseEvent<HTMLButtonElement>) => {
    event.stopPropagation();
  };

  return (
    <span
      className="pointer-events-auto grid h-[1.625rem] w-5 shrink-0 grid-rows-2 overflow-hidden rounded-sm border border-[var(--df-border)] text-[0.625rem] text-[var(--df-text-muted)]"
      title={`${label} +${hiddenCount}`}
    >
      <button
        type="button"
        className="flex min-h-0 items-center justify-center border-b border-[var(--df-border)] leading-none transition-colors hover:bg-[color-mix(in_srgb,var(--df-text-muted)_10%,transparent)] hover:text-[var(--df-text)] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[var(--df-primary)]"
        aria-label={`${label} previous cards`}
        onClick={(event) => {
          event.stopPropagation();
          onPreviousPage();
        }}
        onMouseDown={stopDrag}
      >
        <MdKeyboardArrowUp className="text-[0.75rem]" />
      </button>
      <button
        type="button"
        className="flex min-h-0 items-center justify-center leading-none transition-colors hover:bg-[color-mix(in_srgb,var(--df-text-muted)_10%,transparent)] hover:text-[var(--df-text)] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[var(--df-primary)]"
        aria-label={`${label} next cards`}
        onClick={(event) => {
          event.stopPropagation();
          onNextPage();
        }}
        onMouseDown={stopDrag}
      >
        <MdKeyboardArrowDown className="text-[0.6875rem]" />
      </button>
    </span>
  );
}

function HeaderHardwareSeparator() {
  return (
    <span
      aria-hidden="true"
      className="mx-1.5 inline-flex h-[1.625rem] shrink-0 items-center text-[0.6875rem] leading-none text-[var(--df-text-dimmed)] opacity-80"
    >
      |
    </span>
  );
}

function HeaderHardwareCardCell({
  card,
  compact,
}: {
  card: HeaderHardwareCard;
  compact: boolean;
}) {
  return (
    <span className="grid shrink-0 grid-rows-2 gap-y-0.5">
      <HeaderHardwareCardRow card={card} compact={compact} row="utilization" />
      <HeaderHardwareCardRow card={card} compact={compact} row="memory" />
    </span>
  );
}

function HeaderHardwareCardRow({
  card,
  compact,
  row,
}: {
  card: HeaderHardwareCard;
  compact: boolean;
  row: "utilization" | "memory";
}) {
  const value = row === "utilization" ? card.utilizationPercent : card.memoryPercent;
  const text = row === "utilization" ? formatOptionalPct(card.utilizationPercent) : card.memoryText;

  return (
    <span
      className={`grid shrink-0 items-center gap-x-1 text-[0.625rem] ${
        compact ? "w-[2.95rem] grid-cols-[1rem_1.75rem]" : "w-[7.45rem] grid-cols-[1rem_2.75rem_1fr]"
      }`}
    >
      <span className="text-right text-[var(--df-text-muted)]">
        {row === "utilization" ? card.indexLabel : ""}
      </span>
      <HeaderMiniProgress value={value} />
      {!compact && <span className="truncate text-[var(--df-text-muted)]">{text}</span>}
    </span>
  );
}

function HeaderMiniProgress({ value }: { value: number | null }) {
  const safeValue = value == null ? 0 : clampPercent(value);
  const color = getPressureColor(value) ?? "var(--df-primary)";

  return (
    <span
      className="block h-1 overflow-hidden rounded-full"
      style={{ backgroundColor: "color-mix(in_srgb,var(--df-text-dimmed)_24%,transparent)" }}
    >
      <span
        className="block h-full rounded-full transition-all duration-700"
        style={{
          width: `${safeValue}%`,
          backgroundColor: color,
          opacity: value == null ? 0.35 : 0.9,
        }}
      />
    </span>
  );
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
  remoteStatsEnabled?: boolean;
  remoteStats?: RemoteStatsState;
  gpuOverviewState?: RemoteGpuOverviewState;
  npuOverviewState?: RemoteNpuOverviewState;
  onSmartSplit?: (mode: "auto" | "horizontal" | "vertical") => void;
  onUnsplit?: () => void;
  canUnsplit?: boolean;
  onManageSyncGroups?: () => void;
  onBroadcastToAll?: () => void;
  broadcastToAll?: boolean;
  onOpenCommandPalette?: () => void;
  onClearTerminal?: () => void;
  onRefitTerminals?: () => void;
  locked?: boolean;
  onRequestQuit?: () => void;
  onToggleActivityBarItemVisibility?: (itemId: string) => void;
  onRequestActivityBarReset?: () => void;
  onPanelOpenModeChange?: (mode: PanelOpenMode) => void;
}

interface MenuItem {
  id?: string;
  label: string;
  action?: () => void;
  separator?: boolean;
  submenu?: MenuItem[];
  checked?: boolean;
  disabled?: boolean;
  icon?: string;
  shortcut?: string;
  accelerator?: string | null;
}

type MacosPredefinedRole =
  | "undo"
  | "redo"
  | "cut"
  | "copy"
  | "paste"
  | "selectAll"
  | "services"
  | "hide"
  | "hideOthers"
  | "showAll";

type MacosMenuSpecItem =
  | {
      kind: "item";
      id: string;
      label: string;
      enabled: boolean;
      accelerator?: string | null;
    }
  | {
      kind: "check";
      id: string;
      label: string;
      enabled: boolean;
      checked: boolean;
      accelerator?: string | null;
    }
  | {
      kind: "submenu";
      id: string;
      label: string;
      enabled: boolean;
      items: MacosMenuSpecItem[];
    }
  | { kind: "separator" }
  | { kind: "predefined"; role: MacosPredefinedRole; label?: string };

interface MacosMenuSpec {
  menus: {
    id: string;
    label: string;
    items: MacosMenuSpecItem[];
  }[];
}

interface MacosMenuActionPayload {
  actionId: string;
  targetWindowLabel?: string | null;
}

const MACOS_ALLOWED_LOCKED_ACTIONS = new Set(["app.about", "app.quit"]);

function getMacosAccelerator(shortcutId: string, keybindings: Record<string, string>) {
  const keys = resolveShortcutKeys(shortcutId, keybindings);
  const combo =
    keys
      .split(",")
      .map((part) => part.trim())
      .find((part) => part.toLowerCase().includes("meta")) ??
    keys
      .split(",")
      .map((part) => part.trim())
      .find(Boolean);

  if (!combo) return null;

  const pieces = combo
    .split("+")
    .map((part) => part.trim().toLowerCase())
    .filter(Boolean);
  const key = pieces.find((part) => !["meta", "cmd", "command", "ctrl", "control", "shift", "alt", "option"].includes(part));
  if (!key || key.includes("-")) return null;

  const modifiers: string[] = [];
  if (pieces.some((part) => part === "meta" || part === "cmd" || part === "command")) {
    modifiers.push("Cmd");
  } else if (pieces.some((part) => part === "ctrl" || part === "control")) {
    modifiers.push("Ctrl");
  }
  if (pieces.includes("shift")) modifiers.push("Shift");
  if (pieces.some((part) => part === "alt" || part === "option")) modifiers.push("Alt");

  const normalizedKey =
    key === "comma"
      ? ","
      : key === "period"
        ? "."
        : key === "space"
          ? "Space"
          : key.length === 1
            ? key.toUpperCase()
            : key;

  return [...modifiers, normalizedKey].join("+");
}

function addNativeAccelerator(
  item: Omit<MenuItem, "shortcut" | "accelerator">,
  shortcutId: string,
  keybindings: Record<string, string>,
): MenuItem {
  return {
    ...item,
    shortcut: resolveDisplayKeys(shortcutId, keybindings),
    accelerator: getMacosAccelerator(shortcutId, keybindings),
  };
}

function getActivityBarPanelLabel(id: string, t: (key: string, opts?: Record<string, unknown>) => string) {
  switch (id) {
    case "securityAuth":
      return t("securityAuth.title");
    case "aiAssistant":
      return t("ai.title");
    default:
      return t(`panel.${id}`);
  }
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
  remoteStatsEnabled = true,
  remoteStats,
  gpuOverviewState,
  npuOverviewState,
  onSmartSplit,
  onUnsplit,
  canUnsplit,
  onManageSyncGroups,
  onBroadcastToAll,
  broadcastToAll,
  onOpenCommandPalette,
  onClearTerminal,
  onRefitTerminals,
  locked = false,
  onRequestQuit,
  onToggleActivityBarItemVisibility,
  onRequestActivityBarReset,
  onPanelOpenModeChange,
}: HeaderProps) {
  const [appWindow] = useState(() => getCurrentWindow());
  const { themeName, setTheme, themeNames, terminalThemeName, setTerminalTheme } = useTheme();
  const { updateAppSettings, updateUi, appSettings, tabs } = useApp();
  const [showImportDialog, setShowImportDialog] = useState(false);
  const [isMaximized, setIsMaximized] = useState(false);
  const [showCloseConfirm, setShowCloseConfirm] = useState(false);
  const [showHeaderStatusHideConfirm, setShowHeaderStatusHideConfirm] = useState(false);
  const [currentMinute, setCurrentMinute] = useState(() => new Date());
  const [hardwareCardLimit, setHardwareCardLimit] = useState(() =>
    typeof window === "undefined" ? 2 : getHardwareCardLimit(window.innerWidth),
  );
  const [hardwarePage, setHardwarePage] = useState({ gpu: 0, npu: 0 });
  const { t, i18n } = useTranslation();
  useDynamicTitles();
  const { handleExport, passwordAlert } = useConfigTransfer();
  const lastMacosMenuSpecRef = useRef("");
  const nativeMenuActionRef = useRef<(actionId: string) => void>(() => {});

  const activePane = activeTab ? getActivePane(activeTab) : null;
  const activeConnection = activePane?.connectionId
    ? savedConnections?.find((c) => c.id === activePane.connectionId)
    : undefined;
  const activeDisplayName = activeTab
    ? getActiveSessionTabDisplayName(activeTab, getDynamicTitle)
    : "NyaTerm";
  const terminalZoomEnabled = appSettings.interaction.terminal_zoom_enabled;
  const headerStatusMode = normalizeHeaderStatusMode(appSettings.ui.header_status_mode);
  const headerStatusVisible = appSettings.ui.header_status_visible !== false;

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

  useEffect(() => {
    if (!headerStatusVisible || headerStatusMode !== "datetime") {
      return;
    }

    const updateCurrentMinute = () => setCurrentMinute(new Date());
    updateCurrentMinute();

    const now = new Date();
    const nextMinuteDelay = (60 - now.getSeconds()) * 1000 - now.getMilliseconds();
    let intervalId: ReturnType<typeof setInterval> | undefined;

    const timeoutId = setTimeout(() => {
      updateCurrentMinute();
      intervalId = setInterval(updateCurrentMinute, 60_000);
    }, nextMinuteDelay);

    return () => {
      clearTimeout(timeoutId);
      if (intervalId) {
        clearInterval(intervalId);
      }
    };
  }, [headerStatusMode, headerStatusVisible]);

  useEffect(() => {
    if (!headerStatusVisible || (headerStatusMode !== "gpu" && headerStatusMode !== "npu")) {
      return;
    }

    const updateHardwareCardLimit = () => {
      setHardwareCardLimit(getHardwareCardLimit(window.innerWidth));
    };
    updateHardwareCardLimit();
    window.addEventListener("resize", updateHardwareCardLimit);
    return () => window.removeEventListener("resize", updateHardwareCardLimit);
  }, [headerStatusMode, headerStatusVisible]);

  const changeLanguage = (lng: string) => {
    i18n.changeLanguage(lng);
    updateUi({ language: lng });
    void invoke("save_app_language", { language: lng }).catch(() => {});
  };

  const updateAppearance = (patch: Partial<AppearanceSettings>) => {
    updateAppSettings({
      appearance: {
        ...appSettings.appearance,
        ...patch,
      },
    });
  };

  const handleZoom = (delta: number) => {
    if (!terminalZoomEnabled) return;
    updateAppSettings((prev) => ({
      terminal: {
        ...prev.terminal,
        font_size_delta:
          delta > 0
            ? increaseTerminalFontSizeDelta(
                prev.appearance.font_size,
                prev.terminal.font_size_delta,
              )
            : decreaseTerminalFontSizeDelta(
                prev.appearance.font_size,
                prev.terminal.font_size_delta,
              ),
      },
    }));
  };

  const handleResetZoom = () => {
    if (!terminalZoomEnabled) return;
    updateAppSettings((prev) => ({
      terminal: { ...prev.terminal, font_size_delta: resetTerminalFontSizeDelta() },
    }));
  };

  const menuKeys = [
    { key: "file", label: t("menu.file") },
    { key: "view", label: t("menu.view") },
    { key: "terminal", label: t("menu.terminal") },
    { key: "help", label: t("menu.help") },
  ];

  const buildActivityBarPanelMenuItems = (side: "left" | "right"): MenuItem[] => {
    const seen = new Set<string>();
    const hiddenItems = new Set(appSettings.ui.activity_bar_layout.hidden_items ?? []);
    return getActivityBarItemIdsForSide(appSettings.ui.activity_bar_layout, side)
      .filter((id) => {
        if (seen.has(id)) return false;
        seen.add(id);
        return (
          ACTIVITY_BAR_PANEL_ITEM_IDS.has(id) && isActivityItemAvailable(id, appSettings.ui)
        );
      })
      .map((id) => ({
        id: `view.panels.${id}`,
        label: getActivityBarPanelLabel(id, t),
        checked: !hiddenItems.has(id),
        action: () => onToggleActivityBarItemVisibility?.(id),
      }));
  };

  const leftActivityBarPanelMenuItems = buildActivityBarPanelMenuItems("left");
  const rightActivityBarPanelMenuItems = buildActivityBarPanelMenuItems("right");
  const panelOpenMode = normalizePanelOpenMode(appSettings.ui.panel_open_mode);

  const menus: Record<string, MenuItem[]> = {
    file: [
      addNativeAccelerator({
        id: "file.newSession",
        label: t("menu.newSession"),
        action: onNewSession,
        icon: "add",
      }, "tab.newSession", appSettings.keybindings),
      { label: "separator", separator: true },
      {
        id: "file.importConfig",
        label: t("settings.importConfig"),
        action: () => setShowImportDialog(true),
        icon: "file_import",
      },
      {
        id: "file.exportConfig",
        label: t("settings.exportConfig"),
        action: handleExport,
        icon: "file_export",
      },
    ],
    view: [
      {
        id: "view.theme",
        label: t("menu.theme"),
        icon: "palette",
        submenu: themeNames.map((th) => ({
          id: `view.theme.${th.id}`,
          label: th.name,
          checked: themeName === th.id,
          action: () => setTheme(th.id),
        })),
      },
      {
        id: "view.terminalTheme",
        label: t("menu.terminalTheme"),
        icon: "terminal",
        submenu: [
          {
            id: "view.terminalTheme.followUi",
            label: t("settings.followUiTheme"),
            checked: terminalThemeName === null,
            action: () => setTerminalTheme(null),
          },
          ...themeNames.map((th) => ({
            id: `view.terminalTheme.${th.id}`,
            label: th.name,
            checked: terminalThemeName === th.id,
            action: () => setTerminalTheme(th.id),
          })),
        ],
      },
      {
        id: "view.language",
        label: t("menu.language"),
        icon: "translate",
        submenu: AVAILABLE_LANGUAGES.map((l) => ({
          id: `view.language.${l.id}`,
          label: l.name,
          checked: i18n.language === l.id,
          action: () => changeLanguage(l.id),
        })),
      },
      { label: "separator", separator: true },
      {
        id: "view.headerStatus",
        label: t("menu.headerStatus"),
        icon: "info",
        submenu: [
          {
            id: "view.headerStatus.hidden",
            label: t("headerStatus.hidden"),
            checked: !headerStatusVisible,
            action: () => setShowHeaderStatusHideConfirm(true),
          },
          ...HEADER_STATUS_MODES.map((mode) => ({
            id: `view.headerStatus.${mode}`,
            label: t(`headerStatus.${mode}`),
            checked: headerStatusVisible && headerStatusMode === mode,
            action: () =>
              updateUi({
                header_status_mode: mode,
                header_status_visible: true,
              }),
          })),
        ],
      },
      {
        id: "view.panels",
        label: t("menu.panels"),
        icon: "view_sidebar",
        submenu: [
          {
            id: "view.panels.floatingMode",
            label: t("panel.floatingMode"),
            icon: "view_sidebar",
            checked: panelOpenMode === "floating",
            action: () =>
              onPanelOpenModeChange?.(
                panelOpenMode === "floating" ? "docked" : "floating",
              ),
          },
          {
            id: "view.panels.multiOpen",
            label: t("settings.panelMultiOpen"),
            icon: "view_sidebar",
            checked: appSettings.appearance.panel_multi_open,
            action: () =>
              updateAppearance({ panel_multi_open: !appSettings.appearance.panel_multi_open }),
          },
          { label: "separator", separator: true },
          {
            id: "view.panels.left",
            label: t("activityBar.leftSide"),
            submenu:
              leftActivityBarPanelMenuItems.length > 0
                ? leftActivityBarPanelMenuItems
                : [
                    {
                      id: "view.panels.left.empty",
                      label: t("activityBar.noPanels"),
                      disabled: true,
                    },
                  ],
          },
          {
            id: "view.panels.right",
            label: t("activityBar.rightSide"),
            submenu:
              rightActivityBarPanelMenuItems.length > 0
                ? rightActivityBarPanelMenuItems
                : [
                    {
                      id: "view.panels.right.empty",
                      label: t("activityBar.noPanels"),
                      disabled: true,
                    },
                  ],
          },
          { label: "separator", separator: true },
          {
            id: "view.panels.resetActivityBarLayout",
            label: t("activityBar.resetLayout"),
            action: onRequestActivityBarReset,
          },
        ],
      },
      { label: "separator", separator: true },
      addNativeAccelerator({
        id: "view.zoomIn",
        label: t("menu.zoomIn"),
        action: () => handleZoom(0.1),
        icon: "zoom_in",
        disabled: !terminalZoomEnabled,
      }, "view.zoomIn", appSettings.keybindings),
      addNativeAccelerator({
        id: "view.zoomOut",
        label: t("menu.zoomOut"),
        action: () => handleZoom(-0.1),
        icon: "zoom_out",
        disabled: !terminalZoomEnabled,
      }, "view.zoomOut", appSettings.keybindings),
      addNativeAccelerator({
        id: "view.resetZoom",
        label: t("menu.resetZoom"),
        action: handleResetZoom,
        icon: "restart_alt",
        disabled: !terminalZoomEnabled,
      }, "view.resetZoom", appSettings.keybindings),
    ],
    terminal: [
      addNativeAccelerator({
        id: "terminal.commandPalette",
        label: t("menu.commandPalette"),
        icon: "search",
        action: () => onOpenCommandPalette?.(),
      }, "tab.quickSwitch", appSettings.keybindings),
      { label: "separator", separator: true },
      {
        id: "terminal.display",
        label: t("menu.terminalDisplay"),
        icon: "visibility",
        submenu: [
          {
            id: "terminal.display.workspacePadding",
            label: t("settings.showWorkspacePadding"),
            checked: appSettings.terminal.show_workspace_padding ?? false,
            action: () =>
              updateAppSettings({
                terminal: {
                  ...appSettings.terminal,
                  show_workspace_padding: !(appSettings.terminal.show_workspace_padding ?? false),
                },
              }),
          },
          {
            id: "terminal.display.lineNumbers",
            label: t("settings.showLineNumbers"),
            checked: appSettings.terminal.show_line_numbers,
            action: () =>
              updateAppSettings({
                terminal: {
                  ...appSettings.terminal,
                  show_line_numbers: !appSettings.terminal.show_line_numbers,
                },
              }),
          },
          {
            id: "terminal.display.timestamps",
            label: t("settings.showTimestamps"),
            checked: appSettings.terminal.show_timestamps,
            action: () =>
              updateAppSettings({
                terminal: {
                  ...appSettings.terminal,
                  show_timestamps: !appSettings.terminal.show_timestamps,
                },
              }),
          },
        ],
      },
      {
        id: "terminal.actionLinks",
        label: t("settings.actionLinks"),
        checked: appSettings.terminal.action_links_enabled ?? false,
        action: () =>
          updateAppSettings({
            terminal: {
              ...appSettings.terminal,
              action_links_enabled: !(appSettings.terminal.action_links_enabled ?? false),
            },
          }),
      },
      {
        id: "terminal.zoomEnabled",
        label: t("settings.terminalZoomEnabled"),
        checked: terminalZoomEnabled,
        action: () =>
          updateAppSettings({
            interaction: {
              ...appSettings.interaction,
              terminal_zoom_enabled: !terminalZoomEnabled,
            },
          }),
      },
      { label: "separator", separator: true },
      {
        id: "terminal.smartSplit",
        label: t("menu.smartSplit"),
        icon: "splitscreen",
        submenu: [
          {
            id: "terminal.smartSplit.auto",
            label: t("menu.autoTile"),
            icon: "dashboard",
            action: () => onSmartSplit?.("auto"),
          },
          {
            id: "terminal.smartSplit.horizontal",
            label: t("menu.tileHorizontally"),
            icon: "swap_horiz",
            action: () => onSmartSplit?.("horizontal"),
          },
          {
            id: "terminal.smartSplit.vertical",
            label: t("menu.tileVertically"),
            icon: "swap_vert",
            action: () => onSmartSplit?.("vertical"),
          },
        ],
      },
      {
        id: "terminal.unsplit",
        label: t("menu.unsplit"),
        icon: "merge",
        action: () => onUnsplit?.(),
        disabled: !canUnsplit,
      },
      { label: "separator", separator: true },
      {
        id: "terminal.syncInput",
        label: t("menu.syncInput"),
        icon: "sync",
        submenu: [
          addNativeAccelerator({
            id: "terminal.syncInput.manageGroups",
            label: t("menu.manageGroups"),
            icon: "settings",
            action: () => onManageSyncGroups?.(),
          }, "terminal.manageSyncGroups", appSettings.keybindings),
        ],
      },
      { label: "separator", separator: true },
      {
        id: "terminal.broadcastToAll",
        label: t("menu.broadcastToAll"),
        icon: "cell_tower",
        action: () => onBroadcastToAll?.(),
        checked: broadcastToAll,
      },
      { label: "separator", separator: true },
      addNativeAccelerator({
        id: "terminal.clear",
        label: t("menu.clearTerminal"),
        icon: "delete_sweep",
        action: () => onClearTerminal?.(),
      }, "terminal.clear", appSettings.keybindings),
      {
        id: "terminal.refit",
        label: t("menu.refitTerminals"),
        icon: "fit_screen",
        action: () => onRefitTerminals?.(),
      },
    ],
    help: [
      {
        id: "help.documentation",
        label: t("menu.documentation"),
        icon: "menu_book",
        action: () => openUrl(`${packageJson.docspage}`),
      },
      {
        id: "help.checkUpdates",
        label: t("menu.checkForUpdates"),
        icon: hasUpdate ? "upgrade" : "update",
        action: onCheckForUpdates,
      },
      {
        id: "help.viewLogs",
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
      { id: "app.about", label: t("menu.about"), action: onAbout, icon: "info" },
    ],
  };

  const isActionEnabledForNativeMenu = (item: MenuItem) =>
    !item.disabled && (!locked || !item.id || MACOS_ALLOWED_LOCKED_ACTIONS.has(item.id));

  const convertMenuItemsForMacos = (items: MenuItem[]): MacosMenuSpecItem[] =>
    items.flatMap((item): MacosMenuSpecItem[] => {
      if (item.separator) return [{ kind: "separator" }];
      if (!item.id) return [];
      if (item.submenu) {
        return [
          {
            kind: "submenu",
            id: item.id,
            label: item.label,
            enabled: !item.disabled,
            items: convertMenuItemsForMacos(item.submenu),
          },
        ];
      }
      const enabled = isActionEnabledForNativeMenu(item);
      if (typeof item.checked === "boolean") {
        return [
          {
            kind: "check",
            id: item.id,
            label: item.label,
            enabled,
            checked: item.checked,
            accelerator: item.accelerator,
          },
        ];
      }
      return [
        {
          kind: "item",
          id: item.id,
          label: item.label,
          enabled,
          accelerator: item.accelerator,
        },
      ];
    });

  const macosMenuSpec: MacosMenuSpec = {
    menus: [
      {
        id: "app",
        label: "NyaTerm",
        items: [
          {
            kind: "item",
            id: "app.about",
            label: t("menu.about"),
            enabled: true,
            accelerator: null,
          },
          { kind: "separator" },
          { kind: "predefined", role: "services" },
          { kind: "separator" },
          { kind: "predefined", role: "hide" },
          { kind: "predefined", role: "hideOthers" },
          { kind: "predefined", role: "showAll" },
          { kind: "separator" },
          {
            kind: "item",
            id: "app.quit",
            label: t("menu.exit"),
            enabled: true,
            accelerator: "Cmd+Q",
          },
        ],
      },
      {
        id: "file",
        label: t("menu.file"),
        items: convertMenuItemsForMacos(menus.file),
      },
      {
        id: "edit",
        label: t("menu.edit"),
        items: [
          { kind: "predefined", role: "undo", label: t("menu.undo") },
          { kind: "predefined", role: "redo", label: t("menu.redo") },
          { kind: "separator" },
          { kind: "predefined", role: "cut", label: t("menu.cut") },
          { kind: "predefined", role: "copy", label: t("menu.copy") },
          { kind: "predefined", role: "paste", label: t("menu.paste") },
          { kind: "predefined", role: "selectAll", label: t("menu.selectAll") },
        ],
      },
      {
        id: "view",
        label: t("menu.view"),
        items: convertMenuItemsForMacos(menus.view),
      },
      {
        id: "terminal",
        label: t("menu.terminal"),
        items: convertMenuItemsForMacos(menus.terminal),
      },
      {
        id: "help",
        label: t("menu.help"),
        items: convertMenuItemsForMacos(menus.help.filter((item) => item.id !== "app.about")),
      },
    ],
  };

  const runNativeMenuAction = (actionId: string) => {
    if (locked && !MACOS_ALLOWED_LOCKED_ACTIONS.has(actionId)) return;
    if (actionId === "app.quit") {
      onRequestQuit?.();
      return;
    }

    const visit = (items: MenuItem[]): MenuItem | null => {
      for (const item of items) {
        if (item.id === actionId) return item;
        if (item.submenu) {
          const found = visit(item.submenu);
          if (found) return found;
        }
      }
      return null;
    };

    for (const menu of Object.values(menus)) {
      const item = visit(menu);
      if (item && isActionEnabledForNativeMenu(item)) {
        item.action?.();
        return;
      }
    }
  };
  nativeMenuActionRef.current = runNativeMenuAction;

  useEffect(() => {
    if (!isMacOS) return;

    const specKey = JSON.stringify(macosMenuSpec);
    if (lastMacosMenuSpecRef.current === specKey) return;
    lastMacosMenuSpecRef.current = specKey;

    invoke("set_macos_app_menu", { spec: macosMenuSpec }).catch((error) => {
      logger.error({
        domain: "ui.error",
        event: "macos_menu.set_failed",
        message: "Failed to update macOS app menu",
        error,
      });
    });
  });

  useEffect(() => {
    if (!isMacOS) return;

    let disposed = false;
    let dispose: (() => void) | undefined;

    listen<MacosMenuActionPayload>("macos-menu-action", ({ payload }) => {
      if (payload.targetWindowLabel && payload.targetWindowLabel !== appWindow.label) return;
      nativeMenuActionRef.current(payload.actionId);
    })
      .then((unlisten) => {
        if (disposed) {
          unlisten();
        } else {
          dispose = unlisten;
        }
      })
      .catch(() => {});

    return () => {
      disposed = true;
      dispose?.();
    };
  }, [appWindow.label]);

  const renderMenuItem = (item: MenuItem, idx: number) => {
    if (item.separator) {
      return <MenubarSeparator key={`sep-${idx}`} />;
    }

    if (item.submenu) {
      return (
        <MenubarSub key={item.label}>
          <MenubarSubTrigger disabled={item.disabled} className="gap-2">
            {item.icon && (
              <DynamicIcon name={item.icon} className="text-[1rem] text-[var(--df-text-muted)]" />
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
          disabled={item.disabled}
          onCheckedChange={() => {
            item.action?.();
          }}
        >
          {item.icon && (
            <DynamicIcon name={item.icon} className="text-[1rem] text-[var(--df-text-muted)]" />
          )}
          <span className="flex-1">{item.label}</span>
          {item.shortcut && <MenubarShortcut>{item.shortcut}</MenubarShortcut>}
        </MenubarCheckboxItem>
      );
    }

    return (
      <MenubarItem
        key={item.label}
        disabled={item.disabled}
        onClick={() => {
          item.action?.();
        }}
      >
        {item.icon && (
          <DynamicIcon
            name={item.icon}
            className={`text-[1rem] ${item.icon === "upgrade" ? "text-green-500" : "text-[var(--df-text-muted)]"}`}
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

  const handleConfirmHideHeaderStatus = () => {
    setShowHeaderStatusHideConfirm(false);
    updateUi({ header_status_visible: false });
  };

  const hasActiveStatsSession = Boolean(
    activePane && activePane.type === "SSH" && !activePane.connecting && !activePane.connectError,
  );

  const sessionStatus = useMemo(() => {
    if (!activeTab || !activePane) {
      return {
        icon: null,
        text: "NyaTerm",
        title: "NyaTerm",
      };
    }

    const getSessionIcon = () => {
      if (activePane.type === "SSH" && activeConnection) {
        const def = resolveConnectionIcon(activeConnection.icon);
        const IconComp = def.icon;
        return <IconComp className="text-sm shrink-0" style={{ color: def.color }} />;
      }

      if (activeConnection?.icon) {
        const def = resolveConnectionIcon(activeConnection.icon);
        const IconComp = def.icon;
        return <IconComp className="text-sm shrink-0" style={{ color: def.color }} />;
      }

      if (activePane.type === "Telnet") {
        return <MdDns className="text-sm shrink-0" />;
      }

      if (activePane.type === "Serial") {
        return <MdCellTower className="text-sm shrink-0" />;
      }

      return <MdTerminal className="text-sm shrink-0" />;
    };

    if (activePane.type === "SSH" && activeConnection && !activeTab.customName) {
      const icon = getSessionIcon();
      const text = `${activeConnection.name} - ${activeConnection.username}@${activeConnection.host}:${activeConnection.port}`;
      return {
        icon,
        text,
        title: text,
      };
    }

    if (activePane.type === "SSH") {
      return {
        icon: getSessionIcon(),
        text: activeDisplayName,
        title: activeDisplayName,
      };
    }

    return {
      icon: getSessionIcon(),
      text: activeDisplayName,
      title: activeDisplayName,
    };
  }, [activeConnection, activeDisplayName, activePane, activeTab]);

  const remoteStatusFallback = useMemo(() => {
    if (!hasActiveStatsSession) return t("panel.resourceMonitorNoSession");
    if (!remoteStatsEnabled) return t("panel.resourceMonitorDisabled");
    if (remoteStats?.stats) return null;
    if (remoteStats?.error) return t("panel.resourceMonitorError");
    return t("common.loading");
  }, [hasActiveStatsSession, remoteStats?.error, remoteStats?.stats, remoteStatsEnabled, t]);

  const headerStatus = useMemo(() => {
    if (headerStatusMode === "session") {
      return {
        icon: sessionStatus.icon,
        text: sessionStatus.text,
        title: sessionStatus.title,
      };
    }

    if (headerStatusMode === "datetime") {
      const text = new Intl.DateTimeFormat(i18n.language, {
        year: "numeric",
        month: "2-digit",
        day: "2-digit",
        weekday: "short",
        hour: "2-digit",
        minute: "2-digit",
      }).format(currentMinute);

      return {
        icon: <MdAccessTime />,
        text,
        title: text,
      };
    }

    if (headerStatusMode === "gpu") {
      const overview = gpuOverviewState?.overview;
      const fallback = !hasActiveStatsSession
        ? t("gpuMonitor.noSession")
        : gpuOverviewState?.error && !overview
          ? t("gpuMonitor.error")
          : !overview
            ? t("common.loading")
            : !overview.available
              ? t("gpuMonitor.unavailable")
              : overview.gpus.length === 0
                ? t("gpuMonitor.noGpus")
                : null;

      if (fallback || !overview || !overview.available || overview.gpus.length === 0) {
        return {
          icon: <SiNvidia />,
          text: fallback ?? t("common.loading"),
          title: fallback ?? t("common.loading"),
        };
      }

      const cards = sortHardwareCardsByIndex(buildGpuHardwareCards(overview));
      const pageCount = Math.max(1, Math.ceil(cards.length / hardwareCardLimit));
      const currentPage = Math.min(hardwarePage.gpu, pageCount - 1);
      const visibleCards = cards.slice(
        currentPage * hardwareCardLimit,
        currentPage * hardwareCardLimit + hardwareCardLimit,
      );
      const hiddenCount = cards.length - visibleCards.length;
      const compact = getHardwareStatusCompact(visibleCards.length, hiddenCount, hardwareCardLimit);
      const title = buildHardwareTitle("GPU", cards, "GPU");

      return {
        icon: null,
        interactive: true,
        text: (
          <HeaderHardwareStatus
            cards={visibleCards}
            compact={compact}
            hiddenCount={hiddenCount}
            icon={<SiNvidia />}
            label="GPU"
            onNextPage={() =>
              setHardwarePage((current) => ({ ...current, gpu: (currentPage + 1) % pageCount }))
            }
            onPreviousPage={() =>
              setHardwarePage((current) => ({
                ...current,
                gpu: (currentPage - 1 + pageCount) % pageCount,
              }))
            }
          />
        ),
        title,
      };
    }

    if (headerStatusMode === "npu") {
      const overview = npuOverviewState?.overview;
      const fallback = !hasActiveStatsSession
        ? t("ascendNpuMonitor.noSession")
        : npuOverviewState?.error && !overview
          ? t("ascendNpuMonitor.error")
          : !overview
            ? t("common.loading")
            : !overview.available
              ? t("ascendNpuMonitor.unavailable")
              : overview.npus.length === 0
                ? t("ascendNpuMonitor.noNpus")
                : null;

      if (fallback || !overview || !overview.available || overview.npus.length === 0) {
        return {
          icon: <AscendIcon />,
          text: fallback ?? t("common.loading"),
          title: fallback ?? t("common.loading"),
        };
      }

      const cards = sortHardwareCardsByIndex(buildNpuHardwareCards(overview));
      const pageCount = Math.max(1, Math.ceil(cards.length / hardwareCardLimit));
      const currentPage = Math.min(hardwarePage.npu, pageCount - 1);
      const visibleCards = cards.slice(
        currentPage * hardwareCardLimit,
        currentPage * hardwareCardLimit + hardwareCardLimit,
      );
      const hiddenCount = cards.length - visibleCards.length;
      const compact = getHardwareStatusCompact(visibleCards.length, hiddenCount, hardwareCardLimit);
      const title = buildHardwareTitle("NPU", cards, "AI Core");

      return {
        icon: null,
        interactive: true,
        text: (
          <HeaderHardwareStatus
            cards={visibleCards}
            compact={compact}
            hiddenCount={hiddenCount}
            icon={<AscendIcon />}
            label="NPU"
            onNextPage={() =>
              setHardwarePage((current) => ({ ...current, npu: (currentPage + 1) % pageCount }))
            }
            onPreviousPage={() =>
              setHardwarePage((current) => ({
                ...current,
                npu: (currentPage - 1 + pageCount) % pageCount,
              }))
            }
          />
        ),
        title,
      };
    }

    const stats = remoteStats?.stats;
    if (remoteStatusFallback || !stats) {
      return {
        icon: null,
        text: remoteStatusFallback ?? t("common.loading"),
        title: remoteStatusFallback ?? t("common.loading"),
      };
    }

    if (headerStatusMode === "host") {
      const uptime = formatUptimeShort(stats.system.uptime_sec, t);
      const text = `${stats.system.hostname} - ${stats.system.os}/${stats.system.arch} - ${uptime}`;
      return {
        icon: null,
        text: (
          <span className="flex min-w-0 items-center gap-1.5">
            <HeaderStatusPart icon={<MdDns />} iconColor="#38bdf8">
              {stats.system.hostname}
            </HeaderStatusPart>
            <HeaderStatusDivider />
            <HeaderStatusPart icon={<MdComputer />} iconColor="#a78bfa">
              {stats.system.os}/{stats.system.arch}
            </HeaderStatusPart>
            <HeaderStatusDivider />
            <HeaderStatusPart icon={<MdAccessTime />} iconColor="#34d399">
              {uptime}
            </HeaderStatusPart>
          </span>
        ),
        title: text,
      };
    }

    const memTotal = stats.memory.used + stats.memory.available;
    const memoryUsedText = formatBytes(stats.memory.used);
    const memoryTotalText = formatBytes(memTotal);
    const memoryText = `${memoryUsedText}/${memoryTotalText}`;
    const cpuColor = getPressureColor(stats.cpu.usage);
    const memoryUsagePercent = memTotal > 0 ? (stats.memory.used / memTotal) * 100 : 0;
    const memoryColor = getPressureColor(memoryUsagePercent);
    const txText = formatRate(stats.network_summary.tx_bytes_per_sec);
    const rxText = formatRate(stats.network_summary.rx_bytes_per_sec);
    const text = `CPU ${formatPct(stats.cpu.usage)} - RAM ${memoryText} - NET ↑ ${txText} ↓ ${rxText}`;
    return {
      icon: null,
      text: (
        <span className="flex min-w-0 items-center gap-1.5 font-mono tabular-nums">
          <HeaderStatusPart icon={<MdSpeed />} iconColor="#38bdf8">
            CPU{" "}
            <span style={cpuColor ? { color: cpuColor } : undefined}>
              {formatPct(stats.cpu.usage)}
            </span>
          </HeaderStatusPart>
          <HeaderStatusDivider />
          <HeaderStatusPart icon={<MdMemory />} iconColor="#a78bfa">
            RAM{" "}
            <span style={memoryColor ? { color: memoryColor } : undefined}>{memoryUsedText}</span>/
            {memoryTotalText}
          </HeaderStatusPart>
          <HeaderStatusDivider />
          <HeaderStatusPart icon={<MdUpload />} iconColor="#f59e0b">
            {txText}
          </HeaderStatusPart>
          <HeaderStatusPart icon={<MdDownload />} iconColor="#34d399">
            {rxText}
          </HeaderStatusPart>
        </span>
      ),
      title: text,
    };
  }, [
    currentMinute,
    gpuOverviewState?.error,
    gpuOverviewState?.overview,
    hardwarePage.gpu,
    hardwarePage.npu,
    hardwareCardLimit,
    headerStatusMode,
    hasActiveStatsSession,
    i18n.language,
    npuOverviewState?.error,
    npuOverviewState?.overview,
    remoteStats?.stats,
    remoteStatusFallback,
    sessionStatus,
    t,
  ]);

  return (
    <header
      className="h-10 border-b flex items-center gap-2 px-2 select-none shrink-0"
      style={{ backgroundColor: "var(--df-bg-panel)", borderColor: "var(--df-border)" }}
    >
      <div className={`flex items-center gap-2 shrink-0${isMacOS ? " pl-[84px]" : ""}`}>
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

        {!isMacOS && (
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
        )}
      </div>

      <div className="flex-1 min-w-0 h-full flex items-center justify-center gap-2 px-2">
        <div className="h-full min-w-0 flex-1" data-tauri-drag-region />
        {headerStatusVisible && (
          <div
            className="flex max-w-full min-w-0 items-center gap-0.5 rounded-md text-xs font-medium"
            style={{ color: "var(--df-text-muted)" }}
            title={headerStatus.title}
            data-tauri-drag-region
          >
            <div
              className="flex min-w-0 items-center gap-1.5 overflow-hidden whitespace-nowrap px-2 py-1"
              data-tauri-drag-region
            >
              <span className="pointer-events-none inline-flex shrink-0" data-tauri-drag-region>
                {headerStatus.icon}
              </span>
              <span
                className={`flex min-w-0 items-center overflow-hidden whitespace-nowrap ${
                  headerStatus.interactive ? "" : "pointer-events-none"
                }`}
                data-tauri-drag-region
              >
                {headerStatus.text}
              </span>
            </div>
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <button
                  type="button"
                  className="group flex h-6 w-6 shrink-0 items-center justify-center rounded-md transition-colors hover:bg-[color-mix(in_srgb,var(--df-text-muted)_10%,transparent)] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[var(--df-primary)]"
                  aria-label={t("headerStatus.select")}
                >
                  <MdKeyboardArrowDown className="text-sm opacity-60 transition-opacity group-hover:opacity-100" />
                </button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="center" className="min-w-[190px]">
                <DropdownMenuRadioGroup
                  value={headerStatusMode}
                  onValueChange={(value) => {
                    updateUi({
                      header_status_mode: normalizeHeaderStatusMode(value),
                      header_status_visible: true,
                    });
                  }}
                >
                  {HEADER_STATUS_MODES.map((mode) => (
                    <DropdownMenuRadioItem key={mode} value={mode}>
                      {t(`headerStatus.${mode}`)}
                    </DropdownMenuRadioItem>
                  ))}
                </DropdownMenuRadioGroup>
                <DropdownMenuSeparator />
                <DropdownMenuItem onClick={() => setShowHeaderStatusHideConfirm(true)}>
                  <MdVisibilityOff className="text-sm text-muted-foreground" />
                  <span>{t("headerStatus.hide")}</span>
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          </div>
        )}
        <div className="h-full min-w-0 flex-1" data-tauri-drag-region />
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
      <HeaderStatusHideConfirmDialog
        open={showHeaderStatusHideConfirm}
        onOpenChange={setShowHeaderStatusHideConfirm}
        onConfirm={handleConfirmHideHeaderStatus}
      />
    </header>
  );
}
