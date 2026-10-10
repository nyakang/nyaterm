import { randomUUID } from "@/lib/uuid";
import { runtime } from "@/lib/backend/runtime";
import { downloadJson } from "@/lib/backend/browserArtifacts";
import { listen } from "@/lib/backend/api";
import { save as saveFileDialog } from "@/lib/backend/platform/dialog";
import { MoreHorizontalIcon } from "lucide-react";
import {
  type DragEvent,
  memo,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import { BiExport, BiImport } from "react-icons/bi";
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
  MdFolder,
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
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
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
import { Label } from "@/components/ui/label";
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
import { getErrorMessage } from "@/lib/errors";
import { invoke } from "@/lib/invoke";
import { logger } from "@/lib/logger";
import {
  buildQuickCommandCategoryList,
  buildQuickCommandCategoryPath,
  buildQuickCommandCategoryTree,
  collectQuickCommandCategoryAncestorIds,
  collectQuickCommandCategoryDescendantIds,
  deleteQuickCommandCategoryTree,
  filterQuickCommandsByCategory,
  flattenVisibleQuickCommandCategoryTree,
  getQuickCommandCategoryMoveState,
  getQuickCommandUncategorizedCount,
  getNextQuickCommandCategorySortOrder,
  hasQuickCommandCategorySiblingName,
  moveQuickCommandCategory,
  moveQuickCommandCategoryToTarget,
  type QuickCommandCategoryDropPosition,
} from "@/lib/quickCommandCategories";
import {
  compareQuickCommandsByMode,
  reorderQuickCommandsWithinCategory,
} from "@/lib/quickCommands";
import { normalizeQuickCommandSortMode } from "@/lib/quickCommandSettings";
import { isWindows } from "@/lib/platform";
import { cn } from "@/lib/utils";
import type {
  NyaScriptRunEvent,
  NyaScriptRunStatus,
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
  sendDisabled?: boolean;
  currentSessionId?: string | null;
}

interface NewQuickCommandCategoryDraft {
  parentId: string | null;
}

interface QuickCommandCategoryDragTarget {
  categoryId: string | null;
  position: QuickCommandCategoryDropPosition;
}

interface QuickCommandDragTarget {
  commandId: string;
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
const QUICK_COMMAND_CATEGORY_DRAG_MIME = "application/x-nyaterm-quick-category";
const QUICK_COMMAND_DRAG_MIME = "application/x-nyaterm-quick-command";

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

function NewQuickCommandCategoryDialog({
  draft,
  categories,
  parentLabel,
  onCancel,
  onConfirm,
}: {
  draft: NewQuickCommandCategoryDraft | null;
  categories: QuickCommandCategory[];
  parentLabel: string;
  onCancel: () => void;
  onConfirm: (name: string) => void;
}) {
  const { t } = useTranslation();
  const [name, setName] = useState("");
  const [submitted, setSubmitted] = useState(false);

  const trimmedName = name.trim();
  const hasDuplicateName = draft
    ? hasQuickCommandCategorySiblingName(
        categories,
        draft.parentId,
        trimmedName,
      )
    : false;
  const errorMessage =
    submitted && !trimmedName
      ? t("quickCommands.categoryNameRequired")
      : hasDuplicateName
        ? t("quickCommands.categoryNameDuplicated")
        : "";

  return (
    <Dialog
      disablePointerDismissal
      open={!!draft}
      onOpenChange={(open) => {
        if (!open) onCancel();
      }}
    >
      <DialogContent
        key={draft?.parentId ?? "root"}
        showCloseButton={false}
        className="max-w-sm"
      >
        <form
          className="space-y-4"
          onSubmit={(event) => {
            event.preventDefault();
            setSubmitted(true);
            if (!trimmedName || hasDuplicateName) return;
            onConfirm(trimmedName);
          }}
        >
          <DialogHeader>
            <DialogTitle className="text-sm">
              {t("quickCommands.addCategory")}
            </DialogTitle>
            <DialogDescription className="text-xs">
              {parentLabel
                ? t("quickCommands.newCategoryParentHint", {
                    category: parentLabel,
                  })
                : t("quickCommands.newCategoryRootHint")}
            </DialogDescription>
          </DialogHeader>

          <div className="space-y-1.5">
            <div className="flex items-center justify-between gap-2">
              <Label
                htmlFor="new-quick-command-category-name"
                className="text-xs text-muted-foreground"
              >
                {t("quickCommands.categoryName")}
              </Label>
              {errorMessage && (
                <span className="text-[0.6875rem] text-destructive">
                  {errorMessage}
                </span>
              )}
            </div>
            <Input
              id="new-quick-command-category-name"
              autoFocus
              value={name}
              className={cn(
                "h-9 text-sm",
                errorMessage &&
                  "border-destructive focus-visible:ring-destructive",
              )}
              placeholder={t("quickCommands.categoryPlaceholder")}
              onChange={(event) => {
                setName(event.target.value);
                setSubmitted(false);
              }}
            />
          </div>

          <DialogFooter>
            <Button type="button" variant="outline" onClick={onCancel}>
              {t("common.cancel")}
            </Button>
            <Button type="submit" disabled={!trimmedName || hasDuplicateName}>
              {t("common.confirm")}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

function QuickCommands({
  onSend,
  onSendToAll,
  sendDisabled = false,
  currentSessionId,
}: QuickCommandsProps) {
  const { t } = useTranslation();
  const { appSettings, updateUi } = useApp();
  const nyascriptSupported = runtime === "desktop" && isWindows;
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
  const [newCategoryDraft, setNewCategoryDraft] =
    useState<NewQuickCommandCategoryDraft | null>(null);
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
  const [contextCommandId, setContextCommandId] = useState<string | null>(null);
  const [draggingCategoryId, setDraggingCategoryId] = useState<string | null>(
    null,
  );
  const [categoryDragTarget, setCategoryDragTarget] =
    useState<QuickCommandCategoryDragTarget | null>(null);
  const [draggingCommandId, setDraggingCommandId] = useState<string | null>(
    null,
  );
  const [commandDragTarget, setCommandDragTarget] =
    useState<QuickCommandDragTarget | null>(null);
  const categoryDragSourceRef = useRef<string | null>(null);
  const commandDragSourceRef = useRef<string | null>(null);

  // Variable Prompt State
  const [promptCmd, setPromptCmd] = useState<QuickCommand | null>(null);
  const [promptVars, setPromptVars] = useState<VariableDef[]>([]);
  const [promptSendToAll, setPromptSendToAll] = useState(false);
  const [nyascriptRun, setNyaScriptRun] = useState<NyaScriptRunStatus | null>(null);
  const nyascriptRunIdRef = useRef<string | null>(null);

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

  useEffect(() => {
    const unsub = listen<NyaScriptRunEvent>("nyascript-run-event", ({ payload }) => {
      if (payload.runId !== nyascriptRunIdRef.current) return;
      setNyaScriptRun((current) => {
        if (!current || current.runId !== payload.runId) return current;
        const logs = payload.log
          ? [...current.logs, payload.log].slice(-5)
          : current.logs;
        return {
          ...current,
          state: payload.state,
          currentLine: payload.currentLine ?? current.currentLine,
          activeAlias: payload.activeAlias ?? current.activeAlias,
          sessionId: payload.sessionId ?? current.sessionId,
          logs,
          error: payload.error ?? current.error,
        };
      });
    });
    return () => {
      unsub.then((dispose) => dispose());
    };
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

  const openNewCategoryDialog = useCallback((parentId: string | null) => {
    setNewCategoryDraft({ parentId });
  }, []);

  const handleConfirmCreateCategory = useCallback(
    (name: string) => {
      if (!newCategoryDraft) return;
      const newCategory: QuickCommandCategory = {
        id: randomUUID(),
        name,
        parent_id: newCategoryDraft.parentId || undefined,
        sort_order: getNextQuickCommandCategorySortOrder(
          savedCategories,
          newCategoryDraft.parentId,
        ),
      };
      setSavedCategories((prev) => [...prev, newCategory]);
      if (newCategory.parent_id) {
        setExpandedCategoryIds((prev) => new Set([...prev, newCategory.parent_id!]));
      }
      setNewCategoryDraft(null);
    },
    [newCategoryDraft, savedCategories],
  );

  const openNewCommandForCategory = useCallback((categoryId: string | null) => {
    openQuickCommand(undefined, { categoryId });
  }, []);

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

  const startNyaScript = useCallback(
    async (cmd: QuickCommand) => {
      if (!nyascriptSupported) {
        toast.error(t("quickCommands.nyascriptUnsupported"));
        return;
      }
      try {
        const runId = await invoke<string>("start_nyascript", {
          source: cmd.command,
          currentSessionId: currentSessionId ?? null,
        });
        nyascriptRunIdRef.current = runId;
        incrementUseCount(cmd.id);
        const status = await invoke<NyaScriptRunStatus>("get_nyascript_status", { runId });
        setNyaScriptRun(status);
      } catch (error) {
        toast.error(getErrorMessage(error));
      }
    },
    [currentSessionId, incrementUseCount, nyascriptSupported, t],
  );

  const cancelNyaScript = useCallback(() => {
    const runId = nyascriptRunIdRef.current;
    if (!runId) return;
    void invoke("cancel_nyascript", { runId }).catch((error) => {
      toast.error(getErrorMessage(error));
    });
  }, []);

  const commandDisabled = useCallback(
    (cmd: QuickCommand) =>
      cmd.execution_mode === "nyascript" ? !nyascriptSupported : sendDisabled,
    [nyascriptSupported, sendDisabled],
  );

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
      if (cmd.execution_mode === "nyascript") {
        void startNyaScript(cmd);
        return;
      }
      if (sendDisabled) return;
      incrementUseCount(cmd.id);
      const vars = parseCommandVariables(cmd.command);

      if (vars.length > 0) {
        setPromptCmd(cmd);
        setPromptVars(vars);
      } else {
        onSend(cmd.command, cmd.execution_mode !== "append");
      }
    },
    [incrementUseCount, onSend, sendDisabled, startNyaScript],
  );

  const handleSendToAll = useCallback(
    (cmd: QuickCommand) => {
      if (cmd.execution_mode === "nyascript" || !onSendToAll || sendDisabled) return;
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
    [incrementUseCount, onSendToAll, sendDisabled],
  );

  const handlePromptSubmit = useCallback(
    (resolvedCommand: string) => {
      if (sendDisabled) return;
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
    [onSend, onSendToAll, promptCmd, promptSendToAll, sendDisabled],
  );

  const handleAiPromptSubmit = useCallback(() => {
    if (sendDisabled) return;
    const userInput = aiPrompt.trim();
    if (!userInput) return;
    setAiPrompt("");
    setAiPopoverOpen(false);
    openAIAssistant({ action: "generate_command", userInput });
  }, [aiPrompt, sendDisabled]);

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

  const handleExportQuickCommands = useCallback(async () => {
    try {
      if (runtime === "web") {
        downloadJson("nyaterm-quick-commands.json", { commands, categories: savedCategories });
        toast.success(t("quickCommands.exportSuccess"));
        return;
      }
      const outputPath = await saveFileDialog({
        defaultPath: "nyaterm-quick-commands.json",
        filters: [{ name: "NyaTerm JSON", extensions: ["json"] }],
      });
      if (!outputPath) return;

      await invoke("export_quick_commands", {
        outputPath,
        config: { commands, categories: savedCategories },
      });
      toast.success(t("quickCommands.exportSuccess"));
    } catch (error) {
      logger.error({
        domain: "settings.persistence",
        event: "quick_commands.export_failed",
        message: "Export quick commands failed",
        error,
      });
      toast.error(t("quickCommands.exportFailed", { error: String(error) }));
    }
  }, [commands, savedCategories, t]);

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
  const newCommandCategoryId =
    selectedCategory !== "all" &&
    selectedCategory !== "uncategorized" &&
    savedCategories.some((category) => category.id === selectedCategory)
      ? selectedCategory
      : null;
  const contextCommand = contextCommandId
    ? (commands.find((command) => command.id === contextCommandId) ?? null)
    : null;
  const commandDragCategoryId =
    selectedCategory === "uncategorized"
      ? null
      : selectedCategory !== "all"
        ? selectedCategory
        : undefined;
  const canDragCommands =
    sortMode === "custom" &&
    !search.trim() &&
    selectedCategory !== "all" &&
    commandDragCategoryId !== undefined;
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
    let filtered = filterQuickCommandsByCategory(commands, selectedCategory);

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
    sorted.sort((a, b) => compareQuickCommandsByMode(a, b, sortMode));

    return sorted;
  }, [commands, search, selectedCategory, sortMode]);

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
  const newCategoryParentLabel = useMemo(() => {
    const parentId = newCategoryDraft?.parentId;
    return parentId
      ? buildQuickCommandCategoryPath(allCategories, parentId) || parentId
      : "";
  }, [allCategories, newCategoryDraft]);
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
  const resolveCategoryDropPosition = useCallback(
    (event: DragEvent<HTMLElement>): QuickCommandCategoryDropPosition => {
      const rect = event.currentTarget.getBoundingClientRect();
      const ratio = (event.clientY - rect.top) / Math.max(rect.height, 1);
      if (ratio < 0.25) return "before";
      if (ratio > 0.75) return "after";
      return "inside";
    },
    [],
  );
  const handleCategoryDragStart = useCallback(
    (event: DragEvent<HTMLElement>, categoryId: string) => {
      categoryDragSourceRef.current = categoryId;
      setDraggingCategoryId(categoryId);
      event.dataTransfer.effectAllowed = "move";
      event.dataTransfer.setData(QUICK_COMMAND_CATEGORY_DRAG_MIME, categoryId);
      event.dataTransfer.setData("text/plain", categoryId);
    },
    [],
  );
  const resetCategoryDrag = useCallback(() => {
    categoryDragSourceRef.current = null;
    setDraggingCategoryId(null);
    setCategoryDragTarget(null);
  }, []);
  const handleCategoryDragOver = useCallback(
    (event: DragEvent<HTMLElement>, targetCategoryId: string) => {
      const sourceId =
        categoryDragSourceRef.current ||
        event.dataTransfer.getData(QUICK_COMMAND_CATEGORY_DRAG_MIME);
      if (!sourceId) return;
      event.preventDefault();
      event.stopPropagation();
      event.dataTransfer.dropEffect = "move";
      setCategoryDragTarget({
        categoryId: targetCategoryId,
        position: resolveCategoryDropPosition(event),
      });
    },
    [resolveCategoryDropPosition],
  );
  const handleCategoryDrop = useCallback(
    (event: DragEvent<HTMLElement>, targetCategoryId: string) => {
      event.preventDefault();
      event.stopPropagation();
      const sourceId =
        categoryDragSourceRef.current ||
        event.dataTransfer.getData(QUICK_COMMAND_CATEGORY_DRAG_MIME);
      const position = resolveCategoryDropPosition(event);
      resetCategoryDrag();
      if (!sourceId) return;
      setSavedCategories((prev) =>
        moveQuickCommandCategoryToTarget(prev, sourceId, {
          categoryId: targetCategoryId,
          position,
        }),
      );
      if (position === "inside") {
        setExpandedCategoryIds((prev) => new Set([...prev, targetCategoryId]));
      }
    },
    [resetCategoryDrag, resolveCategoryDropPosition],
  );
  const handleCategoryRootDragOver = useCallback(
    (event: DragEvent<HTMLElement>) => {
      if (!categoryDragSourceRef.current) return;
      event.preventDefault();
      event.dataTransfer.dropEffect = "move";
      if (event.target === event.currentTarget) {
        setCategoryDragTarget({ categoryId: null, position: "inside" });
      }
    },
    [],
  );
  const handleCategoryRootDrop = useCallback(
    (event: DragEvent<HTMLElement>) => {
      event.preventDefault();
      const sourceId =
        categoryDragSourceRef.current ||
        event.dataTransfer.getData(QUICK_COMMAND_CATEGORY_DRAG_MIME);
      resetCategoryDrag();
      if (!sourceId) return;
      setSavedCategories((prev) =>
        moveQuickCommandCategoryToTarget(prev, sourceId, {
          categoryId: null,
          position: "inside",
        }),
      );
    },
    [resetCategoryDrag],
  );
  const handleCommandDragStart = useCallback(
    (event: DragEvent<HTMLElement>, commandId: string) => {
      if (!canDragCommands) return;
      commandDragSourceRef.current = commandId;
      setDraggingCommandId(commandId);
      event.dataTransfer.effectAllowed = "move";
      event.dataTransfer.setData(QUICK_COMMAND_DRAG_MIME, commandId);
      event.dataTransfer.setData("text/plain", commandId);
    },
    [canDragCommands],
  );
  const resetCommandDrag = useCallback(() => {
    commandDragSourceRef.current = null;
    setDraggingCommandId(null);
    setCommandDragTarget(null);
  }, []);
  const handleCommandDragOver = useCallback(
    (event: DragEvent<HTMLElement>, targetCommandId: string) => {
      if (!canDragCommands) return;
      const sourceId =
        commandDragSourceRef.current ||
        event.dataTransfer.getData(QUICK_COMMAND_DRAG_MIME);
      if (!sourceId || sourceId === targetCommandId) return;
      const sourceCommand = commands.find((command) => command.id === sourceId);
      const targetCommand = commands.find(
        (command) => command.id === targetCommandId,
      );
      if (
        !sourceCommand ||
        !targetCommand ||
        (commandDragCategoryId
          ? targetCommand.category_id !== commandDragCategoryId
          : !!targetCommand.category_id) ||
        Boolean(sourceCommand.pinned) !== Boolean(targetCommand.pinned)
      ) {
        return;
      }
      event.preventDefault();
      event.dataTransfer.dropEffect = "move";
      setCommandDragTarget({ commandId: targetCommandId });
    },
    [canDragCommands, commandDragCategoryId, commands],
  );
  const handleCommandDrop = useCallback(
    (event: DragEvent<HTMLElement>, targetCommandId: string) => {
      if (!canDragCommands || commandDragCategoryId === undefined) return;
      const targetCommand = commands.find(
        (command) => command.id === targetCommandId,
      );
      if (
        !targetCommand ||
        (commandDragCategoryId
          ? targetCommand.category_id !== commandDragCategoryId
          : !!targetCommand.category_id)
      ) {
        return;
      }
      event.preventDefault();
      const sourceId =
        commandDragSourceRef.current ||
        event.dataTransfer.getData(QUICK_COMMAND_DRAG_MIME);
      resetCommandDrag();
      if (!sourceId) return;
      setCommands((prev) =>
        reorderQuickCommandsWithinCategory(
          prev,
          sourceId,
          targetCommandId,
          commandDragCategoryId,
        ),
      );
    },
    [canDragCommands, commandDragCategoryId, commands, resetCommandDrag],
  );
  const canDragCommand = useCallback(
    (cmd: QuickCommand) =>
      canDragCommands &&
      (commandDragCategoryId
        ? cmd.category_id === commandDragCategoryId
        : !cmd.category_id),
    [canDragCommands, commandDragCategoryId],
  );
  const renderCategoryContextMenuContent = useCallback(
    (categoryId: string | null, options?: { includeEdit?: QuickCommandCategory }) => (
      <ContextMenuContent className="min-w-[140px]">
        <ContextMenuItem
          className="text-xs gap-2"
          onClick={() => openNewCategoryDialog(categoryId)}
        >
          <MdFolder className="text-[0.875rem]" />
          {t("quickCommands.addCategory")}
        </ContextMenuItem>
        <ContextMenuItem
          className="text-xs gap-2"
          onClick={() => openNewCommandForCategory(categoryId)}
        >
          <MdTerminal className="text-[0.875rem]" />
          {t("quickCommands.addCommand")}
        </ContextMenuItem>
        {options?.includeEdit && (
          <>
            <ContextMenuSeparator />
            <ContextMenuItem
              className="text-xs gap-2"
              onClick={() => setCategoryToRename(options.includeEdit ?? null)}
            >
              <MdEdit className="text-[0.875rem]" />
              {t("quickCommands.edit")}
            </ContextMenuItem>
            <ContextMenuItem
              className="text-xs gap-2 text-destructive focus:text-destructive"
              onClick={() => setCategoryToDelete(options.includeEdit ?? null)}
            >
              <MdDelete className="text-[0.875rem]" />
              {t("quickCommands.delete")}
            </ContextMenuItem>
          </>
        )}
      </ContextMenuContent>
    ),
    [openNewCategoryDialog, openNewCommandForCategory, t],
  );
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
        {cmd.execution_mode === "nyascript" ? (
          <MdTerminal className="text-[0.7rem]" />
        ) : cmd.execution_mode === "append" ? (
          <MdKeyboardReturn className="text-[0.7rem]" />
        ) : (
          <MdBolt className="text-[0.7rem]" />
        )}
        <span className="truncate">
          {cmd.execution_mode === "nyascript"
            ? t("quickCommands.nyascript")
            : cmd.execution_mode === "append"
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
          {onSendToAll && cmd.execution_mode !== "nyascript" && (
            <DropdownMenuItem
              disabled={sendDisabled}
              onClick={() => handleSendToAll(cmd)}
            >
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
    [handleSendToAll, onSendToAll, sendDisabled, t],
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
              disabled={commandDisabled(cmd)}
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
      commandDisabled,
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
        <ContextMenuItem
          className="text-xs gap-2"
          onClick={() => void handleCopyCommand(cmd.command)}
        >
          <MdContentCopy className="text-[0.875rem]" />
          {t("quickCommands.copyCommand")}
        </ContextMenuItem>
        {onSendToAll && cmd.execution_mode !== "nyascript" && (
          <ContextMenuItem
            className="text-xs gap-2"
            disabled={commandDisabled(cmd)}
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
    [commandDisabled, handleCopyCommand, handleSendToAll, onSendToAll, t],
  );
  const renderCommandListItem = useCallback(
    (cmd: QuickCommand) => {
      const draggable = canDragCommand(cmd);
      const isDragging = draggingCommandId === cmd.id;
      const isDropTarget = commandDragTarget?.commandId === cmd.id;
      return (
        <div
          key={cmd.id}
          data-quick-command-id={cmd.id}
          draggable={draggable}
          onDragStart={(event) => handleCommandDragStart(event, cmd.id)}
          onDragOver={(event) => handleCommandDragOver(event, cmd.id)}
          onDrop={(event) => handleCommandDrop(event, cmd.id)}
          onDragEnd={resetCommandDrag}
          className={cn(
            "group flex min-h-11 w-full min-w-0 items-center gap-2 rounded-md border border-border/35 bg-muted/15 px-2 py-1.5 text-xs transition-colors hover:bg-muted/45 hover:text-foreground",
            draggable && "cursor-grab active:cursor-grabbing",
            isDragging && "opacity-50",
            isDropTarget && "ring-1 ring-primary/70",
          )}
          style={{ color: "var(--df-text)" }}
        >
          <button
            type="button"
            className="flex min-w-0 flex-1 cursor-pointer items-center gap-2 rounded px-1 text-left"
            disabled={commandDisabled(cmd)}
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
      );
    },
    [
      canDragCommand,
      commandDisabled,
      commandDragTarget,
      draggingCommandId,
      handleCommandClick,
      handleCommandDragOver,
      handleCommandDragStart,
      handleCommandDrop,
      resetCommandDrag,
      renderCommandActions,
      renderCommandIcon,
    ],
  );
  const renderCommandCompactItem = useCallback(
    (cmd: QuickCommand) => {
      const draggable = canDragCommand(cmd);
      const isDragging = draggingCommandId === cmd.id;
      const isDropTarget = commandDragTarget?.commandId === cmd.id;
      return (
        <div
          key={cmd.id}
          data-quick-command-id={cmd.id}
          draggable={draggable}
          onDragStart={(event) => handleCommandDragStart(event, cmd.id)}
          onDragOver={(event) => handleCommandDragOver(event, cmd.id)}
          onDrop={(event) => handleCommandDrop(event, cmd.id)}
          onDragEnd={resetCommandDrag}
          className={cn(
            "group flex h-8 w-full min-w-0 items-center gap-1.5 rounded px-1.5 text-xs transition-colors hover:bg-muted/45 hover:text-foreground",
            draggable && "cursor-grab active:cursor-grabbing",
            isDragging && "opacity-50",
            isDropTarget && "ring-1 ring-primary/70",
          )}
          style={{ color: "var(--df-text)" }}
        >
          <button
            type="button"
            className="flex min-w-0 flex-1 cursor-pointer items-center gap-1.5 rounded px-0.5 text-left"
            disabled={commandDisabled(cmd)}
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
      );
    },
    [
      canDragCommand,
      commandDisabled,
      commandDragTarget,
      draggingCommandId,
      handleCommandClick,
      handleCommandDragOver,
      handleCommandDragStart,
      handleCommandDrop,
      resetCommandDrag,
      renderCommandActions,
      renderCommandIcon,
    ],
  );
  const renderCommandTile = useCallback(
    (cmd: QuickCommand) => {
      const categoryName = getCommandCategoryName(cmd);
      const draggable = canDragCommand(cmd);
      const isDragging = draggingCommandId === cmd.id;
      const isDropTarget = commandDragTarget?.commandId === cmd.id;

      return (
        <Tooltip key={cmd.id}>
          <TooltipTrigger asChild>
            <button
              type="button"
              data-quick-command-id={cmd.id}
              draggable={draggable}
              onDragStart={(event) => handleCommandDragStart(event, cmd.id)}
              onDragOver={(event) => handleCommandDragOver(event, cmd.id)}
              onDrop={(event) => handleCommandDrop(event, cmd.id)}
              onDragEnd={resetCommandDrag}
              className={cn(
                "group flex max-w-full shrink-0 cursor-pointer items-center gap-1.5 rounded-md border border-border/35 bg-muted/20 px-2 py-1 text-left text-[0.6875rem] font-medium text-foreground/80 transition-colors hover:bg-muted/50 hover:text-foreground",
                draggable && "cursor-grab active:cursor-grabbing",
                isDragging && "opacity-50",
                isDropTarget && "ring-1 ring-primary/70",
              )}
              style={{ color: "var(--df-text)" }}
              disabled={commandDisabled(cmd)}
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
                  {cmd.execution_mode === "nyascript" ? (
                    <MdTerminal className="text-[0.75rem]" />
                  ) : cmd.execution_mode === "append" ? (
                    <MdKeyboardReturn className="text-[0.75rem]" />
                  ) : (
                    <MdBolt className="text-[0.75rem]" />
                  )}
                  {cmd.execution_mode === "nyascript"
                    ? t("quickCommands.nyascript")
                    : cmd.execution_mode === "append"
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
      );
    },
    [
      canDragCommand,
      commandDisabled,
      commandDragTarget,
      draggingCommandId,
      getCommandCategoryName,
      handleCommandClick,
      handleCommandDragOver,
      handleCommandDragStart,
      handleCommandDrop,
      resetCommandDrag,
      renderCommandIcon,
      renderCommandPreview,
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
                    <DropdownMenuRadioItem value="custom" className="text-xs">
                      {t("quickCommands.sortByCustom")}
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
                      onClick={() =>
                        openNewCommandForCategory(newCommandCategoryId)
                      }
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
                      aria-label={t("quickCommands.export")}
                      onClick={() => void handleExportQuickCommands()}
                    >
                      <BiExport className="text-[1.05rem]" />
                    </Button>
                  </TooltipTrigger>
                  <TooltipContent side="top">
                    {t("quickCommands.export")}
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
                      disabled={sendDisabled}
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
                        disabled={sendDisabled || !aiPrompt.trim()}
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

        {nyascriptRun && (
          <div
            data-testid="nyascript-run-status"
            className="flex shrink-0 items-center gap-2 border-b border-border/40 bg-muted/20 px-2.5 py-1.5 text-[0.6875rem]"
          >
            <MdTerminal className="shrink-0 text-sm text-primary" />
            <span className="font-medium">
              {t(`quickCommands.nyascriptState.${nyascriptRun.state}`)}
            </span>
            {nyascriptRun.activeAlias && (
              <span className="text-muted-foreground">{nyascriptRun.activeAlias}</span>
            )}
            {nyascriptRun.currentLine && (
              <span className="text-muted-foreground">
                {t("quickCommands.nyascriptLine", { line: nyascriptRun.currentLine })}
              </span>
            )}
            <span className="min-w-0 flex-1 truncate text-muted-foreground">
              {nyascriptRun.error || nyascriptRun.logs[nyascriptRun.logs.length - 1] || ""}
            </span>
            {nyascriptRun.state === "running" && (
              <Button size="xs" variant="outline" onClick={cancelNyaScript}>
                {t("common.cancel")}
              </Button>
            )}
          </div>
        )}

        <div className="flex min-h-0 flex-1">
          <aside
            className="shrink-0 overflow-y-auto overflow-x-hidden p-1.5 terminal-scroll flex flex-col"
            style={{ width: categorySidebarWidth }}
            onDragOver={handleCategoryRootDragOver}
            onDrop={handleCategoryRootDrop}
          >
            <div className="flex shrink-0 flex-col gap-1">
              {(() => {
                const active = selectedCategory === "all";
                return (
                  <ContextMenu>
                    <ContextMenuTrigger asChild>
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
                    </ContextMenuTrigger>
                    {renderCategoryContextMenuContent(null)}
                  </ContextMenu>
                );
              })()}

              {categoryDragTarget?.categoryId === null && (
                <div className="h-0.5 rounded-full bg-primary/70" />
              )}

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
                const categoryTargetPosition =
                  categoryDragTarget?.categoryId === category.id
                    ? categoryDragTarget.position
                    : null;
                const categoryIsDragging = draggingCategoryId === category.id;

                return (
                  <ContextMenu key={category.id}>
                    <ContextMenuTrigger asChild disabled={!savedCategory}>
                      <div className="relative">
                        {categoryTargetPosition === "before" && (
                          <span className="absolute left-1 right-1 top-0 z-10 h-0.5 rounded-full bg-primary/70" />
                        )}
                        {categoryTargetPosition === "after" && (
                          <span className="absolute bottom-0 left-1 right-1 z-10 h-0.5 rounded-full bg-primary/70" />
                        )}
                        <div
                          draggable={!!savedCategory}
                          onDragStart={(event) => {
                            if (savedCategory)
                              handleCategoryDragStart(event, savedCategory.id);
                          }}
                          onDragOver={(event) =>
                            handleCategoryDragOver(event, category.id)
                          }
                          onDrop={(event) => handleCategoryDrop(event, category.id)}
                          onDragEnd={resetCategoryDrag}
                          className={cn(
                            "group flex h-8 w-full min-w-0 items-center rounded-md text-xs transition-colors hover:bg-[var(--df-bg-hover)]",
                            savedCategory && "cursor-grab active:cursor-grabbing",
                            categoryIsDragging && "opacity-50",
                            categoryTargetPosition === "inside" &&
                              "ring-1 ring-primary/70",
                          )}
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
                              {node.count}
                            </span>
                          </button>
                        </div>
                      </div>
                    </ContextMenuTrigger>
                    {savedCategory && (
                      <ContextMenuContent className="min-w-[140px]">
                        <ContextMenuItem
                          className="text-xs gap-2"
                          onClick={() => openNewCategoryDialog(savedCategory.id)}
                        >
                          <MdFolder className="text-[0.875rem]" />
                          {t("quickCommands.addCategory")}
                        </ContextMenuItem>
                        <ContextMenuItem
                          className="text-xs gap-2"
                          onClick={() => openNewCommandForCategory(savedCategory.id)}
                        >
                          <MdTerminal className="text-[0.875rem]" />
                          {t("quickCommands.addCommand")}
                        </ContextMenuItem>
                        <ContextMenuSeparator />
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
                  <ContextMenu>
                    <ContextMenuTrigger asChild>
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
                    </ContextMenuTrigger>
                    {renderCategoryContextMenuContent(null)}
                  </ContextMenu>
                );
              })()}
            </div>
            <ContextMenu>
              <ContextMenuTrigger asChild>
                <div
                  data-testid="quick-command-category-empty-area"
                  className="min-h-6 flex-1"
                />
              </ContextMenuTrigger>
              <ContextMenuContent className="min-w-[140px]">
                <ContextMenuItem
                  className="text-xs gap-2"
                  onClick={() => openNewCategoryDialog(null)}
                >
                  <MdFolder className="text-[0.875rem]" />
                  {t("quickCommands.addCategory")}
                </ContextMenuItem>
              </ContextMenuContent>
            </ContextMenu>
          </aside>
          <ResizeHandle
            direction="horizontal"
            onResize={resizeCategorySidebar}
            className="opacity-70 hover:opacity-100 active:opacity-100"
          />

          <ContextMenu>
            <ContextMenuTrigger asChild>
              <div
                data-testid="quick-command-pane"
                className="min-w-0 flex-1 overflow-y-auto overflow-x-hidden terminal-scroll p-1.5"
                onContextMenu={(event) => {
                  const target =
                    event.target instanceof Element ? event.target : null;
                  const item = target?.closest<HTMLElement>(
                    "[data-quick-command-id]",
                  );
                  setContextCommandId(item?.dataset.quickCommandId ?? null);
                }}
              >
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
                        onClick={() =>
                          openNewCommandForCategory(newCommandCategoryId)
                        }
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
            </ContextMenuTrigger>
            {contextCommand ? (
              renderContextMenuContent(contextCommand)
            ) : (
              <ContextMenuContent className="min-w-[140px]">
                <ContextMenuItem
                  className="text-xs gap-2"
                  onClick={() =>
                    openNewCommandForCategory(newCommandCategoryId)
                  }
                >
                  <MdTerminal className="text-[0.875rem]" />
                  {t("quickCommands.addCommand")}
                </ContextMenuItem>
              </ContextMenuContent>
            )}
          </ContextMenu>
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
        <NewQuickCommandCategoryDialog
          draft={newCategoryDraft}
          categories={allCategories}
          parentLabel={newCategoryParentLabel}
          onCancel={() => setNewCategoryDraft(null)}
          onConfirm={handleConfirmCreateCategory}
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
