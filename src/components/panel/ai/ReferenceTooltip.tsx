import type { ReactElement } from "react";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";

interface ReferenceTooltipProps {
  text: string;
  children: ReactElement;
  disabled?: boolean;
  triggerClassName?: string;
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
}

/** 复用项目浮层主题；原生 title 无法跟随主题，也不能可靠展示长路径。 */
export function ReferenceTooltip({
  text,
  children,
  disabled = false,
  triggerClassName,
  open,
  onOpenChange,
}: ReferenceTooltipProps) {
  const trigger = disabled ? (
    // biome-ignore lint/a11y/noNoninteractiveTabindex: 禁用项仍需键盘可读提示；此处只读，不赋予选择动作。
    <span tabIndex={0} className={triggerClassName}>
      {children}
    </span>
  ) : (
    children
  );
  return (
    <Tooltip delayDuration={250} disableHoverableContent open={open} onOpenChange={onOpenChange}>
      <TooltipTrigger asChild>{trigger}</TooltipTrigger>
      <TooltipContent
        side="top"
        align="start"
        collisionPadding={12}
        className="pointer-events-none max-w-[min(26rem,calc(100vw-2rem))] whitespace-pre-wrap border-border text-[0.6875rem] leading-5 shadow-lg [overflow-wrap:anywhere]"
      >
        {text.split("\n").map((line, index) => (
          <div
            key={`${index}:${line}`}
            className={index === 0 ? "font-mono" : "mt-1 text-muted-foreground"}
          >
            {line}
          </div>
        ))}
      </TooltipContent>
    </Tooltip>
  );
}
