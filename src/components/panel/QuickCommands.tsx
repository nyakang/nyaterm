import { listen } from "@tauri-apps/api/event";
import { MoreHorizontalIcon } from "lucide-react";
import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { BiImport } from "react-icons/bi";
import { BsFillSendPlusFill } from "react-icons/bs";
import {
  MdAdd,
  MdAutoAwesome,
  MdBolt,
  MdClose,
  MdDelete,
  MdEdit,
  MdKeyboardReturn,
  MdPushPin,
  MdSearch,
  MdSend,
  MdSort,
  MdTerminal,
  MdVisibility,
} from "react-icons/md";
import DeleteQuickCommandCategoryDialog from "@/components/dialog/quick-commands/DeleteQuickCommandCategoryDialog";
import DeleteQuickCommandDialog from "@/components/dialog/quick-commands/DeleteQuickCommandDialog";
import QuickCommandsImportDialog from "@/components/dialog/quick-commands/QuickCommandsImportDialog";
import RenameQuickCommandCategoryDialog from "@/components/dialog/quick-commands/RenameQuickCommandCategoryDialog";
import PanelHeader from "@/components/layout/PanelHeader";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import { openAIAssistant } from "@/lib/aiEvents";
import { invoke } from "@/lib/invoke";
import type {
  QuickCommand,
  QuickCommandCategory,
  QuickCommandImportResult,
  QuickCommandsConfig,
} from "@/types/global";
import { openQuickCommand } from "../../lib/windowManager";
import VariablePromptDialog, {
  parseCommandVariables,
  type VariableDef,
} from "../dialog/terminal/VariablePromptDialog";
import { QUICK_ICONS } from "../icons";

interface QuickCommandsProps {
  onSend: (command: string, execute?: boolean) => void;
  onSendToAll?: (command: string) => void;
}

const COLOR_DOT: Record<string, string> = {
  default: "bg-muted-foreground",
  red: "bg-red-500",
  green: "bg-green-500",
  blue: "bg-blue-500",
  yellow: "bg-yellow-500",
  purple: "bg-purple-500",
};

