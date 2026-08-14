import { listen } from "@tauri-apps/api/event";
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ButtonHTMLAttributes,
} from "react";
import { useTranslation } from "react-i18next";
import { MdAdd, MdDelete, MdEdit, MdSearch, MdSend } from "react-icons/md";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import PanelHeader from "@/components/layout/PanelHeader";
import type { OneClickRuntime } from "@/components/app/AppPanelContent";
import { invoke } from "@/lib/invoke";
import { logger } from "@/lib/logger";
import { openQuickCommand } from "@/lib/windowManager";
import { compareQuickCommandsByMode } from "@/lib/quickCommands";
import { buildQuickCommandCategoryPath } from "@/lib/quickCommandCategories";
import { cn } from "@/lib/utils";
import { getErrorMessage } from "@/lib/errors";
import { executeOneClickCommand } from "@/lib/oneClickCommandsExecute";
import type {
  QuickCommand,
  QuickCommandsConfig,
  QuickCommandSortMode,
} from "@/types/global";

interface OneClickCommandsProps {
  runtime: OneClickRuntime;
}

/**
 * 一键命令面板。
 * - 复用 QuickCommands 的持久化（get_quick_commands / upsert / increment_use_count）。
 * - 每条命令若 target_connection_ids 非空，则视为「一键命令」：
 *   已打开目标 → 立即发送命令；未打开目标 → SSH/Telnet 自动连接并用 startup_command 执行。
 */
