import type { AssetAccelerator, AssetDisk, AssetMetadata, SavedConnection } from "@/types/global";

export const NOT_APPLICABLE = "-";

export interface AssetDisplayLabels {
  none: string;
  notApplicable: string;
  localMachine: string;
}

export const DEFAULT_ASSET_DISPLAY_LABELS: AssetDisplayLabels = {
  none: "None",
  notApplicable: NOT_APPLICABLE,
  localMachine: "Local",
};

export function naturalCompare(left: string, right: string): number {
  return left.localeCompare(right, undefined, { numeric: true, sensitivity: "base" });
}

function text(value: unknown): string {
  if (value === null || value === undefined) return "";
  return String(value).trim();
}

export function formatMissing(
  value: string | number | null | undefined,
  labels: Pick<AssetDisplayLabels, "notApplicable"> = DEFAULT_ASSET_DISPLAY_LABELS,
): string {
  const formatted = text(value);
  return formatted || labels.notApplicable;
}

export function formatBytes(
  bytes: number | null | undefined,
  labels: Pick<AssetDisplayLabels, "notApplicable"> = DEFAULT_ASSET_DISPLAY_LABELS,
): string {
  if (bytes === null || bytes === undefined) return labels.notApplicable;
  if (!Number.isFinite(bytes) || bytes < 0) return labels.notApplicable;
  if (bytes === 0) return "0 B";

  const units = ["B", "KB", "MB", "GB", "TB", "PB"];
  const unitIndex = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  const value = bytes / 1024 ** unitIndex;
  const precision = value >= 10 || unitIndex === 0 ? 0 : 1;
  return `${value.toFixed(precision)} ${units[unitIndex]}`;
}

export function formatCpuSummary(
  asset: Pick<AssetMetadata, "cpu_cores" | "cpu_threads"> | null | undefined,
  labels: Pick<AssetDisplayLabels, "notApplicable"> = DEFAULT_ASSET_DISPLAY_LABELS,
): string {
  if (!asset) return labels.notApplicable;
  const parts: string[] = [];
  if (asset.cpu_cores !== undefined && asset.cpu_cores !== null) parts.push(`${asset.cpu_cores}C`);
  if (asset.cpu_threads !== undefined && asset.cpu_threads !== null) {
    parts.push(`${asset.cpu_threads}T`);
  }
  return parts.length > 0 ? parts.join(" / ") : labels.notApplicable;
}

function formatAcceleratorItem(item: AssetAccelerator): string {
  const label = [item.vendor, item.model].map(text).filter(Boolean).join(" ");
  const memory = item.memory_bytes === undefined ? "" : formatBytes(item.memory_bytes);
  const count = item.count && item.count > 1 ? ` × ${item.count}` : "";
  const fallback = item.type.toUpperCase();
  return [label || fallback, memory].filter(Boolean).join(" ") + count;
}

export function formatAccelerators(
  accelerators: AssetAccelerator[] | null | undefined,
  labels: Pick<AssetDisplayLabels, "notApplicable" | "none"> = DEFAULT_ASSET_DISPLAY_LABELS,
  options: { maxItems?: number } = {},
): string {
  if (accelerators === null || accelerators === undefined) return labels.notApplicable;
  if (accelerators.length === 0) return labels.none;
  const maxItems = options.maxItems ?? accelerators.length;
  const visible = accelerators.slice(0, maxItems).map(formatAcceleratorItem);
  const remaining = accelerators.length - visible.length;
  return remaining > 0 ? `${visible.join(", ")} +${remaining}` : visible.join(", ");
}

export function formatDiskSummary(
  disks: AssetDisk[] | null | undefined,
  labels: Pick<AssetDisplayLabels, "notApplicable" | "none"> = DEFAULT_ASSET_DISPLAY_LABELS,
): string {
  if (disks === null || disks === undefined) return labels.notApplicable;
  if (disks.length === 0) return labels.none;
  const total = getDiskTotalBytes(disks) ?? 0;
  return total > 0 ? formatBytes(total, labels) : labels.notApplicable;
}

