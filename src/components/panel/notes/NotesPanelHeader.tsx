import type React from "react";
import { MdAdd, MdClose, MdCreateNewFolder, MdMoreVert, MdRefresh, MdSearch } from "react-icons/md";
import PanelHeader from "@/components/layout/PanelHeader";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";

interface NotesPanelHeaderProps {
  title: string;
  search: string;
  onSearchChange: (value: string) => void;
  onNewNote: () => void;
  onNewFolder: () => void;
  onExpandAll: () => void;
  onCollapseAll: () => void;
  onRefresh: () => void;
  onExport: () => void;
  labels: {
    search: string;
    newNote: string;
    newFolder: string;
    expandAll: string;
    collapseAll: string;
    refresh: string;
    more: string;
    export: string;
  };
}

function HeaderIconButton({
  label,
  onClick,
  children,
}: {
  label: string;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          aria-label={label}
          type="button"
          variant="ghost"
          size="icon-sm"
          className="h-6 w-6 rounded-md p-0 text-[var(--df-text-muted)] hover:bg-[var(--df-bg-hover)]"
          onClick={onClick}
        >
          {children}
        </Button>
      </TooltipTrigger>
      <TooltipContent side="top">{label}</TooltipContent>
    </Tooltip>
  );
}

export default function NotesPanelHeader({
  title,
  search,
  onSearchChange,
  onNewNote,
  onNewFolder,
  onExpandAll,
  onCollapseAll,
  onRefresh,
  onExport,
  labels,
}: NotesPanelHeaderProps) {
  return (
    <>
      <PanelHeader title={title} />
      <div
        className="nyaterm-wallpaper-transparent-surface flex shrink-0 items-center gap-1.5 border-b px-2 py-1.5"
        style={{
          borderColor: "color-mix(in srgb, var(--df-border) 40%, transparent)",
          backgroundColor: "var(--df-bg-section-header)",
        }}
      >
        <div className="relative min-w-0 flex-1 text-[var(--df-text-dimmed)] transition-colors focus-within:text-[var(--df-primary)]">
          <MdSearch className="pointer-events-none absolute left-2.5 top-1/2 -translate-y-1/2 text-sm" />
          <input
            value={search}
            onChange={(event) => onSearchChange(event.target.value)}
            placeholder={labels.search}
            autoCapitalize="none"
            autoCorrect="off"
            spellCheck={false}
            className="h-7 w-full rounded-md border border-transparent bg-[var(--df-bg-hover)] py-1 pl-8 pr-7 text-xs text-[var(--df-text)] outline-none transition-all placeholder:text-[var(--df-text-dimmed)] focus:border-[var(--df-primary)] focus:bg-transparent focus:ring-1 focus:ring-[var(--df-primary)]"
          />
          {search ? (
            <button
              type="button"
              aria-label={labels.search}
              className="absolute right-1.5 top-1/2 -translate-y-1/2 rounded p-0.5 text-[var(--df-text-dimmed)] transition-colors hover:text-[var(--df-text)]"
              onClick={() => onSearchChange("")}
            >
              <MdClose className="text-xs" />
            </button>
          ) : null}
        </div>
        <HeaderIconButton label={labels.newFolder} onClick={onNewFolder}>
          <MdCreateNewFolder className="text-base" />
        </HeaderIconButton>
        <HeaderIconButton label={labels.newNote} onClick={onNewNote}>
          <MdAdd className="text-lg" />
        </HeaderIconButton>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button
              aria-label={labels.more}
              type="button"
              variant="ghost"
              size="icon-sm"
              className="h-6 w-6 rounded-md p-0 text-[var(--df-text-muted)] hover:bg-[var(--df-bg-hover)] data-[state=open]:bg-[var(--df-bg-hover)]"
            >
              <MdMoreVert className="text-lg" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="w-36 text-xs">
            <DropdownMenuItem onClick={onExport}>{labels.export}</DropdownMenuItem>
            <DropdownMenuItem onClick={onExpandAll}>{labels.expandAll}</DropdownMenuItem>
            <DropdownMenuItem onClick={onCollapseAll}>{labels.collapseAll}</DropdownMenuItem>
            <DropdownMenuItem onClick={onRefresh}>
              <MdRefresh className="text-sm" />
              {labels.refresh}
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
    </>
  );
}
