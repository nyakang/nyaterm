import type { TFunction } from "i18next";
import { ArrowDown, ArrowUp, ChevronsUpDown } from "lucide-react";
import {
  type PointerEvent as ReactPointerEvent,
  useMemo,
  useState,
} from "react";
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
import type { AssetRecord, AssetSortKey, AssetSortState } from "./types";

interface AssetTableProps {
  t: TFunction;
  labels: AssetDisplayLabels;
  records: AssetRecord[];
  sortState: AssetSortState | null;
  onSortChange: (key: AssetSortKey) => void;
  onConnectConnection: (connection: SavedConnection) => Promise<void> | void;
  onEditConnection: (connection: SavedConnection) => void;
}

const ASSET_TABLE_ROW_HEIGHT = 56;
type AssetTableColumnKey = AssetSortKey | "actions";

const ASSET_TABLE_COLUMNS: AssetTableColumnKey[] = [
  "name",
  "address",
  "cpu",
  "memory",
  "storage",
  "accelerators",
  "actions",
];

const DEFAULT_COLUMN_WIDTHS: Record<AssetTableColumnKey, number> = {
  name: 280,
  address: 150,
  cpu: 110,
  memory: 110,
  storage: 120,
  accelerators: 190,
  actions: 88,
};

const MIN_COLUMN_WIDTHS: Record<AssetTableColumnKey, number> = {
  name: 180,
  address: 120,
  cpu: 80,
  memory: 88,
  storage: 96,
  accelerators: 140,
  actions: 88,
};

