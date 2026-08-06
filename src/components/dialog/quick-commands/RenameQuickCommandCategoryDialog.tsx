import { type FormEvent, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { hasQuickCommandCategorySiblingName } from "@/lib/quickCommandCategories";
import type { QuickCommandCategory } from "@/types/global";

interface RenameQuickCommandCategoryDialogProps {
  category: QuickCommandCategory | null;
  categories: QuickCommandCategory[];
  onCancel: () => void;
  onConfirm: (name: string) => void;
}

export default function RenameQuickCommandCategoryDialog({
  category,
  categories,
  onCancel,
  onConfirm,
}: RenameQuickCommandCategoryDialogProps) {
  const { t } = useTranslation();
  const [name, setName] = useState("");
  const [submitted, setSubmitted] = useState(false);

  useEffect(() => {
    setName(category?.name ?? "");
    setSubmitted(false);
  }, [category]);

  const trimmedName = name.trim();
  const hasDuplicateName = category
    ? hasQuickCommandCategorySiblingName(
        categories,
        category.parent_id ?? null,
        trimmedName,
        category.id,
      )
    : false;
  const isUnchanged = !!category && trimmedName === category.name;
  const errorMessage =
    submitted && !trimmedName
      ? t("quickCommands.categoryNameRequired")
      : hasDuplicateName
        ? t("quickCommands.categoryNameDuplicated")
        : "";

  const handleSubmit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    setSubmitted(true);

    if (!category || !trimmedName || hasDuplicateName) return;
    onConfirm(trimmedName);
  };

  return (
    <Dialog
      disablePointerDismissal
      open={!!category}
      onOpenChange={(open) => {
        if (!open) onCancel();
      }}
    >
      <DialogContent showCloseButton={false} className="max-w-sm">
        <form className="space-y-4" onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle className="text-sm">
              {t("quickCommands.renameCategory")}
            </DialogTitle>
            <DialogDescription className="sr-only">
              {t("quickCommands.renameCategory")}
            </DialogDescription>
          </DialogHeader>

          <div className="space-y-1.5">
            <div className="flex items-center justify-between gap-2">
              <Label
                htmlFor="quick-command-category-name"
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
              id="quick-command-category-name"
              autoFocus
              value={name}
              className={`h-9 text-sm ${
                errorMessage
                  ? "border-destructive focus-visible:ring-destructive"
                  : ""
              }`}
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
            <Button
              type="submit"
              disabled={!trimmedName || hasDuplicateName || isUnchanged}
            >
              {t("common.confirm")}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
