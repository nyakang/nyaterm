import type { TFunction } from "i18next";
import { type ComponentProps, type ReactNode, useEffect, useMemo, useState } from "react";
import { MdClose, MdTerminal } from "react-icons/md";
import AboutDialog from "@/components/dialog/app/AboutDialog";
import LockScreen from "@/components/dialog/app/LockScreen";
import QuitConfirmDialog from "@/components/dialog/app/QuitConfirmDialog";
import UpdateDialog from "@/components/dialog/app/UpdateDialog";
import type { HostKeyVerifyRequest } from "@/components/dialog/connections/HostKeyVerifyDialog";
import { HostKeyVerifyDialog } from "@/components/dialog/connections/HostKeyVerifyDialog";
import type { OtpRequest } from "@/components/dialog/connections/OtpDialog";
import { OtpDialog } from "@/components/dialog/connections/OtpDialog";
import SyncGroupDialog from "@/components/dialog/terminal/SyncGroupDialog";
import ActivityBar from "@/components/layout/ActivityBar";
import Header from "@/components/layout/Header";
import ResizeHandle from "@/components/layout/ResizeHandle";
import NyaTermLogo from "@/components/NyaTermLogo";
import QuickCommands from "@/components/panel/QuickCommands";
import SerialSendPanel from "@/components/panel/SendCommandPanel";
import TabWindowsWorkspace from "@/components/terminal/TabWindowsWorkspace";
import { Kbd, KbdGroup } from "@/components/ui/kbd";
import { useTheme } from "@/context/ThemeContext";
import {
  buildBackgroundImageLayerStyle,
  buildSurfaceCssVariables,
  loadBackgroundImageDataUrl,
} from "@/lib/backgroundImage";
import { isMacOS } from "@/lib/platform";
import type { SendCommandPanelDraft } from "@/lib/sendCommandPanelEvents";
import type { UpdateInfo } from "@/lib/updater";
import { bounceTopModalWindow } from "@/lib/windowManager";
import type { AppearanceSettings, UiConfig } from "@/types/global";

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
  workspace: WorkspaceProps;
  tabsCount: number;
  emptyWorkspace: {
    openChatShortcut: string;
    showCommandsShortcut: string;
    switchTerminalShortcut: string;
    onOpenChat: () => void;
    onShowCommands: () => void;
    onSwitchTerminal: () => void;
  };
  bottomPanel: {
    activePanel: "quickCmdBar" | "serialSend" | null;
    quickCmdHeight: number;
    serialSendHeight: number;
    activeSerialSessionId: string | null;
    activeNonSerialSessionId: string | null;
    activeNonSerialSessionIds: string[];
    sendCommandDraft: SendCommandPanelDraft | null;
    onSendCommandDraftConsumed: () => void;
    onQuickCmdResize: (delta: number) => void;
    onSerialSendResize: (delta: number) => void;
    onCommandSend: (command: string, execute?: boolean) => void;
    onSendToAllSessions: (command: string) => void;
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
    onOtpDone: () => void;
    hostKeyVerifyRequest: HostKeyVerifyRequest | null;
    onHostKeyVerifyDone: () => void;
    modalChildWindowCount: number;
    locked: boolean;
    hasMasterPassword: boolean;
    onUnlock: () => void;
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
  workspace,
  tabsCount,
  emptyWorkspace,
  bottomPanel,
  dialogs,
}: AppLayoutProps) {
  const { theme } = useTheme();
  const backgroundImagePath = appearance.background_image_path?.trim() ?? "";
  const [backgroundDataUrl, setBackgroundDataUrl] = useState("");

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
    () => buildBackgroundImageLayerStyle(effectiveAppearance, backgroundDataUrl),
    [effectiveAppearance, backgroundDataUrl],
  );
  const shellStyle = useMemo(
    () => ({
      ...buildSurfaceCssVariables(theme.colors, effectiveAppearance),
      backgroundColor: theme.colors.bg,
      color: "var(--df-text)",
    }),
    [effectiveAppearance, theme.colors],
  );
  const hasLeftActivityItems =
    leftActivityBar.items.length > 0 || (leftActivityBar.bottomItems?.length ?? 0) > 0;
  const hasRightActivityItems =
    rightActivityBar.items.length > 0 || (rightActivityBar.bottomItems?.length ?? 0) > 0;
  const rightPanelOpen = hasRightActivityItems && Boolean(uiConfig.active_right_panel);
  const leftMobileOpen = hasLeftActivityItems && mobile.leftOpen;
  const rightMobileOpen = hasRightActivityItems && mobile.rightOpen;

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

          {hasLeftActivityItems && uiConfig.active_left_panel && (
            <>
              <div
                style={{ width: uiConfig.left_width, backgroundColor: "var(--df-bg-panel)" }}
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
                  {panelContent(uiConfig.active_left_panel)}
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
              backgroundColor: backgroundEnabled ? "transparent" : "var(--df-bg-terminal)",
            }}
          >
            <div className="flex-1 relative overflow-hidden">
              {tabsCount === 0 ? (
                <EmptyWorkspaceState
                  t={t}
                  backgroundEnabled={backgroundEnabled}
                  openChatShortcut={emptyWorkspace.openChatShortcut}
                  showCommandsShortcut={emptyWorkspace.showCommandsShortcut}
                  switchTerminalShortcut={emptyWorkspace.switchTerminalShortcut}
                  onOpenChat={emptyWorkspace.onOpenChat}
                  onShowCommands={emptyWorkspace.onShowCommands}
                  onSwitchTerminal={emptyWorkspace.onSwitchTerminal}
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
            </div>

            {bottomPanel.activePanel === "quickCmdBar" && (
              <>
                <ResizeHandle direction="vertical" onResize={bottomPanel.onQuickCmdResize} />
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

            {bottomPanel.activePanel === "serialSend" && (
              <>
                <ResizeHandle direction="vertical" onResize={bottomPanel.onSerialSendResize} />
                <div
                  style={{
                    height: bottomPanel.serialSendHeight,
                    backgroundColor: "var(--df-bg-panel)",
                  }}
                  className="shrink-0 overflow-hidden"
                >
                  <SerialSendPanel
                    serialSessionId={bottomPanel.activeSerialSessionId}
                    currentShellSessionId={bottomPanel.activeNonSerialSessionId}
                    shellSessionIds={bottomPanel.activeNonSerialSessionIds}
                    draft={bottomPanel.sendCommandDraft}
                    onDraftConsumed={bottomPanel.onSendCommandDraftConsumed}
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
                  {panelContent(uiConfig.active_right_panel)}
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

        <AboutDialog open={dialogs.aboutOpen} onClose={() => dialogs.onAboutOpenChange(false)} />

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
        <HostKeyVerifyDialog
          request={dialogs.hostKeyVerifyRequest}
          onDone={dialogs.onHostKeyVerifyDone}
        />

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
          <LockScreen hasPassword={dialogs.hasMasterPassword} onUnlock={dialogs.onUnlock} />
        )}
      </div>
    </div>
  );
}

function EmptyWorkspaceState({
  t,
  backgroundEnabled,
  openChatShortcut,
  showCommandsShortcut,
  switchTerminalShortcut,
  onOpenChat,
  onShowCommands,
  onSwitchTerminal,
}: {
  t: TFunction;
  backgroundEnabled: boolean;
  openChatShortcut: string;
  showCommandsShortcut: string;
  switchTerminalShortcut: string;
  onOpenChat: () => void;
  onShowCommands: () => void;
  onSwitchTerminal: () => void;
}) {
  const emptyWorkspaceActions = [
    {
      label: t("app.openChat"),
      shortcut: openChatShortcut,
      onClick: onOpenChat,
    },
    {
      label: t("app.showAllCommands"),
      shortcut: showCommandsShortcut,
      onClick: onShowCommands,
    },
    {
      label: t("app.switchTerminal"),
      shortcut: switchTerminalShortcut,
      onClick: onSwitchTerminal,
    },
  ];

  return (
    <div
      className="flex h-full items-center justify-center px-6"
      style={{
        backgroundColor: backgroundEnabled ? "var(--df-bg-terminal)" : undefined,
      }}
    >
      <div className="flex w-full max-w-[34rem] flex-col items-center">
        <NyaTermLogo
          aria-hidden="true"
          className="mb-9 h-64 w-64 opacity-[0.13] grayscale"
          style={{
            color: "var(--df-text-dimmed)",
            ["--grad-from" as string]: "currentColor",
            ["--grad-to" as string]: "currentColor",
          }}
        />

        <div className="grid w-fit max-w-[30rem] grid-cols-[max-content_auto] gap-x-4 gap-y-3 text-sm">
          {emptyWorkspaceActions.map((item) => (
            <button
              key={item.label}
              type="button"
              className="contents text-left"
              onClick={item.onClick}
            >
              <span
                className="justify-self-start transition-colors hover:text-[var(--df-primary)]"
                style={{ color: "var(--df-primary)" }}
              >
                {item.label}
              </span>
              <ShortcutKeys value={item.shortcut} />
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}

function ShortcutKeys({ value }: { value: string }) {
  const keys = value
    .split("+")
    .map((part) => part.trim())
    .filter(Boolean);

  if (!keys.length) return null;

  return (
    <KbdGroup className="justify-self-end text-[0.8125rem]" aria-hidden="true">
      {keys.map((key, index) => (
        <span key={key} className="inline-flex items-center gap-1">
          {index > 0 ? <span style={{ color: "var(--df-text-dimmed)" }}>+</span> : null}
          <Kbd className="h-6 min-w-7 border border-[var(--df-border)] bg-[var(--df-bg-hover)] px-1.5 text-[0.8125rem] text-[var(--df-text)] shadow-sm">
            {key}
          </Kbd>
        </span>
      ))}
    </KbdGroup>
  );
}