export default function OneClickCommands({ runtime }: OneClickCommandsProps) {
  const { t } = useTranslation();

  const [commands, setCommands] = useState<QuickCommand[]>([]);
  const [savedCategories, setSavedCategories] = useState<
    QuickCommandsConfig["categories"]
  >([]);
  const [quickCommandsLoaded, setQuickCommandsLoaded] = useState(false);
  const [searchText, setSearchText] = useState("");
  const [sortMode] = useState<QuickCommandSortMode>("created");
  const loaded = useRef(false);

  // 加载 QuickCommands 列表（与 QuickCommands 面板共用同一数据源）。
  const reloadCommands = useCallback(async () => {
    try {
      const cfg = await invoke<QuickCommandsConfig>("get_quick_commands");
      setCommands(cfg.commands || []);
      setSavedCategories(cfg.categories || []);
      setQuickCommandsLoaded(true);
      loaded.current = true;
    } catch (error) {
      logger.error({
        domain: "ui.error",
        event: "one_click_commands.load_failed",
        message: "Failed to load one-click commands",
        error,
      });
      toast.error(
        t("panel.oneClickCommands.errorExecuteFailed", {
          defaultValue: "Load failed: {{error}}",
          error: getErrorMessage(error),
        }),
      );
    }
  }, [t]);

  useEffect(() => {
    if (loaded.current) return;
    void reloadCommands();
  }, [reloadCommands]);

  // 与 QuickCommand 编辑子窗口同步：保存事件后刷新列表。
  useEffect(() => {
    const unsubPromise = listen<{ command?: QuickCommand }>(
      "quick-command-saved",
      () => {
        void reloadCommands();
      },
    );
    return () => {
      unsubPromise.then((unsub) => unsub()).catch(() => {});
    };
  }, [reloadCommands]);

  // 按「创建时间」排序，然后按搜索关键字过滤。
  const visibleCommands = useMemo(() => {
    const sorted = [...commands].sort((a, b) =>
      compareQuickCommandsByMode(a, b, sortMode),
    );
    const q = searchText.trim().toLowerCase();
    if (!q) return sorted;
    return sorted.filter((c) => {
      const label = c.label.toLowerCase();
      const body = c.command.toLowerCase();
      const desc = (c.description ?? "").toLowerCase();
      return label.includes(q) || body.includes(q) || desc.includes(q);
    });
  }, [commands, searchText, sortMode]);

  const flatCategoryPath = useCallback(
    (categoryId: string | undefined) =>
      buildQuickCommandCategoryPath(savedCategories, categoryId),
    [savedCategories],
  );

  // 点击命令：根据 target_connection_ids 决定是否批量。
  const handleRunCommand = useCallback(
    async (cmd: QuickCommand) => {
      const connectionsById = new Map(
        runtime.savedConnections.map((c) => [c.id, c]),
      );
      await executeOneClickCommand({
        cmd,
        tabs: runtime.tabs,
        collectSessionPanes: runtime.collectSessionPanes,
        connectionsById,
        sendToCurrent: runtime.sendToCurrent,
        connectAndRun: runtime.connectAndRun,
        incrementUseCount: runtime.incrementUseCount,
        t: (key, fallback, args) =>
          t(key, {
            defaultValue: fallback,
            ...(args ?? {}),
          }) as string,
      });
    },
    [runtime, t],
  );

  // 打开 QuickCommand 编辑对话框（复用现有编辑子窗口）。
  const handleEdit = useCallback((cmd?: QuickCommand) => {
    openQuickCommand(cmd ? JSON.stringify(cmd) : undefined);
  }, []);

  const handleDelete = useCallback(
    async (cmd: QuickCommand) => {
      try {
        const cfg = await invoke<QuickCommandsConfig>("get_quick_commands");
        const next: QuickCommandsConfig = {
          commands: cfg.commands.filter((c) => c.id !== cmd.id),
          categories: cfg.categories,
        };
        await invoke("save_quick_commands", { config: next });
        setCommands(next.commands);
        toast.success(t("common.deleted", { defaultValue: "已删除" }));
      } catch (error) {
        toast.error(
          t("panel.oneClickCommands.errorExecuteFailed", {
            defaultValue: "Delete failed: {{error}}",
            error: getErrorMessage(error),
          }),
        );
      }
    },
    [t],
  );

  return (
    <div className="h-full flex flex-col overflow-hidden">
      <PanelHeader
        title={t("panel.oneClickCommands.commands", {
          defaultValue: "一键命令",
        })}
        actions={
          <IconButton
            aria-label={t("panel.oneClickCommands.addCommand", {
              defaultValue: "新增",
            })}
            title={t("panel.oneClickCommands.addCommand", {
              defaultValue: "新增",
            })}
            onClick={() => handleEdit(undefined)}
          >
            <MdAdd />
          </IconButton>
        }
      />
      <div className="px-3 py-2 shrink-0">
        <div className="relative">
          <MdSearch
            className="absolute left-2.5 top-1/2 -translate-y-1/2"
            style={{ color: "var(--df-text-muted)" }}
          />
          <Input
            value={searchText}
            onChange={(e) => setSearchText(e.target.value)}
            placeholder={t("panel.oneClickCommands.searchPlaceholder", {
              defaultValue: "搜索命令、分类…",
            })}
            className="pl-8"
          />
        </div>
      </div>
      <div className="flex-1 min-h-0 overflow-auto px-2 pb-2">
        {!quickCommandsLoaded ? (
          <LoadingHint />
        ) : visibleCommands.length === 0 && !searchText.trim() ? (
          <EmptyHint onAdd={() => handleEdit(undefined)} />
        ) : visibleCommands.length === 0 ? (
          <div
            className="p-4 text-xs text-center"
            style={{ color: "var(--df-text-muted)" }}
          >
            {t("panel.oneClickCommands.noSearchMatches", {
              defaultValue: "没有匹配「{{query}}」的命令",
              query: searchText.trim(),
            })}
          </div>
        ) : (
          <CommandList
            commands={visibleCommands}
            flatCategoryPath={flatCategoryPath}
            savedConnections={runtime.savedConnections}
            onRun={handleRunCommand}
            onEdit={handleEdit}
            onDelete={handleDelete}
          />
        )}
      </div>
    </div>
  );
}

interface IconButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  size?: "default" | "sm" | "lg" | "icon";
  variant?: "default" | "ghost" | "outline";
}

function IconButton({
  children,
  className,
  variant = "ghost",
  size = "icon",
  ...rest
}: IconButtonProps) {
  return (
    <Button
      variant={variant}
      size={size}
      className={cn("h-8 w-8", className)}
      {...(rest as ButtonHTMLAttributes<HTMLButtonElement>)}
    >
      {children}
    </Button>
  );
}

function LoadingHint() {
  const { t } = useTranslation();
  return (
    <div className="p-4 text-xs" style={{ color: "var(--df-text-muted)" }}>
      {t("panel.oneClickCommands.loading", { defaultValue: "加载中…" })}
    </div>
  );
}

