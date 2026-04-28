import { openUrl } from "@tauri-apps/plugin-opener";
import type { Terminal } from "@xterm/xterm";
import { useCallback, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  MdAutoAwesome,
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
import { useTerminalAppSettings } from "@/context/AppContext";
import { MOD } from "@/hooks/useGlobalShortcuts";
import { openAIAssistant } from "@/lib/aiEvents";
import { readClipboardText } from "@/lib/clipboard";
import type { SearchEngine } from "@/types/global";
import TranslationDialog from "../dialog/terminal/TranslationDialog";
import { type QuickIconDef, SEARCH_ICONS } from "../icons";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuShortcut,
  ContextMenuSub,
  ContextMenuSubContent,
  ContextMenuSubTrigger,
  ContextMenuTrigger,
} from "../ui/context-menu";

interface TerminalContextMenuProps {
  children: React.ReactNode;
  terminalRef: React.RefObject<Terminal | null>;
  onFind: (selection?: string) => void;
}

export default function TerminalContextMenu({
  children,
  terminalRef,
  onFind,
}: TerminalContextMenuProps) {
  const { t } = useTranslation();
  const { interaction, translation, search } = useTerminalAppSettings();

  const [ctxSelection, setCtxSelection] = useState({ text: "", hasSelection: false });
  const [translateState, setTranslateState] = useState({ open: false, text: "", provider: "" });
  const pasteText = useCallback(
    (text: string) => {
      if (!text) return;
      terminalRef.current?.paste(text);
    },
    [terminalRef],
  );

  const translationProviders = [
    { id: "google", free: true },
    { id: "microsoft", free: true },
    { id: "deepl", free: false, configured: !!translation.deepl_api_key },
    {
      id: "baidu",
      free: false,
      configured: !!(translation.baidu_app_id && translation.baidu_app_key),
    },
    {
      id: "ali",
      free: false,
      configured: !!(translation.ali_app_id && translation.ali_app_key),
    },
    {
      id: "youdao",
      free: false,
      configured: !!(translation.youdao_app_id && translation.youdao_app_key),
    },
  ].filter((p) => p.free || p.configured);

  // Right-click context menu: capture selection state
  const handleContextMenu = (e: React.MouseEvent) => {
    const terminal = terminalRef.current;
    if (!terminal) return;

    const selection = terminal.getSelection();
    const hasSelection = selection.length > 0;

    // When right_click_paste is on and nothing is selected, paste directly
    // and prevent the Radix ContextMenu from opening.
    if (interaction.right_click_paste && !hasSelection) {
      e.preventDefault();
      e.stopPropagation();
      (async () => {
        try {
          const text = await readClipboardText();
          pasteText(text);
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
      const text = await readClipboardText();
      pasteText(text);
    } catch {
      /* clipboard access denied */
    }
    terminalRef.current?.focus();
  }, [pasteText, terminalRef]);

  const doCopy = useCallback(
    (text: string) => {
      navigator.clipboard.writeText(text);
      terminalRef.current?.focus();
    },
    [terminalRef],
  );

  const doSearchOnline = useCallback(
    (text: string, engine: SearchEngine) => {
      let url = `https://www.google.com/search?q=${encodeURIComponent(text)}`;

      if (engine.url_template) {
        url = engine.url_template.replace("%s", encodeURIComponent(text));
      }
      openUrl(url);
      terminalRef.current?.focus();
    },
    [terminalRef],
  );

  const doPasteSelected = useCallback(() => {
    pasteText(ctxSelection.text);
    terminalRef.current?.focus();
  }, [ctxSelection.text, pasteText, terminalRef]);

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
                  {search.custom_engines
                    ?.filter((engine) => engine.show_in_menu !== false)
                    .map((engine) => {
                      let IconComponent = null;
                      let color: string | undefined;
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
              <ContextMenuItem
                onClick={() =>
                  openAIAssistant({
                    action: "explain_selected",
                    selectedText: ctxSelection.text,
                  })
                }
              >
                <MdAutoAwesome className="text-[0.875rem] text-muted-foreground mr-2" />
                {t("ai.explainSelected")}
              </ContextMenuItem>
              <ContextMenuItem
                onClick={() =>
                  openAIAssistant({
                    action: "repair_from_selection",
                    selectedText: ctxSelection.text,
                  })
                }
              >
                <MdAutoAwesome className="text-[0.875rem] text-muted-foreground mr-2" />
                {t("ai.repairSelected")}
              </ContextMenuItem>
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
