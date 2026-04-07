import { useTranslation } from "react-i18next";
import {
  MdContentCopy,
  MdCopyAll,
  MdDelete,
  MdDownload,
  MdDriveFileMove,
  MdDriveFolderUpload,
  MdEdit,
  MdFileOpen,
  MdFolderCopy,
  MdInfo,
  MdKeyboardArrowRight,
  MdKeyboardDoubleArrowRight,
  MdKeyboardReturn,
  MdOpenInNew,
  MdRefresh,
  MdUpload,
} from "react-icons/md";
import { getFileIcon } from "@/components/icons";
import { formatSize } from "@/lib/utils";
import type { FileEntry } from "@/types/global";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuSub,
  ContextMenuSubContent,
  ContextMenuSubTrigger,
  ContextMenuTrigger,
} from "../ui/context-menu";

interface FileListItemProps {
  entry: FileEntry;
  isSelected: boolean;
  activeSessionId: string | null;
  onSelect: (entry: FileEntry, event: React.MouseEvent) => void;
  onItemClick: (entry: FileEntry) => void;
  onOpenDefault: (entry: FileEntry) => void;
  onRefresh: () => void;
  onUpload: () => void;
  onUploadFolder: () => void;
  onDownload: (entry: FileEntry) => void;
  onRename: (entry: FileEntry) => void;
  onMove: (entry: FileEntry) => void;
  onDelete: (entry: FileEntry) => void;
  onCopyPath: (entry: FileEntry, mode: "dir" | "name" | "full") => void;
  onSendToTerminal: (entry: FileEntry, mode: "dir" | "name" | "full") => void;
  onProperties: (entry: FileEntry) => void;
}