function EmptyHint({ onAdd }: { onAdd: () => void }) {
  const { t } = useTranslation();
  return (
    <div className="p-4 flex flex-col items-center gap-2 text-xs text-center">
      <div style={{ color: "var(--df-text-muted)" }}>
        {t("panel.oneClickCommands.emptyNoCommands", {
          defaultValue:
            "还没有一键命令。点击右上角「+」创建第一条，记得选择目标服务器。",
        })}
      </div>
      <Button variant="default" size="sm" onClick={onAdd}>
        <MdAdd className="mr-1" />
        {t("panel.oneClickCommands.addCommand", { defaultValue: "新增" })}
      </Button>
    </div>
  );
}

interface CommandListProps {
  commands: QuickCommand[];
  flatCategoryPath: (id: string | undefined) => string;
  savedConnections: OneClickRuntime["savedConnections"];
  onRun: (cmd: QuickCommand) => void;
  onEdit: (cmd: QuickCommand) => void;
  onDelete: (cmd: QuickCommand) => void;
}

function CommandList({
  commands,
  flatCategoryPath,
  savedConnections,
  onRun,
  onEdit,
  onDelete,
}: CommandListProps) {
  const { t } = useTranslation();
  const nameById = useMemo(() => {
    const map = new Map<string, string>();
    for (const conn of savedConnections) map.set(conn.id, conn.name);
    return map;
  }, [savedConnections]);

  return (
    <div className="flex flex-col gap-1">
      {commands.map((cmd) => {
        const targetCount = cmd.target_connection_ids?.length ?? 0;
        const targetSummary =
          targetCount === 0
            ? t("quickCommands.appendOnlyBadge", { defaultValue: "当前会话" })
            : (cmd.target_connection_ids ?? [])
                .map((id) => nameById.get(id) ?? id)
                .slice(0, 2)
                .join("、") + (targetCount > 2 ? ` +${targetCount - 2}` : "");
        const category = flatCategoryPath(cmd.category_id);
        return (
          <div
            key={cmd.id}
            className="group rounded-md border px-2 py-2 hover:bg-opacity-20"
            style={{ borderColor: "var(--df-border)" }}
          >
            <div className="flex items-start justify-between gap-2">
              <button
                type="button"
                onClick={() => onRun(cmd)}
                className="flex-1 text-left min-w-0"
              >
                <div
                  className="text-sm font-medium truncate"
                  style={{ color: "var(--df-text)" }}
                >
                  {cmd.label}
                </div>
                <div
                  className="mt-0.5 text-[11px] truncate font-mono"
                  style={{ color: "var(--df-text-muted)" }}
                  title={cmd.command}
                >
                  $ {cmd.command}
                </div>
                <div
                  className="mt-1 text-[11px] truncate flex gap-2 flex-wrap"
                  style={{ color: "var(--df-text-muted)" }}
                >
                  {category && <span>[{category}]</span>}
                  <span>
                    🎯 {targetSummary}
                    {targetCount > 0 && <span>（{targetCount}）</span>}
                  </span>
                </div>
              </button>
              <div className="shrink-0 flex items-center gap-0.5 opacity-0 group-hover:opacity-100 transition-opacity">
                <IconButton
                  size="icon"
                  variant="ghost"
                  aria-label={t("panel.oneClickCommands.run", {
                    defaultValue: "执行",
                  })}
                  title={t("panel.oneClickCommands.run", {
                    defaultValue: "执行",
                  })}
                  onClick={() => onRun(cmd)}
                >
                  <MdSend />
                </IconButton>
                <IconButton
                  size="icon"
                  variant="ghost"
                  aria-label={t("panel.oneClickCommands.edit", {
                    defaultValue: "编辑",
                  })}
                  title={t("panel.oneClickCommands.edit", {
                    defaultValue: "编辑",
                  })}
                  onClick={() => onEdit(cmd)}
                >
                  <MdEdit />
                </IconButton>
                <IconButton
                  size="icon"
                  variant="ghost"
                  aria-label={t("panel.oneClickCommands.delete", {
                    defaultValue: "删除",
                  })}
                  title={t("panel.oneClickCommands.delete", {
                    defaultValue: "删除",
                  })}
                  onClick={() => onDelete(cmd)}
                >
                  <MdDelete />
                </IconButton>
              </div>
            </div>
          </div>
        );
      })}
    </div>
  );
}
