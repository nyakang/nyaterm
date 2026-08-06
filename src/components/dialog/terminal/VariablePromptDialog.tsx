import { useEffect, useState } from "react";
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
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";

export interface VariableDef {
  raw: string;
  name: string;
  options?: string[];
  defaultValue?: string;
}

interface VariablePromptDialogProps {
  open: boolean;
  command: string;
  variables: VariableDef[];
  onCancel: () => void;
  onSubmit: (resolvedCommand: string) => void;
}

export default function VariablePromptDialog({
  open,
  command,
  variables,
  onCancel,
  onSubmit,
}: VariablePromptDialogProps) {
  const { t } = useTranslation();

  const [values, setValues] = useState<Record<string, string>>({});

  useEffect(() => {
    if (open) {
      const initial: Record<string, string> = {};

      variables.forEach((v) => {
        initial[v.name] = v.defaultValue || (v.options && v.options.length > 0 ? v.options[0] : "");
      });
      setValues(initial);
    }
  }, [open, variables]);

  const handleSubmit = () => {
    let finalCmd = command;
    variables.forEach((v) => {
      finalCmd = finalCmd.split(v.raw).join(values[v.name] || "");
    });
    onSubmit(finalCmd);
  };

  return (
    <Dialog disablePointerDismissal open={open} onOpenChange={(v) => !v && onCancel()}>
      <DialogContent className="w-[min(400px,calc(100vw-2rem))] sm:max-w-[400px] p-0 gap-0">
        <DialogHeader className="px-5 py-3 border-b">
          <DialogTitle className="text-sm">{t("quickCommands.fillVariables")}</DialogTitle>
          <DialogDescription className="sr-only">
            {t("quickCommands.fillVariables")}
          </DialogDescription>
        </DialogHeader>

        <div className="p-5 space-y-4 max-h-[60vh] overflow-y-auto">
          {variables.map((v) => (
            <div key={v.name}>
              <Label className="text-[0.6875rem] text-muted-foreground">{v.name}</Label>
              {v.options && v.options.length > 0 ? (
                <Select
                  value={values[v.name] || ""}
                  onValueChange={(val) => setValues({ ...values, [v.name]: val })}
                >
                  <SelectTrigger className="mt-1 h-8 text-xs">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {v.options.map((opt) => (
                      <SelectItem key={opt} value={opt} className="text-xs">
                        {opt}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              ) : (
                <Input
                  className="mt-1 text-xs h-8"
                  value={values[v.name] || ""}
                  onChange={(e) => setValues({ ...values, [v.name]: e.target.value })}
                  autoFocus
                  onKeyDown={(e) => {
                    if (e.key === "Enter") {
                      handleSubmit();
                    }
                  }}
                />
              )}
            </div>
          ))}

          <div className="bg-muted/50 p-2 rounded relative mt-4">
            <Label className="text-[0.625rem] text-muted-foreground absolute -top-2 left-2 px-1 bg-popover">
              Preview
            </Label>
            <div className="text-[0.6875rem] font-mono break-all text-muted-foreground mt-2">
              {(() => {
                let preview = command;
                variables.forEach((v) => {
                  preview = preview.split(v.raw).join(values[v.name] || "");
                });
                return preview || <span className="opacity-50">Empty command</span>;
              })()}
            </div>
          </div>
        </div>

        <DialogFooter className="px-5 py-3 border-t">
          <Button variant="ghost" size="sm" className="text-xs" onClick={onCancel}>
            {t("dialog.cancel")}
          </Button>
          <Button size="sm" className="text-xs" onClick={handleSubmit}>
            {t("quickCommands.run")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

export function parseCommandVariables(command: string): VariableDef[] {
  const regex = /\{\{([^}]+)\}\}/g;
  const matches = [...command.matchAll(regex)];

  const vars: VariableDef[] = [];
  const seen = new Set<string>();

  for (const match of matches) {
    const raw = match[0];
    const content = match[1];

    if (seen.has(raw)) continue;
    seen.add(raw);

    if (content.includes("|")) {
      const [name, optsStr] = content.split("|");
      const options = optsStr.split(",").map((s) => s.trim());
      vars.push({ raw, name: name.trim(), options });
    } else if (content.includes("=")) {
      const [name, defaultVal] = content.split("=");
      vars.push({ raw, name: name.trim(), defaultValue: defaultVal.trim() });
    } else {
      vars.push({ raw, name: content.trim() });
    }
  }

  return vars;
}