export default function AssetTable({
  t,
  labels,
  records,
  sortState,
  onSortChange,
  onConnectConnection,
  onEditConnection,
}: AssetTableProps) {
  const [columnWidths, setColumnWidths] = useState<
    Record<AssetTableColumnKey, number>
  >(DEFAULT_COLUMN_WIDTHS);
  const { containerRef, visibleItems, paddingTop, paddingBottom, onScroll } =
    useVirtualList<AssetRecord>(records, {
      itemHeight: ASSET_TABLE_ROW_HEIGHT,
      overscan: 8,
    });
  const tableMinWidth = useMemo(
    () =>
      ASSET_TABLE_COLUMNS.reduce(
        (total, column) => total + columnWidths[column],
        0,
      ),
    [columnWidths],
  );

  const handleColumnResizeStart = (
    columnKey: AssetTableColumnKey,
    event: ReactPointerEvent<HTMLSpanElement>,
  ) => {
    event.preventDefault();
    event.stopPropagation();

    const startX = event.clientX;
    const startWidth = columnWidths[columnKey];
    const minWidth = MIN_COLUMN_WIDTHS[columnKey];

    const handlePointerMove = (moveEvent: PointerEvent) => {
      const nextWidth = Math.max(
        minWidth,
        Math.round(startWidth + moveEvent.clientX - startX),
      );
      setColumnWidths((current) =>
        current[columnKey] === nextWidth
          ? current
          : { ...current, [columnKey]: nextWidth },
      );
    };

    const handlePointerUp = () => {
      document.removeEventListener("pointermove", handlePointerMove);
      document.removeEventListener("pointerup", handlePointerUp);
    };

    document.addEventListener("pointermove", handlePointerMove);
    document.addEventListener("pointerup", handlePointerUp);
  };

  return (
    <div
      ref={containerRef}
      data-asset-table
      className="asset-table-container terminal-scroll h-full min-h-0 overflow-auto"
      onScroll={onScroll}
    >
      <table
        className="w-full table-fixed border-separate border-spacing-0 text-left text-xs"
        style={{ minWidth: tableMinWidth }}
      >
        <colgroup>
          {ASSET_TABLE_COLUMNS.map((column) => (
            <col
              key={column}
              data-asset-column={column}
              style={{ width: columnWidths[column] }}
            />
          ))}
        </colgroup>
        <thead className="sticky top-0 z-[2]">
          <tr
            className="h-8 border-b"
            style={{
              color: "var(--df-text-muted)",
              backgroundColor: "var(--df-bg-terminal)",
            }}
          >
            <SortableHeaderCell
              label={t("assets.name")}
              sortKey="name"
              sortState={sortState}
              onSortChange={onSortChange}
              width={columnWidths.name}
              onResizeStart={(event) => handleColumnResizeStart("name", event)}
            />
            <SortableHeaderCell
              label={t("assets.address")}
              sortKey="address"
              sortState={sortState}
              onSortChange={onSortChange}
              width={columnWidths.address}
              onResizeStart={(event) =>
                handleColumnResizeStart("address", event)
              }
            />
            <SortableHeaderCell
              label={t("assets.cpu")}
              sortKey="cpu"
              sortState={sortState}
              onSortChange={onSortChange}
              className="asset-col-cpu"
              width={columnWidths.cpu}
              onResizeStart={(event) => handleColumnResizeStart("cpu", event)}
            />
            <SortableHeaderCell
              label={t("assets.memory")}
              sortKey="memory"
              sortState={sortState}
              onSortChange={onSortChange}
              className="asset-col-memory"
              width={columnWidths.memory}
              onResizeStart={(event) =>
                handleColumnResizeStart("memory", event)
              }
            />
            <SortableHeaderCell
              label={t("assets.storage")}
              sortKey="storage"
              sortState={sortState}
              onSortChange={onSortChange}
              className="asset-col-storage"
              width={columnWidths.storage}
              onResizeStart={(event) =>
                handleColumnResizeStart("storage", event)
              }
            />
            <SortableHeaderCell
              label={t("assets.accelerators")}
              sortKey="accelerators"
              sortState={sortState}
              onSortChange={onSortChange}
              className="asset-col-accelerators"
              width={columnWidths.accelerators}
              onResizeStart={(event) =>
                handleColumnResizeStart("accelerators", event)
              }
            />
            <HeaderCell
              className="asset-col-actions sticky right-0 z-[3] text-right"
              width={columnWidths.actions}
            >
              {t("assets.actions")}
            </HeaderCell>
          </tr>
        </thead>
        <tbody>
          {paddingTop > 0 ? <SpacerRow height={paddingTop} /> : null}
          {visibleItems.map(({ item: { connection, groupPath } }) => {
            const asset = connection.asset;
            return (
              <tr
                key={connection.id}
                className="group h-14 outline-none transition-colors hover:bg-[var(--df-bg-hover)]"
                style={{
                  color: "var(--df-text)",
                }}
                onDoubleClick={() => void onConnectConnection(connection)}
              >
                <BodyCell>
                  <div className="flex min-w-0 items-center gap-2.5">
                    <AssetConnectionIcon connection={connection} />
                    <div className="min-w-0">
                      <div className="truncate font-medium" data-asset-name>
                        {connection.name}
                      </div>
                      <div
                        className="asset-row-secondary truncate"
                        style={{ color: "var(--df-text-dimmed)" }}
                      >
                        {groupPath}
                      </div>
                    </div>
                  </div>
                </BodyCell>
                <BodyCell>{formatAssetAddress(connection, labels)}</BodyCell>
                <BodyCell className="asset-col-cpu">
                  {formatCpuSummary(asset, labels)}
                </BodyCell>
                <BodyCell className="asset-col-memory">
                  {formatBytes(asset?.memory_bytes, labels)}
                </BodyCell>
                <BodyCell className="asset-col-storage">
                  {formatDiskSummary(asset?.disks, labels)}
                </BodyCell>
                <BodyCell className="asset-col-accelerators">
                  {formatAccelerators(asset?.accelerators, labels, {
                    maxItems: 2,
                  })}
                </BodyCell>
                <BodyCell className="asset-col-actions sticky right-0 text-right">
                  <div className="flex justify-end gap-1">
                    <ActionButton
                      label={t("savedConnections.connect")}
                      onClick={() => void onConnectConnection(connection)}
                    >
                      <MdLink className="text-[0.95rem]" />
                    </ActionButton>
                    <ActionButton
                      label={t("savedConnections.edit")}
                      onClick={() => onEditConnection(connection)}
                    >
                      <MdEdit className="text-[0.95rem]" />
                    </ActionButton>
                  </div>
                </BodyCell>
              </tr>
            );
          })}
          {paddingBottom > 0 ? <SpacerRow height={paddingBottom} /> : null}
        </tbody>
      </table>
    </div>
  );
}

