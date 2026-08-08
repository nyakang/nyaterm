import type { TFunction } from "i18next";
import { useEffect, useMemo, useState } from "react";
import { MdEdit, MdLink } from "react-icons/md";
import { useVirtualList } from "@/hooks/useVirtualList";
import type { SavedConnection } from "@/types/global";
import AssetConnectionIcon from "./AssetConnectionIcon";
import type { AssetDisplayLabels } from "./assetFormatters";
import {
  formatAccelerators,
  formatAssetAddress,
  formatBytes,
  formatCpuSummary,
  formatDiskSummary,
} from "./assetFormatters";
import type { AssetRecord } from "./types";

interface AssetCardGridProps {
  t: TFunction;
  labels: AssetDisplayLabels;
  records: AssetRecord[];
  onConnectConnection: (connection: SavedConnection) => Promise<void> | void;
  onEditConnection: (connection: SavedConnection) => void;
}

const ASSET_CARD_ROW_HEIGHT = 190;
const ASSET_CARD_GAP = 8;

export default function AssetCardGrid({
  t,
  labels,
  records,
  onConnectConnection,
  onEditConnection,
}: AssetCardGridProps) {
  const [columnCount, setColumnCount] = useState(1);
  const rows = useMemo(
    () => chunkRecords(records, columnCount),
    [columnCount, records],
  );
  const { containerRef, visibleItems, paddingTop, paddingBottom, onScroll } =
    useVirtualList(rows, {
      itemHeight: ASSET_CARD_ROW_HEIGHT + ASSET_CARD_GAP,
      overscan: 5,
    });

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;

    const syncColumnCount = () => {
      const width = container.clientWidth;
      setColumnCount(width >= 1024 ? 3 : width >= 672 ? 2 : 1);
    };
    syncColumnCount();
    const observer = new ResizeObserver(syncColumnCount);
    observer.observe(container);
    return () => observer.disconnect();
  }, [containerRef]);

  return (
    <div
      ref={containerRef}
      data-asset-card-grid
      className="terminal-scroll h-full min-h-0 overflow-y-auto p-3"
      onScroll={onScroll}
    >
      <div style={{ paddingTop, paddingBottom }}>
        {visibleItems.map(({ item: row, index }) => (
          <AssetCardRow
            key={row.map((record) => record.connection.id).join(":")}
            t={t}
            labels={labels}
            row={row}
            rowIndex={index}
            columnCount={columnCount}
            onConnectConnection={onConnectConnection}
            onEditConnection={onEditConnection}
          />
        ))}
      </div>
    </div>
  );
}

function AssetCardRow({
  t,
  labels,
  row,
  rowIndex,
  columnCount,
  onConnectConnection,
  onEditConnection,
}: {
  t: TFunction;
  labels: AssetDisplayLabels;
  row: AssetRecord[];
  rowIndex: number;
  columnCount: number;
  onConnectConnection: (connection: SavedConnection) => Promise<void> | void;
  onEditConnection: (connection: SavedConnection) => void;
}) {
  const rowKey = row.map((record) => record.connection.id).join(":");
  const spacerKeys = createSpacerKeys(
    rowKey || `row-${rowIndex}`,
    columnCount - row.length,
  );

  return (
    <div
      className="grid gap-2 pb-2"
      style={{
        gridTemplateColumns: `repeat(${columnCount}, minmax(0, 1fr))`,
        height: ASSET_CARD_ROW_HEIGHT + ASSET_CARD_GAP,
      }}
    >
      {row.map((record) => (
        <AssetCard
          key={record.connection.id}
          t={t}
          labels={labels}
          record={record}
          onConnectConnection={onConnectConnection}
          onEditConnection={onEditConnection}
        />
      ))}
      {spacerKeys.map((spacerKey) => (
        <div key={spacerKey} aria-hidden="true" />
      ))}
    </div>
  );
}

