import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { Terminal } from "@xterm/xterm";
import { useCallback, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  MdClearAll,
  MdContentCopy,
  MdContentPaste,
  MdContentPasteGo,
  MdDeleteSweep,
  MdSearch,
  MdSelectAll,
  MdTranslate,
  MdTravelExplore,
} from "react-icons/md";
import { useApp } from "@/context/AppContext";
import type { SearchEngine } from "@/types/global";
import { SEARCH_ICONS, type QuickIconDef } from "../icons";
import TranslationDialog from "../dialog/terminal/TranslationDialog";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuSub,
  ContextMenuSubContent,
  ContextMenuSubTrigger,
  ContextMenuTrigger,
  ContextMenuShortcut,
} from "../ui/context-menu";
import { MOD } from "@/hooks/useGlobalShortcuts";

interface TerminalContextMenuProps {
  children: React.ReactNode;
  sessionId: string;
  terminalRef: React.RefObject<Terminal | null>;
  onFind: (selection?: string) => void;
}

export default function TerminalContextMenu({
  children,
  sessionId,
  terminalRef,
  onFind,
}: TerminalContextMenuProps) {
  const { t } = useTranslation();
  const { appSettings } = useApp();

  const [ctxSelection, setCtxSelection] = useState({ text: "", hasSelection: false });
  const [translateState, setTranslateState] = useState({ open: false, text: "", provider: "" });

  const translationProviders = [
    { id: "google", free: true },
    { id: "microsoft", free: true },
    { id: "deepl", free: false, configured: !!appSettings.translation.deepl_api_key },
    { id: "baidu", free: false, configured: !!(appSettings.translation.baidu_app_id && appSettings.translation.baidu_app_key) },
    { id: "ali", free: false, configured: !!(appSettings.translation.ali_app_id && appSettings.translation.ali_app_key) },
    { id: "youdao", free: false, configured: !!(appSettings.translation.youdao_app_id && appSettings.translation.youdao_app_key) },
  ].filter((p) => p.free || p.configured);

  // Right-click context menu: capture selection state
  const handleContextMenu = (e: React.MouseEvent) => {
    const terminal = terminalRef.current;
    if (!terminal) return;

    const selection = terminal.getSelection();
    const hasSelection = selection.length > 0;

    // When right_click_paste is on and nothing is selected, paste directly
    // and prevent the Radix ContextMenu from opening.
    if (appSettings?.interaction?.right_click_paste && !hasSelection) {
      e.preventDefault();
      e.stopPropagation();
      (async () => {
        try {
          const text = await navigator.clipboard.readText();
          if (text) {
            invoke("write_to_session", { sessionId, data: text }).catch(() => { });
          }
        } catch {
          /* clipboard access denied */
        }
        terminal.focus();
      })();
      return;
    }

    setCtxSelection({ text: selection, hasSelection });
  };

  const doPaste = useCallback(async () => {
    try {
      const text = await navigator.clipboard.readText();
      if (text) {
        invoke("write_to_session", { sessionId, data: text }).catch(() => { });
      }
    } catch {
      /* clipboard access denied */
    }
    terminalRef.current?.focus();
  }, [sessionId, terminalRef]);

  const doCopy = useCallback(
    (text: string) => {
      navigator.clipboard.writeText(text);
      terminalRef.current?.focus();
    },
    [terminalRef],
  );

  const doSearchOnline = useCallback(
    (text: string, engine?: SearchEngine) => {
      const searchSettings = appSettings?.search;
      let url = `https://www.google.com/search?q=${encodeURIComponent(text)}`;

      if (engine && engine.url_template) {
        url = engine.url_template.replace("%s", encodeURIComponent(text));
      } else if (searchSettings && searchSettings.custom_engines.length > 0) {
        const defaultEngine = searchSettings.custom_engines[0];
        if (defaultEngine?.url_template) {
          url = defaultEngine.url_template.replace("%s", encodeURIComponent(text));
        }
      }
      openUrl(url);
      terminalRef.current?.focus();
    },
    [appSettings?.search, terminalRef],
  );

  const doPasteSelected = useCallback(() => {
    if (ctxSelection.text) {
      invoke("write_to_session", { sessionId, data: ctxSelection.text }).catch(() => { });
    }
    terminalRef.current?.focus();
  }, [sessionId, ctxSelection.text, terminalRef]);

  const doClearScreen = useCallback(() => {
    terminalRef.current?.clear();
    terminalRef.current?.focus();
  }, [terminalRef]);

  const doClearAll = useCallback(() => {
    terminalRef.current?.reset();
    terminalRef.current?.focus();
  }, [terminalRef]);

  const doSelectAll = useCallback(() => {
    terminalRef.current?.selectAll();
    terminalRef.current?.focus();
  }, [terminalRef]);

  return (
    <>
      <ContextMenu>
        <ContextMenuTrigger asChild>
          <div className="h-full w-full" onContextMenu={handleContextMenu}>
            {children}
          </div>
        </ContextMenuTrigger>
        <ContextMenuContent className="min-w-[200px]">
          {ctxSelection.hasSelection ? (
            <>
              <ContextMenuItem onClick={() => doCopy(ctxSelection.text)}>
                <MdContentCopy className="text-[0.875rem] text-muted-foreground mr-2" />
                {t("terminalCtx.copy")}
                <ContextMenuShortcut>{MOD}+Shift+C</ContextMenuShortcut>
              </ContextMenuItem>
              <ContextMenuItem onClick={() => onFind(ctxSelection.text)}>
                <MdSearch className="text-[0.875rem] text-muted-foreground mr-2" />
                {t("terminalCtx.find")}
                <ContextMenuShortcut>{MOD}+Shift+F</ContextMenuShortcut>
              </ContextMenuItem>
              <ContextMenuSub>
                <ContextMenuSubTrigger>
                  <MdTravelExplore className="text-[0.875rem] text-muted-foreground mr-2" />
                  {t("terminalCtx.searchOnline")}
                </ContextMenuSubTrigger>
                <ContextMenuSubContent>
                  {appSettings?.search?.custom_engines?.map((engine) => {
                    let IconComponent = null;
                    let color = undefined;
                    if (engine.icon && SEARCH_ICONS[engine.icon]) {
                      const iconDef = SEARCH_ICONS[engine.icon] as QuickIconDef;
                      IconComponent = iconDef.icon;
                      color = iconDef.color;
                    }

                    return (
                      <ContextMenuItem
                        key={engine.name}
                        onClick={() => doSearchOnline(ctxSelection.text, engine)}
                      >
                        {IconComponent && (
                          <IconComponent className="text-[0.875rem] mr-2" style={{ color }} />
                        )}
                        {engine.name}
                      </ContextMenuItem>
                    );
                  })}
                </ContextMenuSubContent>
              </ContextMenuSub>
              {translationProviders.length > 0 && (
                <ContextMenuSub>
                  <ContextMenuSubTrigger>
                    <MdTranslate className="text-[0.875rem] text-muted-foreground mr-2" />
                    {t("terminalCtx.translate")}
                  </ContextMenuSubTrigger>
                  <ContextMenuSubContent>
                    {translationProviders.map((p) => (
                      <ContextMenuItem
                        key={p.id}
                        onClick={() =>
                          setTranslateState({ open: true, text: ctxSelection.text, provider: p.id })
                        }
                      >
                        {t(`translation.${p.id}`)}
                      </ContextMenuItem>
                    ))}
                  </ContextMenuSubContent>
                </ContextMenuSub>
              )}
              <ContextMenuSeparator />
              <ContextMenuItem onClick={doPaste}>
                <MdContentPaste className="text-[0.875rem] text-muted-foreground mr-2" />
                {t("terminalCtx.paste")}
                <ContextMenuShortcut>{MOD}+Shift+V</ContextMenuShortcut>
              </ContextMenuItem>
              <ContextMenuItem onClick={doPasteSelected}>
                <MdContentPasteGo className="text-[0.875rem] text-muted-foreground mr-2" />
                {t("terminalCtx.pasteSelectedText")}
                <ContextMenuShortcut>{MOD}+Shift+X</ContextMenuShortcut>
              </ContextMenuItem>
            </>
          ) : (
            <>
              <ContextMenuItem onClick={doPaste}>
                <MdContentPaste className="text-[0.875rem] text-muted-foreground mr-2" />
                {t("terminalCtx.paste")}
                <ContextMenuShortcut>{MOD}+Shift+V</ContextMenuShortcut>
              </ContextMenuItem>
              <ContextMenuItem onClick={() => onFind()}>
                <MdSearch className="text-[0.875rem] text-muted-foreground mr-2" />
                {t("terminalCtx.find")}
                <ContextMenuShortcut>{MOD}+Shift+F</ContextMenuShortcut>
              </ContextMenuItem>
            </>
          )}
          <ContextMenuSeparator />
          <ContextMenuItem onClick={doClearScreen}>
            <MdClearAll className="text-[0.875rem] text-muted-foreground mr-2" />
            {t("terminalCtx.clearScreen")}
            <ContextMenuShortcut>{MOD}+Shift+K</ContextMenuShortcut>
          </ContextMenuItem>
          <ContextMenuItem onClick={doClearAll}>
            <MdDeleteSweep className="text-[0.875rem] text-muted-foreground mr-2" />
            {t("terminalCtx.clearAll")}
          </ContextMenuItem>
          <ContextMenuSeparator />
          <ContextMenuItem onClick={doSelectAll}>
            <MdSelectAll className="text-[0.875rem] text-muted-foreground mr-2" />
            {t("terminalCtx.selectAll")}
            <ContextMenuShortcut>{MOD}+Shift+A</ContextMenuShortcut>
          </ContextMenuItem>
        </ContextMenuContent>
      </ContextMenu>
      <TranslationDialog
        open={translateState.open}
        onClose={() => setTranslateState({ open: false, text: "", provider: "" })}
        text={translateState.text}
        provider={translateState.provider}
      />
    </>
  );
}