export function getDiskTotalBytes(disks: AssetDisk[] | null | undefined): number | null {
  if (!disks) return null;
  return disks.reduce((sum, disk) => {
    const capacity = disk.capacity_bytes ?? 0;
    const count = disk.count && disk.count > 0 ? disk.count : 1;
    return sum + capacity * count;
  }, 0);
}

export function formatAssetUpdatedAt(
  updatedAt: string | null | undefined,
  labels: Pick<AssetDisplayLabels, "notApplicable"> = DEFAULT_ASSET_DISPLAY_LABELS,
): string {
  const raw = text(updatedAt);
  if (!raw) return labels.notApplicable;
  const date = new Date(raw);
  if (Number.isNaN(date.getTime())) return raw;
  return date.toLocaleDateString();
}

export function formatAssetConnectionTime(
  value: string | number | null | undefined,
  labels: Pick<AssetDisplayLabels, "notApplicable"> = DEFAULT_ASSET_DISPLAY_LABELS,
): string {
  const time = getAssetConnectionTimeMs(value);
  if (time === null) return labels.notApplicable;
  const date = new Date(time);
  return date.toLocaleString();
}

export function getAssetConnectionTimeMs(value: string | number | null | undefined): number | null {
  if (typeof value === "number") {
    return Number.isFinite(value) && value >= 0 ? value : null;
  }
  const raw = text(value);
  if (!raw) return null;
  const time = Date.parse(raw);
  return Number.isFinite(time) ? time : null;
}

export function formatAssetAddress(
  connection: Pick<SavedConnection, "type" | "host" | "port_name">,
  labels: Pick<AssetDisplayLabels, "localMachine" | "notApplicable"> = DEFAULT_ASSET_DISPLAY_LABELS,
): string {
  if (connection.type === "local_terminal") return labels.localMachine;
  if (connection.type === "serial") return text(connection.port_name) || labels.notApplicable;
  return text(connection.host) || labels.notApplicable;
}

export function formatAssetSystem(
  asset: Pick<AssetMetadata, "os_name" | "os_version" | "architecture"> | null | undefined,
  labels: Pick<AssetDisplayLabels, "notApplicable"> = DEFAULT_ASSET_DISPLAY_LABELS,
): string {
  if (!asset) return labels.notApplicable;
  const os = [asset.os_name, asset.os_version].map(text).filter(Boolean).join(" ");
  const arch = text(asset.architecture);
  const result = [os, arch].filter(Boolean).join(" · ");
  return result || labels.notApplicable;
}

export function buildAssetSearchText(connection: SavedConnection, groupPath: string): string {
  const asset = connection.asset;
  const accelerators = asset?.accelerators ?? [];
  const disks = asset?.disks ?? [];
  return [
    connection.name,
    connection.host,
    connection.username,
    connection.port_name,
    asset?.hostname,
    asset?.os_name,
    asset?.os_version,
    asset?.architecture,
    asset?.cpu_model,
    connection.tags?.join(" "),
    asset?.notes,
    groupPath,
    ...accelerators.flatMap((item) => [item.vendor, item.model, item.type]),
    ...disks.map((disk) => disk.model),
  ]
    .map(text)
    .filter(Boolean)
    .join(" ")
    .toLowerCase();
}

function parseIpv4(value: string): number[] | null {
  const match = value.trim().match(/^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/);
  if (!match) return null;
  const parts = match.slice(1).map(Number);
  return parts.every((part) => part >= 0 && part <= 255) ? parts : null;
}

export function compareAssetAddress(left: string, right: string): number {
  const leftIpv4 = parseIpv4(left);
  const rightIpv4 = parseIpv4(right);
  if (leftIpv4 && rightIpv4) {
    for (let index = 0; index < leftIpv4.length; index += 1) {
      const diff = leftIpv4[index] - rightIpv4[index];
      if (diff !== 0) return diff;
    }
    return 0;
  }
  if (leftIpv4) return -1;
  if (rightIpv4) return 1;
  return naturalCompare(left, right);
}
