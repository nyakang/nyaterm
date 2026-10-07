import { useCallback, useMemo, useState } from "react";
import { toast } from "sonner";
import type { ResolvedLocalDropPathEntry } from "@/components/panel/file-explorer/model";
import { useTerminalFileDrop } from "@/hooks/useTerminalFileDrop";
import { invoke } from "@/lib/invoke";
import { logger } from "@/lib/logger";
import { getTerminalDropOverlayCopy, handleTerminalFileDrop } from "@/lib/terminalFileDrop";
import type { SessionType, SshRuntimeMode } from "@/types/global";

interface UseTerminalExternalDropParams {
  sessionId: string;
  sessionType: SessionType;
  sshRuntimeMode?: SshRuntimeMode | null;
  visible: boolean;
  containerRef: React.RefObject<HTMLDivElement | null>;
  t: (key: string) => string;
  duplicateStrategy: string;
}

export function useTerminalExternalDrop({
  sessionId,
  sessionType,
  sshRuntimeMode,
  visible,
  containerRef,
  t,
  duplicateStrategy,
}: UseTerminalExternalDropParams) {
  const [isExternalDropActive, setIsExternalDropActive] = useState(false);
  const sshZmodemEnabled =
    sessionType !== "SSH" || sshRuntimeMode === "standard" || sshRuntimeMode === "terminal";

  const resetExternalDropHover = useCallback(() => {
    setIsExternalDropActive(false);
  }, []);

  const resolveLocalDropPaths = useCallback(async (paths: string[]) => {
    const uniquePaths = Array.from(
      new Set(paths.map((path) => path.trim()).filter((path) => !!path)),
    );
    if (uniquePaths.length === 0) {
      return [];
    }

    return invoke<ResolvedLocalDropPathEntry[]>("resolve_local_drop_paths", {
      paths: uniquePaths,
    });
  }, []);

  const processTerminalDropPaths = useCallback(
    async (dropPaths: string[]) => {
      try {
        const resolvedLocalEntries = await resolveLocalDropPaths(dropPaths);
        if (resolvedLocalEntries.length === 0) {
          logger.warn({
            domain: "ui.error",
            event: "terminal.external_drop_paths_unresolved",
            message: "Native terminal drop did not resolve to usable local paths",
            ids: { session_id: sessionId },
            data: { path_count: dropPaths.length },
          });
          toast.error(t("terminal.dropPathsRequired"));
          return;
        }

        await handleTerminalFileDrop({
          sessionId,
          sessionType,
          entries: resolvedLocalEntries,
          t,
          duplicateStrategy,
          sshTransport:
            sessionType === "SSH" ? (sshZmodemEnabled ? "ssh" : "mosh") : undefined,
        });
      } catch (error) {
        logger.error({
          domain: "ui.error",
          event: "terminal.external_drop_failed",
          message: "Failed to process terminal file drop",
          ids: { session_id: sessionId },
          data: { path_count: dropPaths.length },
          error,
        });
        toast.error(String(error));
      }
    },
    [duplicateStrategy, resolveLocalDropPaths, sessionId, sessionType, sshZmodemEnabled, t],
  );

  useTerminalFileDrop({
    sessionId,
    sessionType,
    enabled: visible && sshZmodemEnabled,
    containerRef,
    resetExternalDropHover,
    setIsExternalDropActive,
    processDropPaths: processTerminalDropPaths,
    externalDropPathsRequiredMessage: t("terminal.dropPathsRequired"),
  });

  const dropOverlayCopy = useMemo(
    () => getTerminalDropOverlayCopy(sessionType, t),
    [sessionType, t],
  );

  return {
    isExternalDropActive,
    dropOverlayCopy,
  };
}
