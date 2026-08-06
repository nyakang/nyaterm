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
  MdChevronRight,
  MdClose,
  MdContentCopy,
  MdDelete,
  MdEdit,
  MdFormatListBulleted,
  MdGridView,
  MdKeyboardArrowDown,
  MdKeyboardArrowUp,
  MdKeyboardReturn,
  MdPushPin,
  MdSearch,
  MdSend,
  MdSort,
  MdTerminal,
  MdViewList,
  MdVisibility,
} from "react-icons/md";
import { toast } from "sonner";
import DeleteQuickCommandCategoryDialog from "@/components/dialog/quick-commands/DeleteQuickCommandCategoryDialog";
import DeleteQuickCommandDialog from "@/components/dialog/quick-commands/DeleteQuickCommandDialog";
import QuickCommandsImportDialog from "@/components/dialog/quick-commands/QuickCommandsImportDialog";
import RenameQuickCommandCategoryDialog from "@/components/dialog/quick-commands/RenameQuickCommandCategoryDialog";
import PanelHeader from "@/components/layout/PanelHeader";
import ResizeHandle from "@/components/layout/ResizeHandle";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
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
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { useApp } from "@/context/AppContext";
import { openAIAssistant } from "@/lib/aiEvents";
import { writeClipboardText } from "@/lib/clipboard";
import { invoke } from "@/lib/invoke";
import {
  buildQuickCommandCategoryList,
  buildQuickCommandCategoryPath,
  buildQuickCommandCategoryTree,
  collectQuickCommandCategoryAncestorIds,
  collectQuickCommandCategoryDescendantIds,
  deleteQuickCommandCategoryTree,
  flattenVisibleQuickCommandCategoryTree,
  getQuickCommandCategoryMoveState,
  getQuickCommandUncategorizedCount,
  moveQuickCommandCategory,
} from "@/lib/quickCommandCategories";
import { cn } from "@/lib/utils";
import type {
  QuickCommand,
  QuickCommandCategory,
  QuickCommandImportResult,
  QuickCommandSortMode,
  QuickCommandsConfig,
  QuickCommandViewMode,
} from "@/types/global";
import { openQuickCommand } from "../../lib/windowManager";
import VariablePromptDialog, {
  parseCommandVariables,
  type VariableDef,
} from "../dialog/terminal/VariablePromptDialog";
import { QUICK_ICONS } from "../icons";

interface QuickCommandsProps {
  onSend: (command: string, execute?: boolean) => void;
  onSendToAll?: (command: string, execute?: boolean) => void;
}

const COLOR_DOT: Record<string, string> = {
  default: "bg-muted-foreground",
  red: "bg-red-500",
  green: "bg-green-500",
  blue: "bg-blue-500",
  yellow: "bg-yellow-500",
  purple: "bg-purple-500",
};

const QUICK_COMMAND_CATEGORY_WIDTH_DEFAULT = 176;
const QUICK_COMMAND_CATEGORY_WIDTH_MIN = 128;
const QUICK_COMMAND_CATEGORY_WIDTH_MAX = 320;

function clampQuickCommandCategoryWidth(width: unknown) {
  const numericWidth = typeof width === "number" ? width : Number(width);
  if (!Number.isFinite(numericWidth))
    return QUICK_COMMAND_CATEGORY_WIDTH_DEFAULT;
  return Math.max(
    QUICK_COMMAND_CATEGORY_WIDTH_MIN,
    Math.min(QUICK_COMMAND_CATEGORY_WIDTH_MAX, Math.round(numericWidth)),
  );
}

function normalizeQuickCommandViewMode(mode: unknown): QuickCommandViewMode {
  return mode === "list" || mode === "compact" || mode === "tile"
    ? mode
    : "tile";
}

function normalizeQuickCommandSortMode(mode: unknown): QuickCommandSortMode {
  return mode === "name" || mode === "useCount" ? mode : "created";
}

