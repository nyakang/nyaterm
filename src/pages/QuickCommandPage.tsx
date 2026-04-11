import { invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { type CSSProperties, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { MdAdd } from "react-icons/md";
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
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Switch } from "@/components/ui/switch";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import { parseJsonSearchParam } from "@/lib/utils";
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

export default function QuickCommandPage() {
  const { t } = useTranslation();
  const params = new URLSearchParams(window.location.search);
  const dataParam = params.get("data");
  const initialData = parseJsonSearchParam<QuickCommand>(dataParam);

  const [savedCategories, setSavedCategories] = useState<QuickCommandCategory[]>([]);
  const [label, setLabel] = useState(initialData?.label || "");
  const [command, setCommand] = useState(initialData?.command || "");
  const [categoryId, setCategoryId] = useState(initialData?.category_id || "none");
  const [categorySearchQuery, setCategorySearchQuery] = useState("");
  const [showCategoryDropdown, setShowCategoryDropdown] = useState(false);
  const [description, setDescription] = useState(initialData?.description || "");
  const [colorTag, setColorTag] = useState(initialData?.color_tag || "default");
  const [iconTag, setIconTag] = useState<string | undefined>(initialData?.icon_tag);
  const [pinned, setPinned] = useState(initialData?.pinned || false);
  const [executionMode, setExecutionMode] = useState<"execute" | "append">(
    (initialData?.execution_mode as "execute" | "append") || "execute",
  );
  const [errors, setErrors] = useState<{ label?: string; command?: string; general?: string }>({});

  useEffect(() => {
    invoke<QuickCommandsConfig>("get_quick_commands")
      .then((cfg) => setSavedCategories(cfg.categories || []))
      .catch(() => {});
  }, []);

  const filteredCategories = savedCategories.filter((c) =>
    c.name.toLowerCase().includes(categorySearchQuery.toLowerCase()),
  );
  const exactMatchExists = savedCategories.some(
    (c) => c.name.toLowerCase() === categorySearchQuery.trim().toLowerCase(),
  );

  const handleSave = async () => {
    const newErrors: { label?: string; command?: string } = {};
    if (!label.trim()) {
      newErrors.label = t("quickCommands.errorLabelRequired");
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
    if (categoryId === "new" && categorySearchQuery.trim()) {
      const newId = crypto.randomUUID();
      newCategory = { id: newId, name: categorySearchQuery.trim() };
      finalCategoryId = newId;
    }

    const cmd: QuickCommand = {
      id: initialData?.id || `qc-${Date.now()}`,
      label: label.trim(),
      command: command.trim(),
      category_id: finalCategoryId,
      description: description.trim() || undefined,
      color_tag: colorTag === "default" && !iconTag ? undefined : colorTag,
      icon_tag: iconTag,
      pinned,
      execution_mode: executionMode,
    };

    await emit("quick-command-saved", { command: cmd, newCategory });
    getCurrentWindow().close();
  };

  const handleClose = () => getCurrentWindow().close();

  return (
    <div className="h-full min-h-0 flex flex-col overflow-hidden bg-background text-foreground">
      <ChildWindowHeader
        title={initialData ? t("quickCommands.editCommand") : t("quickCommands.addCommand")}
        onClose={handleClose}
      />

      <div className="terminal-scroll flex-1 min-h-0 space-y-5 overflow-y-auto p-4 sm:p-5">
        <div className="flex flex-col gap-4 md:flex-row">
          <div className="min-w-0 flex-1 space-y-1.5">
            <div className="flex justify-between items-center">
              <Label htmlFor="qc-label" className="text-xs text-muted-foreground">
                {t("quickCommands.labelName")}
              </Label>
              {errors.label && (
                <span className="text-[0.6875rem] text-destructive">{errors.label}</span>
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
            <Label htmlFor="qc-category" className="text-xs text-muted-foreground">
              {t("quickCommands.category")}
            </Label>
            <Popover
              open={showCategoryDropdown}
              onOpenChange={(open) => {
                setShowCategoryDropdown(open);
                if (!open) setCategorySearchQuery("");
              }}
            >
              <PopoverTrigger asChild>
                <Button
                  type="button"
                  variant="outline"
                  className="w-full h-9 justify-between text-sm font-normal px-3"
                >
                  <span className={categoryId !== "none" ? "" : "text-muted-foreground truncate"}>
                    {categoryId === "new"
                      ? categorySearchQuery.trim()
                      : categoryId === "none"
                        ? t("quickCommands.uncategorized")
                        : savedCategories.find((c) => c.id === categoryId)?.name || categoryId}
                  </span>
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
                      if (e.key === "Enter" && categorySearchQuery.trim() && !exactMatchExists) {
                        setCategoryId("new");
                        setShowCategoryDropdown(false);
                        e.preventDefault();
                      }
                    }}
                  />
                </div>
                <div className="max-h-48 overflow-y-auto terminal-scroll py-1">
                  {!categorySearchQuery && (
                    <div
                      className={`px-3 py-2 text-sm cursor-pointer transition-colors hover:bg-accent ${categoryId === "none" ? "bg-primary/15 text-primary" : "text-muted-foreground"}`}
                      onClick={() => {
                        setCategoryId("none");
                        setShowCategoryDropdown(false);
                        setCategorySearchQuery("");
                      }}
                    >
                      {t("quickCommands.uncategorized")}
                    </div>
                  )}
                  {filteredCategories.map((c) => (
                    <div
                      key={c.id}
                      className={`px-3 py-2 text-sm cursor-pointer transition-colors hover:bg-accent ${categoryId === c.id ? "bg-primary/15 text-primary" : ""}`}
                      onClick={() => {
                        setCategoryId(c.id);
                        setShowCategoryDropdown(false);
                        setCategorySearchQuery("");
                      }}
                    >
                      {c.name}
                    </div>
                  ))}
                  {categorySearchQuery.trim() && !exactMatchExists && (
                    <div
                      className="px-3 py-2 text-sm cursor-pointer transition-colors hover:bg-accent text-primary flex items-center"
                      onClick={() => {
                        setCategoryId("new");
                        setShowCategoryDropdown(false);
                      }}
                    >
                      {t("quickCommands.createCategory", { name: categorySearchQuery.trim() })}
                    </div>
                  )}
                </div>
              </PopoverContent>
            </Popover>
          </div>
        </div>

        {/* Description */}
        <div className="space-y-1.5">
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
        <div className="flex flex-row items-center gap-4 border p-2 rounded-md bg-muted/20">
          <div className="min-w-0 flex-1 space-y-1">
            <Label className="text-[0.6875rem] text-muted-foreground">{t("quickCommands.colorTag")}</Label>
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
                    return <iconDef.icon className="text-xs" style={{ color: iconDef.color }} />;
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
                        <iconDef.icon className="text-base" style={{ color: iconDef.color }} />
                      </DropdownMenuItem>
                    ))}
                  </div>
                </DropdownMenuContent>
              </DropdownMenu>
            </div>
          </div>
          <div className="flex flex-col items-end gap-1.5 shrink-0 pl-4 border-l">
            <Label htmlFor="qc-pinned" className="text-[0.6875rem] text-muted-foreground mr-1 cursor-pointer select-none">
              {t("quickCommands.pin")}
            </Label>
            <Switch checked={pinned} onCheckedChange={setPinned} id="qc-pinned" className="mr-1" />
          </div>
        </div>

        {/* Execution Mode */}
        <div className="space-y-1.5">
          <Label className="text-xs text-muted-foreground">
            {t("quickCommands.executionMode")}
          </Label>
          <Tabs
            value={executionMode}
            onValueChange={(val) => setExecutionMode(val as "execute" | "append")}
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
        <div className="space-y-1.5">
          <div className="flex justify-between items-center">
            <Label htmlFor="qc-command" className="text-xs text-muted-foreground">
              {t("quickCommands.commandScript")}
            </Label>
            {errors.command && (
              <span className="text-[0.6875rem] text-destructive">{errors.command}</span>
            )}
          </div>
          <Textarea
            id="qc-command"
            className={`font-mono text-sm resize-none h-28 bg-muted/30 ${errors.command ? "border-destructive focus-visible:ring-destructive" : ""}`}
            style={{ fieldSizing: "fixed" } as CSSProperties}
            placeholder={t("quickCommands.commandPlaceholder")}
            value={command}
            onChange={(e) => {
              setCommand(e.target.value);
              setErrors((p) => ({ ...p, command: undefined }));
            }}
          />
        </div>

        {errors.general && (
          <div className="text-sm text-destructive bg-destructive/10 p-2.5 rounded border border-destructive/30">
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
        <Button size="sm" className="text-xs px-4" onClick={handleSave}>
          {t("dialog.save")}
        </Button>
      </div>
    </div>
  );
}
