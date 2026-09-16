import type { TFunction } from "i18next";
import type { CSSProperties } from "react";
import { useCallback, useDeferredValue, useEffect, useMemo, useState } from "react";
import { useApp } from "@/context/AppContext";
import { buildGroupPath, getConnectionsForAssetGroup, getGroupPathLabel } from "@/lib/assetGroups";
import type { Group, SavedConnection } from "@/types/global";
import AssetBreadcrumb from "./AssetBreadcrumb";
import AssetCardGrid from "./AssetCardGrid";
import AssetTable from "./AssetTable";
import AssetToolbar from "./AssetToolbar";
import {
  type AssetDisplayLabels,
  buildAssetSearchText,
  compareAssetAddress,
  DEFAULT_ASSET_DISPLAY_LABELS,
  formatAccelerators,
  formatAssetAddress,
  getAssetConnectionTimeMs,
  getDiskTotalBytes,
  naturalCompare,
} from "./assetFormatters";
import type {
  AssetGroupOption,
  AssetRecord,
  AssetSortKey,
  AssetSortState,
  AssetViewMode,
} from "./types";

interface AssetViewProps {
  t: TFunction;
  transparentBackground?: boolean;
  onConnectConnection: (connection: SavedConnection) => Promise<void> | void;
  onEditConnection: (connection: SavedConnection) => void;
}

