import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { MdLock, MdMemory, MdSpeed } from "react-icons/md";
import { useApp } from "@/context/AppContext";
import { invoke } from "@/lib/invoke";
import type { RemoteStats } from "@/types";

const STATS_POLL_INTERVAL_MS = 10_000;

function formatMem(usedMb: number, totalMb: number): string {
  if (totalMb >= 1024) {
    return `${(usedMb / 1024).toFixed(1)}/${(totalMb / 1024).toFixed(1)}G`;
  }
  return `${usedMb}/${totalMb}M`;
}

/** Footer bar showing current time, optional remote resource stats, and manual lock button. */
export default function StatusBar() {
  const { t } = useTranslation();
  const { setIsLocked, tabs, activeTabId, appSettings } = useApp();
  const [time, setTime] = useState(new Date());
  const [remoteStats, setRemoteStats] = useState<RemoteStats | null>(null);
  const pollRef = useRef<ReturnType<typeof setInterval> | null>(null);

  const showRemoteStats = appSettings.ui.show_remote_stats ?? false;
  const activeTab = tabs.find((tab) => tab.id === activeTabId);
  const isSSHTab = activeTab?.type === "SSH" && !activeTab?.connecting;

  useEffect(() => {
    const timer = setInterval(() => setTime(new Date()), 1000);
    return () => clearInterval(timer);
  }, []);

  useEffect(() => {
    if (pollRef.current) {
      clearInterval(pollRef.current);
      pollRef.current = null;
    }

    if (!showRemoteStats || !isSSHTab || !activeTab) {
      setRemoteStats(null);
      return;
    }

    const sessionId = activeTab.sessionId;

    const fetchStats = () => {
      invoke<RemoteStats>("get_remote_stats", { sessionId })
        .then(setRemoteStats)
        .catch(() => setRemoteStats(null));
    };

    fetchStats();
    pollRef.current = setInterval(fetchStats, STATS_POLL_INTERVAL_MS);

    return () => {
      if (pollRef.current) clearInterval(pollRef.current);
    };
  }, [showRemoteStats, isSSHTab, activeTab?.sessionId]);

  const yyyy = time.getFullYear();
  const MM = String(time.getMonth() + 1).padStart(2, "0");
  const dd = String(time.getDate()).padStart(2, "0");
  const HH = String(time.getHours()).padStart(2, "0");
  const mm = String(time.getMinutes()).padStart(2, "0");
  const formattedTime = `${yyyy}/${MM}/${dd} ${HH}:${mm}`;

  return (
    <footer
      className="h-7 text-white flex items-center justify-between px-3 text-[0.6875rem] select-none shrink-0"
      style={{ backgroundColor: "var(--df-primary)" }}
    >
      <div className="flex items-center gap-1 h-full">
        {remoteStats && (
          <>
            <div
              className="flex items-center gap-1 bg-black/20 px-2 h-full"
              title={t("statusBar.cpuUsage")}
            >
              <MdSpeed style={{ fontSize: 12 }} />
              <span>CPU {remoteStats.cpu_percent.toFixed(1)}%</span>
            </div>
            <div
              className="flex items-center gap-1 bg-black/20 px-2 h-full"
              title={t("statusBar.memUsage")}
            >
              <MdMemory style={{ fontSize: 12 }} />
              <span>{formatMem(remoteStats.mem_used_mb, remoteStats.mem_total_mb)}</span>
            </div>
          </>
        )}
      </div>
      <div className="flex items-center gap-2 h-full">
        <div className="flex items-center gap-1 bg-black/20 px-3 h-full">
          <span className="font-bold">{formattedTime}</span>
        </div>
        <button
          onClick={() => setIsLocked(true)}
          className="flex items-center gap-1 px-2 h-full hover:bg-white/15 transition-colors cursor-pointer"
          title={t("statusBar.lock")}
        >
          <MdLock style={{ fontSize: 14 }} />
        </button>
      </div>
    </footer>
  );
}
