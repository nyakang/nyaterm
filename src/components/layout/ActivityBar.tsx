import { type DragEvent, type ReactNode, useCallback, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { HiMiniArrowTurnLeftUp, HiMiniArrowTurnRightUp, HiMiniArrowTurnLeftDown, HiMiniArrowTurnRightDown } from "react-icons/hi2";
import {
  ContextMenu,
  ContextMenuCheckboxItem,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuSub,
  ContextMenuSubContent,
  ContextMenuSubTrigger,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import type { ActivityBarZone } from "@/types/global";

export interface ActivityBarItem {
  id: string;
  icon: ReactNode;
  tooltip: string;
}

const DRAG_MIME = "application/x-dragonfly-activity";

const ZONE_LABELS: { zone: ActivityBarZone; key: string; icon: ReactNode }[] = [
  { zone: "left_top", key: "activityBar.leftTop", icon: <HiMiniArrowTurnLeftUp /> },
  { zone: "left_bottom", key: "activityBar.leftBottom", icon: <HiMiniArrowTurnLeftDown /> },
  { zone: "right_top", key: "activityBar.rightTop", icon: <HiMiniArrowTurnRightUp /> },
  { zone: "right_bottom", key: "activityBar.rightBottom", icon: <HiMiniArrowTurnRightDown /> },
];

interface ActivityBarProps {
  items: ActivityBarItem[];
  bottomItems?: ActivityBarItem[];
  activeId: string | null;
  activeBottomIds?: Set<string>;
  onSelect: (id: string) => void;
  onReorder: (zone: "top" | "bottom", orderedIds: string[]) => void;
  onMoveItem: (itemId: string, targetZone: ActivityBarZone) => void;
  onToggleLabel: () => void;
  showLabels: boolean;
  side: "left" | "right";
  zone: { top: ActivityBarZone; bottom: ActivityBarZone };
}

export default function ActivityBar({
  items,
  bottomItems,
  activeId,
  activeBottomIds,
  onSelect,
  onReorder,
  onMoveItem,
  onToggleLabel,
  showLabels,
  side,
  zone,
}: ActivityBarProps) {
  const indicatorSide = side === "left" ? "left-0" : "right-0";
  const tooltipSide = side === "left" ? "right" : "left";

  return (
    <TooltipProvider delayDuration={400}>
      <div
        className="flex flex-col shrink-0 w-10 select-none"
        style={{
          backgroundColor: "var(--df-bg)",
          borderColor: "var(--df-border)",
          borderRightWidth: side === "left" ? 1 : 0,
          borderLeftWidth: side === "right" ? 1 : 0,
        }}
      >
        <DropZone
          items={items}
          zoneKey="top"
          zoneName={zone.top}
          activeId={activeId}
          onSelect={onSelect}
          onReorder={onReorder}
          onMoveItem={onMoveItem}
          onToggleLabel={onToggleLabel}
          showLabels={showLabels}
          indicatorSide={indicatorSide}
          tooltipSide={tooltipSide}
          className="flex flex-col items-center gap-0.5 pt-1"
        />
        {bottomItems && bottomItems.length > 0 && (
          <DropZone
            items={bottomItems}
            zoneKey="bottom"
            zoneName={zone.bottom}
            activeId={activeId}
            activeBottomIds={activeBottomIds}
            onSelect={onSelect}
            onReorder={onReorder}
            onMoveItem={onMoveItem}
            onToggleLabel={onToggleLabel}
            showLabels={showLabels}
            indicatorSide={indicatorSide}
            tooltipSide={tooltipSide}
            className="mt-auto flex flex-col items-center gap-0.5 pb-1"
          />
        )}
      </div>
    </TooltipProvider>
  );
}

interface DropZoneProps {
  items: ActivityBarItem[];
  zoneKey: "top" | "bottom";
  zoneName: ActivityBarZone;
  activeId: string | null;
  activeBottomIds?: Set<string>;
  onSelect: (id: string) => void;
  onReorder: (zone: "top" | "bottom", orderedIds: string[]) => void;
  onMoveItem: (itemId: string, targetZone: ActivityBarZone) => void;
  onToggleLabel: () => void;
  showLabels: boolean;
  indicatorSide: string;
  tooltipSide: "left" | "right";
  className: string;
}

function DropZone({
  items,
  zoneKey,
  zoneName,
  activeId,
  activeBottomIds,
  onSelect,
  onReorder,
  onMoveItem,
  onToggleLabel,
  showLabels,
  indicatorSide,
  tooltipSide,
  className,
}: DropZoneProps) {
  const [dropIndex, setDropIndex] = useState<number | null>(null);
  const dragItemIdRef = useRef<string | null>(null);

  const handleDragStart = useCallback((e: DragEvent, itemId: string) => {
    dragItemIdRef.current = itemId;
    e.dataTransfer.setData(DRAG_MIME, JSON.stringify({ id: itemId, zone: zoneName }));
    e.dataTransfer.effectAllowed = "move";
  }, [zoneName]);

  const handleDragOver = useCallback((e: DragEvent, index: number) => {
    if (!e.dataTransfer.types.includes(DRAG_MIME)) return;
    e.preventDefault();
    e.dataTransfer.dropEffect = "move";
    setDropIndex(index);
  }, []);

  const handleDragLeave = useCallback(() => {
    setDropIndex(null);
  }, []);

  const handleDrop = useCallback(
    (e: DragEvent, targetIndex: number) => {
      e.preventDefault();
      setDropIndex(null);
      const raw = e.dataTransfer.getData(DRAG_MIME);
      if (!raw) return;
      try {
        const { id, zone: srcZone } = JSON.parse(raw) as { id: string; zone: string };
        if (srcZone !== zoneName) {
          onMoveItem(id, zoneName);
          return;
        }
        const currentIds = items.map((i) => i.id);
        const fromIdx = currentIds.indexOf(id);
        if (fromIdx === -1) return;
        const reordered = [...currentIds];
        reordered.splice(fromIdx, 1);
        const insertAt = targetIndex > fromIdx ? targetIndex - 1 : targetIndex;
        reordered.splice(insertAt, 0, id);
        onReorder(zoneKey, reordered);
      } catch { /* ignore malformed data */ }
    },
    [items, zoneName, zoneKey, onReorder, onMoveItem],
  );

  const handleDragEnd = useCallback(() => {
    setDropIndex(null);
    dragItemIdRef.current = null;
  }, []);

  return (
    <div className={className} onDragLeave={handleDragLeave}>
      {items.map((item, idx) => (
        <ActivityBarButton
          key={item.id}
          item={item}
          active={activeId === item.id || !!activeBottomIds?.has(item.id)}
          showLabel={showLabels}
          onSelect={onSelect}
          indicatorSide={indicatorSide}
          tooltipSide={tooltipSide}
          currentZone={zoneName}
          onMoveItem={onMoveItem}
          onToggleLabel={onToggleLabel}
          onDragStart={(e) => handleDragStart(e, item.id)}
          onDragOver={(e) => handleDragOver(e, idx)}
          onDrop={(e) => handleDrop(e, idx)}
          onDragEnd={handleDragEnd}
          showDropIndicator={dropIndex === idx}
        />
      ))}
      {/* Drop target after last item */}
      <div
        className="w-full h-1"
        onDragOver={(e) => handleDragOver(e, items.length)}
        onDrop={(e) => handleDrop(e, items.length)}
      />
    </div>
  );
}

function ActivityBarButton({
  item,
  active,
  showLabel,
  onSelect,
  indicatorSide,
  tooltipSide,
  currentZone,
  onMoveItem,
  onToggleLabel,
  onDragStart,
  onDragOver,
  onDrop,
  onDragEnd,
  showDropIndicator,
}: {
  item: ActivityBarItem;
  active: boolean;
  showLabel: boolean;
  onSelect: (id: string) => void;
  indicatorSide: string;
  tooltipSide: "left" | "right";
  currentZone: ActivityBarZone;
  onMoveItem: (itemId: string, targetZone: ActivityBarZone) => void;
  onToggleLabel: () => void;
  onDragStart: (e: DragEvent) => void;
  onDragOver: (e: DragEvent) => void;
  onDrop: (e: DragEvent) => void;
  onDragEnd: () => void;
  showDropIndicator: boolean;
}) {
  const { t } = useTranslation();

  return (
    <ContextMenu>
      <Tooltip>
        <ContextMenuTrigger asChild>
          <TooltipTrigger asChild>
            <button
              draggable
              onDragStart={onDragStart}
              onDragOver={onDragOver}
              onDrop={onDrop}
              onDragEnd={onDragEnd}
              className={`relative flex flex-col items-center justify-center w-full transition-colors ${showLabel ? "min-h-12 gap-0.5 py-1" : "h-9"}`}
              style={{
                color: active ? "var(--df-primary)" : "var(--df-text-muted)",
                cursor: "default",
              }}
              onClick={() => onSelect(item.id)}
            >
              {showDropIndicator && (
                <span
                  className="absolute left-1 right-1 -top-[1px] h-[2px] rounded-full"
                  style={{ backgroundColor: "var(--df-primary)" }}
                />
              )}
              {active && (
                <span
                  className={`absolute ${indicatorSide} top-1 bottom-1 w-[2px] rounded-full`}
                  style={{ backgroundColor: "var(--df-primary)" }}
                />
              )}
              <span className="text-[1.125rem] shrink-0">{item.icon}</span>
              {showLabel && (
                <span
                  className="text-[0.5rem] leading-tight w-full text-center break-words hyphens-auto"
                  lang="zh"
                  style={{ wordBreak: "break-word", overflowWrap: "break-word" }}
                >
                  {item.tooltip}
                </span>
              )}
            </button>
          </TooltipTrigger>
        </ContextMenuTrigger>
        {!showLabel && (
          <TooltipContent side={tooltipSide} sideOffset={4}>
            <span className="text-xs">{item.tooltip}</span>
          </TooltipContent>
        )}
      </Tooltip>

      <ContextMenuContent>
        <ContextMenuSub>
          <ContextMenuSubTrigger>{t("activityBar.moveTo")}</ContextMenuSubTrigger>
          <ContextMenuSubContent>
            {ZONE_LABELS.map(({ zone, key, icon }) => (
              <ContextMenuItem
                key={zone}
                disabled={zone === currentZone}
                onClick={() => onMoveItem(item.id, zone)}
              >
                {icon}
                {t(key)}
              </ContextMenuItem>
            ))}
          </ContextMenuSubContent>
        </ContextMenuSub>
        <ContextMenuSeparator />
        <ContextMenuCheckboxItem checked={showLabel} onCheckedChange={onToggleLabel}>
          {t("activityBar.showLabel")}
        </ContextMenuCheckboxItem>
      </ContextMenuContent>
    </ContextMenu>
  );
}
