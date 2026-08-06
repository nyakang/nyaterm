import type { TFunction } from "i18next";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { MdContentCopy, MdDelete, MdDriveFileRenameOutline, MdEdit, MdLink } from "react-icons/md";
import { toast } from "sonner";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import type { SavedConnection } from "@/types/global";
import { resolveConnectionIcon } from "../../icons";
import { useSavedConnectionsContext } from "./context";
import { MoveToGroupContextMenu } from "./MoveToGroupMenu";

const TOOLTIP_OPEN_DELAY_MS = 350;
const TOOLTIP_CLOSE_DELAY_MS = 120;

interface ConnectionItemProps {
  conn: SavedConnection;
  indented: boolean;
  depth?: number;
}

interface ConnectionDetailsTooltipProps {
  conn: SavedConnection;
  savedConnections: SavedConnection[];
  t: TFunction;
  onPointerEnter: () => void;
  onPointerLeave: () => void;
}

interface ConnectionDetailRow {
  label: string;
  value: string;
  copyValue?: string;
  multiline?: boolean;
}

function formatRequiredDetailValue(
  value: string | number | null | undefined,
  t: TFunction,
): string {
  if (value === null || value === undefined) return t("savedConnections.notSet");
  const text = String(value).trim();
  return text || t("savedConnections.notSet");
}

function formatOptionalDetailValue(value: string | number | null | undefined): string | null {
  if (value === null || value === undefined) return null;
  const text = String(value).trim();
  return text || null;
}

function getCopyDetailValue(value: string | number | null | undefined): string | undefined {
  if (value === null || value === undefined) return undefined;
  const text = String(value).trim();
  return text || undefined;
}

function formatJumpHostChain(
  conn: SavedConnection,
  savedConnections: SavedConnection[],
  t: TFunction,
): string | null {
  if (conn.type !== "ssh" || !conn.network?.proxy_jump_id) {
    return null;
  }

  const connectionsById = new Map(
    savedConnections.map((connection) => [connection.id, connection]),
  );
  const seen = new Set<string>([conn.id]);
  const labels: string[] = [];
  let currentJumpId = conn.network.proxy_jump_id;

  while (currentJumpId) {
    if (seen.has(currentJumpId)) {
      labels.push(t("savedConnections.jumpHostCycle"));
      break;
    }

    seen.add(currentJumpId);
    const jump = connectionsById.get(currentJumpId);
    if (!jump) {
      labels.push(t("savedConnections.jumpHostMissing"));
      break;
    }

    labels.push(jump.name);
    currentJumpId = jump.network?.proxy_jump_id ?? "";
  }

  return labels.length > 0 ? labels.join(" -> ") : null;
}

