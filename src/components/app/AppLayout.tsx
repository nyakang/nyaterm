import type { TFunction } from "i18next";
import {
  type ComponentProps,
  type ReactNode,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import FloatingPanel from "@/components/app/FloatingPanel";
import { MdClose, MdTerminal } from "react-icons/md";
import PanelStack from "@/components/app/PanelStack";
import AboutDialog from "@/components/dialog/app/AboutDialog";
import LockScreen from "@/components/dialog/app/LockScreen";
import QuitConfirmDialog from "@/components/dialog/app/QuitConfirmDialog";
import UpdateDialog from "@/components/dialog/app/UpdateDialog";
import type { HostKeyVerifyRequest } from "@/components/dialog/connections/HostKeyVerifyDialog";
import { HostKeyVerifyDialog } from "@/components/dialog/connections/HostKeyVerifyDialog";
import type { OtpRequest } from "@/components/dialog/connections/OtpDialog";
import { OtpDialog } from "@/components/dialog/connections/OtpDialog";
import type { RdpCertificateVerifyRequest } from "@/components/dialog/connections/RdpCertificateVerifyDialog";
import { RdpCertificateVerifyDialog } from "@/components/dialog/connections/RdpCertificateVerifyDialog";
import type { SshAuthRequest } from "@/components/dialog/connections/SshAuthDialog";
import { SshAuthDialog } from "@/components/dialog/connections/SshAuthDialog";
import type { SshAgentAuthRequest } from "@/components/dialog/connections/SshAgentAuthDialog";
import { SshAgentAuthDialog } from "@/components/dialog/connections/SshAgentAuthDialog";
import DockerSudoPasswordDialog, {
  type DockerSudoPasswordRequest,
} from "@/components/dialog/docker/DockerSudoPasswordDialog";
import { TransferDuplicateDialog } from "@/components/dialog/file-explorer/TransferDuplicateDialog";
import SyncGroupDialog from "@/components/dialog/terminal/SyncGroupDialog";
import ActivityBar from "@/components/layout/ActivityBar";
import Header from "@/components/layout/Header";
import ResizeHandle from "@/components/layout/ResizeHandle";
import QuickCommands from "@/components/panel/QuickCommands";
import SerialSendPanel from "@/components/panel/SendCommandPanel";
import TabWindowsWorkspace from "@/components/terminal/TabWindowsWorkspace";
import { useTheme } from "@/context/ThemeContext";
import {
  buildBackgroundImageLayerStyle,
  buildSurfaceCssVariables,
  isWindowTransparencyEnabled,
  loadBackgroundImageDataUrl,
} from "@/lib/backgroundImage";
import { isMacOS } from "@/lib/platform";
import type { SendCommandPanelDraft } from "@/lib/sendCommandPanelEvents";
import type { UpdateInfo } from "@/lib/updater";
import { bounceTopModalWindow } from "@/lib/windowManager";
import type {
  AppearanceSettings,
  SavedConnection,
  SessionType,
  SyncGroup,
  UiConfig,
} from "@/types/global";
import StartWorkspace from "./start-workspace/StartWorkspace";

type HeaderProps = ComponentProps<typeof Header>;
type ActivityBarProps = ComponentProps<typeof ActivityBar>;
type WorkspaceProps = ComponentProps<typeof TabWindowsWorkspace>;
type ActivityBarSideProps = Omit<ActivityBarProps, "side" | "zone">;

interface AppLayoutProps {
  t: TFunction;
  uiConfig: UiConfig;
  appearance: AppearanceSettings;
  header: Omit<HeaderProps, "onToggleLeft" | "onToggleRight">;
  mobile: {
    leftOpen: boolean;
    rightOpen: boolean;
    setLeftOpen: (open: boolean) => void;
    setRightOpen: (open: boolean) => void;
  };
  leftActivityBar: ActivityBarSideProps;
  rightActivityBar: ActivityBarSideProps;
  onLeftResize: (delta: number) => void;
  onRightResize: (delta: number) => void;
  panelContent: (panelId: string | null) => ReactNode;
  panelTitle: (panelId: string) => string;
  /** Panels visible per side, ordered top-to-bottom (single id in single-open mode). */
  leftPanelIds: string[];
  rightPanelIds: string[];
  floatingPanelIds: {
    left: string | null;
    right: string | null;
  };
  onCloseFloatingPanel: (side: "left" | "right") => void;
  /** Exclusive panel (e.g. AI assistant) shown alone instead of the stack (multi-open mode). */
  leftOverlayPanelId: string | null;
  rightOverlayPanelId: string | null;
  panelStackSizes: Record<string, number>;
  onPanelStackResize: (
    side: "left" | "right",
    aboveId: string,
    belowId: string,
    delta: number,
    containerHeight: number,
  ) => void;
  workspace: WorkspaceProps;
  tabsCount: number;
  emptyWorkspace: {
    temporarySshShortcut: string;
    openChatShortcut: string;
    showCommandsShortcut: string;
    switchTerminalShortcut: string;
    onTemporarySshLink: () => void;
    onOpenChat: () => void;
    onShowCommands: () => void;
    onSwitchTerminal: () => void;
    onConnectConnection: (connection: SavedConnection) => Promise<void> | void;
    onEditConnection: (connection: SavedConnection) => void;
  };
  bottomPanel: {
    activePanel: "quickCmdBar" | "serialSend" | null;
    quickCmdHeight: number;
    serialSendHeight: number;
    clearAfterSend: boolean;
    activeSerialSessionId: string | null;
    activeNonSerialSessionId: string | null;
    activeNonSerialSessionIds: string[];
    syncGroups: SyncGroup[];
    currentWindowLabel: string;
    sessionTargets: {
      id: string;
      name: string;
      tabName: string;
      type: SessionType;
      ownerWindowLabel?: string | null;
    }[];
    sendCommandDraft: SendCommandPanelDraft | null;
    onSendCommandDraftConsumed: () => void;
    onQuickCmdResize: (delta: number) => void;
    onSerialSendResize: (delta: number) => void;
    onClearAfterSendChange: (enabled: boolean) => void;
    onCommandSend: (command: string, execute?: boolean) => void;
    onSendToAllSessions: (command: string, execute?: boolean) => void;
  };
  dialogs: {
    aboutOpen: boolean;
    onAboutOpenChange: (open: boolean) => void;
    syncGroupOpen: boolean;
    onSyncGroupOpenChange: (open: boolean) => void;
    updateOpen: boolean;
    onUpdateOpenChange: (open: boolean) => void;
    onUpdateFound: (info: UpdateInfo) => void;
    quitConfirmOpen: boolean;
    onQuitConfirmOpenChange: (open: boolean) => void;
    onQuitConfirm: () => void;
    otpRequest: OtpRequest | null;
    onOtpDone: (requestId: string) => void;
    sshAuthRequest: SshAuthRequest | null;
    onSshAuthDone: (requestId: string) => void;
    sshAgentAuthRequest: SshAgentAuthRequest | null;
    onSshAgentAuthDone: (requestId: string) => void;
    dockerSudoPasswordRequest: DockerSudoPasswordRequest | null;
    onDockerSudoPasswordDone: (requestId: string) => void;
    hostKeyVerifyRequest: HostKeyVerifyRequest | null;
    onHostKeyVerifyDone: (requestId: string) => void;
    rdpCertificateVerifyRequest: RdpCertificateVerifyRequest | null;
    onRdpCertificateVerifyDone: (requestId: string) => void;
    modalChildWindowCount: number;
    locked: boolean;
    hasMasterPassword: boolean;
    onUnlock: () => void;
    onRequestClose: () => void;
  };
}

export default function AppLayout({
  t,
  uiConfig,
  appearance,
  header,
  mobile,
  leftActivityBar,
  rightActivityBar,
  onLeftResize,
  onRightResize,
  panelContent,
  panelTitle,
  leftPanelIds,
  rightPanelIds,
  floatingPanelIds,
  onCloseFloatingPanel,
  leftOverlayPanelId,
  rightOverlayPanelId,
  panelStackSizes,
  onPanelStackResize,
  workspace,
  tabsCount,
  emptyWorkspace,
  bottomPanel,
  dialogs,
}: AppLayoutProps) {
  const { theme } = useTheme();
  const backgroundImagePath = appearance.background_image_path?.trim() ?? "";
  const [backgroundDataUrl, setBackgroundDataUrl] = useState("");
  const [serialSendRunning, setSerialSendRunning] = useState(false);
  // Latch the first time the serial send panel is shown so it stays mounted
  // (but hidden) afterwards, preserving the user's input across hide/show cycles.
  const serialSendEverShownRef = useRef(false);

  useEffect(() => {
    let cancelled = false;

    setBackgroundDataUrl("");
    if (!backgroundImagePath) return;

    void loadBackgroundImageDataUrl(backgroundImagePath).then((dataUrl) => {
      if (!cancelled) setBackgroundDataUrl(dataUrl);
    });

    return () => {
      cancelled = true;
    };
  }, [backgroundImagePath]);

  const backgroundEnabled = Boolean(backgroundDataUrl);
  const effectiveAppearance = useMemo(
    () =>
      backgroundEnabled
        ? appearance
        : {
            ...appearance,
            background_image_path: null,
          },
    [appearance, backgroundEnabled],
  );
  const backgroundLayerStyle = useMemo(
    () =>
      buildBackgroundImageLayerStyle(effectiveAppearance, backgroundDataUrl),
    [effectiveAppearance, backgroundDataUrl],
  );
  const windowTransparencyEnabled =
    isWindowTransparencyEnabled(effectiveAppearance);
  const shellStyle = useMemo(
    () => ({
      ...buildSurfaceCssVariables(theme.colors, effectiveAppearance),
      // When native window transparency is on, the shell background must be
      // transparent so the native backdrop is visible through the webview.
      backgroundColor: windowTransparencyEnabled
        ? "transparent"
        : theme.colors.bg,
      color: "var(--df-text)",
    }),
    [effectiveAppearance, theme.colors, windowTransparencyEnabled],
  );
  const hasLeftActivityItems =
    leftActivityBar.items.length > 0 ||
    (leftActivityBar.bottomItems?.length ?? 0) > 0 ||
    (leftActivityBar.hiddenItems?.length ?? 0) > 0;
  const hasRightActivityItems =
    rightActivityBar.items.length > 0 ||
    (rightActivityBar.bottomItems?.length ?? 0) > 0 ||
    (rightActivityBar.hiddenItems?.length ?? 0) > 0;
  const leftPanelOpen =
    hasLeftActivityItems &&
    (leftPanelIds.length > 0 || Boolean(leftOverlayPanelId));
  const rightPanelOpen =
    hasRightActivityItems &&
    (rightPanelIds.length > 0 || Boolean(rightOverlayPanelId));
  const leftMobileOpen = hasLeftActivityItems && mobile.leftOpen;
  const rightMobileOpen = hasRightActivityItems && mobile.rightOpen;
  const serialSendVisible = bottomPanel.activePanel === "serialSend";
  if (serialSendVisible) {
    serialSendEverShownRef.current = true;
  }
  const serialSendMounted =
    serialSendVisible || serialSendRunning || serialSendEverShownRef.current;

  useEffect(() => {
    const roots = [document.documentElement, document.body];
    for (const root of roots) {
      if (windowTransparencyEnabled) {
        root.dataset.windowTransparency = "true";
      } else {
        delete root.dataset.windowTransparency;
      }
    }

    return () => {
      for (const root of roots) {
        delete root.dataset.windowTransparency;
      }
    };
  }, [windowTransparencyEnabled]);

  useEffect(() => {
    if (!hasLeftActivityItems && mobile.leftOpen) {
      mobile.setLeftOpen(false);
    }
    if (!hasRightActivityItems && mobile.rightOpen) {
      mobile.setRightOpen(false);
    }
  }, [
    hasLeftActivityItems,
    hasRightActivityItems,
    mobile.leftOpen,
    mobile.rightOpen,
    mobile.setLeftOpen,
    mobile.setRightOpen,
  ]);

  return (
    <div
      className="nyaterm-wallpaper-shell font-display relative h-full min-h-0 overflow-hidden"
      data-wallpaper-enabled={backgroundEnabled ? "true" : "false"}
      data-window-transparency={windowTransparencyEnabled ? "true" : "false"}
      data-window-transparency-blur={
        windowTransparencyEnabled &&
        effectiveAppearance.window_transparency_blur
          ? "true"
          : "false"
      }
      style={shellStyle}
    >
      {backgroundEnabled && (
        <div
          aria-hidden="true"
          className="pointer-events-none absolute inset-0 z-0"
          style={backgroundLayerStyle}
        />
      )}
      <div className="relative z-10 flex h-full min-h-0 flex-col">
        <Header
          {...header}
          onToggleLeft={() => {
            if (hasLeftActivityItems) mobile.setLeftOpen(!mobile.leftOpen);
          }}
          onToggleRight={() => {
            if (hasRightActivityItems) mobile.setRightOpen(!mobile.rightOpen);
          }}
        />

        <main className="flex-1 flex overflow-hidden relative">
          {!isMacOS && (leftMobileOpen || rightMobileOpen) && (
            <div
              className="absolute inset-0 bg-black/50 z-40 lg:hidden"
              onClick={() => {
                mobile.setLeftOpen(false);
                mobile.setRightOpen(false);
              }}
            />
          )}

          {hasLeftActivityItems && (
            <ActivityBar
              {...leftActivityBar}
              side="left"
              zone={{ top: "left_top", bottom: "left_bottom" }}
            />
          )}

          {leftPanelOpen && (
            <>
              <div
                style={{
                  width: uiConfig.left_width,
                  backgroundColor: "var(--df-bg-panel)",
                }}
                className={
                  isMacOS
                    ? "relative flex flex-col"
                    : `
                    fixed inset-y-0 left-10 z-40 flex flex-col shadow-xl transition-transform duration-200
                    lg:relative lg:left-0 lg:translate-x-0 lg:z-0 lg:shadow-none
                    ${
                      leftMobileOpen
                        ? "translate-x-0"
                        : "-translate-x-[calc(100%+2.5rem)] lg:translate-x-0"
                    }
                  `
                }
              >
                {!isMacOS && (
                  <div
                    className="lg:hidden h-10 flex items-center justify-end px-2 border-b shrink-0"
                    style={{ borderColor: "var(--df-border)" }}
                  >
                    <button
                      onClick={() => mobile.setLeftOpen(false)}
                      style={{ color: "var(--df-text-muted)" }}
                    >
                      <MdClose />
                    </button>
                  </div>
                )}

                <div className="flex-1 min-h-0 overflow-hidden">
                  <PanelStack
                    panelIds={leftPanelIds}
                    overlayPanelId={leftOverlayPanelId}
                    sizes={panelStackSizes}
                    renderPanel={panelContent}
                    onResizePair={(aboveId, belowId, delta, containerHeight) =>
                      onPanelStackResize(
                        "left",
                        aboveId,
                        belowId,
                        delta,
                        containerHeight,
                      )
                    }
                  />
                </div>
              </div>
              <ResizeHandle
                direction="horizontal"
                onResize={onLeftResize}
                className={isMacOS ? "" : "hidden lg:block"}
              />
            </>
          )}

          <section
            className="flex-1 flex flex-col relative min-w-0 origin-top-left"
            style={{
              backgroundColor: backgroundEnabled
                ? "transparent"
                : "var(--df-bg-terminal)",
            }}
          >
            <div className="flex-1 relative overflow-hidden">
              {tabsCount === 0 ? (
                <StartWorkspace
                  t={t}
                  backgroundEnabled={backgroundEnabled}
                  temporarySshShortcut={emptyWorkspace.temporarySshShortcut}
                  openChatShortcut={emptyWorkspace.openChatShortcut}
                  showCommandsShortcut={emptyWorkspace.showCommandsShortcut}
                  switchTerminalShortcut={emptyWorkspace.switchTerminalShortcut}
                  onTemporarySshLink={emptyWorkspace.onTemporarySshLink}
                  onOpenChat={emptyWorkspace.onOpenChat}
                  onShowCommands={emptyWorkspace.onShowCommands}
                  onSwitchTerminal={emptyWorkspace.onSwitchTerminal}
                  onConnectConnection={emptyWorkspace.onConnectConnection}
                  onEditConnection={emptyWorkspace.onEditConnection}
                />
              ) : workspace.layout ? (
                <TabWindowsWorkspace {...workspace} />
              ) : (
                <div className="flex items-center justify-center h-full text-slate-500">
                  <div className="text-center space-y-3">
                    <MdTerminal className="text-4xl mx-auto" />
                    <p className="text-sm">{t("common.loading")}</p>
                  </div>
                </div>
              )}
              {floatingPanelIds.left && (
                <FloatingPanel
                  side="left"
                  panelId={floatingPanelIds.left}
                  width={uiConfig.left_width}
                  title={panelTitle(floatingPanelIds.left)}
                  onClose={() => onCloseFloatingPanel("left")}
                  onResize={onLeftResize}
                >
                  {panelContent(floatingPanelIds.left)}
                </FloatingPanel>
              )}
              {floatingPanelIds.right && (
                <FloatingPanel
                  side="right"
                  panelId={floatingPanelIds.right}
                  width={uiConfig.right_width}
                  title={panelTitle(floatingPanelIds.right)}
                  onClose={() => onCloseFloatingPanel("right")}
                  onResize={onRightResize}
                >
                  {panelContent(floatingPanelIds.right)}
                </FloatingPanel>
              )}
            </div>

            {bottomPanel.activePanel === "quickCmdBar" && (
              <>
                <ResizeHandle
                  direction="vertical"
                  onResize={bottomPanel.onQuickCmdResize}
                />
                <div
                  style={{
                    height: bottomPanel.quickCmdHeight,
                    backgroundColor: "var(--df-bg-panel)",
                  }}
                  className="shrink-0 overflow-hidden"
                >
                  <QuickCommands
                    onSend={bottomPanel.onCommandSend}
                    onSendToAll={bottomPanel.onSendToAllSessions}
                  />
                </div>
              </>
            )}

            {serialSendVisible && (
              <ResizeHandle
                direction="vertical"
                onResize={bottomPanel.onSerialSendResize}
              />
            )}

            {serialSendMounted && (
              <>
                <div
                  style={{
                    ...(serialSendVisible
                      ? {
                          height: bottomPanel.serialSendHeight,
                          backgroundColor: "var(--df-bg-panel)",
                        }
                      : {}),
                  }}
                  className={serialSendVisible ? "shrink-0 overflow-hidden" : "hidden"}
                >
                  <SerialSendPanel
                    serialSessionId={bottomPanel.activeSerialSessionId}
                    currentShellSessionId={bottomPanel.activeNonSerialSessionId}
                    shellSessionIds={bottomPanel.activeNonSerialSessionIds}
                    syncGroups={bottomPanel.syncGroups}
                    currentWindowLabel={bottomPanel.currentWindowLabel}
                    sessionTargets={bottomPanel.sessionTargets}
                    clearAfterSend={bottomPanel.clearAfterSend}
                    draft={bottomPanel.sendCommandDraft}
                    onDraftConsumed={bottomPanel.onSendCommandDraftConsumed}
                    onSendingChange={setSerialSendRunning}
                    onClearAfterSendChange={bottomPanel.onClearAfterSendChange}
                  />
                </div>
              </>
            )}
          </section>

          {hasRightActivityItems && (
            <>
              {rightPanelOpen && (
                <ResizeHandle
                  direction="horizontal"
                  onResize={onRightResize}
                  className={isMacOS ? "" : "hidden md:block"}
                />
              )}
              <aside
                style={{
                  width: rightPanelOpen ? uiConfig.right_width : 0,
                  backgroundColor: "var(--df-bg-panel)",
                  borderColor: "var(--df-border)",
                }}
                className={
                  isMacOS
                    ? `relative flex flex-col overflow-hidden ${rightPanelOpen ? "border-l" : "hidden"}`
                    : `
                    fixed inset-y-0 right-10 z-50 flex flex-col overflow-hidden shadow-xl transition-transform duration-200 border-l
                    md:relative md:right-0 md:translate-x-0 md:z-0 md:shadow-none
                    ${
                      rightPanelOpen && rightMobileOpen
                        ? "translate-x-0"
                        : "translate-x-[calc(100%+2.5rem)] md:translate-x-0"
                    }
                    ${rightPanelOpen ? "" : "hidden"}
                  `
                }
              >
                {!isMacOS && (
                  <div
                    className="md:hidden h-10 flex items-center justify-end px-2 border-b shrink-0"
                    style={{ borderColor: "var(--df-border)" }}
                  >
                    <button
                      onClick={() => mobile.setRightOpen(false)}
                      style={{ color: "var(--df-text-muted)" }}
                    >
                      <MdClose />
                    </button>
                  </div>
                )}

                <div className="flex-1 min-h-0 overflow-hidden">
                  <PanelStack
                    panelIds={rightPanelIds}
                    overlayPanelId={rightOverlayPanelId}
                    sizes={panelStackSizes}
                    renderPanel={panelContent}
                    onResizePair={(aboveId, belowId, delta, containerHeight) =>
                      onPanelStackResize(
                        "right",
                        aboveId,
                        belowId,
                        delta,
                        containerHeight,
                      )
                    }
                  />
                </div>
              </aside>
            </>
          )}

          {hasRightActivityItems && (
            <ActivityBar
              {...rightActivityBar}
              side="right"
              zone={{ top: "right_top", bottom: "right_bottom" }}
            />
          )}
        </main>

        <AboutDialog
          open={dialogs.aboutOpen}
          onClose={() => dialogs.onAboutOpenChange(false)}
        />

        <SyncGroupDialog
          open={dialogs.syncGroupOpen}
          onClose={() => dialogs.onSyncGroupOpenChange(false)}
        />

        <UpdateDialog
          open={dialogs.updateOpen}
          onClose={() => dialogs.onUpdateOpenChange(false)}
          onUpdateFound={dialogs.onUpdateFound}
        />

        <QuitConfirmDialog
          open={dialogs.quitConfirmOpen}
          onOpenChange={dialogs.onQuitConfirmOpenChange}
          onConfirm={dialogs.onQuitConfirm}
        />

        <OtpDialog request={dialogs.otpRequest} onDone={dialogs.onOtpDone} />
        <SshAuthDialog
          request={dialogs.sshAuthRequest}
          onDone={dialogs.onSshAuthDone}
        />
        <SshAgentAuthDialog
          request={dialogs.sshAgentAuthRequest}
          onDone={dialogs.onSshAgentAuthDone}
        />
        <DockerSudoPasswordDialog
          request={dialogs.dockerSudoPasswordRequest}
          onDone={dialogs.onDockerSudoPasswordDone}
        />
        <HostKeyVerifyDialog
          request={dialogs.hostKeyVerifyRequest}
          onDone={dialogs.onHostKeyVerifyDone}
        />
        <RdpCertificateVerifyDialog
          request={dialogs.rdpCertificateVerifyRequest}
          onDone={dialogs.onRdpCertificateVerifyDone}
        />
        <TransferDuplicateDialog />

        {dialogs.modalChildWindowCount > 0 && (
          <div
            className="fixed inset-0 z-[9998]"
            onMouseDown={() => {
              void bounceTopModalWindow();
            }}
            style={{
              backgroundColor: "rgba(0, 0, 0, 0.3)",
              backdropFilter: "blur(4px)",
              WebkitBackdropFilter: "blur(4px)",
            }}
          />
        )}

        {dialogs.locked && (
          <LockScreen
            hasPassword={dialogs.hasMasterPassword}
            onUnlock={dialogs.onUnlock}
            onRequestClose={dialogs.onRequestClose}
          />
        )}
      </div>
    </div>
  );
}
