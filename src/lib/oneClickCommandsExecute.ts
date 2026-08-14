import { toast } from "sonner";
import type { SavedConnection, SessionPane, Tab, QuickCommand } from "@/types/global";
import { buildTerminalCommandInput, sendSessionInput } from "./sessionInput";

/** 在已打开的 tabs 中查找与 connectionId 匹配的所有会话 pane。 */
export function findOpenPanesByConnectionId(
  tabs: Tab[],
  connectionId: string,
  collectSessionPanes: (root: unknown) => SessionPane[],
): SessionPane[] {
  const results: SessionPane[] = [];
  for (const tab of tabs) {
    // biome-ignore lint/suspicious/noExplicitAny: pane tree 类型由 workspaceTabs 提供
    const panes = collectSessionPanes((tab as any).root) as SessionPane[];
    for (const pane of panes) {
      if (pane.connectionId === connectionId && !pane.connecting && !pane.connectError) {
        results.push(pane);
      }
    }
  }
  return results;
}

interface ExecuteOneClickCommandOptions {
  cmd: QuickCommand;
  tabs: Tab[];
  collectSessionPanes: (root: unknown) => SessionPane[];
  connectionsById: Map<string, SavedConnection>;
  /** 给当前活动会话发命令（target 为空时使用）。 */
  sendToCurrent: (command: string, execute: boolean) => void;
  /** 自动建立连接并通过 startupCommand 在连接就绪后执行。 */
  connectAndRun: (
    connection: SavedConnection,
    startupCommand: { command: string; delay_ms?: number },
  ) => Promise<unknown> | void;
  /** 增加该命令的使用次数。 */
  incrementUseCount: (id: string) => unknown;
  /** 跳过协议（local/serial/vnc/rdp 未连接时不支持 startup_command）时的 toast 文案 key。 */
  t: (key: string, fallback?: string, args?: Record<string, unknown>) => string;
}

/**
 * 执行一条「一键命令」。
 * 目标选择策略：
 *   - target_connection_ids 为空 → 发当前活动会话（沿用 QuickCommands 原行为）
 *   - 目标已打开 → 直接 write_to_session 发送命令到所有匹配 pane
 *   - 目标未打开 → SSH/Telnet 则自动连接并用 startup_command 自动执行，其他协议跳过并提示
 */
export async function executeOneClickCommand(options: ExecuteOneClickCommandOptions): Promise<void> {
  const {
    cmd,
    tabs,
    collectSessionPanes,
    connectionsById,
    sendToCurrent,
    connectAndRun,
    incrementUseCount,
    t,
  } = options;

  const targets = cmd.target_connection_ids ?? [];

  // 没有指定目标服务器 → 回退到当前活动会话
  if (targets.length === 0) {
    sendToCurrent(cmd.command, cmd.execution_mode !== "append");
    void incrementUseCount(cmd.id);
    return;
  }

  const execute = cmd.execution_mode !== "append";
  const data = buildTerminalCommandInput(cmd.command, execute);
  let skippedNonSsh = 0;
  let missingConnections = 0;

  for (const connId of targets) {
    const openPanes = findOpenPanesByConnectionId(tabs, connId, collectSessionPanes);
    if (openPanes.length > 0) {
      // 已打开 → 对每个 pane 立即发送命令（不等待，避免互相阻塞）
      for (const pane of openPanes) {
        void sendSessionInput(pane.sessionId, data, {
          preview: execute ? { kind: "reset" } : { kind: "data", data: cmd.command },
          registerSubmission: execute ? cmd.command : null,
          origin: "quick_command",
        }).catch(() => {});
      }
      continue;
    }

    // 未打开 → 检查连接是否存在并判断协议是否支持 startup_command
    const conn = connectionsById.get(connId);
    if (!conn) {
      missingConnections += 1;
      continue;
    }

    if (conn.type === "ssh" || conn.type === "telnet") {
      const p = connectAndRun(conn, { command: cmd.command, delay_ms: 0 });
      if (p && typeof (p as Promise<unknown>).then === "function") {
        void (p as Promise<unknown>).catch(() => {});
      }
    } else {
      // local / serial / vnc / rdp：未连接时不支持 startup_command，累计提示
      skippedNonSsh += 1;
    }
  }

  if (missingConnections > 0) {
    toast.warning(
      t(
        "panel.oneClickCommands.errorExecuteFailed",
        "连接不存在或已删除：{{count}} 台",
        { count: missingConnections },
      ),
    );
  }
  if (skippedNonSsh > 0) {
    toast.info(
      t(
        "panel.oneClickCommands.errorExecuteFailed",
        "已跳过 {{count}} 台未打开的非 SSH/Telnet 服务器",
        { count: skippedNonSsh },
      ),
    );
  }

  void incrementUseCount(cmd.id);
}