function getConnectionDetailRows(
  conn: SavedConnection,
  savedConnections: SavedConnection[],
  t: TFunction,
): ConnectionDetailRow[] {
  const description =
    formatOptionalDetailValue(conn.description) ?? t("savedConnections.noDescription");

  switch (conn.type) {
    case "local_terminal": {
      const rows: ConnectionDetailRow[] = [
        {
          label: t("savedConnections.terminalPath"),
          value: formatRequiredDetailValue(conn.shell_path, t),
        },
      ];
      const shellArgs = formatOptionalDetailValue(conn.shell_args);
      if (shellArgs) {
        rows.push({ label: t("savedConnections.shellArgs"), value: shellArgs });
      }
      rows.push(
        {
          label: t("savedConnections.workingDir"),
          value: formatRequiredDetailValue(conn.working_dir, t),
        },
        {
          label: t("savedConnections.description"),
          value: description,
          multiline: true,
        },
      );
      return rows;
    }
    case "telnet":
      return [
        {
          label: t("savedConnections.host"),
          value: formatRequiredDetailValue(conn.host, t),
          copyValue: getCopyDetailValue(conn.host),
        },
        {
          label: t("savedConnections.port"),
          value: formatRequiredDetailValue(conn.port, t),
          copyValue: getCopyDetailValue(conn.port),
        },
        {
          label: t("savedConnections.user"),
          value: formatRequiredDetailValue(conn.username, t),
          copyValue: getCopyDetailValue(conn.username),
        },
        {
          label: t("savedConnections.description"),
          value: description,
          multiline: true,
        },
      ];
    case "serial": {
      const rows: ConnectionDetailRow[] = [
        {
          label: t("savedConnections.serialPort"),
          value: formatRequiredDetailValue(conn.port_name, t),
        },
        {
          label: t("savedConnections.baudRate"),
          value: formatRequiredDetailValue(conn.baud_rate, t),
        },
        {
          label: t("savedConnections.dataBits"),
          value: formatRequiredDetailValue(conn.data_bits, t),
        },
      ];
      const parity = formatOptionalDetailValue(conn.parity);
      const stopBits = formatOptionalDetailValue(conn.stop_bits);
      if (parity) rows.push({ label: t("savedConnections.parity"), value: parity });
      if (stopBits) rows.push({ label: t("savedConnections.stopBits"), value: stopBits });
      rows.push({
        label: t("savedConnections.description"),
        value: description,
        multiline: true,
      });
      return rows;
    }
    default: {
      const rows: ConnectionDetailRow[] = [
        {
          label: t("savedConnections.host"),
          value: formatRequiredDetailValue(conn.host, t),
          copyValue: getCopyDetailValue(conn.host),
        },
        {
          label: t("savedConnections.port"),
          value: formatRequiredDetailValue(conn.port, t),
          copyValue: getCopyDetailValue(conn.port),
        },
        {
          label: t("savedConnections.user"),
          value: formatRequiredDetailValue(conn.username, t),
          copyValue: getCopyDetailValue(conn.username),
        },
      ];
      const jumpHostChain = formatJumpHostChain(conn, savedConnections, t);
      if (jumpHostChain) {
        rows.push({
          label: t("savedConnections.jumpHostChain"),
          value: jumpHostChain,
          multiline: true,
        });
      }
      rows.push({
        label: t("savedConnections.description"),
        value: description,
        multiline: true,
      });
      return rows;
    }
  }
}

function ConnectionDetailsTooltip({
  conn,
  savedConnections,
  t,
  onPointerEnter,
  onPointerLeave,
}: ConnectionDetailsTooltipProps) {
  const rows = useMemo(
    () => getConnectionDetailRows(conn, savedConnections, t),
    [conn, savedConnections, t],
  );

  const handleCopyDetailValue = useCallback(
    async (value: string) => {
      try {
        await navigator.clipboard.writeText(value);
        toast.success(t("common.copied"));
      } catch {
        toast.error(t("savedConnections.copyFailed"));
      }
    },
    [t],
  );

  return (
    <TooltipContent
      side="right"
      align="center"
      sideOffset={6}
      collisionPadding={12}
      className="w-[220px] max-w-[min(220px,calc(100vw-2rem))] px-2 py-1.5"
      onPointerEnter={onPointerEnter}
      onPointerLeave={onPointerLeave}
    >
      <div className="grid grid-cols-[4rem_minmax(0,1fr)_1.25rem] gap-x-2 gap-y-1.5">
        {rows.map((row) => {
          const copyValue = row.copyValue;

          return (
            <div className="contents" key={row.label}>
              <span className="text-[0.6875rem] leading-4 text-[var(--df-text-dimmed)]">
                {row.label}
              </span>
              <span
                className={`min-w-0 text-[0.6875rem] leading-4 text-[var(--df-text)] ${
                  row.multiline
                    ? "overflow-hidden [display:-webkit-box] [-webkit-box-orient:vertical] [-webkit-line-clamp:2]"
                    : "truncate"
                }`}
              >
                {row.value}
              </span>
              <span className="flex h-4 w-5 items-center justify-center">
                {copyValue && (
                  <button
                    type="button"
                    className="flex h-5 w-5 items-center justify-center rounded text-[var(--df-text-dimmed)] transition-colors hover:bg-accent hover:text-[var(--df-primary)]"
                    aria-label={t("savedConnections.copy")}
                    onPointerDown={(event) => {
                      event.stopPropagation();
                    }}
                    onClick={(event) => {
                      event.stopPropagation();
                      void handleCopyDetailValue(copyValue);
                    }}
                  >
                    <MdContentCopy className="text-[12px]" />
                  </button>
                )}
              </span>
            </div>
          );
        })}
      </div>
    </TooltipContent>
  );
}

