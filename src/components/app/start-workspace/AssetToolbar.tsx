import type { TFunction } from "i18next";
import { Grid2X2, List, Search, X } from "lucide-react";
import type { AssetViewMode } from "./types";

interface AssetToolbarProps {
  t: TFunction;
  totalCount: number;
  search: string;
  onSearchChange: (value: string) => void;
  availableTags: string[];
  selectedTags: Set<string>;
  onToggleTag: (tag: string) => void;
  onClearTags: () => void;
  viewMode: AssetViewMode;
  onViewModeChange: (mode: AssetViewMode) => void;
}

export default function AssetToolbar({
  t,
  totalCount,
  search,
  onSearchChange,
  availableTags,
  selectedTags,
  onToggleTag,
  onClearTags,
  viewMode,
  onViewModeChange,
}: AssetToolbarProps) {
  return (
    <div className="shrink-0 space-y-3 px-5 pb-3" data-total-count={totalCount}>
      <div className="relative">
        <Search
          className="pointer-events-none absolute left-3 top-1/2 size-4 -translate-y-1/2"
          style={{ color: "var(--df-text-dimmed)" }}
        />
        <input
          value={search}
          onChange={(event) => onSearchChange(event.target.value)}
          placeholder={t("assets.searchPlaceholder")}
          autoCapitalize="none"
          autoCorrect="off"
          autoComplete="off"
          spellCheck={false}
          className="h-9 w-full rounded-md border bg-transparent pl-9 pr-8 text-sm outline-none transition-colors placeholder:text-[var(--df-text-dimmed)] focus:border-[var(--df-primary)]"
          style={{
            borderColor: "var(--df-border)",
            color: "var(--df-text)",
            backgroundColor: "color-mix(in srgb, var(--df-bg-hover) 55%, transparent)",
          }}
        />
        {search ? (
          <button
            type="button"
            aria-label={t("common.close")}
            className="absolute right-2 top-1/2 flex size-5 -translate-y-1/2 items-center justify-center rounded hover:bg-[var(--df-bg-hover)]"
            style={{ color: "var(--df-text-muted)" }}
            onClick={() => onSearchChange("")}
          >
            <X className="size-3.5" />
          </button>
        ) : null}
      </div>

      <div className="flex min-w-0 items-center gap-2">
        <div className="flex min-w-0 flex-1 items-center gap-1.5">
          <FilterButton active={selectedTags.size === 0} onClick={onClearTags}>
            {t("assets.all")}
          </FilterButton>
          {availableTags.length > 0 ? (
            <div
              data-asset-tag-filters
              className="min-w-0 flex-1 overflow-x-auto overflow-y-hidden"
            >
              <div className="flex w-max items-center gap-1.5 pr-1">
                {availableTags.map((tag) => (
                  <FilterButton
                    key={tag}
                    active={selectedTags.has(tag)}
                    onClick={() => onToggleTag(tag)}
                  >
                    {tag}
                  </FilterButton>
                ))}
              </div>
            </div>
          ) : null}
        </div>

        <div className="flex shrink-0 items-center gap-1">
          <IconToggleButton
            label={t("assets.list")}
            active={viewMode === "list"}
            onClick={() => onViewModeChange("list")}
          >
            <List className="size-4" />
          </IconToggleButton>
          <IconToggleButton
            label={t("assets.cards")}
            active={viewMode === "cards"}
            onClick={() => onViewModeChange("cards")}
          >
            <Grid2X2 className="size-4" />
          </IconToggleButton>
        </div>
      </div>
    </div>
  );
}

function FilterButton({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      aria-pressed={active}
      onClick={onClick}
      className="h-7 shrink-0 whitespace-nowrap rounded-md border px-2.5 text-xs transition-colors"
      style={{
        borderColor: active ? "var(--df-primary)" : "var(--df-border)",
        color: active ? "var(--df-primary)" : "var(--df-text-muted)",
        backgroundColor: active
          ? "color-mix(in srgb, var(--df-primary) 12%, transparent)"
          : "transparent",
      }}
    >
      {children}
    </button>
  );
}

function IconToggleButton({
  label,
  active,
  onClick,
  children,
}: {
  label: string;
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      aria-label={label}
      aria-pressed={active}
      title={label}
      className="flex size-7 items-center justify-center rounded-md border transition-colors"
      style={{
        borderColor: active ? "var(--df-primary)" : "transparent",
        color: active ? "var(--df-primary)" : "var(--df-text-muted)",
        backgroundColor: active
          ? "color-mix(in srgb, var(--df-primary) 10%, transparent)"
          : "transparent",
      }}
      onClick={onClick}
    >
      {children}
    </button>
  );
}
