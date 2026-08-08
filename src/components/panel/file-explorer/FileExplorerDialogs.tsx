import DeleteDialog, {
  type DeleteDialogData,
} from "@/components/dialog/file-explorer/DeleteDialog";
import MoveDialog, { type MoveDialogData } from "@/components/dialog/file-explorer/MoveDialog";
import NewItemDialog, {
  type NewItemDialogData,
} from "@/components/dialog/file-explorer/NewItemDialog";
import NewSymlinkDialog, {
  type NewSymlinkDialogData,
} from "@/components/dialog/file-explorer/NewSymlinkDialog";
import PropertiesDialog, {
  type PropertiesDialogData,
} from "@/components/dialog/file-explorer/PropertiesDialog";
import type { FileEntry } from "@/types/global";

interface FileExplorerDialogsProps {
  deleteDialogData: DeleteDialogData | null;
  moveDialogData: MoveDialogData | null;
  newItemDialogData: NewItemDialogData | null;
  newSymlinkDialogData: NewSymlinkDialogData | null;
  propertiesDialogData: PropertiesDialogData | null;
  onDeleteClose: () => void;
  onMoveClose: () => void;
  onNewItemClose: () => void;
  onNewSymlinkClose: () => void;
  onPropertiesClose: () => void;
  onDeleteSuccess: () => void;
  onRefresh: () => Promise<unknown> | unknown;
  onOpenDirectoryEntry: (entry: FileEntry) => void;
  onOpenDefault: (entry: FileEntry) => void;
}

export function FileExplorerDialogs({
  deleteDialogData,
  moveDialogData,
  newItemDialogData,
  newSymlinkDialogData,
  propertiesDialogData,
  onDeleteClose,
  onMoveClose,
  onNewItemClose,
  onNewSymlinkClose,
  onPropertiesClose,
  onDeleteSuccess,
  onRefresh,
  onOpenDirectoryEntry,
  onOpenDefault,
}: FileExplorerDialogsProps) {
  return (
    <>
      {deleteDialogData && (
        <DeleteDialog data={deleteDialogData} onClose={onDeleteClose} onSuccess={onDeleteSuccess} />
      )}

      {moveDialogData && (
        <MoveDialog
          data={moveDialogData}
          onClose={onMoveClose}
          onSuccess={() => void onRefresh()}
        />
      )}

      {newItemDialogData && (
        <NewItemDialog
          data={newItemDialogData}
          onClose={onNewItemClose}
          onSuccess={async (result) => {
            await onRefresh();
            if (result.openAfterCreate) {
              const mockEntry: FileEntry = {
                name: result.name,
                is_dir: result.is_dir,
                is_symlink: false,
                size: 0,
                permissions: "",
                owner: "",
                group: "",
                mtime: 0,
              };
              if (result.is_dir) {
                onOpenDirectoryEntry(mockEntry);
              } else {
                onOpenDefault(mockEntry);
              }
            }
          }}
        />
      )}

      {propertiesDialogData && (
        <PropertiesDialog
          data={propertiesDialogData}
          onClose={onPropertiesClose}
          onSuccess={onRefresh}
        />
      )}

      {newSymlinkDialogData && (
        <NewSymlinkDialog
          data={newSymlinkDialogData}
          onClose={onNewSymlinkClose}
          onSuccess={() => void onRefresh()}
        />
      )}
    </>
  );
}