export default function AssetView({
  t,
  transparentBackground = false,
  onConnectConnection,
  onEditConnection,
}: AssetViewProps) {
  const { appSettings, savedConnections, savedGroups, refreshConnections, updateUi } = useApp();
  const [search, setSearch] = useState("");
  const deferredSearch = useDeferredValue(search);
  const [selectedTags, setSelectedTags] = useState<Set<string>>(new Set());
  const [viewMode, setViewMode] = useState<AssetViewMode>("list");
  const [selectedGroupId, setSelectedGroupId] = useState<string | null>(null);
  const sortState = useMemo(
    () =>
      normalizeAssetSortState(appSettings.ui.asset_sort_key, appSettings.ui.asset_sort_direction),
    [appSettings.ui.asset_sort_direction, appSettings.ui.asset_sort_key],
  );

  useEffect(() => {
    void refreshConnections();
  }, [refreshConnections]);

  const labels = useMemo<AssetDisplayLabels>(
    () => ({
      ...DEFAULT_ASSET_DISPLAY_LABELS,
      none: t("assets.none"),
      notApplicable: t("assets.notApplicable"),
      localMachine: t("assets.localMachine"),
    }),
    [t],
  );

  const groupSortById = useMemo(
    () => new Map(savedGroups.map((group) => [group.id, group.sort_order])),
    [savedGroups],
  );

  const groupOptions = useMemo(
    () => buildGroupOptions(savedGroups, t("assets.title")),
    [savedGroups, t],
  );

  const allRecords = useMemo<AssetRecord[]>(() => {
    return savedConnections.map((connection) => {
      const groupPath = getGroupPathLabel(savedGroups, connection.group_id, t("assets.title"));
      return {
        connection,
        groupPath,
        groupSortOrder: connection.group_id ? (groupSortById.get(connection.group_id) ?? 0) : 0,
        connectionTimeMs: getAssetConnectionTimeMs(connection.last_used_at_ms),
        searchText: buildAssetSearchText(connection, groupPath),
      };
    });
  }, [groupSortById, savedConnections, savedGroups, t]);

  const availableTags = useMemo(() => collectConnectionTags(savedConnections), [savedConnections]);

  useEffect(() => {
    const available = new Set(availableTags);
    setSelectedTags((current) => {
      const next = new Set([...current].filter((tag) => available.has(tag)));
      if (next.size === current.size) return current;
      return next;
    });
  }, [availableTags]);

  const groupConnections = useMemo(
    () => getConnectionsForAssetGroup(savedConnections, savedGroups, selectedGroupId),
    [savedConnections, savedGroups, selectedGroupId],
  );
  const groupConnectionIds = useMemo(
    () => new Set(groupConnections.map((connection) => connection.id)),
    [groupConnections],
  );

  const filteredRecords = useMemo(() => {
    const keyword = deferredSearch.trim().toLowerCase();
    return allRecords.filter((record) => {
      if (selectedGroupId && !groupConnectionIds.has(record.connection.id)) return false;
      if (keyword && !record.searchText.includes(keyword)) return false;
      return matchesSelectedTags(record.connection, selectedTags);
    });
  }, [allRecords, deferredSearch, groupConnectionIds, selectedGroupId, selectedTags]);

  const sortedRecords = useMemo(
    () => sortAssetRecords(filteredRecords, sortState, labels),
    [filteredRecords, labels, sortState],
  );
  const surfaceStyle = useMemo(
    () =>
      ({
        backgroundColor: transparentBackground ? "transparent" : "var(--df-bg-terminal)",
        "--nyaterm-asset-sticky-bg": "var(--df-bg-terminal-solid)",
      }) as CSSProperties,
    [transparentBackground],
  );

  const toggleTag = useCallback((tag: string) => {
    setSelectedTags((current) => {
      const next = new Set(current);
      if (next.has(tag)) next.delete(tag);
      else next.add(tag);
      return next;
    });
  }, []);

  const handleSearchChange = useCallback((value: string) => {
    setSearch(value);
  }, []);

  const handleSelectGroup = useCallback((groupId: string | null) => {
    setSelectedGroupId(groupId);
  }, []);

  const handleClearTags = useCallback(() => {
    setSelectedTags(new Set());
  }, []);

  const handleSortChange = useCallback(
    (key: AssetSortKey) => {
      updateUi((prev) => {
        const current = normalizeAssetSortState(prev.asset_sort_key, prev.asset_sort_direction);
        if (!current || current.key !== key) {
          return { asset_sort_key: key, asset_sort_direction: "asc" };
        }
        return {
          asset_sort_key: key,
          asset_sort_direction: current.direction === "asc" ? "desc" : "asc",
        };
      });
    },
    [updateUi],
  );

  return (
    <div
      className="@container relative flex h-full min-h-0 flex-col overflow-hidden"
      style={surfaceStyle}
      data-asset-view
    >
      <AssetToolbar
        t={t}
        totalCount={savedConnections.length}
        search={search}
        onSearchChange={handleSearchChange}
        availableTags={availableTags}
        selectedTags={selectedTags}
        onToggleTag={toggleTag}
        onClearTags={handleClearTags}
        viewMode={viewMode}
        onViewModeChange={setViewMode}
      />
      <AssetBreadcrumb
        t={t}
        groups={savedGroups}
        selectedGroupId={selectedGroupId}
        resultCount={sortedRecords.length}
        groupOptions={groupOptions}
        onSelectGroup={handleSelectGroup}
      />

      <div className="relative min-h-0 flex-1 overflow-hidden">
        {sortedRecords.length === 0 ? (
          <div
            className="flex h-full items-center justify-center text-sm"
            style={{ color: "var(--df-text-muted)" }}
          >
            {t("assets.noResults")}
          </div>
        ) : viewMode === "list" ? (
          <AssetTable
            t={t}
            labels={labels}
            records={sortedRecords}
            sortState={sortState}
            onSortChange={handleSortChange}
            onConnectConnection={onConnectConnection}
            onEditConnection={onEditConnection}
          />
        ) : (
          <AssetCardGrid
            t={t}
            labels={labels}
            records={sortedRecords}
            onConnectConnection={onConnectConnection}
            onEditConnection={onEditConnection}
          />
        )}
      </div>
    </div>
  );
}

function collectConnectionTags(connections: SavedConnection[]): string[] {
  const tags = new Set<string>();
  for (const connection of connections) {
    for (const value of connection.tags ?? []) {
      const tag = value.trim();
      if (tag) tags.add(tag);
    }
  }
  return [...tags].sort((left, right) =>
    left.localeCompare(right, undefined, { numeric: true, sensitivity: "variant" }),
  );
}

function matchesSelectedTags(connection: SavedConnection, selectedTags: Set<string>): boolean {
  if (selectedTags.size === 0) return true;
  return connection.tags?.some((value) => selectedTags.has(value.trim())) ?? false;
}

