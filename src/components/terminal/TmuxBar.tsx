import { Columns2, Plus, Rows2, Unplug } from "lucide-react";
import { useCallback, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { invoke } from "@/lib/invoke";
import { useTmuxSessionState } from "@/lib/tmux/store";

interface TmuxBarProps {
  /** SSH session id whose channel is running `tmux -CC`. */
  controlSessionId: string;
}

/** Common tmux commands offered as suggestions in the command input. */
const TMUX_COMMON_COMMANDS = [
  "resize-pane -Z",
  "select-layout even-horizontal",
  "select-layout even-vertical",
  "select-layout main-horizontal",
  "select-layout tiled",
  "kill-pane",
  "respawn-pane -k",
  "rename-window ",
  "break-pane",
  "swap-pane -U",
  "swap-pane -D",
  "last-window",
  "synchronize-panes",
] as const;

/**
 * Minimal command bar shown while a tab is driven by tmux control mode.
 * In `-CC`, prefix keys reach the pane's stdin, so tmux-level actions must go
 * through the control channel instead.
 */
function TmuxBar({ controlSessionId }: TmuxBarProps) {
  const { t } = useTranslation();
  const state = useTmuxSessionState(controlSessionId);
  const [command, setCommand] = useState("");
  const [suggestionsOpen, setSuggestionsOpen] = useState(false);

  const send = useCallback(
    (line: string) => {
      const trimmed = line.trim();
      if (!trimmed) return;
      void invoke("tmux_send_command", {
        sessionId: controlSessionId,
        command: trimmed,
      }).catch(() => {});
    },
    [controlSessionId],
  );

  const detach = useCallback(() => {
    void invoke("tmux_detach", { sessionId: controlSessionId }).catch(() => {});
  }, [controlSessionId]);

  const submitCommand = useCallback(() => {
    if (!command.trim()) return;
    send(command);
    setCommand("");
  }, [command, send]);

  return (
    <div
      className="flex h-8 shrink-0 items-center gap-1.5 border-b px-2"
      style={{
        borderColor: "var(--df-border)",
        background: "var(--df-bg-panel)",
      }}
      data-testid="tmux-bar"
    >
      <span
        className="select-none font-mono text-[11px] font-semibold tracking-wide"
        style={{ color: "var(--df-primary)" }}
      >
        tmux
      </span>

      <Select
        value={state?.activeWindowId ?? ""}
        onValueChange={(windowId) => send(`select-window -t '${windowId}'`)}
      >
        <SelectTrigger
          className="h-6 w-40 gap-1 border-none bg-transparent px-1.5 text-xs shadow-none focus:ring-0"
          aria-label={t("tmux.window")}
        >
          <SelectValue placeholder={t("tmux.window")} />
        </SelectTrigger>
        <SelectContent>
          {(state?.windows ?? []).map((window) => (
            <SelectItem key={window.id} value={window.id}>
              <span className="font-mono text-[11px] opacity-60">
                {window.id}
              </span>
              <span className="truncate">{window.name || window.id}</span>
            </SelectItem>
          ))}
        </SelectContent>
      </Select>

      <Button
        type="button"
        variant="ghost"
        size="icon"
        className="h-6 w-6"
        title={t("tmux.splitVertical")}
        onClick={() => send("split-window -h")}
      >
        <Columns2 className="h-3.5 w-3.5" />
      </Button>
      <Button
        type="button"
        variant="ghost"
        size="icon"
        className="h-6 w-6"
        title={t("tmux.splitHorizontal")}
        onClick={() => send("split-window -v")}
      >
        <Rows2 className="h-3.5 w-3.5" />
      </Button>
      <Button
        type="button"
        variant="ghost"
        size="icon"
        className="h-6 w-6"
        title={t("tmux.newWindow")}
        onClick={() => send("new-window")}
      >
        <Plus className="h-3.5 w-3.5" />
      </Button>
      <Button
        type="button"
        variant="ghost"
        size="icon"
        className="h-6 w-6"
        title={t("tmux.detach")}
        onClick={detach}
      >
        <Unplug className="h-3.5 w-3.5" />
      </Button>

      <div className="mx-1 h-4 w-px" style={{ background: "var(--df-border)" }} />

      <div className="relative min-w-0 flex-1">
        <Input
          value={command}
          onChange={(event) => setCommand(event.target.value)}
          onFocus={() => setSuggestionsOpen(true)}
          onBlur={() => setSuggestionsOpen(false)}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              submitCommand();
            } else if (event.key === "Escape") {
              setSuggestionsOpen(false);
            }
          }}
          placeholder={t("tmux.commandPlaceholder")}
          className="h-6 w-full border-none bg-transparent px-1 font-mono text-xs shadow-none focus-visible:ring-0"
          spellCheck={false}
          autoCapitalize="off"
          autoCorrect="off"
        />
        {suggestionsOpen ? (
          <div
            className="absolute left-0 top-full z-50 mt-1 max-h-64 w-64 overflow-auto rounded-md border p-1 shadow-md"
            style={{
              borderColor: "var(--df-border)",
              // -solid keeps the suggestion list opaque over pane content.
              background: "var(--df-bg-panel-solid, var(--df-bg-panel))",
            }}
          >
            {TMUX_COMMON_COMMANDS.filter((entry) =>
              entry.trim().includes(command.trim()),
            ).map((entry) => (
              <button
                key={entry}
                type="button"
                // Picking a suggestion fires the command immediately.
                onMouseDown={(event) => {
                  event.preventDefault();
                  send(entry);
                  setCommand("");
                  setSuggestionsOpen(false);
                }}
                className="block w-full rounded px-2 py-1.5 text-left font-mono text-xs hover:bg-accent"
              >
                {entry}
              </button>
            ))}
          </div>
        ) : null}
      </div>
    </div>
  );
}

export default TmuxBar;
