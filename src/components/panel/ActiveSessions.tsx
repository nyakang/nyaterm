import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { memo, useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import PanelHeader from "@/components/layout/PanelHeader";
import type { SessionInfo } from "@/types/global";

interface ActiveSessionsProps {
  onSessionClick: (sessionId: string) => void;
}

/** List of active sessions (polled). Click switches to that session's tab. */
function ActiveSessions({ onSessionClick }: ActiveSessionsProps) {
  const { t } = useTranslation();
  const [sessions, setSessions] = useState<SessionInfo[]>([]);

  const fetchSessions = useCallback(async () => {
    try {
      const sess = await invoke<SessionInfo[]>("list_sessions");
      setSessions(sess);
    } catch {
      // Backend might not be ready yet
    }
  }, []);

  useEffect(() => {
    fetchSessions();
    const unlisten = listen("sessions-changed", () => {
      fetchSessions();
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [fetchSessions]);

  return (
    <div className="h-full flex flex-col overflow-hidden">
      <PanelHeader title={t("panel.activeSessions")} meta={sessions.length} />
      <div className="flex-1 overflow-y-auto p-2 text-xs space-y-0.5 terminal-scroll">
        {sessions.length === 0 ? (
          <div
            className="text-center py-4 text-[0.6875rem]"
            style={{ color: "var(--df-text-dimmed)" }}
          >
            {t("panel.noActiveSessions")}
          </div>
        ) : (
          sessions.map((session) => (
            <div
              key={session.id}
              className={`flex items-center gap-2 p-2 rounded cursor-pointer transition-colors df-hover ${!session.connected ? "opacity-50" : ""}`}
              onClick={() => onSessionClick(session.id)}
            >
              <div
                className="w-2 h-2 rounded-full shrink-0"
                style={{ backgroundColor: session.connected ? "#22c55e" : "var(--df-text-dimmed)" }}
              />
              <span className="flex-1 truncate" style={{ color: "var(--df-text)" }}>
                {session.name}
              </span>
              <span className="text-[0.625rem]" style={{ color: "var(--df-text-dimmed)" }}>
                {session.session_type}
              </span>
            </div>
          ))
        )}
      </div>
    </div>
  );
}

export default memo(ActiveSessions);
