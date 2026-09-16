import { X } from "lucide-react";
import { useCallback, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Badge } from "@/components/ui/badge";
import { Label } from "@/components/ui/label";

interface ConnectionTagsFieldProps {
  value: string[];
  suggestions: string[];
  onChange: (tags: string[]) => void;
}

export function ConnectionTagsField({ value, suggestions, onChange }: ConnectionTagsFieldProps) {
  const { t } = useTranslation();
  const inputRef = useRef<HTMLInputElement>(null);
  const [inputValue, setInputValue] = useState("");
  const [focused, setFocused] = useState(false);
  const [activeSuggestion, setActiveSuggestion] = useState(0);

  const filteredSuggestions = useMemo(() => {
    const query = inputValue.trim().toLowerCase();
    const seen = new Set<string>();

    return suggestions
      .map((tag) => tag.trim())
      .filter((tag) => {
        if (!tag || value.includes(tag) || seen.has(tag)) return false;
        seen.add(tag);
        return true;
      })
      .filter((tag) => !query || tag.toLowerCase().includes(query))
      .sort((left, right) => left.localeCompare(right));
  }, [inputValue, suggestions, value]);

  const addTag = useCallback(
    (candidate: string) => {
      const tag = candidate.trim();
      if (!tag) return;
      if (value.includes(tag)) {
        setInputValue("");
        setActiveSuggestion(0);
        return;
      }
      onChange([...value, tag]);
      setInputValue("");
      setActiveSuggestion(0);
    },
    [onChange, value],
  );

  const removeTag = useCallback(
    (tag: string) => {
      onChange(value.filter((current) => current !== tag));
    },
    [onChange, value],
  );

  return (
    <div className="relative">
      <Label className="text-xs font-medium text-foreground/80">{t("dialog.tags")}</Label>
      <div
        className="mt-1 flex min-h-8 w-full flex-wrap items-center gap-1 rounded-md border border-input bg-transparent px-2 py-1 shadow-xs transition-[color,box-shadow] focus-within:border-ring focus-within:ring-[3px] focus-within:ring-ring/50"
        onClick={() => inputRef.current?.focus()}
      >
        {value.map((tag) => (
          <Badge key={tag} variant="secondary" className="h-5 gap-0.5 px-1.5 text-[0.6875rem]">
            <span className="max-w-48 truncate">{tag}</span>
            <button
              type="button"
              className="ml-0.5 flex size-3.5 shrink-0 items-center justify-center rounded-sm text-muted-foreground hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              aria-label={t("dialog.removeTag", { tag })}
              onClick={(event) => {
                event.stopPropagation();
                removeTag(tag);
                inputRef.current?.focus();
              }}
            >
              <X className="size-2.5" />
            </button>
          </Badge>
        ))}
        <input
          ref={inputRef}
          role="combobox"
          aria-expanded={focused && filteredSuggestions.length > 0}
          aria-controls="connection-tag-suggestions"
          aria-autocomplete="list"
          aria-label={t("dialog.tagsPlaceholder")}
          className="h-5 min-w-32 flex-1 bg-transparent px-1 text-xs outline-none placeholder:text-muted-foreground"
          placeholder={value.length === 0 ? t("dialog.tagsPlaceholder") : undefined}
          value={inputValue}
          onFocus={() => setFocused(true)}
          onBlur={() => setFocused(false)}
          onChange={(event) => {
            setInputValue(event.target.value);
            setActiveSuggestion(0);
          }}
          onKeyDown={(event) => {
            if (event.nativeEvent.isComposing || event.key === "Process") return;

            if (event.key === "ArrowDown" && filteredSuggestions.length > 0) {
              event.preventDefault();
              event.stopPropagation();
              setActiveSuggestion((current) => (current + 1) % filteredSuggestions.length);
              return;
            }
            if (event.key === "ArrowUp" && filteredSuggestions.length > 0) {
              event.preventDefault();
              event.stopPropagation();
              setActiveSuggestion(
                (current) =>
                  (current - 1 + filteredSuggestions.length) % filteredSuggestions.length,
              );
              return;
            }
            if (event.key === "Enter") {
              event.preventDefault();
              event.stopPropagation();
              addTag(filteredSuggestions[activeSuggestion] ?? inputValue);
              return;
            }
            if (event.key === ",") {
              event.preventDefault();
              event.stopPropagation();
              addTag(inputValue);
              return;
            }
            if (event.key === "Backspace" && !inputValue && value.length > 0) {
              event.preventDefault();
              event.stopPropagation();
              onChange(value.slice(0, -1));
            }
          }}
        />
      </div>

      {focused && filteredSuggestions.length > 0 ? (
        <div
          id="connection-tag-suggestions"
          role="listbox"
          className="absolute z-40 mt-1 max-h-40 w-full overflow-y-auto rounded-md border bg-popover p-1 text-popover-foreground shadow-md"
        >
          {filteredSuggestions.map((tag, index) => (
            <button
              key={tag}
              type="button"
              role="option"
              aria-selected={index === activeSuggestion}
              className="flex w-full items-center rounded-sm px-2 py-1.5 text-left text-xs hover:bg-accent aria-selected:bg-accent"
              onMouseDown={(event) => event.preventDefault()}
              onClick={() => addTag(tag)}
              onMouseEnter={() => setActiveSuggestion(index)}
            >
              {tag}
            </button>
          ))}
        </div>
      ) : null}
    </div>
  );
}
