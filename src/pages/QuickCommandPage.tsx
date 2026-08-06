import { emit } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  type CSSProperties,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import { MdAdd, MdCheck, MdExpandMore } from "react-icons/md";
import { QUICK_ICONS } from "@/components/icons";
import ChildWindowHeader from "@/components/layout/ChildWindowHeader";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { Switch } from "@/components/ui/switch";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import { getErrorMessage } from "@/lib/errors";
import { invoke } from "@/lib/invoke";
import {
  buildQuickCommandCategoryPath,
  buildQuickCommandCategoryTree,
  flattenVisibleQuickCommandCategoryTree,
  getNextQuickCommandCategorySortOrder,
  hasQuickCommandCategorySiblingName,
} from "@/lib/quickCommandCategories";
import { cn, parseJsonSearchParam } from "@/lib/utils";
import type { QuickCommand, QuickCommandCategory } from "@/types/global";

interface QuickCommandsConfig {
  commands: QuickCommand[];
  categories: QuickCommandCategory[];
}

const THEME_COLORS = ["default", "red", "green", "blue", "yellow", "purple"];
const COLOR_CLASSES: Record<string, string> = {
  default: "bg-secondary",
  red: "bg-red-400",
  green: "bg-green-400",
  blue: "bg-blue-400",
  yellow: "bg-yellow-400",
  purple: "bg-purple-400",
};

const SCRIPT_EDITOR_LINE_HEIGHT = 20;

interface QuickCommandScriptEditorProps {
  value: string;
  placeholder: string;
  invalid?: boolean;
  onChange: (value: string) => void;
}

function QuickCommandScriptEditor({
  value,
  placeholder,
  invalid,
  onChange,
}: QuickCommandScriptEditorProps) {
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);
  const gutterRef = useRef<HTMLDivElement | null>(null);
  const measureLineRefs = useRef<Array<HTMLDivElement | null>>([]);
  const [lineHeights, setLineHeights] = useState<number[]>([]);
  const lines = useMemo(() => value.split("\n"), [value]);
  const lineRows = useMemo(() => {
    let offset = 0;
    return lines.map((text) => {
      const row = { offset, text };
      offset += text.length + 1;
      return row;
    });
  }, [lines]);

  const syncGutterScroll = useCallback(() => {
    if (!textareaRef.current || !gutterRef.current) return;
    gutterRef.current.scrollTop = textareaRef.current.scrollTop;
  }, []);

  const measureLineHeights = useCallback(() => {
    const nextHeights = lineRows.map(
      (_line, index) =>
        measureLineRefs.current[index]?.offsetHeight ||
        SCRIPT_EDITOR_LINE_HEIGHT,
    );

    setLineHeights((current) => {
      if (
        current.length === nextHeights.length &&
        current.every((height, index) => height === nextHeights[index])
      ) {
        return current;
      }
      return nextHeights;
    });
    syncGutterScroll();
  }, [lineRows, syncGutterScroll]);

  useEffect(() => {
    const textarea = textareaRef.current;
    if (!textarea || typeof ResizeObserver === "undefined") return;

    let animationFrame = 0;
    const observer = new ResizeObserver(() => {
      cancelAnimationFrame(animationFrame);
      animationFrame = requestAnimationFrame(measureLineHeights);
    });
    observer.observe(textarea);
    return () => {
      cancelAnimationFrame(animationFrame);
      observer.disconnect();
    };
  }, [measureLineHeights]);

  useLayoutEffect(() => {
    measureLineHeights();
  }, [measureLineHeights]);

  measureLineRefs.current = [];

  return (
    <div
      className={cn(
        "relative min-h-28 flex-1 overflow-hidden rounded-md border border-input bg-muted/30 text-sm shadow-xs transition-[color,box-shadow]",
        "focus-within:border-ring focus-within:ring-[3px] focus-within:ring-ring/50",
        invalid && "border-destructive focus-within:ring-destructive",
      )}
    >
      <div
        ref={gutterRef}
        className="pointer-events-none absolute inset-y-0 left-0 z-20 w-10 overflow-hidden border-r border-border/40 bg-muted/40 py-2"
        aria-hidden="true"
      >
        {lineRows.map((line, index) => (
          <div
            key={`line-number-${line.offset}`}
            className="pr-2 text-right font-mono text-[0.6875rem] leading-5 text-muted-foreground/70"
            style={{ height: lineHeights[index] || SCRIPT_EDITOR_LINE_HEIGHT }}
          >
            {index + 1}
          </div>
        ))}
      </div>
      <div
        className="pointer-events-none invisible absolute inset-y-0 left-10 right-0 overflow-hidden px-3 py-2 font-mono text-sm leading-5 whitespace-pre-wrap break-words"
        aria-hidden="true"
      >
        {lineRows.map((line, index) => (
          <div
            key={`line-measure-${line.offset}`}
            ref={(node) => {
              measureLineRefs.current[index] = node;
            }}
            className="min-h-5 whitespace-pre-wrap break-words"
          >
            {line.text || "\u200b"}
          </div>
        ))}
      </div>
      <Textarea
        ref={textareaRef}
        id="qc-command"
        className="relative z-10 min-h-28 h-full flex-1 resize-none overflow-auto border-0 bg-transparent py-2 pl-[3.25rem] pr-3 font-mono text-sm leading-5 shadow-none focus-visible:border-0 focus-visible:ring-0"
        style={{ fieldSizing: "fixed" } as CSSProperties}
        placeholder={placeholder}
        value={value}
        aria-invalid={invalid || undefined}
        onScroll={syncGutterScroll}
        onChange={(event) => onChange(event.target.value)}
      />
    </div>
  );
}

