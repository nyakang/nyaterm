import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { memo, useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { MdChevronRight } from "react-icons/md";
import PanelHeader from "@/components/layout/PanelHeader";

interface CommandHistoryProps {
  onCommandSend: (command: string) => void;
}

/** Command history list (polled). Double-click sends command to active tab. */
function CommandHistory({ onCommandSend }: CommandHistoryProps) {
  const { t } = useTranslation();
  const [history, setHistory] = useState<string[]>([]);

  const fetchHistory = useCallback(async () => {
    try {
      const cmds = await invoke<string[]>("get_command_history");
      setHistory(cmds);
    } catch {
      // Backend might not be ready
    }
  }, []);

  useEffect(() => {
    fetchHistory();
    const unlisten = listen("command-history-changed", () => {
      fetchHistory();
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [fetchHistory]);

  const handleDoubleClick = useCallback(
    (command: string) => {
      onCommandSend(command);
    },
    [onCommandSend],
  );

  const historyEntries = useMemo(() => {
    const counts = new Map<string, number>();
    return history.map((command) => {
      const occurrence = (counts.get(command) ?? 0) + 1;
      counts.set(command, occurrence);
      return { command, key: `${command}-${occurrence}` };
    });
  }, [history]);

  return (
    <div className="h-full flex flex-col overflow-hidden">
      <PanelHeader title={t("panel.commandHistory")} />
      <div className="flex-1 overflow-y-auto p-2 text-xs font-mono space-y-0.5 terminal-scroll">
        {history.length === 0 ? (
          <div
            className="text-center py-4 font-display text-[0.6875rem]"
            style={{ color: "var(--df-text-dimmed)" }}
          >
            {t("panel.noCommandsYet")}
          </div>
        ) : (
          historyEntries.map(({ command, key }) => (
            <div
              key={key}
              className="px-2 py-1.5 rounded cursor-pointer transition-colors truncate flex items-center gap-1.5 group df-hover"
              style={{ color: "var(--df-text)" }}
              title={command}
              onDoubleClick={() => handleDoubleClick(command)}
            >
              <MdChevronRight
                className="text-[0.625rem] transition-colors"
                style={{ color: "var(--df-text-dimmed)" }}
              />
              <span className="truncate">{command}</span>
            </div>
          ))
        )}
      </div>
    </div>
  );
}

export default memo(CommandHistory);