function sortAssetRecords(
  records: AssetRecord[],
  sortState: AssetSortState | null,
  labels: AssetDisplayLabels,
): AssetRecord[] {
  const sorted = [...records];
  sorted.sort((left, right) => {
    if (!sortState) return compareDefaultAssetOrder(left, right);

    if (sortState.key === "connectionTime") {
      const diff = compareConnectionTime(left, right, sortState.direction);
      if (diff !== 0) return diff;
      return compareDefaultAssetOrder(left, right);
    }

    const direction = sortState.direction === "asc" ? 1 : -1;
    const diff = compareBySortKey(left, right, sortState.key, labels);
    if (diff !== 0) return diff * direction;
    return compareDefaultAssetOrder(left, right);
  });
  return sorted;
}

function compareDefaultAssetOrder(left: AssetRecord, right: AssetRecord): number {
  const groupDiff = left.groupSortOrder - right.groupSortOrder;
  if (groupDiff !== 0) return groupDiff;
  const sortDiff = (left.connection.sort_order ?? 0) - (right.connection.sort_order ?? 0);
  if (sortDiff !== 0) return sortDiff;
  return naturalCompare(left.connection.name, right.connection.name);
}

function compareBySortKey(
  left: AssetRecord,
  right: AssetRecord,
  sortKey: AssetSortKey,
  labels: AssetDisplayLabels,
): number {
  switch (sortKey) {
    case "name":
      return naturalCompare(left.connection.name, right.connection.name);
    case "address":
      return compareAssetAddress(
        formatAssetAddress(left.connection, labels),
        formatAssetAddress(right.connection, labels),
      );
    case "connectionTime":
      return compareConnectionTime(left, right, "asc");
    case "cpu":
      return compareNullableNumber(
        getCpuSortValue(left.connection),
        getCpuSortValue(right.connection),
      );
    case "memory":
      return compareNullableNumber(
        left.connection.asset?.memory_bytes ?? null,
        right.connection.asset?.memory_bytes ?? null,
      );
    case "storage":
      return compareNullableNumber(
        getDiskTotalBytes(left.connection.asset?.disks),
        getDiskTotalBytes(right.connection.asset?.disks),
      );
    case "accelerators": {
      const countDiff = compareNullableNumber(
        left.connection.asset?.accelerators?.length ?? null,
        right.connection.asset?.accelerators?.length ?? null,
      );
      if (countDiff !== 0) return countDiff;
      return naturalCompare(
        formatAccelerators(left.connection.asset?.accelerators, labels),
        formatAccelerators(right.connection.asset?.accelerators, labels),
      );
    }
  }
}

function normalizeAssetSortState(
  key: string | null | undefined,
  direction: string | null | undefined,
): AssetSortState | null {
  if (!isAssetSortKey(key)) return null;
  return {
    key,
    direction: direction === "desc" ? "desc" : "asc",
  };
}

function isAssetSortKey(value: string | null | undefined): value is AssetSortKey {
  return (
    value === "name" ||
    value === "address" ||
    value === "connectionTime" ||
    value === "cpu" ||
    value === "memory" ||
    value === "storage" ||
    value === "accelerators"
  );
}

function compareConnectionTime(
  left: AssetRecord,
  right: AssetRecord,
  direction: AssetSortState["direction"],
): number {
  const leftMs = left.connectionTimeMs;
  const rightMs = right.connectionTimeMs;
  const leftValid = leftMs !== null;
  const rightValid = rightMs !== null;
  if (leftValid && rightValid) return (leftMs - rightMs) * (direction === "asc" ? 1 : -1);
  if (leftValid) return -1;
  if (rightValid) return 1;
  return 0;
}

function getCpuSortValue(connection: SavedConnection): number | null {
  const asset = connection.asset;
  if (!asset) return null;
  return asset.cpu_cores ?? asset.cpu_threads ?? null;
}

function compareNullableNumber(left: number | null | undefined, right: number | null | undefined) {
  const leftValid = left !== null && left !== undefined && Number.isFinite(left);
  const rightValid = right !== null && right !== undefined && Number.isFinite(right);
  if (leftValid && rightValid) return left - right;
  if (leftValid) return -1;
  if (rightValid) return 1;
  return 0;
}

function buildGroupOptions(groups: Group[], rootLabel: string): AssetGroupOption[] {
  return groups
    .map((group) => {
      const path = buildGroupPath(groups, group.id)
        .map((segment) => (segment.id === null ? rootLabel : segment.name))
        .join(" / ");
      return {
        group,
        path,
        searchText: `${group.name} ${path}`.toLowerCase(),
      };
    })
    .sort((left, right) => naturalCompare(left.path, right.path));
}
