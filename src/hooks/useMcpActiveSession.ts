import { useEffect } from "react";
import { invoke } from "@/lib/invoke";

export function useMcpActiveSession(activeSessionId: string | null) {
  useEffect(() => {
    void invoke("report_mcp_active_session", {
      sessionId: activeSessionId,
    }).catch(() => {});
  }, [activeSessionId]);
}
