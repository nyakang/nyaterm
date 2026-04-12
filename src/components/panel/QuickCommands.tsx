import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  MdAdd,
  MdBolt,
  MdDelete,
  MdEdit,
  MdKeyboardReturn,
  MdPushPin,
  MdSearch,
  MdTerminal,
} from "react-icons/md";
import { Button } from "@/components/ui/button";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import type { QuickCommand, QuickCommandCategory, QuickCommandsConfig } from "@/types/global";
import { openQuickCommand } from "../../lib/windowManager";
import VariablePromptDialog, {
  parseCommandVariables,
  type VariableDef,
} from "../dialog/terminal/VariablePromptDialog";
import { QUICK_ICONS } from "../icons";

interface QuickCommandsProps {
  onSend: (command: string, execute?: boolean) => void;
}

const COLOR_DOT: Record<string, string> = {
  default: "bg-muted-foreground",
  red: "bg-red-500",
  green: "bg-green-500",
  blue: "bg-blue-500",
  yellow: "bg-yellow-500",
  purple: "bg-purple-500",
};

function QuickCommands({ onSend }: QuickCommandsProps) {
  const { t } = useTranslation();
  const [commands, setCommands] = useState<QuickCommand[]>([]);
  const [savedCategories, setSavedCategories] = useState<QuickCommandCategory[]>([]);
  const loaded = useRef(false);
  const saveTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // UI State
  const [search, setSearch] = useState("");
  const [selectedCategory, setSelectedCategory] = useState<string>("all");

  // Variable Prompt State
  const [promptCmd, setPromptCmd] = useState<QuickCommand | null>(null);
  const [promptVars, setPromptVars] = useState<VariableDef[]>([]);

  const [isContextMenuOpen, setIsContextMenuOpen] = useState(false);

  // Load from backend on mount
  useEffect(() => {
    invoke<QuickCommandsConfig>("get_quick_commands")
      .then((cfg) => {
        setCommands(cfg.commands || []);
        setSavedCategories(cfg.categories || []);
        loaded.current = true;
      })
      .catch(() => {
        loaded.current = true;
      });
  }, []);

  // Debounced save to backend on change
  useEffect(() => {
    if (!loaded.current) return;
    if (saveTimerRef.current) clearTimeout(saveTimerRef.current);
    saveTimerRef.current = setTimeout(() => {
      invoke("save_quick_commands", { config: { commands, categories: savedCategories } }).catch(
        () => {},
      );
    }, 300);
    return () => {
      if (saveTimerRef.current) clearTimeout(saveTimerRef.current);
    };
  }, [commands, savedCategories]);

  const handleDelete = useCallback((id: string) => {
    setCommands((prev) => prev.filter((c) => c.id !== id));
  }, []);

  // Listen for quick-command-saved events from child window
  useEffect(() => {
    const unsub = listen<{ command: QuickCommand; newCategory?: QuickCommandCategory }>(
      "quick-command-saved",
      (event) => {
        const { command: cmd, newCategory } = event.payload;
        setCommands((prev) => {
          const exists = prev.some((c) => c.id === cmd.id);
          return exists ? prev.map((c) => (c.id === cmd.id ? cmd : c)) : [...prev, cmd];
        });
        if (newCategory) {
          setSavedCategories((prev) =>
            prev.find((c) => c.id === newCategory.id) ? prev : [...prev, newCategory],
          );
        }
      },
    );
    return () => {
      unsub.then((fn) => fn());
    };
  }, []);

  const handleCommandClick = useCallback(
    (cmd: QuickCommand) => {
      const vars = parseCommandVariables(cmd.command);

      if (vars.length > 0) {
        // Need user input
        setPromptCmd(cmd);
        setPromptVars(vars);
      } else {
        // All resolved or no variables
        onSend(cmd.command, cmd.execution_mode !== "append");
      }
    },
    [onSend],
  );

  const handlePromptSubmit = useCallback(
    (resolvedCommand: string) => {
      if (promptCmd) {
        onSend(resolvedCommand, promptCmd.execution_mode !== "append");
        setPromptCmd(null);
      }
    },
    [promptCmd, onSend],
  );

  // Derived state for categories and filtering
  const allCategories = useMemo(() => {
    const catsMap = new Map<string, QuickCommandCategory>();
    savedCategories.forEach((c) => {
      catsMap.set(c.id, c);
    });
    commands.forEach((c) => {
      if (c.category_id && !catsMap.has(c.category_id)) {
        catsMap.set(c.category_id, { id: c.category_id, name: c.category_id });
      }
    });
    return Array.from(catsMap.values()).sort((a, b) => a.name.localeCompare(b.name));
  }, [commands, savedCategories]);

  const filteredCommands = useMemo(() => {
    let filtered = commands;

    // Category filter
    if (selectedCategory === "uncategorized") {
      filtered = filtered.filter((c) => !c.category_id);
    } else if (selectedCategory !== "all") {
      filtered = filtered.filter((c) => c.category_id === selectedCategory);
    }

    // Search filter
    if (search.trim()) {
      const q = search.trim().toLowerCase();
      filtered = filtered.filter(
        (c) =>
          c.label.toLowerCase().includes(q) ||
          c.command.toLowerCase().includes(q) ||
          c.description?.toLowerCase().includes(q),
      );
    }

    // Sort: Pinned first
    filtered.sort((a, b) => {
      if (a.pinned && !b.pinned) return -1;
      if (!a.pinned && b.pinned) return 1;
      return 0; // maintain original order within pinned/unpinned
    });

    return filtered;
  }, [commands, search, selectedCategory]);

  return (
    <div className="h-full flex flex-col" style={{ backgroundColor: "var(--df-bg-panel)" }}>
      {/* Toolbar */}
      <div
        className="flex items-center gap-2 px-2 py-1 border-b shrink-0"
        style={{ borderColor: "var(--df-border)" }}
      >
        <MdBolt className="text-[0.9375rem] shrink-0" style={{ color: "var(--df-text-dimmed)" }} />

        {/* Search */}
        <div className="relative shrink-0 flex-1 sm:flex-none">
          <Input
            className="w-full sm:w-32 focus-visible:w-full placeholder:text-xs sm:focus-visible:w-64 bg-transparent h-6 pl-6 pr-2 py-0 border-input transition-all duration-300 shadow-none focus-visible:ring-0 focus-visible:ring-offset-0"
            placeholder={t("quickCommands.search")}
            value={search}
            onChange={(e) => setSearch(e.target.value)}
          />
          <MdSearch className="text-[0.6875rem] absolute left-1.5 top-1/2 -translate-y-1/2 text-muted-foreground pointer-events-none" />
        </div>

        <div className="flex-1 hidden sm:block" />

        {/* Categories */}
        <Select value={selectedCategory} onValueChange={setSelectedCategory}>
          <SelectTrigger className="h-6 text-[0.6875rem] w-auto border min-w-[100px] max-w-[150px] shadow-none py-0 px-2 rounded bg-transparent focus:ring-0">
            <SelectValue placeholder={t("quickCommands.allCategories")} />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="all" className="text-[0.6875rem]">
              {t("quickCommands.allCategories")}
            </SelectItem>
            {allCategories.map((c) => (
              <SelectItem key={c.id} value={c.id} className="text-[0.6875rem]">
                {c.name}
              </SelectItem>
            ))}
            <SelectItem value="uncategorized" className="text-[0.6875rem]">
              {t("quickCommands.uncategorized")}
            </SelectItem>
          </SelectContent>
        </Select>

        {/* Add Button */}
        <Button
          variant="ghost"
          size="icon-xs"
          className="bg-muted/50 hover:bg-muted text-foreground transition-colors"
          title={t("quickCommands.addCommand")}
          onClick={() => openQuickCommand()}
        >
          <MdAdd className="text-[0.875rem]" />
        </Button>
      </div>

      {/* Commands List */}
      <TooltipProvider delayDuration={500}>
        <div className="flex-1 overflow-y-auto overflow-x-hidden terminal-scroll p-1.5">
          <div className="flex flex-wrap gap-1.5 content-start">
            {filteredCommands.length === 0 ? (
              <div className="flex flex-col items-center justify-center w-full mt-10 p-4 border border-dashed rounded-lg text-muted-foreground opacity-70">
                <MdTerminal className="text-2xl mb-2" />
                <span className="text-xs mb-3">{t("quickCommands.noCommandsFound")}</span>
                <Button
                  variant="outline"
                  size="sm"
                  className="h-7 text-xs bg-muted/20 hover:bg-muted"
                  onClick={() => openQuickCommand()}
                >
                  <MdAdd className="mr-1 text-sm" />
                  {t("quickCommands.addCommand")}
                </Button>
              </div>
            ) : (
              filteredCommands.map((cmd) => {
                const dotColor = COLOR_DOT[cmd.color_tag || "default"] || COLOR_DOT.default;

                return (
                  <ContextMenu key={cmd.id} onOpenChange={setIsContextMenuOpen}>
                    <Tooltip open={isContextMenuOpen ? false : undefined}>
                      <ContextMenuTrigger asChild>
                        <TooltipTrigger asChild>
                          <button
                            className="flex items-center gap-1.5 rounded-md px-2 py-1 text-[0.6875rem] font-medium cursor-pointer transition-colors border bg-muted/20 hover:bg-muted/50 shrink-0 max-w-full text-foreground/80 hover:text-foreground"
                            onClick={() => handleCommandClick(cmd)}
                          >
                            {cmd.icon_tag && QUICK_ICONS[cmd.icon_tag] ? (
                              (() => {
                                const iconDef = QUICK_ICONS[cmd.icon_tag!];
                                return (
                                  <iconDef.icon
                                    className="text-[0.75rem] opacity-80"
                                    style={{ color: iconDef.color }}
                                  />
                                );
                              })()
                            ) : (
                              <span
                                className={`inline-block w-2 h-2 rounded-full shrink-0 ${dotColor}`}
                              />
                            )}
                            {cmd.pinned && <MdPushPin className="text-[0.625rem] opacity-60" />}
                            <span className="whitespace-nowrap overflow-hidden text-ellipsis">
                              {cmd.label}
                            </span>
                          </button>
                        </TooltipTrigger>
                      </ContextMenuTrigger>
                      <TooltipContent
                        side="top"
                        align="start"
                        showArrow={false}
                        className="w-[320px] p-0 shadow-2xl border-border/60 bg-popover/95 backdrop-blur-md rounded-xl overflow-hidden"
                      >
                        <div className="flex flex-col">
                          <div className="flex flex-col gap-1.5 p-3 bg-muted/30 border-b border-border/30">
                            <div className="flex items-center gap-2">
                              {cmd.icon_tag && QUICK_ICONS[cmd.icon_tag] ? (
                                (() => {
                                  const iconDef = QUICK_ICONS[cmd.icon_tag!];
                                  return (
                                    <iconDef.icon
                                      className="text-[0.875rem] opacity-80 shrink-0"
                                      style={{ color: iconDef.color }}
                                    />
                                  );
                                })()
                              ) : (
                                <span
                                  className={`inline-block w-2.5 h-2.5 rounded-full shrink-0 ${dotColor}`}
                                />
                              )}
                              <span className="font-semibold text-sm text-foreground truncate">
                                {cmd.label}
                              </span>
                              <div className="flex-1" />
                              {cmd.category_id && (
                                <span className="text-[0.625rem] px-2 py-0.5 rounded-full bg-primary/10 text-primary font-medium border border-primary/20">
                                  {allCategories.find((c) => c.id === cmd.category_id)?.name ||
                                    cmd.category_id}
                                </span>
                              )}
                            </div>
                            <div className="flex items-center gap-1.5 text-[0.6875rem] text-muted-foreground mt-0.5">
                              {cmd.execution_mode === "append" ? (
                                <MdKeyboardReturn className="text-[0.75rem]" />
                              ) : (
                                <MdBolt className="text-[0.75rem]" />
                              )}
                              {cmd.execution_mode === "append"
                                ? t("quickCommands.appendOnly")
                                : t("quickCommands.executeImmediately")}
                            </div>
                          </div>

                          <div className="p-3 flex flex-col gap-3">
                            {cmd.description && (
                              <div className="text-xs text-muted-foreground/90 leading-relaxed">
                                {cmd.description}
                              </div>
                            )}

                            <div className="relative group">
                              <div className="absolute -inset-0.5 bg-gradient-to-r from-primary/10 to-transparent opacity-0 group-hover:opacity-100 transition-opacity rounded-md blur-sm" />
                              <pre
                                className="relative text-[0.6875rem] font-mono text-foreground/80 bg-background/50 border border-border/40 p-2.5 rounded-md whitespace-pre-wrap break-all max-h-[120px] overflow-y-auto terminal-scroll custom-scrollbar"
                                title={cmd.command}
                              >
                                {cmd.command}
                              </pre>
                            </div>
                          </div>
                        </div>
                      </TooltipContent>
                    </Tooltip>

                    <ContextMenuContent className="min-w-[120px]">
                      <ContextMenuItem
                        className="text-xs gap-2"
                        onClick={() => openQuickCommand(JSON.stringify(cmd))}
                      >
                        <MdEdit className="text-[0.875rem]" />
                        {t("quickCommands.edit")}
                      </ContextMenuItem>
                      <ContextMenuItem
                        className="text-xs gap-2 text-destructive focus:text-destructive"
                        onClick={() => handleDelete(cmd.id)}
                      >
                        <MdDelete className="text-[0.875rem]" />
                        {t("quickCommands.delete")}
                      </ContextMenuItem>
                    </ContextMenuContent>
                  </ContextMenu>
                );
              })
            )}
          </div>
        </div>
      </TooltipProvider>
      {promptCmd && (
        <VariablePromptDialog
          open={!!promptCmd}
          command={promptCmd.command}
          variables={promptVars}
          onCancel={() => {
            setPromptCmd(null);
          }}
          onSubmit={handlePromptSubmit}
        />
      )}
    </div>
  );
}

export default memo(QuickCommands);