export function FileListItem({
  entry,
  isSelected,
  activeSessionId,
  onSelect,
  onItemClick,
  onOpenDefault,
  onRefresh,
  onUpload,
  onUploadFolder,
  onDownload,
  onRename,
  onMove,
  onDelete,
  onCopyPath,
  onSendToTerminal,
  onProperties,
}: FileListItemProps) {
  const { t } = useTranslation();
  const entryIcon = getFileIcon(entry);

  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>
        <li
          className="flex items-center gap-2 px-2 py-1 rounded cursor-pointer transition-colors"
          style={{
            backgroundColor: isSelected
              ? "color-mix(in srgb, var(--df-primary) 10%, transparent)"
              : undefined,
            color: isSelected ? "var(--df-primary)" : "var(--df-text)",
          }}
          onMouseEnter={(e) => {
            if (!isSelected) e.currentTarget.style.backgroundColor = "var(--df-bg-hover)";
          }}
          onMouseLeave={(e) => {
            if (!isSelected) e.currentTarget.style.backgroundColor = "";
          }}
          onClick={(e) => onSelect(entry, e)}
          onDoubleClick={() => {
            if (entry.is_dir) {
              onItemClick(entry);
            } else {
              onOpenDefault(entry);
            }
          }}
          onContextMenu={(e) => onSelect(entry, e)}
          title={`${entry.permissions} ${formatSize(entry.size)}`}
        >
          <entryIcon.icon
            className="text-base"
            style={{ color: isSelected ? "var(--df-primary)" : entryIcon.color }}
          />
          <span className="flex-1 truncate text-xs">{entry.name}</span>
          {!entry.is_dir && (
            <span className="text-[0.625rem]" style={{ color: "var(--df-text-dimmed)" }}>
              {formatSize(entry.size)}
            </span>
          )}
        </li>
      </ContextMenuTrigger>
      <ContextMenuContent className="min-w-[200px]">
        <ContextMenuItem onClick={() => onItemClick(entry)}>
          <MdFileOpen className="text-[0.875rem] text-muted-foreground mr-2" />
          {t("fileExplorer.cmOpen")}
        </ContextMenuItem>
        {!entry.is_dir && (
          <ContextMenuItem onClick={() => onOpenDefault(entry)}>
            <MdOpenInNew className="text-[0.875rem] text-muted-foreground mr-2" />
            {t("fileExplorer.cmOpenDefault")}
          </ContextMenuItem>
        )}
        <ContextMenuSeparator />
        <ContextMenuItem onClick={onRefresh}>
          <MdRefresh className="text-[0.875rem] text-muted-foreground mr-2" />
          {t("fileExplorer.cmRefresh")}
        </ContextMenuItem>
        <ContextMenuSub>
          <ContextMenuSubTrigger>
            <MdUpload className="text-[0.875rem] text-muted-foreground mr-2" />
            {t("fileExplorer.cmUpload")}
          </ContextMenuSubTrigger>
          <ContextMenuSubContent className="w-48">
            <ContextMenuItem onClick={onUpload}>
              <MdUpload className="text-[0.875rem] text-muted-foreground mr-2" />
              {t("fileExplorer.upload")}
            </ContextMenuItem>
            <ContextMenuItem onClick={onUploadFolder}>
              <MdDriveFolderUpload className="text-[0.875rem] text-muted-foreground mr-2" />
              {t("fileExplorer.uploadFolder")}
            </ContextMenuItem>
          </ContextMenuSubContent>
        </ContextMenuSub>
        <ContextMenuItem onClick={() => onDownload(entry)}>
          <MdDownload className="text-[0.875rem] text-muted-foreground mr-2" />
          {t("fileExplorer.cmDownload")}
        </ContextMenuItem>
        <ContextMenuSeparator />
        <ContextMenuItem onClick={() => activeSessionId && onRename(entry)}>
          <MdEdit className="text-[0.875rem] text-muted-foreground mr-2" />
          {t("fileExplorer.cmRename")}
        </ContextMenuItem>
        <ContextMenuItem onClick={() => activeSessionId && onMove(entry)}>
          <MdDriveFileMove className="text-[0.875rem] text-muted-foreground mr-2" />
          {t("fileExplorer.cmMove")}
        </ContextMenuItem>
        <ContextMenuItem variant="destructive" onClick={() => onDelete(entry)}>
          <MdDelete className="text-[0.875rem] mr-2" />
          {t("fileExplorer.cmDelete")}
        </ContextMenuItem>
        <ContextMenuSeparator />
        <ContextMenuItem onClick={() => onCopyPath(entry, "full")}>
          <MdContentCopy className="text-[0.875rem] text-muted-foreground mr-2" />
          {t("fileExplorer.cmCopyPath")}
        </ContextMenuItem>
        <ContextMenuItem onClick={() => onCopyPath(entry, "name")}>
          <MdCopyAll className="text-[0.875rem] text-muted-foreground mr-2" />
          {t("fileExplorer.cmCopyName")}
        </ContextMenuItem>
        <ContextMenuItem onClick={() => onCopyPath(entry, "dir")}>
          <MdFolderCopy className="text-[0.875rem] text-muted-foreground mr-2" />
          {t("fileExplorer.cmCopyDirPath")}
        </ContextMenuItem>
        <ContextMenuSeparator />
        <ContextMenuItem onClick={() => onSendToTerminal(entry, "full")}>
          <MdKeyboardReturn className="text-[0.875rem] text-muted-foreground mr-2" />
          {t("fileExplorer.cmTerminalPath")}
        </ContextMenuItem>
        <ContextMenuItem onClick={() => onSendToTerminal(entry, "name")}>
          <MdKeyboardArrowRight className="text-[0.875rem] text-muted-foreground mr-2" />
          {t("fileExplorer.cmTerminalName")}
        </ContextMenuItem>
        <ContextMenuItem onClick={() => onSendToTerminal(entry, "dir")}>
          <MdKeyboardDoubleArrowRight className="text-[0.875rem] text-muted-foreground mr-2" />
          {t("fileExplorer.cmTerminalDirPath")}
        </ContextMenuItem>
        <ContextMenuSeparator />
        <ContextMenuItem onClick={() => activeSessionId && onProperties(entry)}>
          <MdInfo className="text-[0.875rem] text-muted-foreground mr-2" />
          {t("fileExplorer.cmProperties")}
        </ContextMenuItem>
      </ContextMenuContent>
    </ContextMenu>
  );
}