function QuickCommands({ onSend, onSendToAll }: QuickCommandsProps) {
  const { t } = useTranslation();
  const { appSettings, updateUi } = useApp();
  const [commands, setCommands] = useState<QuickCommand[]>([]);
  const [savedCategories, setSavedCategories] = useState<
    QuickCommandCategory[]
  >([]);
  const [quickCommandsLoaded, setQuickCommandsLoaded] = useState(false);
  const loaded = useRef(false);
  const saveTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const skipNextSaveRef = useRef(false);
  const initializedExpandedRootIdsRef = useRef<Set<string>>(new Set());

  // UI State
  const [search, setSearch] = useState("");
  const [aiPrompt, setAiPrompt] = useState("");
  const [aiPopoverOpen, setAiPopoverOpen] = useState(false);
  const [importDialogOpen, setImportDialogOpen] = useState(false);
  const [expandedCategoryIds, setExpandedCategoryIds] = useState<Set<string>>(
    () => new Set(),
  );
  const [categoryToRename, setCategoryToRename] =
    useState<QuickCommandCategory | null>(null);
  const [categoryToDelete, setCategoryToDelete] =
    useState<QuickCommandCategory | null>(null);
  const [commandToDelete, setCommandToDelete] = useState<QuickCommand | null>(
    null,
  );

  // Variable Prompt State
  const [promptCmd, setPromptCmd] = useState<QuickCommand | null>(null);
  const [promptVars, setPromptVars] = useState<VariableDef[]>([]);
  const [promptSendToAll, setPromptSendToAll] = useState(false);

  const loadQuickCommands = useCallback(async () => {
    const cfg = await invoke<QuickCommandsConfig>("get_quick_commands");
    skipNextSaveRef.current = true;
    setCommands(cfg.commands || []);
    setSavedCategories(cfg.categories || []);
    setQuickCommandsLoaded(true);
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
      invoke("save_quick_commands", {
        config: { commands, categories: savedCategories },
      }).catch(() => {});
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

    const currentCategories = buildQuickCommandCategoryList(
      savedCategories,
      commands,
    );
    const { deleteIds } = deleteQuickCommandCategoryTree(
      currentCategories,
      commands,
      categoryToDelete.id,
    );
    setSavedCategories((prev) =>
      prev.filter((category) => !deleteIds.has(category.id)),
    );
    setCommands((prev) =>
      prev.filter((cmd) => !cmd.category_id || !deleteIds.has(cmd.category_id)),
    );
    setExpandedCategoryIds((prev) => {
      const next = new Set(prev);
      deleteIds.forEach((id) => {
        next.delete(id);
      });
      return next;
    });
    updateUi((current) =>
      current.quick_cmd_selected_category &&
      deleteIds.has(current.quick_cmd_selected_category)
        ? { quick_cmd_selected_category: "all" }
        : {},
    );
    setCategoryToDelete(null);
  }, [categoryToDelete, commands, savedCategories, updateUi]);

  const handleConfirmRenameCategory = useCallback(
    (name: string) => {
      if (!categoryToRename) return;

      const renamedCategory = { ...categoryToRename, name };
      setSavedCategories((prev) => {
        const exists = prev.some(
          (category) => category.id === renamedCategory.id,
        );
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

  const handleMoveCategory = useCallback(
    (categoryId: string, direction: "up" | "down") => {
      setSavedCategories((prev) =>
        moveQuickCommandCategory(prev, categoryId, direction),
      );
    },
    [],
  );

  const incrementUseCount = useCallback((id: string) => {
    setCommands((prev) =>
      prev.map((c) =>
        c.id === id
          ? { ...c, use_count: (c.use_count ?? 0) + 1, updated_at: Date.now() }
          : c,
      ),
    );
    invoke("increment_quick_command_use_count", { id }).catch(() => {});
  }, []);

  // Listen for quick-command-saved events from child window
  useEffect(() => {
    const unsub = listen<{
      command: QuickCommand;
      newCategory?: QuickCommandCategory;
    }>("quick-command-saved", (event) => {
      const { command: cmd, newCategory } = event.payload;
      skipNextSaveRef.current = true;
      setCommands((prev) => {
        const exists = prev.some((c) => c.id === cmd.id);
        return exists
          ? prev.map((c) => (c.id === cmd.id ? cmd : c))
          : [...prev, cmd];
      });
      if (newCategory) {
        setSavedCategories((prev) =>
          prev.find((c) => c.id === newCategory.id)
            ? prev
            : [...prev, newCategory],
        );
      }
    });
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
        onSendToAll(cmd.command, cmd.execution_mode !== "append");
      }
    },
    [onSendToAll, incrementUseCount],
  );

  const handlePromptSubmit = useCallback(
    (resolvedCommand: string) => {
      if (promptCmd) {
        if (promptSendToAll && onSendToAll) {
          onSendToAll(resolvedCommand, promptCmd.execution_mode !== "append");
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

  const handleCopyCommand = useCallback(
    async (command: string) => {
      try {
        await writeClipboardText(command);
        toast.success(t("common.copied"));
      } catch {
        toast.error(t("quickCommands.copyFailed"));
      }
    },
    [t],
  );

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
    return buildQuickCommandCategoryList(savedCategories, commands);
  }, [commands, savedCategories]);

  const categoryById = useMemo(
    () => new Map(allCategories.map((category) => [category.id, category])),
    [allCategories],
  );
  const categoryTree = useMemo(
    () => buildQuickCommandCategoryTree(allCategories, commands),
    [allCategories, commands],
  );
  const visibleCategoryRows = useMemo(
    () =>
      flattenVisibleQuickCommandCategoryTree(categoryTree, expandedCategoryIds),
    [categoryTree, expandedCategoryIds],
  );
  const uncategorizedCount = useMemo(
    () => getQuickCommandUncategorizedCount(commands),
    [commands],
  );

  const viewMode = normalizeQuickCommandViewMode(
    appSettings.ui.quick_cmd_view_mode,
  );
  const sortMode = normalizeQuickCommandSortMode(
    appSettings.ui.quick_cmd_sort_mode,
  );
  const categorySidebarWidth = clampQuickCommandCategoryWidth(
    appSettings.ui.quick_cmd_category_width,
  );
  const storedSelectedCategory =
    appSettings.ui.quick_cmd_selected_category || "all";
  const selectedCategory =
    storedSelectedCategory === "all" ||
    storedSelectedCategory === "uncategorized" ||
    categoryById.has(storedSelectedCategory)
      ? storedSelectedCategory
      : "all";
  const setViewMode = useCallback(
    (mode: QuickCommandViewMode) => {
      updateUi({ quick_cmd_view_mode: mode });
    },
    [updateUi],
  );
  const setSortMode = useCallback(
    (mode: QuickCommandSortMode) => {
      updateUi({ quick_cmd_sort_mode: mode });
    },
    [updateUi],
  );
  const resizeCategorySidebar = useCallback(
    (delta: number) => {
      updateUi((current) => ({
        quick_cmd_category_width: clampQuickCommandCategoryWidth(
          (current.quick_cmd_category_width ??
            QUICK_COMMAND_CATEGORY_WIDTH_DEFAULT) + delta,
        ),
      }));
    },
    [updateUi],
  );
  const setSelectedCategory = useCallback(
    (categoryId: string) => {
      updateUi({ quick_cmd_selected_category: categoryId });
    },
    [updateUi],
  );

  useEffect(() => {
    if (!quickCommandsLoaded) return;
    if (storedSelectedCategory === selectedCategory) return;
    updateUi({ quick_cmd_selected_category: selectedCategory });
  }, [quickCommandsLoaded, selectedCategory, storedSelectedCategory, updateUi]);

  useEffect(() => {
    setExpandedCategoryIds((prev) => {
      const next = new Set(prev);
      let changed = false;
      categoryTree.forEach((node) => {
        if (!initializedExpandedRootIdsRef.current.has(node.category.id)) {
          initializedExpandedRootIdsRef.current.add(node.category.id);
          next.add(node.category.id);
          changed = true;
        }
      });
      return changed ? next : prev;
    });
  }, [categoryTree]);

  useEffect(() => {
    if (selectedCategory === "all" || selectedCategory === "uncategorized")
      return;
    const ancestorIds = collectQuickCommandCategoryAncestorIds(
      allCategories,
      selectedCategory,
    );
    if (ancestorIds.length === 0) return;
    setExpandedCategoryIds((prev) => {
      const next = new Set(prev);
      let changed = false;
      ancestorIds.forEach((id) => {
        if (!next.has(id)) {
          next.add(id);
          changed = true;
        }
      });
      return changed ? next : prev;
    });
  }, [allCategories, selectedCategory]);

  const filteredCommands = useMemo(() => {
    let filtered = commands;

    if (selectedCategory === "uncategorized") {
      filtered = filtered.filter((c) => !c.category_id);
    } else if (selectedCategory !== "all") {
      const selectedCategoryIds = collectQuickCommandCategoryDescendantIds(
        allCategories,
        selectedCategory,
      );
      filtered = filtered.filter(
        (c) => !!c.category_id && selectedCategoryIds.has(c.category_id),
      );
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
  }, [allCategories, commands, search, selectedCategory, sortMode]);

  const searchQuery = search.trim();
  const hasActiveFilters = searchQuery.length > 0 || selectedCategory !== "all";
  const headerMetaText =
    hasActiveFilters && commands.length > 0
      ? `${filteredCommands.length}/${commands.length}`
      : `${commands.length}`;
  const categoryToDeleteCommandCount = useMemo(() => {
    if (!categoryToDelete) return 0;
    const deleteIds = collectQuickCommandCategoryDescendantIds(
      allCategories,
      categoryToDelete.id,
    );
    return commands.filter(
      (cmd) => !!cmd.category_id && deleteIds.has(cmd.category_id),
    ).length;
  }, [allCategories, categoryToDelete, commands]);
  const headerControlClassName =
    "h-7 border-0 bg-[var(--df-bg-hover)] py-1 text-xs text-[var(--df-text)] shadow-none";
  const getCommandCategoryName = useCallback(
    (cmd: QuickCommand) =>
      cmd.category_id
        ? buildQuickCommandCategoryPath(allCategories, cmd.category_id) ||
          cmd.category_id
        : null,
    [allCategories],
  );
  const toggleCategoryExpanded = useCallback((categoryId: string) => {
    setExpandedCategoryIds((prev) => {
      const next = new Set(prev);
      if (next.has(categoryId)) {
        next.delete(categoryId);
      } else {
        next.add(categoryId);
      }
      return next;
    });
  }, []);
  const renderCommandIcon = useCallback(
    (cmd: QuickCommand, className = "text-[0.9rem]") => {
      const dotColor =
        COLOR_DOT[cmd.color_tag || "default"] || COLOR_DOT.default;

      if (cmd.icon_tag && QUICK_ICONS[cmd.icon_tag]) {
        const iconDef = QUICK_ICONS[cmd.icon_tag];
        return (
          <iconDef.icon
            className={cn(className, "opacity-85")}
            style={{ color: iconDef.color }}
          />
        );
      }

      return <span className={cn("h-2.5 w-2.5 rounded-full", dotColor)} />;
    },
    [],
  );
  const renderExecutionBadge = useCallback(
    (cmd: QuickCommand, className?: string) => (
      <Badge
        variant="outline"
        className={cn(
          "max-w-[6.5rem] gap-1 border-border/40 bg-background/35 px-1.5 py-0 text-[0.625rem] leading-4 text-muted-foreground",
          className,
        )}
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
    ),
    [t],
  );
  const renderCommandPreview = useCallback(
    (cmd: QuickCommand) => (
      <div className="relative">
        <pre
          className="custom-scrollbar terminal-scroll max-h-[120px] overflow-y-auto whitespace-pre-wrap break-all rounded-md border border-border/40 bg-background/50 p-2.5 pr-9 font-mono text-[0.6875rem] text-foreground/80"
          title={cmd.command}
        >
          {cmd.command}
        </pre>
        <button
          type="button"
          className="absolute right-1.5 top-1.5 flex h-6 w-6 items-center justify-center rounded text-muted-foreground transition-colors hover:bg-accent hover:text-[var(--df-primary)]"
          aria-label={t("quickCommands.copyCommand")}
          title={t("quickCommands.copyCommand")}
          onClick={(event) => {
            event.preventDefault();
            event.stopPropagation();
            void handleCopyCommand(cmd.command);
          }}
        >
          <MdContentCopy className="text-[0.8rem]" />
        </button>
      </div>
    ),
    [handleCopyCommand, t],
  );
  const renderCommandDetailsPopover = useCallback(
    (cmd: QuickCommand) => {
      const categoryName = getCommandCategoryName(cmd);

      return (
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
                    {renderCommandIcon(cmd, "text-[0.875rem]")}
                  </span>
                  <span className="truncate text-sm font-semibold text-foreground">
                    {cmd.label}
                  </span>
                  <div className="flex-1" />
                  {categoryName && (
                    <span className="max-w-[7rem] truncate rounded-full border border-primary/20 bg-primary/10 px-2 py-0.5 text-[0.625rem] font-medium text-primary">
                      {categoryName}
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

                {renderCommandPreview(cmd)}
              </div>
            </div>
          </PopoverContent>
        </Popover>
      );
    },
    [getCommandCategoryName, renderCommandIcon, renderCommandPreview, t],
  );
  const renderMoreMenu = useCallback(
    (cmd: QuickCommand) => (
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
    ),
    [handleSendToAll, onSendToAll, t],
  );
  const renderCommandActions = useCallback(
    (cmd: QuickCommand, options?: { showBadge?: boolean }) => (
      <span className="flex shrink-0 items-center gap-1 opacity-85 transition-opacity group-hover:opacity-100">
        {options?.showBadge !== false && renderExecutionBadge(cmd)}
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
        {renderCommandDetailsPopover(cmd)}
        {renderMoreMenu(cmd)}
      </span>
    ),
    [
      handleCommandClick,
      renderCommandDetailsPopover,
      renderExecutionBadge,
      renderMoreMenu,
      t,
    ],
  );
  const renderContextMenuContent = useCallback(
    (cmd: QuickCommand) => (
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
    ),
    [handleSendToAll, onSendToAll, t],
  );
  const renderCommandListItem = useCallback(
    (cmd: QuickCommand) => (
      <ContextMenu key={cmd.id}>
        <ContextMenuTrigger asChild>
          <div
            className="group flex min-h-11 w-full min-w-0 items-center gap-2 rounded-md border border-border/35 bg-muted/15 px-2 py-1.5 text-xs transition-colors hover:bg-muted/45 hover:text-foreground"
            style={{ color: "var(--df-text)" }}
          >
            <button
              type="button"
              className="flex min-w-0 flex-1 cursor-pointer items-center gap-2 rounded px-1 text-left"
              onClick={() => handleCommandClick(cmd)}
            >
              <span className="flex h-4 w-4 shrink-0 items-center justify-center">
                {renderCommandIcon(cmd)}
              </span>
              <span className="flex min-w-0 flex-1 flex-col gap-0.5">
                <span className="flex min-w-0 items-center gap-1.5">
                  {cmd.pinned && (
                    <MdPushPin className="shrink-0 text-[0.7rem] opacity-60" />
                  )}
                  <span className="min-w-0 truncate font-medium">
                    {cmd.label}
                  </span>
                </span>
                <span className="min-w-0 truncate font-mono text-[0.6875rem] leading-none text-muted-foreground">
                  {cmd.command}
                </span>
              </span>
            </button>
            {renderCommandActions(cmd)}
          </div>
        </ContextMenuTrigger>
        {renderContextMenuContent(cmd)}
      </ContextMenu>
    ),
    [
      handleCommandClick,
      renderCommandActions,
      renderCommandIcon,
      renderContextMenuContent,
    ],
  );
  const renderCommandCompactItem = useCallback(
    (cmd: QuickCommand) => (
      <ContextMenu key={cmd.id}>
        <ContextMenuTrigger asChild>
          <div
            className="group flex h-8 w-full min-w-0 items-center gap-1.5 rounded px-1.5 text-xs transition-colors hover:bg-muted/45 hover:text-foreground"
            style={{ color: "var(--df-text)" }}
          >
            <button
              type="button"
              className="flex min-w-0 flex-1 cursor-pointer items-center gap-1.5 rounded px-0.5 text-left"
              onClick={() => handleCommandClick(cmd)}
            >
              <span className="flex h-4 w-4 shrink-0 items-center justify-center">
                {renderCommandIcon(cmd, "text-[0.8rem]")}
              </span>
              {cmd.pinned && (
                <MdPushPin className="shrink-0 text-[0.65rem] opacity-60" />
              )}
              <span className="min-w-[4rem] max-w-[38%] truncate font-medium">
                {cmd.label}
              </span>
              <span className="min-w-0 flex-1 truncate font-mono text-[0.6875rem] text-muted-foreground/85">
                {cmd.command}
              </span>
            </button>
            {renderCommandActions(cmd, { showBadge: false })}
          </div>
        </ContextMenuTrigger>
        {renderContextMenuContent(cmd)}
      </ContextMenu>
    ),
    [
      handleCommandClick,
      renderCommandActions,
      renderCommandIcon,
      renderContextMenuContent,
    ],
  );
  const renderCommandTile = useCallback(
    (cmd: QuickCommand) => {
      const categoryName = getCommandCategoryName(cmd);

      return (
        <ContextMenu key={cmd.id}>
          <Tooltip>
            <ContextMenuTrigger asChild>
              <TooltipTrigger asChild>
                <button
                  type="button"
                  className="group flex max-w-full shrink-0 cursor-pointer items-center gap-1.5 rounded-md border border-border/35 bg-muted/20 px-2 py-1 text-left text-[0.6875rem] font-medium text-foreground/80 transition-colors hover:bg-muted/50 hover:text-foreground"
                  style={{ color: "var(--df-text)" }}
                  onClick={() => handleCommandClick(cmd)}
                >
                  <span className="flex h-3.5 w-3.5 shrink-0 items-center justify-center">
                    {renderCommandIcon(cmd, "text-[0.75rem]")}
                  </span>
                  {cmd.pinned && (
                    <MdPushPin className="shrink-0 text-[0.625rem] opacity-60" />
                  )}
                  <span className="min-w-0 truncate whitespace-nowrap">
                    {cmd.label}
                  </span>
                </button>
              </TooltipTrigger>
            </ContextMenuTrigger>
            <TooltipContent
              side="top"
              align="start"
              showArrow={false}
              className="w-[320px] overflow-hidden rounded-xl border-border/60 bg-popover/95 p-0 shadow-2xl backdrop-blur-md"
            >
              <div className="flex flex-col">
                <div className="flex flex-col gap-1.5 border-b border-border/30 bg-muted/30 p-3">
                  <div className="flex items-center gap-2">
                    <span className="flex h-4 w-4 shrink-0 items-center justify-center">
                      {renderCommandIcon(cmd, "text-[0.875rem]")}
                    </span>
                    <span className="truncate text-sm font-semibold text-foreground">
                      {cmd.label}
                    </span>
                    <div className="flex-1" />
                    {categoryName && (
                      <span className="max-w-[7rem] truncate rounded-full border border-primary/20 bg-primary/10 px-2 py-0.5 text-[0.625rem] font-medium text-primary">
                        {categoryName}
                      </span>
                    )}
                  </div>
                  <div className="mt-0.5 flex items-center gap-1.5 text-[0.6875rem] text-muted-foreground">
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

                <div className="flex flex-col gap-3 p-3">
                  {cmd.description && (
                    <div className="text-xs leading-relaxed text-muted-foreground/90">
                      {cmd.description}
                    </div>
                  )}

                  {renderCommandPreview(cmd)}
                </div>
              </div>
            </TooltipContent>
          </Tooltip>
          {renderContextMenuContent(cmd)}
        </ContextMenu>
      );
    },
    [
      getCommandCategoryName,
      handleCommandClick,
      renderCommandIcon,
      renderCommandPreview,
      renderContextMenuContent,
      t,
    ],
  );
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
              <span
                className="text-[0.6875rem]"
                style={{ color: "var(--df-text-dimmed)" }}
              >
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

              <span
                aria-hidden
                className="mx-1 h-4 w-px shrink-0 bg-border/50"
              />

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
                            sortMode !== "created"
                              ? "var(--df-primary)"
                              : "var(--df-text-muted)",
                        }}
                        aria-label={t("quickCommands.sort")}
                      >
                        <MdSort className="text-[1.05rem]" />
                      </Button>
                    </DropdownMenuTrigger>
                  </TooltipTrigger>
                  <TooltipContent side="top">
                    {t("quickCommands.sort")}
                  </TooltipContent>
                </Tooltip>
                <DropdownMenuContent align="end">
                  <DropdownMenuRadioGroup
                    value={sortMode}
                    onValueChange={(value) =>
                      setSortMode(normalizeQuickCommandSortMode(value))
                    }
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

              <DropdownMenu>
                <Tooltip>
                  <TooltipTrigger asChild>
                    <DropdownMenuTrigger asChild>
                      <Button
                        variant="ghost"
                        size="icon-sm"
                        className="h-6 w-6 shrink-0 rounded-md p-0 transition-colors hover:bg-[var(--df-bg-hover)]"
                        style={{ color: "var(--df-primary)" }}
                        aria-label={t("quickCommands.viewMode")}
                      >
                        {viewMode === "tile" ? (
                          <MdGridView className="text-[1rem]" />
                        ) : viewMode === "compact" ? (
                          <MdViewList className="text-[1.05rem]" />
                        ) : (
                          <MdFormatListBulleted className="text-[1rem]" />
                        )}
                      </Button>
                    </DropdownMenuTrigger>
                  </TooltipTrigger>
                  <TooltipContent side="top">
                    {t("quickCommands.viewMode")}
                  </TooltipContent>
                </Tooltip>
                <DropdownMenuContent align="end" className="min-w-[150px]">
                  <DropdownMenuRadioGroup
                    value={viewMode}
                    onValueChange={(value) =>
                      setViewMode(normalizeQuickCommandViewMode(value))
                    }
                  >
                    <DropdownMenuRadioItem value="list" className="text-xs">
                      <MdFormatListBulleted className="text-[0.95rem]" />
                      {t("quickCommands.listMode")}
                    </DropdownMenuRadioItem>
                    <DropdownMenuRadioItem value="compact" className="text-xs">
                      <MdViewList className="text-[1rem]" />
                      {t("quickCommands.compactListMode")}
                    </DropdownMenuRadioItem>
                    <DropdownMenuRadioItem value="tile" className="text-xs">
                      <MdGridView className="text-[0.95rem]" />
                      {t("quickCommands.tileMode")}
                    </DropdownMenuRadioItem>
                  </DropdownMenuRadioGroup>
                </DropdownMenuContent>
              </DropdownMenu>

              <span
                aria-hidden
                className="mx-1 h-4 w-px shrink-0 bg-border/50"
              />

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
                  <TooltipContent side="top">
                    {t("quickCommands.addCommand")}
                  </TooltipContent>
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
                  <TooltipContent side="top">
                    {t("quickCommands.import")}
                  </TooltipContent>
                </Tooltip>
              </div>

              <span
                aria-hidden
                className="mx-1 h-4 w-px shrink-0 bg-border/50"
              />

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
                  <TooltipContent side="top">
                    {t("ai.generateCommand")}
                  </TooltipContent>
                </Tooltip>
                <PopoverContent align="end" className="w-80 p-3">
                  <div className="space-y-2">
                    <div className="text-xs font-medium">
                      {t("ai.generateCommand")}
                    </div>
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
                      <Button
                        size="xs"
                        disabled={!aiPrompt.trim()}
                        onClick={handleAiPromptSubmit}
                      >
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
          <aside
            className="shrink-0 overflow-y-auto overflow-x-hidden p-1.5 terminal-scroll"
            style={{ width: categorySidebarWidth }}
          >
            <div className="flex flex-col gap-1">
              {(() => {
                const active = selectedCategory === "all";
                return (
                  <button
                    type="button"
                    className="group flex h-8 w-full min-w-0 items-center gap-2 rounded-md px-2 text-left text-xs transition-colors hover:bg-[var(--df-bg-hover)]"
                    style={{
                      backgroundColor: active
                        ? "var(--df-bg-hover)"
                        : "transparent",
                      color: active ? "var(--df-primary)" : "var(--df-text)",
                    }}
                    onClick={() => setSelectedCategory("all")}
                  >
                    <span
                      className="h-1.5 w-1.5 shrink-0 rounded-full"
                      style={{
                        backgroundColor: active
                          ? "var(--df-primary)"
                          : "var(--df-text-dimmed)",
                        opacity: active ? 1 : 0.6,
                      }}
                    />
                    <span className="min-w-0 flex-1 truncate font-medium">
                      {t("quickCommands.allCategories")}
                    </span>
                    <span
                      className="shrink-0 rounded px-1.5 py-0.5 text-[0.625rem] leading-none"
                      style={{
                        backgroundColor: active
                          ? "color-mix(in_srgb,var(--df-primary)_14%,transparent)"
                          : "var(--df-bg-hover)",
                        color: active
                          ? "var(--df-primary)"
                          : "var(--df-text-dimmed)",
                      }}
                    >
                      {commands.length}
                    </span>
                  </button>
                );
              })()}

              {visibleCategoryRows.map(({ node, depth }) => {
                const category = node.category;
                const active = selectedCategory === category.id;
                const expanded = expandedCategoryIds.has(category.id);
                const savedCategory = savedCategories.find(
                  (item) => item.id === category.id,
                );
                const moveState = savedCategory
                  ? getQuickCommandCategoryMoveState(
                      savedCategories,
                      savedCategory.id,
                    )
                  : { canMoveUp: false, canMoveDown: false };

                return (
                  <ContextMenu key={category.id}>
                    <ContextMenuTrigger asChild disabled={!savedCategory}>
                      <div
                        className="group flex h-8 w-full min-w-0 items-center rounded-md text-xs transition-colors hover:bg-[var(--df-bg-hover)]"
                        style={{
                          backgroundColor: active
                            ? "var(--df-bg-hover)"
                            : "transparent",
                          color: active
                            ? "var(--df-primary)"
                            : "var(--df-text)",
                          paddingLeft: `${Math.min(depth, 4) * 0.7 + 0.25}rem`,
                        }}
                      >
                        {node.children.length > 0 ? (
                          <button
                            type="button"
                            className="flex h-7 w-5 shrink-0 items-center justify-center rounded text-muted-foreground transition-colors hover:text-foreground"
                            aria-label={category.name}
                            onClick={(event) => {
                              event.preventDefault();
                              event.stopPropagation();
                              toggleCategoryExpanded(category.id);
                            }}
                          >
                            <MdChevronRight
                              className={cn(
                                "text-[0.875rem] transition-transform",
                                expanded && "rotate-90",
                              )}
                            />
                          </button>
                        ) : depth > 0 ? (
                          <span className="h-7 w-5 shrink-0" />
                        ) : null}
                        <button
                          type="button"
                          className={cn(
                            "flex h-8 min-w-0 flex-1 items-center gap-2 pr-2 text-left",
                            node.children.length > 0 || depth > 0
                              ? "rounded-r-md"
                              : "rounded-md pl-2",
                          )}
                          onClick={() => setSelectedCategory(category.id)}
                        >
                          <span
                            className="h-1.5 w-1.5 shrink-0 rounded-full"
                            style={{
                              backgroundColor: active
                                ? "var(--df-primary)"
                                : "var(--df-text-dimmed)",
                              opacity: active ? 1 : 0.6,
                            }}
                          />
                          <span className="min-w-0 flex-1 truncate font-medium">
                            {category.name}
                          </span>
                          <span
                            className="shrink-0 rounded px-1.5 py-0.5 text-[0.625rem] leading-none"
                            style={{
                              backgroundColor: active
                                ? "color-mix(in_srgb,var(--df-primary)_14%,transparent)"
                                : "var(--df-bg-hover)",
                              color: active
                                ? "var(--df-primary)"
                                : "var(--df-text-dimmed)",
                            }}
                          >
                            {node.totalCount}
                          </span>
                        </button>
                      </div>
                    </ContextMenuTrigger>
                    {savedCategory && (
                      <ContextMenuContent className="min-w-[120px]">
                        <ContextMenuItem
                          className="text-xs gap-2"
                          disabled={!moveState.canMoveUp}
                          onClick={() => handleMoveCategory(savedCategory.id, "up")}
                        >
                          <MdKeyboardArrowUp className="text-[0.875rem]" />
                          {t("dialog.moveUp")}
                        </ContextMenuItem>
                        <ContextMenuItem
                          className="text-xs gap-2"
                          disabled={!moveState.canMoveDown}
                          onClick={() =>
                            handleMoveCategory(savedCategory.id, "down")
                          }
                        >
                          <MdKeyboardArrowDown className="text-[0.875rem]" />
                          {t("dialog.moveDown")}
                        </ContextMenuItem>
                        <ContextMenuSeparator />
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

              {(() => {
                const active = selectedCategory === "uncategorized";
                return (
                  <button
                    type="button"
                    className="group flex h-8 w-full min-w-0 items-center gap-2 rounded-md px-2 text-left text-xs transition-colors hover:bg-[var(--df-bg-hover)]"
                    style={{
                      backgroundColor: active
                        ? "var(--df-bg-hover)"
                        : "transparent",
                      color: active ? "var(--df-primary)" : "var(--df-text)",
                    }}
                    onClick={() => setSelectedCategory("uncategorized")}
                  >
                    <span
                      className="h-1.5 w-1.5 shrink-0 rounded-full"
                      style={{
                        backgroundColor: active
                          ? "var(--df-primary)"
                          : "var(--df-text-dimmed)",
                        opacity: active ? 1 : 0.6,
                      }}
                    />
                    <span className="min-w-0 flex-1 truncate font-medium">
                      {t("quickCommands.uncategorized")}
                    </span>
                    <span
                      className="shrink-0 rounded px-1.5 py-0.5 text-[0.625rem] leading-none"
                      style={{
                        backgroundColor: active
                          ? "color-mix(in_srgb,var(--df-primary)_14%,transparent)"
                          : "var(--df-bg-hover)",
                        color: active
                          ? "var(--df-primary)"
                          : "var(--df-text-dimmed)",
                      }}
                    >
                      {uncategorizedCount}
                    </span>
                  </button>
                );
              })()}
            </div>
          </aside>
          <ResizeHandle
            direction="horizontal"
            onResize={resizeCategorySidebar}
            className="opacity-70 hover:opacity-100 active:opacity-100"
          />

          <div className="min-w-0 flex-1 overflow-y-auto overflow-x-hidden terminal-scroll p-1.5">
            <div
              className={cn(
                "min-w-0 gap-1.5",
                viewMode === "tile"
                  ? "flex flex-wrap content-start"
                  : "flex flex-col",
              )}
            >
              {filteredCommands.length === 0 ? (
                <div className="mx-auto mt-8 flex w-full max-w-md flex-col items-center justify-center rounded-lg border border-dashed p-4 text-muted-foreground opacity-70">
                  <MdTerminal className="text-2xl mb-2" />
                  <span className="text-xs mb-3">
                    {t("quickCommands.noCommandsFound")}
                  </span>
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
                filteredCommands.map((cmd) =>
                  viewMode === "tile"
                    ? renderCommandTile(cmd)
                    : viewMode === "compact"
                      ? renderCommandCompactItem(cmd)
                      : renderCommandListItem(cmd),
                )
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