function SpacerRow({ height }: { height: number }) {
  return (
    <tr>
      <td colSpan={7} style={{ height, padding: 0, border: 0 }} />
    </tr>
  );
}

function HeaderCell({
  children,
  className = "",
  width,
  onResizeStart,
}: {
  children: React.ReactNode;
  className?: string;
  width?: number;
  onResizeStart?: (event: ReactPointerEvent<HTMLSpanElement>) => void;
}) {
  return (
    <th
      className={`relative border-b px-3 py-2 font-medium ${className}`}
      style={{
        borderColor: "var(--df-border)",
        backgroundColor: "var(--df-bg-terminal)",
        width,
      }}
    >
      {children}
      {onResizeStart ? (
        <span
          aria-hidden="true"
          data-asset-column-resizer
          className="absolute inset-y-0 right-0 z-10 w-2 cursor-col-resize touch-none select-none transition-colors hover:bg-[var(--df-bg-hover)]"
          onPointerDown={onResizeStart}
        />
      ) : null}
    </th>
  );
}

function SortableHeaderCell({
  label,
  sortKey,
  sortState,
  onSortChange,
  className = "",
  width,
  onResizeStart,
}: {
  label: string;
  sortKey: AssetSortKey;
  sortState: AssetSortState | null;
  onSortChange: (key: AssetSortKey) => void;
  className?: string;
  width: number;
  onResizeStart: (event: ReactPointerEvent<HTMLSpanElement>) => void;
}) {
  const active = sortState?.key === sortKey;
  const SortIcon = !active
    ? ChevronsUpDown
    : sortState.direction === "asc"
      ? ArrowUp
      : ArrowDown;

  return (
    <HeaderCell
      className={className}
      width={width}
      onResizeStart={onResizeStart}
    >
      <button
        type="button"
        className="flex min-w-0 cursor-pointer items-center gap-1.5 rounded px-1 py-0.5 text-left transition-colors hover:bg-[var(--df-bg-hover)]"
        style={{ color: active ? "var(--df-primary)" : "inherit" }}
        onClick={() => onSortChange(sortKey)}
      >
        <span className="truncate">{label}</span>
        <SortIcon className="size-3 shrink-0" />
      </button>
    </HeaderCell>
  );
}

function BodyCell({
  children,
  className = "",
}: {
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <td
      className={`border-b px-3 py-2 align-middle ${className}`}
      style={{
        borderColor: "color-mix(in srgb, var(--df-border) 50%, transparent)",
        backgroundColor: className.includes("asset-col-actions")
          ? "var(--df-bg-terminal)"
          : undefined,
      }}
    >
      <div className="truncate">{children}</div>
    </td>
  );
}

function ActionButton({
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
      className="inline-flex size-7 cursor-pointer items-center justify-center rounded border transition-colors hover:bg-[var(--df-bg-hover)] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[var(--df-primary)]"
      style={{
        borderColor: "color-mix(in srgb, var(--df-border) 78%, transparent)",
        color: "var(--df-text-muted)",
        backgroundColor:
          "color-mix(in srgb, var(--df-bg-panel) 50%, transparent)",
      }}
      onClick={onClick}
    >
      {children}
    </button>
  );
}