function QuickCommands({ onSend, onSendToAll }: QuickCommandsProps) {
  const { t } = useTranslation();
  const [commands, setCommands] = useState<QuickCommand[]>([]);
  const [savedCategories, setSavedCategories] = useState<QuickCommandCategory[]>([]);
  const loaded = useRef(false);
  const saveTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const skipNextSaveRef = useRef(false);

  // UI State
  const [search, setSearch] = useState("");
  const [selectedCategory, setSelectedCategory] = useState<string>("all");
  const [sortMode, setSortMode] = useState<"created" | "name" | "useCount">("created");
  const [aiPrompt, setAiPrompt] = useState("");
  const [aiPopoverOpen, setAiPopoverOpen] = useState(false);
  const [importDialogOpen, setImportDialogOpen] = useState(false);
  const [categoryToRename, setCategoryToRename] = useState<QuickCommandCategory | null>(null);
  const [categoryToDelete, setCategoryToDelete] = useState<QuickCommandCategory | null>(null);
  const [commandToDelete, setCommandToDelete] = useState<QuickCommand | null>(null);

  // Variable Prompt State
  const [promptCmd, setPromptCmd] = useState<QuickCommand | null>(null);
  const [promptVars, setPromptVars] = useState<VariableDef[]>([]);
  const [promptSendToAll, setPromptSendToAll] = useState(false);

  const loadQuickCommands = useCallback(async () => {
    const cfg = await invoke<QuickCommandsConfig>("get_quick_commands");
    skipNextSaveRef.current = true;
    setCommands(cfg.commands || []);
    setSavedCategories(cfg.categories || []);
    loaded.current = true;
  }, []);

  // Load from backend on mount
  useEffect(() => {
    loadQuickCommands().catch(() => {
      loaded.current = true;
    });
  }, [loadQuickCommands]);

  // Debounced save to backend on change
  useEffect(() => {
    if (!loaded.current) return;
    if (skipNextSaveRef.current) {
      skipNextSaveRef.current = false;
      return;
    }
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

  const handleConfirmDeleteCommand = useCallback(() => {
    if (!commandToDelete) return;
    handleDelete(commandToDelete.id);
    setCommandToDelete(null);
  }, [commandToDelete, handleDelete]);

  const handleConfirmDeleteCategory = useCallback(() => {
    if (!categoryToDelete) return;

    const categoryId = categoryToDelete.id;
    setSavedCategories((prev) => prev.filter((category) => category.id !== categoryId));
    setCommands((prev) => prev.filter((cmd) => cmd.category_id !== categoryId));
    setSelectedCategory((current) => (current === categoryId ? "all" : current));
    setCategoryToDelete(null);
  }, [categoryToDelete]);

  const handleConfirmRenameCategory = useCallback(
    (name: string) => {
      if (!categoryToRename) return;

      const renamedCategory = { ...categoryToRename, name };
      setSavedCategories((prev) => {
        const exists = prev.some((category) => category.id === renamedCategory.id);
        return exists
          ? prev.map((category) =>
              category.id === renamedCategory.id ? renamedCategory : category,
            )
          : [...prev, renamedCategory];
      });
      setCategoryToRename(null);
    },
    [categoryToRename],
  );

  const incrementUseCount = useCallback((id: string) => {
    setCommands((prev) =>
      prev.map((c) =>
        c.id === id ? { ...c, use_count: (c.use_count ?? 0) + 1, updated_at: Date.now() } : c,
      ),
    );
    invoke("increment_quick_command_use_count", { id }).catch(() => {});
  }, []);

  // Listen for quick-command-saved events from child window
  useEffect(() => {
    const unsub = listen<{ command: QuickCommand; newCategory?: QuickCommandCategory }>(
      "quick-command-saved",
      (event) => {
        const { command: cmd, newCategory } = event.payload;
        skipNextSaveRef.current = true;
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
      incrementUseCount(cmd.id);
      const vars = parseCommandVariables(cmd.command);

      if (vars.length > 0) {
        setPromptCmd(cmd);
        setPromptVars(vars);
      } else {
        onSend(cmd.command, cmd.execution_mode !== "append");
      }
    },
    [onSend, incrementUseCount],
  );

  const handleSendToAll = useCallback(
    (cmd: QuickCommand) => {
      if (!onSendToAll) return;
      incrementUseCount(cmd.id);
      const vars = parseCommandVariables(cmd.command);
      if (vars.length > 0) {
        setPromptCmd(cmd);
        setPromptVars(vars);
        setPromptSendToAll(true);
      } else {
        onSendToAll(cmd.command);
      }
    },
    [onSendToAll, incrementUseCount],
  );

  const handlePromptSubmit = useCallback(
    (resolvedCommand: string) => {
      if (promptCmd) {
        if (promptSendToAll && onSendToAll) {
          onSendToAll(resolvedCommand);
        } else {
          onSend(resolvedCommand, promptCmd.execution_mode !== "append");
        }
        setPromptCmd(null);
        setPromptSendToAll(false);
      }
    },
    [promptCmd, promptSendToAll, onSend, onSendToAll],
  );

  const handleAiPromptSubmit = useCallback(() => {
    const userInput = aiPrompt.trim();
    if (!userInput) return;
    setAiPrompt("");
    setAiPopoverOpen(false);
    openAIAssistant({ action: "generate_command", userInput });
  }, [aiPrompt]);

  const handleImported = useCallback(
    (_result: QuickCommandImportResult) => {
      void loadQuickCommands();
    },
    [loadQuickCommands],
  );

  useEffect(() => {
    const unsub = listen("quick-commands-changed", () => {
      void loadQuickCommands();
    });
    return () => {
      unsub.then((fn) => fn());
    };
  }, [loadQuickCommands]);

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

  const categoryItems = useMemo(() => {
    const commandCounts = new Map<string, number>();
    let uncategorizedCount = 0;

    commands.forEach((cmd) => {
      if (!cmd.category_id) {
        uncategorizedCount += 1;
        return;
      }
      commandCounts.set(cmd.category_id, (commandCounts.get(cmd.category_id) ?? 0) + 1);
    });

    return [
      {
        id: "all",
        label: t("quickCommands.allCategories"),
        count: commands.length,
        isManageable: false,
      },
      ...allCategories.map((category) => ({
        id: category.id,
        label: category.name,
        count: commandCounts.get(category.id) ?? 0,
        isManageable: savedCategories.some((savedCategory) => savedCategory.id === category.id),
      })),
      {
        id: "uncategorized",
        label: t("quickCommands.uncategorized"),
        count: uncategorizedCount,
        isManageable: false,
      },
    ];
  }, [allCategories, commands, savedCategories, t]);

  const filteredCommands = useMemo(() => {
    let filtered = commands;

    if (selectedCategory === "uncategorized") {
      filtered = filtered.filter((c) => !c.category_id);
    } else if (selectedCategory !== "all") {
      filtered = filtered.filter((c) => c.category_id === selectedCategory);
    }

    if (search.trim()) {
      const q = search.trim().toLowerCase();
      filtered = filtered.filter(
        (c) =>
          c.label.toLowerCase().includes(q) ||
          c.command.toLowerCase().includes(q) ||
          c.description?.toLowerCase().includes(q),
      );
    }

    const sorted = [...filtered];
    sorted.sort((a, b) => {
      const pinDiff = (b.pinned ? 1 : 0) - (a.pinned ? 1 : 0);
      if (pinDiff !== 0) return pinDiff;

      switch (sortMode) {
        case "name":
          return a.label.localeCompare(b.label);
        case "useCount":
          return (b.use_count ?? 0) - (a.use_count ?? 0);
        default:
          return (
            (a.created_at ?? a.updated_at ?? Number.MAX_SAFE_INTEGER) -
            (b.created_at ?? b.updated_at ?? Number.MAX_SAFE_INTEGER)
          );
      }
    });

    return sorted;
  }, [commands, search, selectedCategory, sortMode]);

  const searchQuery = search.trim();
  const hasActiveFilters = searchQuery.length > 0 || selectedCategory !== "all";
  const headerMetaText =
    hasActiveFilters && commands.length > 0
      ? `${filteredCommands.length}/${commands.length}`
      : `${commands.length}`;
  const categoryToDeleteCommandCount = categoryToDelete
    ? commands.filter((cmd) => cmd.category_id === categoryToDelete.id).length
    : 0;
  const headerControlClassName =
    "h-7 border-0 bg-[var(--df-bg-hover)] py-1 text-xs text-[var(--df-text)] shadow-none";

  return (
    <TooltipProvider delayDuration={500}>
      <div
        className="nyaterm-wallpaper-transparent-surface h-full flex flex-col"
        style={{ backgroundColor: "var(--df-bg-panel)" }}
      >
        <PanelHeader
          title={t("panel.quickCommands")}
          meta={
            commands.length > 0 ? (
              <span className="text-[0.6875rem]" style={{ color: "var(--df-text-dimmed)" }}>
                {headerMetaText}
              </span>
            ) : null
          }
          actions={
            <>
              <div className="flex min-w-0 items-center gap-1">
                <div className="relative w-[9rem] shrink-0 transition-colors focus-within:text-[var(--df-primary)] text-[var(--df-text-dimmed)]">
                  <MdSearch className="pointer-events-none absolute left-2 top-1/2 -translate-y-1/2 text-[0.875rem]" />
                  <Input
                    value={search}
                    onChange={(event) => setSearch(event.target.value)}
                    placeholder={t("quickCommands.search")}
                    className={`${headerControlClassName} pl-7 pr-7 placeholder:text-[var(--df-text-dimmed)] focus-visible:ring-1 focus-visible:ring-[var(--df-primary)] focus-visible:bg-transparent`}
                  />
                  {search && (
                    <button
                      className="absolute right-1.5 top-1/2 -translate-y-1/2 rounded p-0.5 transition-colors hover:text-[var(--df-text)] text-[var(--df-text-dimmed)]"
                      onClick={() => setSearch("")}
                    >
                      <MdClose className="text-[0.75rem]" />
                    </button>
                  )}
                </div>
              </div>

              <span aria-hidden className="mx-1 h-4 w-px shrink-0 bg-border/50" />

              <DropdownMenu>
                <Tooltip>
                  <TooltipTrigger asChild>
                    <DropdownMenuTrigger asChild>
                      <Button
                        variant="ghost"
                        size="icon-sm"
                        className="h-6 w-6 shrink-0 rounded-md p-0 transition-colors hover:bg-[var(--df-bg-hover)]"
                        style={{
                          color:
                            sortMode !== "created" ? "var(--df-primary)" : "var(--df-text-muted)",
                        }}
                        aria-label={t("quickCommands.sort")}
                      >
                        <MdSort className="text-[1.05rem]" />
                      </Button>
                    </DropdownMenuTrigger>
                  </TooltipTrigger>
                  <TooltipContent side="top">{t("quickCommands.sort")}</TooltipContent>
                </Tooltip>
                <DropdownMenuContent align="end">
                  <DropdownMenuRadioGroup
                    value={sortMode}
                    onValueChange={(v) => setSortMode(v as typeof sortMode)}
                  >
                    <DropdownMenuRadioItem value="created" className="text-xs">
                      {t("quickCommands.sortByCreated")}
                    </DropdownMenuRadioItem>
                    <DropdownMenuRadioItem value="name" className="text-xs">
                      {t("quickCommands.sortByName")}
                    </DropdownMenuRadioItem>
                    <DropdownMenuRadioItem value="useCount" className="text-xs">
                      {t("quickCommands.sortByUseCount")}
                    </DropdownMenuRadioItem>
                  </DropdownMenuRadioGroup>
                </DropdownMenuContent>
              </DropdownMenu>

              <span aria-hidden className="mx-1 h-4 w-px shrink-0 bg-border/50" />

              <div className="flex items-center gap-1">
                <Tooltip>
                  <TooltipTrigger asChild>
                    <Button
                      variant="ghost"
                      size="icon-sm"
                      className="h-6 w-6 shrink-0 rounded-md p-0 transition-colors hover:bg-[var(--df-bg-hover)]"
                      style={{ color: "var(--df-text-muted)" }}
                      aria-label={t("quickCommands.addCommand")}
                      onClick={() => openQuickCommand()}
                    >
                      <MdAdd className="text-[1.05rem]" />
                    </Button>
                  </TooltipTrigger>
                  <TooltipContent side="top">{t("quickCommands.addCommand")}</TooltipContent>
                </Tooltip>

                <Tooltip>
                  <TooltipTrigger asChild>
                    <Button
                      variant="ghost"
                      size="icon-sm"
                      className="h-6 w-6 shrink-0 rounded-md p-0 transition-colors hover:bg-[var(--df-bg-hover)]"
                      style={{ color: "var(--df-text-muted)" }}
                      aria-label={t("quickCommands.import")}
                      onClick={() => setImportDialogOpen(true)}
                    >
                      <BiImport className="text-[1.05rem]" />
                    </Button>
                  </TooltipTrigger>
                  <TooltipContent side="top">{t("quickCommands.import")}</TooltipContent>
                </Tooltip>
              </div>

              <span aria-hidden className="mx-1 h-4 w-px shrink-0 bg-border/50" />

              <Popover open={aiPopoverOpen} onOpenChange={setAiPopoverOpen}>
                <Tooltip>
                  <TooltipTrigger asChild>
                    <PopoverTrigger asChild>
                      <Button
                        variant="ghost"
                        size="icon-sm"
                        className="h-6 w-6 shrink-0 rounded-md p-0 transition-colors hover:bg-[var(--df-bg-hover)]"
                        style={{ color: "var(--df-text-muted)" }}
                        aria-label={t("ai.generateCommand")}
                      >
                        <MdAutoAwesome className="text-[1.05rem]" />
                      </Button>
                    </PopoverTrigger>
                  </TooltipTrigger>
                  <TooltipContent side="top">{t("ai.generateCommand")}</TooltipContent>
                </Tooltip>
                <PopoverContent align="end" className="w-80 p-3">
                  <div className="space-y-2">
                    <div className="text-xs font-medium">{t("ai.generateCommand")}</div>
                    <Input
                      value={aiPrompt}
                      onChange={(event) => setAiPrompt(event.target.value)}
                      placeholder={t("ai.quickPrompt")}
                      className="h-8 text-xs"
                      onKeyDown={(event) => {
                        if (event.key === "Enter") {
                          event.preventDefault();
                          handleAiPromptSubmit();
                        }
                      }}
                    />
                    <div className="flex justify-end">
                      <Button size="xs" disabled={!aiPrompt.trim()} onClick={handleAiPromptSubmit}>
                        <MdAutoAwesome />
                        {t("ai.generate")}
                      </Button>
                    </div>
                  </div>
                </PopoverContent>
              </Popover>
            </>
          }
        />

        <div className="flex min-h-0 flex-1">
          <aside className="w-[11rem] shrink-0 overflow-y-auto overflow-x-hidden border-r border-border/35 p-1.5 terminal-scroll">
            <div className="flex flex-col gap-1">
              {categoryItems.map((category) => {
                const active = selectedCategory === category.id;
                const savedCategory = category.isManageable
                  ? savedCategories.find((item) => item.id === category.id)
                  : undefined;

                return (
                  <ContextMenu key={category.id}>
                    <ContextMenuTrigger asChild disabled={!savedCategory}>
                      <button
                        type="button"
                        className="group flex h-8 w-full min-w-0 items-center gap-2 rounded-md px-2 text-left text-xs transition-colors hover:bg-[var(--df-bg-hover)]"
                        style={{
                          backgroundColor: active ? "var(--df-bg-hover)" : "transparent",
                          color: active ? "var(--df-primary)" : "var(--df-text)",
                        }}
                        onClick={() => setSelectedCategory(category.id)}
                      >
                        <span
                          className="h-1.5 w-1.5 shrink-0 rounded-full"
                          style={{
                            backgroundColor: active ? "var(--df-primary)" : "var(--df-text-dimmed)",
                            opacity: active ? 1 : 0.6,
                          }}
                        />
                        <span className="min-w-0 flex-1 truncate font-medium">
                          {category.label}
                        </span>
                        <span
                          className="shrink-0 rounded px-1.5 py-0.5 text-[0.625rem] leading-none"
                          style={{
                            backgroundColor: active
                              ? "color-mix(in_srgb,var(--df-primary)_14%,transparent)"
                              : "var(--df-bg-hover)",
                            color: active ? "var(--df-primary)" : "var(--df-text-dimmed)",
                          }}
                        >
                          {category.count}
                        </span>
                      </button>
                    </ContextMenuTrigger>
                    {savedCategory && (
                      <ContextMenuContent className="min-w-[120px]">
                        <ContextMenuItem
                          className="text-xs gap-2"
                          onClick={() => setCategoryToRename(savedCategory)}
                        >
                          <MdEdit className="text-[0.875rem]" />
                          {t("quickCommands.edit")}
                        </ContextMenuItem>
                        <ContextMenuItem
                          className="text-xs gap-2 text-destructive focus:text-destructive"
                          onClick={() => setCategoryToDelete(savedCategory)}
                        >
                          <MdDelete className="text-[0.875rem]" />
                          {t("quickCommands.delete")}
                        </ContextMenuItem>
                      </ContextMenuContent>
                    )}
                  </ContextMenu>
                );
              })}
            </div>
          </aside>

          <div className="min-w-0 flex-1 overflow-y-auto overflow-x-hidden terminal-scroll p-1.5">
            <div className="flex min-w-0 flex-col gap-1.5">
              {filteredCommands.length === 0 ? (
                <div className="mx-auto mt-8 flex w-full max-w-md flex-col items-center justify-center rounded-lg border border-dashed p-4 text-muted-foreground opacity-70">
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
                    <ContextMenu key={cmd.id}>
                      <ContextMenuTrigger asChild>
                        <div
                          className="group flex min-h-11 w-full min-w-0 items-center gap-2 rounded-md border border-border/35 bg-muted/15 px-2 py-1.5 text-xs transition-colors hover:bg-muted/45 hover:text-foreground"
                          style={{ color: "var(--df-text)" }}
                        >
                          <button
                            className="flex min-w-0 flex-1 cursor-pointer items-center gap-2 rounded px-1 text-left"
                            onClick={() => handleCommandClick(cmd)}
                          >
                            <span className="flex h-4 w-4 shrink-0 items-center justify-center">
                              {cmd.icon_tag && QUICK_ICONS[cmd.icon_tag] ? (
                                (() => {
                                  const iconDef = QUICK_ICONS[cmd.icon_tag!];
                                  return (
                                    <iconDef.icon
                                      className="text-[0.9rem] opacity-85"
                                      style={{ color: iconDef.color }}
                                    />
                                  );
                                })()
                              ) : (
                                <span className={`h-2.5 w-2.5 rounded-full ${dotColor}`} />
                              )}
                            </span>
                            <span className="flex min-w-0 flex-1 flex-col gap-0.5">
                              <span className="flex min-w-0 items-center gap-1.5">
                                {cmd.pinned && (
                                  <MdPushPin className="shrink-0 text-[0.7rem] opacity-60" />
                                )}
                                <span className="min-w-0 truncate font-medium">{cmd.label}</span>
                              </span>
                              <span className="min-w-0 truncate font-mono text-[0.6875rem] leading-none text-muted-foreground">
                                {cmd.command}
                              </span>
                            </span>
                          </button>
                          <span className="flex shrink-0 items-center gap-1 opacity-85 transition-opacity group-hover:opacity-100">
                            <Badge
                              variant="outline"
                              className="max-w-[6.5rem] gap-1 border-border/40 bg-background/35 px-1.5 py-0 text-[0.625rem] leading-4 text-muted-foreground"
                            >
                              {cmd.execution_mode === "append" ? (
                                <MdKeyboardReturn className="text-[0.7rem]" />
                              ) : (
                                <MdBolt className="text-[0.7rem]" />
                              )}
                              <span className="truncate">
                                {cmd.execution_mode === "append"
                                  ? t("quickCommands.appendOnlyBadge")
                                  : t("quickCommands.executeImmediately")}
                              </span>
                            </Badge>
                            <Tooltip>
                              <TooltipTrigger asChild>
                                <Button
                                  variant="ghost"
                                  size="icon-sm"
                                  className="h-7 w-7 rounded p-0 text-muted-foreground hover:bg-[var(--df-bg-hover)] hover:text-foreground"
                                  aria-label={t("quickCommands.send")}
                                  onClick={() => handleCommandClick(cmd)}
                                >
                                  <MdSend className="text-[0.875rem]" />
                                </Button>
                              </TooltipTrigger>
                              <TooltipContent side="top">{t("quickCommands.send")}</TooltipContent>
                            </Tooltip>
                            <Popover>
                              <Tooltip>
                                <TooltipTrigger asChild>
                                  <PopoverTrigger asChild>
                                    <Button
                                      variant="ghost"
                                      size="icon-sm"
                                      className="h-7 w-7 rounded p-0 text-muted-foreground hover:bg-[var(--df-bg-hover)] hover:text-foreground"
                                      aria-label={t("quickCommands.view")}
                                    >
                                      <MdVisibility className="text-[0.875rem]" />
                                    </Button>
                                  </PopoverTrigger>
                                </TooltipTrigger>
                                <TooltipContent side="top">
                                  {t("quickCommands.view")}
                                </TooltipContent>
                              </Tooltip>
                              <PopoverContent
                                side="top"
                                align="end"
                                sideOffset={6}
                                className="w-[320px] overflow-hidden rounded-xl border-border/60 bg-popover/95 p-0 shadow-2xl backdrop-blur-md"
                              >
                                <div className="flex flex-col">
                                  <div className="flex flex-col gap-1.5 border-b border-border/30 bg-muted/30 p-3">
                                    <div className="flex items-center gap-2">
                                      <span className="flex h-4 w-4 shrink-0 items-center justify-center">
                                        {cmd.icon_tag && QUICK_ICONS[cmd.icon_tag] ? (
                                          (() => {
                                            const iconDef = QUICK_ICONS[cmd.icon_tag!];
                                            return (
                                              <iconDef.icon
                                                className="text-[0.875rem] opacity-80"
                                                style={{ color: iconDef.color }}
                                              />
                                            );
                                          })()
                                        ) : (
                                          <span
                                            className={`h-2.5 w-2.5 rounded-full ${dotColor}`}
                                          />
                                        )}
                                      </span>
                                      <span className="truncate text-sm font-semibold text-foreground">
                                        {cmd.label}
                                      </span>
                                      <div className="flex-1" />
                                      {cmd.category_id && (
                                        <span className="rounded-full border border-primary/20 bg-primary/10 px-2 py-0.5 text-[0.625rem] font-medium text-primary">
                                          {allCategories.find((c) => c.id === cmd.category_id)
                                            ?.name || cmd.category_id}
                                        </span>
                                      )}
                                    </div>
                                  </div>

                                  <div className="flex flex-col gap-3 p-3">
                                    {cmd.description && (
                                      <div className="text-xs leading-relaxed text-muted-foreground/90">
                                        {cmd.description}
                                      </div>
                                    )}

                                    <pre
                                      className="custom-scrollbar terminal-scroll max-h-[120px] overflow-y-auto whitespace-pre-wrap break-all rounded-md border border-border/40 bg-background/50 p-2.5 font-mono text-[0.6875rem] text-foreground/80"
                                      title={cmd.command}
                                    >
                                      {cmd.command}
                                    </pre>
                                  </div>
                                </div>
                              </PopoverContent>
                            </Popover>
                            <DropdownMenu>
                              <DropdownMenuTrigger asChild>
                                <Button
                                  variant="ghost"
                                  size="icon-sm"
                                  className="h-7 w-7 rounded p-0 text-muted-foreground hover:bg-[var(--df-bg-hover)] hover:text-foreground"
                                  aria-label="More"
                                >
                                  <MoreHorizontalIcon className="size-4" />
                                </Button>
                              </DropdownMenuTrigger>
                              <DropdownMenuContent align="end" className="min-w-[140px]">
                                <DropdownMenuItem
                                  onClick={() => openQuickCommand(JSON.stringify(cmd))}
                                >
                                  <MdEdit className="text-[0.875rem]" />
                                  {t("quickCommands.edit")}
                                </DropdownMenuItem>
                                {onSendToAll && (
                                  <DropdownMenuItem onClick={() => handleSendToAll(cmd)}>
                                    <BsFillSendPlusFill className="text-[0.875rem]" />
                                    {t("quickCommands.sendToAll")}
                                  </DropdownMenuItem>
                                )}
                                <DropdownMenuSeparator />
                                <DropdownMenuItem
                                  variant="destructive"
                                  onClick={() => setCommandToDelete(cmd)}
                                >
                                  <MdDelete className="text-[0.875rem]" />
                                  {t("quickCommands.delete")}
                                </DropdownMenuItem>
                              </DropdownMenuContent>
                            </DropdownMenu>
                          </span>
                        </div>
                      </ContextMenuTrigger>

                      <ContextMenuContent className="min-w-[120px]">
                        <ContextMenuItem
                          className="text-xs gap-2"
                          onClick={() => openQuickCommand(JSON.stringify(cmd))}
                        >
                          <MdEdit className="text-[0.875rem]" />
                          {t("quickCommands.edit")}
                        </ContextMenuItem>
                        {onSendToAll && (
                          <ContextMenuItem
                            className="text-xs gap-2"
                            onClick={() => handleSendToAll(cmd)}
                          >
                            <BsFillSendPlusFill className="text-[0.875rem]" />
                            {t("quickCommands.sendToAll")}
                          </ContextMenuItem>
                        )}
                        <ContextMenuItem
                          className="text-xs gap-2 text-destructive focus:text-destructive"
                          onClick={() => setCommandToDelete(cmd)}
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
        </div>
        {promptCmd && (
          <VariablePromptDialog
            open={!!promptCmd}
            command={promptCmd.command}
            variables={promptVars}
            onCancel={() => {
              setPromptCmd(null);
              setPromptSendToAll(false);
            }}
            onSubmit={handlePromptSubmit}
          />
        )}
        <QuickCommandsImportDialog
          open={importDialogOpen}
          onClose={() => setImportDialogOpen(false)}
          onImported={handleImported}
        />
        <DeleteQuickCommandDialog
          command={commandToDelete}
          onCancel={() => setCommandToDelete(null)}
          onConfirm={handleConfirmDeleteCommand}
        />
        <RenameQuickCommandCategoryDialog
          category={categoryToRename}
          categories={allCategories}
          onCancel={() => setCategoryToRename(null)}
          onConfirm={handleConfirmRenameCategory}
        />
        <DeleteQuickCommandCategoryDialog
          category={categoryToDelete}
          commandCount={categoryToDeleteCommandCount}
          onCancel={() => setCategoryToDelete(null)}
          onConfirm={handleConfirmDeleteCategory}
        />
      </div>
    </TooltipProvider>
  );
}

export default memo(QuickCommands);