function AssetCard({
  t,
  labels,
  record,
  onConnectConnection,
  onEditConnection,
}: {
  t: TFunction;
  labels: AssetDisplayLabels;
  record: AssetRecord;
  onConnectConnection: (connection: SavedConnection) => Promise<void> | void;
  onEditConnection: (connection: SavedConnection) => void;
}) {
  const { connection, groupPath } = record;
  const asset = connection.asset;

  return (
    <article
      className="flex h-[182px] min-w-0 flex-col rounded-md border text-left transition-colors hover:bg-[var(--df-bg-hover)]"
      style={{
        borderColor: "color-mix(in srgb, var(--df-border) 78%, transparent)",
        color: "var(--df-text)",
        backgroundColor:
          "color-mix(in srgb, var(--df-bg-panel) 36%, transparent)",
      }}
      onDoubleClick={() => void onConnectConnection(connection)}
    >
      <div className="flex min-w-0 items-start gap-2.5 px-3 py-2.5">
        <AssetConnectionIcon connection={connection} />
        <div className="min-w-0 flex-1">
          <div className="truncate text-sm font-semibold">
            {connection.name}
          </div>
          <div
            className="mt-0.5 truncate font-mono text-xs"
            style={{ color: "var(--df-text-muted)" }}
          >
            {formatAssetAddress(connection, labels)}
          </div>
          <div
            className="mt-0.5 truncate text-[0.6875rem]"
            style={{ color: "var(--df-text-dimmed)" }}
          >
            {groupPath}
          </div>
        </div>
      </div>

      <div
        className="grid min-h-0 flex-1 grid-cols-2 gap-x-3 gap-y-2 px-3 pb-2 text-xs"
        style={{ color: "var(--df-text-muted)" }}
      >
        <Metric
          label={t("assets.cpu")}
          value={formatCpuSummary(asset, labels)}
        />
        <Metric
          label={t("assets.memory")}
          value={formatBytes(asset?.memory_bytes, labels)}
        />
        <Metric
          label={t("assets.storage")}
          value={formatDiskSummary(asset?.disks, labels)}
        />
        <Metric
          label={t("assets.accelerators")}
          value={formatAccelerators(asset?.accelerators, labels, {
            maxItems: 1,
          })}
        />
      </div>

      <div
        className="flex min-h-9 shrink-0 items-center justify-end gap-1.5 border-t px-2.5"
        style={{
          borderColor: "color-mix(in srgb, var(--df-border) 70%, transparent)",
        }}
      >
        <CardActionButton
          label={t("savedConnections.connect")}
          onClick={() => void onConnectConnection(connection)}
        >
          <MdLink className="text-[0.95rem]" />
          <span>{t("savedConnections.connect")}</span>
        </CardActionButton>
        <CardActionButton
          label={t("savedConnections.edit")}
          onClick={() => onEditConnection(connection)}
        >
          <MdEdit className="text-[0.95rem]" />
          <span>{t("savedConnections.edit")}</span>
        </CardActionButton>
      </div>
    </article>
  );
}

function Metric({ label, value }: { label: string; value: string }) {
  return (
    <div className="min-w-0">
      <div
        className="truncate text-[0.625rem]"
        style={{ color: "var(--df-text-dimmed)" }}
      >
        {label}
      </div>
      <div className="truncate">{value}</div>
    </div>
  );
}

function CardActionButton({
  label,
  onClick,
  children,
}: {
  label: string;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      className="inline-flex h-7 cursor-pointer items-center gap-1.5 rounded border px-2 text-xs transition-colors hover:bg-[var(--df-bg-hover)] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[var(--df-primary)]"
      style={{
        borderColor: "color-mix(in srgb, var(--df-border) 78%, transparent)",
        color: "var(--df-text-muted)",
        backgroundColor:
          "color-mix(in srgb, var(--df-bg-terminal) 42%, transparent)",
      }}
      onClick={onClick}
    >
      {children}
    </button>
  );
}

function chunkRecords(
  records: AssetRecord[],
  columnCount: number,
): AssetRecord[][] {
  const rows: AssetRecord[][] = [];
  const safeColumnCount = Math.max(1, columnCount);
  for (let index = 0; index < records.length; index += safeColumnCount) {
    rows.push(records.slice(index, index + safeColumnCount));
  }
  return rows;
}

function createSpacerKeys(rowKey: string, count: number): string[] {
  const keys: string[] = [];
  for (let position = 0; position < count; position += 1) {
    keys.push(`${rowKey}:spacer:${position}`);
  }
  return keys;
}