export default function QuickCommandPage() {
  const { t } = useTranslation();
  const params = new URLSearchParams(window.location.search);
  const dataParam = params.get("data");
  const initialData = parseJsonSearchParam<QuickCommand>(dataParam);

  const [savedCategories, setSavedCategories] = useState<
    QuickCommandCategory[]
  >([]);
  const [label, setLabel] = useState(initialData?.label || "");
  const [command, setCommand] = useState(initialData?.command || "");
  const [categoryId, setCategoryId] = useState(
    initialData?.category_id || "none",
  );
  const [categorySearchQuery, setCategorySearchQuery] = useState("");
  const [newCategoryDraftName, setNewCategoryDraftName] = useState("");
  const [newCategoryName, setNewCategoryName] = useState("");
  const [newCategoryParentId, setNewCategoryParentId] = useState<string | null>(
    null,
  );
  const [showCategoryDropdown, setShowCategoryDropdown] = useState(false);
  const [description, setDescription] = useState(
    initialData?.description || "",
  );
  const [colorTag, setColorTag] = useState(initialData?.color_tag || "default");
  const [iconTag, setIconTag] = useState<string | undefined>(
    initialData?.icon_tag,
  );
  const [pinned, setPinned] = useState(initialData?.pinned || false);
  const [executionMode, setExecutionMode] = useState<"execute" | "append">(
    (initialData?.execution_mode as "execute" | "append") || "execute",
  );
  const [errors, setErrors] = useState<{
    label?: string;
    category?: string;
    command?: string;
    general?: string;
  }>({});

  const [saving, setSaving] = useState(false);

  useEffect(() => {
    invoke<QuickCommandsConfig>("get_quick_commands")
      .then((cfg) => setSavedCategories(cfg.categories || []))
      .catch((e) => setErrors((p) => ({ ...p, general: getErrorMessage(e) })));
  }, []);

  const categoryTreeRows = useMemo(() => {
    const tree = buildQuickCommandCategoryTree(savedCategories, []);
    return flattenVisibleQuickCommandCategoryTree(
      tree,
      new Set(savedCategories.map((category) => category.id)),
    );
  }, [savedCategories]);
  const categorySearchText = categorySearchQuery.trim().toLowerCase();
  const filteredCategoryRows = categorySearchText
    ? categoryTreeRows.filter(({ node }) => {
        const path = buildQuickCommandCategoryPath(
          savedCategories,
          node.category.id,
        );
        return (
          node.category.name.toLowerCase().includes(categorySearchText) ||
          path.toLowerCase().includes(categorySearchText)
        );
      })
    : categoryTreeRows;
  const newCategoryDraftParentId =
    categoryId !== "none" && categoryId !== "new" ? categoryId : null;
  const newCategoryDraftParentLabel = newCategoryDraftParentId
    ? buildQuickCommandCategoryPath(savedCategories, newCategoryDraftParentId) ||
      newCategoryDraftParentId
    : "";
  const newCategoryDraftDuplicate = hasQuickCommandCategorySiblingName(
    savedCategories,
    newCategoryDraftParentId,
    newCategoryDraftName,
  );
  const newCategoryParentLabel = newCategoryParentId
    ? buildQuickCommandCategoryPath(savedCategories, newCategoryParentId) ||
      newCategoryParentId
    : t("quickCommands.rootCategory");
  const selectedCategoryLabel =
    categoryId === "new"
      ? newCategoryParentId
        ? `${newCategoryParentLabel} / ${newCategoryName}`
        : newCategoryName
      : categoryId === "none"
        ? t("quickCommands.uncategorized")
        : buildQuickCommandCategoryPath(savedCategories, categoryId) ||
          categoryId;
  const newCategoryDuplicate =
    categoryId === "new" &&
    hasQuickCommandCategorySiblingName(
      savedCategories,
      newCategoryParentId,
      newCategoryName,
    );

  const selectExistingCategory = (id: string) => {
    setCategoryId(id);
    setNewCategoryName("");
    setNewCategoryParentId(null);
    setErrors((p) => ({ ...p, category: undefined }));
    setShowCategoryDropdown(false);
    setCategorySearchQuery("");
  };
  const selectNewCategory = () => {
    const name = newCategoryDraftName.trim();
    if (!name) return;
    if (newCategoryDraftDuplicate) {
      setErrors((p) => ({
        ...p,
        category: t("quickCommands.categoryNameDuplicated"),
      }));
      return;
    }
    setNewCategoryName(name);
    setNewCategoryParentId(newCategoryDraftParentId);
    setCategoryId("new");
    setErrors((p) => ({ ...p, category: undefined }));
    setShowCategoryDropdown(false);
    setCategorySearchQuery("");
    setNewCategoryDraftName("");
  };

  const handleSave = async () => {
    const newErrors: { label?: string; category?: string; command?: string } =
      {};
    if (!label.trim()) {
      newErrors.label = t("quickCommands.errorLabelRequired");
    }
    if (categoryId === "new" && !newCategoryName.trim()) {
      newErrors.category = t("quickCommands.categoryNameRequired");
    } else if (newCategoryDuplicate) {
      newErrors.category = t("quickCommands.categoryNameDuplicated");
    }
    if (!command.trim()) {
      newErrors.command = t("quickCommands.errorCommandRequired");
    }
    if (Object.keys(newErrors).length > 0) {
      setErrors(newErrors);
      return;
    }

    let finalCategoryId = categoryId === "none" ? undefined : categoryId;
    let newCategory: QuickCommandCategory | undefined;
    if (categoryId === "new" && newCategoryName.trim()) {
      const newId = crypto.randomUUID();
      newCategory = {
        id: newId,
        name: newCategoryName.trim(),
        parent_id: newCategoryParentId || undefined,
        sort_order: getNextQuickCommandCategorySortOrder(
          savedCategories,
          newCategoryParentId,
        ),
      };
      finalCategoryId = newId;
    }

    const now = Date.now();
    const cmd: QuickCommand = {
      id: initialData?.id || `qc-${now}`,
      label: label.trim(),
      command,
      category_id: finalCategoryId,
      description: description.trim() || undefined,
      color_tag: colorTag === "default" && !iconTag ? undefined : colorTag,
      icon_tag: iconTag,
      pinned,
      execution_mode: executionMode,
      updated_at: now,
      created_at: initialData?.created_at ?? now,
      use_count: initialData?.use_count,
    };

    setSaving(true);
    try {
      await invoke("upsert_quick_command", { command: cmd, newCategory });
      await emit("quick-command-saved", { command: cmd, newCategory });
      getCurrentWindow().close();
    } catch (e) {
      setErrors((p) => ({ ...p, general: getErrorMessage(e) }));
    } finally {
      setSaving(false);
    }
  };

  const handleClose = () => getCurrentWindow().close();

  return (
    <div className="h-full min-h-0 flex flex-col overflow-hidden bg-background text-foreground">
      <ChildWindowHeader
        title={
          initialData
            ? t("quickCommands.editCommand")
            : t("quickCommands.addCommand")
        }
        onClose={handleClose}
      />

      <div className="terminal-scroll flex min-h-0 flex-1 flex-col gap-5 overflow-y-auto p-4 sm:p-5">
        <div className="flex shrink-0 flex-col gap-4 md:flex-row">
          <div className="min-w-0 flex-1 space-y-1.5">
            <div className="flex justify-between items-center">
              <Label
                htmlFor="qc-label"
                className="text-xs text-muted-foreground"
              >
                {t("quickCommands.labelName")}
              </Label>
              {errors.label && (
                <span className="text-[0.6875rem] text-destructive">
                  {errors.label}
                </span>
              )}
            </div>
            <Input
              id="qc-label"
              className={`text-sm h-9 ${errors.label ? "border-destructive focus-visible:ring-destructive" : ""}`}
              placeholder={t("quickCommands.labelPlaceholder")}
              value={label}
              onChange={(e) => {
                setLabel(e.target.value);
                setErrors((p) => ({ ...p, label: undefined }));
              }}
            />
          </div>

          <div className="min-w-0 flex-1 space-y-1.5">
            <div className="flex justify-between items-center">
              <Label
                htmlFor="qc-category"
                className="text-xs text-muted-foreground"
              >
                {t("quickCommands.category")}
              </Label>
              {errors.category && (
                <span className="text-[0.6875rem] text-destructive">
                  {errors.category}
                </span>
              )}
            </div>
            <Popover
              open={showCategoryDropdown}
              onOpenChange={(open) => {
                setShowCategoryDropdown(open);
                if (!open) {
                  setCategorySearchQuery("");
                  setNewCategoryDraftName("");
                }
              }}
            >
              <PopoverTrigger asChild>
                <Button
                  type="button"
                  variant="outline"
                  className="w-full h-9 justify-between text-sm font-normal px-3"
                >
                  <span
                    className={
                      categoryId !== "none"
                        ? "truncate"
                        : "text-muted-foreground truncate"
                    }
                  >
                    {selectedCategoryLabel}
                  </span>
                  <MdExpandMore className="shrink-0 text-xs text-muted-foreground" />
                </Button>
              </PopoverTrigger>
              <PopoverContent
                className="p-0 flex flex-col shadow-xl"
                style={{ width: "var(--radix-popover-trigger-width)" }}
                align="start"
                sideOffset={4}
              >
                <div className="p-1 border-b">
                  <Input
                    autoFocus
                    className="h-8 text-sm bg-transparent border-none focus-visible:ring-0 shadow-none px-2"
                    placeholder={t("quickCommands.searchOrCreateCategory")}
                    value={categorySearchQuery}
                    onChange={(e) => setCategorySearchQuery(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter" && categorySearchQuery.trim()) {
                        const exactRow = filteredCategoryRows.find(
                          ({ node }) => {
                            const path = buildQuickCommandCategoryPath(
                              savedCategories,
                              node.category.id,
                            );
                            return (
                              node.category.name.toLowerCase() ===
                                categorySearchText ||
                              path.toLowerCase() === categorySearchText
                            );
                          },
                        );
                        if (exactRow) {
                          selectExistingCategory(exactRow.node.category.id);
                        }
                        e.preventDefault();
                      }
                    }}
                  />
                </div>
                <div className="max-h-64 overflow-y-auto terminal-scroll py-1">
                  {!categorySearchQuery && (
                    <button
                      type="button"
                      className={`flex w-full min-w-0 items-center gap-2 px-3 py-2 text-left text-sm transition-colors hover:bg-accent ${categoryId === "none" ? "bg-primary/15 text-primary" : "text-muted-foreground"}`}
                      onClick={() => {
                        setCategoryId("none");
                        setNewCategoryName("");
                        setNewCategoryParentId(null);
                        setErrors((p) => ({ ...p, category: undefined }));
                        setShowCategoryDropdown(false);
                        setCategorySearchQuery("");
                      }}
                    >
                      <span className="min-w-0 flex-1 truncate">
                        {t("quickCommands.uncategorized")}
                      </span>
                      {categoryId === "none" && (
                        <MdCheck className="shrink-0 text-sm text-primary" />
                      )}
                    </button>
                  )}
                  {filteredCategoryRows.map(({ node, depth }) => (
                    <button
                      type="button"
                      key={node.category.id}
                      className={`flex w-full min-w-0 items-center gap-2 py-2 pr-3 text-left text-sm transition-colors hover:bg-accent ${categoryId === node.category.id ? "bg-primary/15 text-primary" : ""}`}
                      style={{ paddingLeft: `${depth * 0.85 + 0.75}rem` }}
                      onClick={() => selectExistingCategory(node.category.id)}
                    >
                      <span
                        className="block min-w-0 flex-1 truncate"
                        title={buildQuickCommandCategoryPath(
                          savedCategories,
                          node.category.id,
                        )}
                      >
                        {node.category.name}
                      </span>
                      {categoryId === node.category.id && (
                        <MdCheck className="shrink-0 text-sm text-primary" />
                      )}
                    </button>
                  ))}
                </div>
                <div className="border-t p-1.5">
                  <div className="flex items-center gap-1.5">
                    <Input
                      className="h-7 min-w-0 flex-1 text-xs"
                      placeholder={t("quickCommands.newCategoryPlaceholder")}
                      value={newCategoryDraftName}
                      onChange={(e) => {
                        setNewCategoryDraftName(e.target.value);
                        setErrors((p) => ({ ...p, category: undefined }));
                      }}
                      onKeyDown={(e) => {
                        if (
                          e.key === "Enter" &&
                          newCategoryDraftName.trim() &&
                          !newCategoryDraftDuplicate
                        ) {
                          selectNewCategory();
                          e.preventDefault();
                        }
                      }}
                    />
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon-xs"
                      disabled={
                        !newCategoryDraftName.trim() ||
                        newCategoryDraftDuplicate
                      }
                      onClick={selectNewCategory}
                    >
                      <MdAdd className="text-sm" />
                    </Button>
                  </div>
                  <p className="px-1 pt-1 text-[0.6875rem] leading-snug text-muted-foreground">
                    {newCategoryDraftDuplicate
                      ? t("quickCommands.categoryNameDuplicated")
                      : newCategoryDraftParentLabel
                        ? t("quickCommands.newCategoryParentHint", {
                            category: newCategoryDraftParentLabel,
                          })
                        : t("quickCommands.newCategoryRootHint")}
                  </p>
                </div>
              </PopoverContent>
            </Popover>
          </div>
        </div>

        {/* Description */}
        <div className="shrink-0 space-y-1.5">
          <Label htmlFor="qc-desc" className="text-xs text-muted-foreground">
            {t("quickCommands.description")}
          </Label>
          <Input
            id="qc-desc"
            className="text-sm h-9"
            placeholder={t("quickCommands.descriptionPlaceholder")}
            value={description}
            onChange={(e) => setDescription(e.target.value)}
          />
        </div>

        {/* Color Tag & Pinned */}
        <div className="flex shrink-0 flex-row items-center gap-4 border p-2 rounded-md bg-muted/20">
          <div className="min-w-0 flex-1 space-y-1">
            <Label className="text-[0.6875rem] text-muted-foreground">
              {t("quickCommands.colorTag")}
            </Label>
            <div className="flex min-w-0 flex-wrap items-center gap-2">
              {THEME_COLORS.map((color) => (
                <button
                  key={color}
                  type="button"
                  onClick={() => {
                    setColorTag(color);
                    setIconTag(undefined);
                  }}
                  className={`w-6 h-6 rounded-full border-2 focus:outline-none transition-all ${
                    colorTag === color && !iconTag
                      ? "border-foreground scale-110 shadow-sm"
                      : "border-transparent hover:scale-105"
                  } ${COLOR_CLASSES[color]}`}
                  title={color}
                />
              ))}
              {iconTag && QUICK_ICONS[iconTag] && (
                <div className="w-6 h-6 rounded-full border-2 border-foreground scale-110 shadow-sm flex items-center justify-center bg-secondary">
                  {(() => {
                    const iconDef = QUICK_ICONS[iconTag];
                    return (
                      <iconDef.icon
                        className="text-xs"
                        style={{ color: iconDef.color }}
                      />
                    );
                  })()}
                </div>
              )}
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <button
                    type="button"
                    className="w-6 h-6 rounded-full border-2 border-dashed border-muted-foreground/50 hover:border-foreground flex items-center justify-center transition-all hover:scale-110 ml-1"
                    title={t("quickCommands.selectIcon")}
                  >
                    <MdAdd className="text-xs" />
                  </button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end" className="p-2 w-48">
                  <div className="grid grid-cols-6 gap-1 max-h-48 overflow-y-auto terminal-scroll">
                    {Object.entries(QUICK_ICONS).map(([name, iconDef]) => (
                      <DropdownMenuItem
                        key={name}
                        className="p-1 cursor-pointer flex items-center justify-center hover:bg-secondary rounded"
                        onSelect={() => {
                          setIconTag(name);
                          setColorTag("default");
                        }}
                      >
                        <iconDef.icon
                          className="text-base"
                          style={{ color: iconDef.color }}
                        />
                      </DropdownMenuItem>
                    ))}
                  </div>
                </DropdownMenuContent>
              </DropdownMenu>
            </div>
          </div>
          <div className="flex flex-col items-end gap-1.5 shrink-0 pl-4 border-l">
            <Label
              htmlFor="qc-pinned"
              className="text-[0.6875rem] text-muted-foreground mr-1 cursor-pointer select-none"
            >
              {t("quickCommands.pin")}
            </Label>
            <Switch
              checked={pinned}
              onCheckedChange={setPinned}
              id="qc-pinned"
              className="mr-1"
            />
          </div>
        </div>

        {/* Execution Mode */}
        <div className="shrink-0 space-y-1.5">
          <Label className="text-xs text-muted-foreground">
            {t("quickCommands.executionMode")}
          </Label>
          <Tabs
            value={executionMode}
            onValueChange={(val) =>
              setExecutionMode(val as "execute" | "append")
            }
            className="w-full"
          >
            <TabsList className="grid w-full grid-cols-2 h-8">
              <TabsTrigger value="execute" className="text-xs">
                {t("quickCommands.executeImmediately")}
              </TabsTrigger>
              <TabsTrigger value="append" className="text-xs">
                {t("quickCommands.appendOnly")}
              </TabsTrigger>
            </TabsList>
          </Tabs>
          <p className="text-[0.6875rem] text-muted-foreground pl-1 mt-1">
            {executionMode === "execute"
              ? t("quickCommands.executeHint")
              : t("quickCommands.appendHint")}
          </p>
        </div>

        {/* Script / Command */}
        <div className="flex min-h-32 flex-1 flex-col gap-1.5">
          <div className="flex justify-between items-center">
            <Label
              htmlFor="qc-command"
              className="text-xs text-muted-foreground"
            >
              {t("quickCommands.commandScript")}
            </Label>
            {errors.command && (
              <span className="text-[0.6875rem] text-destructive">
                {errors.command}
              </span>
            )}
          </div>
          <QuickCommandScriptEditor
            placeholder={t("quickCommands.commandPlaceholder")}
            value={command}
            invalid={Boolean(errors.command)}
            onChange={(value) => {
              setCommand(value);
              setErrors((p) => ({ ...p, command: undefined }));
            }}
          />
        </div>

        {errors.general && (
          <div className="shrink-0 text-sm text-destructive bg-destructive/10 p-2.5 rounded border border-destructive/30">
            {errors.general}
          </div>
        )}
      </div>

      {/* Footer */}
      <div className="flex shrink-0 flex-row gap-2 border-t px-5 py-3 justify-end items-center">
        <Button
          variant="ghost"
          size="sm"
          className="text-xs px-4"
          onClick={handleClose}
        >
          {t("dialog.cancel")}
        </Button>
        <Button
          size="sm"
          className="text-xs px-4"
          onClick={handleSave}
          disabled={saving}
        >
          {saving ? t("dialog.saving") : t("dialog.save")}
        </Button>
      </div>
    </div>
  );
}