export default function ConnectionItem({ conn, indented, depth = 0 }: ConnectionItemProps) {
  const {
    isDragEnabled,
    isPointerDragEnabled,
    dragTarget,
    selectedConnectionIds,
    keyboardActiveConnectionId,
    savedConnections,
    savedGroups,
    handleConnect,
    handleConnectOnly,
    handleConnectSelected,
    handleCopyConnection,
    requestMoveConnectionToGroup,
    handleConnectionSelectionStart,
    handleConnectionContextMenu,
    registerConnectionElement,
    onEditConnection,
    requestDeleteConnection,
    setRenamingConn,
    setRenameValue,
    handleDragStart,
    handleDragEnd,
    handleDragEnterItem,
    handleDragOverItem,
    handleDragLeaveItem,
    handleDropItem,
    handlePointerDragStart,
    handlePointerDragMove,
    handlePointerDragEnd,
    handlePointerDragCancel,
    t,
  } = useSavedConnectionsContext();

  const isTarget = dragTarget?.id === conn.id && dragTarget.type === "connection";
  const showBefore = isTarget && dragTarget.position === "before";
  const showAfter = isTarget && dragTarget.position === "after";
  const iconDef = resolveConnectionIcon(conn.icon);
  const ConnIcon = iconDef.icon;
  const isSelected = selectedConnectionIds.has(conn.id);
  const isKeyboardActive = keyboardActiveConnectionId === conn.id;
  const connectLabel =
    isSelected && selectedConnectionIds.size > 1
      ? t("savedConnections.connectSelected")
      : t("savedConnections.connect");
  const directConnectLabel = t("savedConnections.connect");
  const iconStyle = { color: isSelected || isKeyboardActive ? "var(--df-primary)" : iconDef.color };
  const indentLeft = indented ? `${8 + depth * 16 + 16}px` : "0.5rem";
  const [detailsOpen, setDetailsOpen] = useState(false);
  const detailsOpenTimerRef = useRef<number | null>(null);
  const detailsCloseTimerRef = useRef<number | null>(null);
  const suppressDetailsUntilLeaveRef = useRef(false);
  const registerSelf = useCallback(
    (element: HTMLDivElement | null) => {
      registerConnectionElement(conn.id, element);
    },
    [conn.id, registerConnectionElement],
  );

  const clearDetailsOpenTimer = useCallback(() => {
    if (detailsOpenTimerRef.current) {
      window.clearTimeout(detailsOpenTimerRef.current);
      detailsOpenTimerRef.current = null;
    }
  }, []);

  const clearDetailsCloseTimer = useCallback(() => {
    if (detailsCloseTimerRef.current) {
      window.clearTimeout(detailsCloseTimerRef.current);
      detailsCloseTimerRef.current = null;
    }
  }, []);

  const closeDetails = useCallback(
    (suppressUntilLeave = false) => {
      clearDetailsOpenTimer();
      clearDetailsCloseTimer();
      if (suppressUntilLeave) suppressDetailsUntilLeaveRef.current = true;
      setDetailsOpen(false);
    },
    [clearDetailsCloseTimer, clearDetailsOpenTimer],
  );

  const scheduleDetailsOpen = useCallback(() => {
    if (suppressDetailsUntilLeaveRef.current) return;
    clearDetailsCloseTimer();
    clearDetailsOpenTimer();
    detailsOpenTimerRef.current = window.setTimeout(() => {
      if (!suppressDetailsUntilLeaveRef.current) {
        setDetailsOpen(true);
      }
      detailsOpenTimerRef.current = null;
    }, TOOLTIP_OPEN_DELAY_MS);
  }, [clearDetailsCloseTimer, clearDetailsOpenTimer]);

  const keepDetailsOpen = useCallback(() => {
    if (suppressDetailsUntilLeaveRef.current) return;
    clearDetailsCloseTimer();
  }, [clearDetailsCloseTimer]);

  const scheduleDetailsClose = useCallback(() => {
    clearDetailsOpenTimer();
    clearDetailsCloseTimer();
    detailsCloseTimerRef.current = window.setTimeout(() => {
      setDetailsOpen(false);
      detailsCloseTimerRef.current = null;
    }, TOOLTIP_CLOSE_DELAY_MS);
  }, [clearDetailsCloseTimer, clearDetailsOpenTimer]);

  const handlePointerLeave = useCallback(() => {
    suppressDetailsUntilLeaveRef.current = false;
    scheduleDetailsClose();
  }, [scheduleDetailsClose]);

  const closeAndSuppressDetails = useCallback(() => {
    closeDetails(true);
  }, [closeDetails]);

  useEffect(
    () => () => {
      clearDetailsOpenTimer();
      clearDetailsCloseTimer();
    },
    [clearDetailsCloseTimer, clearDetailsOpenTimer],
  );

  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>
        <div
          ref={registerSelf}
          data-saved-drop-type="connection"
          data-saved-drop-id={conn.id}
          className="relative min-w-full w-max"
          draggable={isDragEnabled && !isPointerDragEnabled}
          onWheel={closeAndSuppressDetails}
          onPointerDown={
            isPointerDragEnabled
              ? (e) => {
                  e.stopPropagation();
                  closeAndSuppressDetails();
                  handlePointerDragStart(e, "connection", conn.id);
                }
              : undefined
          }
          onPointerMove={
            isPointerDragEnabled
              ? (e) => {
                  e.stopPropagation();
                  handlePointerDragMove(e);
                }
              : undefined
          }
          onPointerUp={
            isPointerDragEnabled
              ? (e) => {
                  e.stopPropagation();
                  handlePointerDragEnd(e);
                }
              : undefined
          }
          onPointerCancel={
            isPointerDragEnabled
              ? (e) => {
                  e.stopPropagation();
                  handlePointerDragCancel(e);
                }
              : undefined
          }
          onDragStart={
            isDragEnabled
              ? (e) => {
                  closeAndSuppressDetails();
                  handleDragStart(e, "connection", conn.id);
                }
              : undefined
          }
          onDragEnter={
            isDragEnabled ? (e) => handleDragEnterItem(e, conn.id, "connection") : undefined
          }
          onDragOver={
            isDragEnabled ? (e) => handleDragOverItem(e, conn.id, "connection") : undefined
          }
          onDragLeave={
            isDragEnabled ? (e) => handleDragLeaveItem(e, conn.id, "connection") : undefined
          }
          onDrop={isDragEnabled ? (e) => handleDropItem(e, conn.id, "connection") : undefined}
          onDragEnd={
            isDragEnabled
              ? () => {
                  closeAndSuppressDetails();
                  handleDragEnd();
                }
              : undefined
          }
        >
          {showBefore && (
            <div
              className="absolute top-0 right-2 h-0.5 rounded-full z-10"
              style={{ backgroundColor: "var(--df-primary)", left: indentLeft }}
            />
          )}
          <div
            className={`group/item relative flex min-w-full w-max items-center gap-2 py-1.5 px-2 rounded cursor-pointer transition-colors df-hover ${isTarget && dragTarget.position === "inside" ? "ring-1 ring-primary/60" : ""}`}
            style={{
              ...(indented ? { paddingLeft: `${8 + depth * 16 + 16}px` } : undefined),
              backgroundColor: isSelected
                ? "color-mix(in srgb, var(--df-primary) 10%, transparent)"
                : isKeyboardActive
                  ? "color-mix(in srgb, var(--df-primary) 7%, transparent)"
                  : undefined,
              boxShadow: isKeyboardActive ? "inset 0 0 0 1px var(--df-primary)" : undefined,
            }}
            onMouseDown={(e) => {
              closeAndSuppressDetails();
              handleConnectionSelectionStart(conn, e);
            }}
            onContextMenu={(e) => {
              closeAndSuppressDetails();
              handleConnectionContextMenu(conn, e);
            }}
            onDoubleClick={() => {
              closeAndSuppressDetails();
              handleConnectOnly(conn);
            }}
          >
            <Tooltip open={detailsOpen}>
              <TooltipTrigger asChild>
                <span
                  className="flex min-w-0 shrink-0 items-center gap-2 pr-14"
                  onPointerEnter={scheduleDetailsOpen}
                  onPointerLeave={handlePointerLeave}
                >
                  <ConnIcon className="text-sm shrink-0" style={iconStyle} />
                  <span
                    className="shrink-0 whitespace-nowrap text-xs font-medium"
                    style={{
                      color:
                        isSelected || isKeyboardActive ? "var(--df-primary)" : "var(--df-text)",
                    }}
                  >
                    {conn.name}
                  </span>
                </span>
              </TooltipTrigger>
              <ConnectionDetailsTooltip
                conn={conn}
                savedConnections={savedConnections}
                t={t}
                onPointerEnter={keepDetailsOpen}
                onPointerLeave={scheduleDetailsClose}
              />
            </Tooltip>
            <div
              className="pointer-events-none sticky right-2 z-10 ml-auto flex shrink-0 items-center gap-1 rounded px-1 opacity-0 backdrop-blur-sm transition-opacity group-hover/item:pointer-events-auto group-hover/item:opacity-100"
              style={{ backgroundColor: "var(--df-bg-hover)" }}
            >
              <button
                className="flex h-5 w-5 items-center justify-center cursor-pointer rounded transition-colors hover:opacity-80"
                style={{ color: "var(--df-text-dimmed)" }}
                aria-label={directConnectLabel}
                onPointerEnter={closeAndSuppressDetails}
                onPointerDown={(e) => {
                  e.stopPropagation();
                }}
                onMouseDown={(e) => {
                  e.stopPropagation();
                  closeAndSuppressDetails();
                }}
                onClick={(e) => {
                  e.stopPropagation();
                  closeAndSuppressDetails();
                  handleConnectOnly(conn);
                }}
              >
                <MdLink className="text-[0.95rem] cursor-pointer" />
              </button>
              <button
                className="flex h-5 w-5 items-center justify-center cursor-pointer rounded transition-colors hover:opacity-80"
                style={{ color: "var(--df-text-dimmed)" }}
                aria-label={t("savedConnections.edit")}
                onPointerEnter={closeAndSuppressDetails}
                onPointerDown={(e) => {
                  e.stopPropagation();
                }}
                onMouseDown={(e) => {
                  e.stopPropagation();
                  closeAndSuppressDetails();
                }}
                onClick={(e) => {
                  e.stopPropagation();
                  closeAndSuppressDetails();
                  onEditConnection(conn);
                }}
              >
                <MdEdit className="text-[0.95rem] cursor-pointer" />
              </button>
            </div>
          </div>
          {showAfter && (
            <div
              className="absolute bottom-0 right-2 h-0.5 rounded-full z-10"
              style={{ backgroundColor: "var(--df-primary)", left: indentLeft }}
            />
          )}
        </div>
      </ContextMenuTrigger>
      <ContextMenuContent className="min-w-[160px]">
        <ContextMenuItem
          onClick={() => {
            closeAndSuppressDetails();
            if (isSelected && selectedConnectionIds.size > 1) {
              handleConnectSelected();
              return;
            }
            handleConnect(conn);
          }}
        >
          <MdLink className="text-[0.875rem] text-muted-foreground mr-2" />
          {connectLabel}
        </ContextMenuItem>
        <ContextMenuItem
          onClick={() => {
            closeAndSuppressDetails();
            onEditConnection(conn);
          }}
        >
          <MdEdit className="text-[0.875rem] text-muted-foreground mr-2" />
          {t("savedConnections.edit")}
        </ContextMenuItem>
        <ContextMenuSeparator />
        <ContextMenuItem
          onClick={() => {
            closeAndSuppressDetails();
            setRenameValue(conn.name);
            setRenamingConn(conn);
          }}
        >
          <MdDriveFileRenameOutline className="text-[0.875rem] text-muted-foreground mr-2" />
          {t("savedConnections.rename")}
        </ContextMenuItem>
        <ContextMenuItem
          onClick={() => {
            closeAndSuppressDetails();
            handleCopyConnection(conn);
          }}
        >
          <MdContentCopy className="text-[0.875rem] text-muted-foreground mr-2" />
          {t("savedConnections.copy")}
        </ContextMenuItem>
        <MoveToGroupContextMenu
          groups={savedGroups}
          onMove={(groupId) => {
            closeAndSuppressDetails();
            requestMoveConnectionToGroup(conn, groupId);
          }}
          t={t}
        />
        <ContextMenuSeparator />
        <ContextMenuItem
          className="text-red-400"
          onClick={() => {
            closeAndSuppressDetails();
            requestDeleteConnection(conn);
          }}
        >
          <MdDelete className="text-[0.875rem] mr-2" />
          {isSelected && selectedConnectionIds.size > 1
            ? t("savedConnections.deleteSelected")
            : t("savedConnections.delete")}
        </ContextMenuItem>
      </ContextMenuContent>
    </ContextMenu>
  );
}
