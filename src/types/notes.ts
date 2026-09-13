export type NoteNodeKind = "folder" | "note";

export interface NoteExportResult {
  outputPath: string;
  folderCount: number;
  noteCount: number;
}

export interface NoteFolder {
  id: string;
  parent_id: string | null;
  name: string;
  sort_order: number;
  created_at_ms: number;
  updated_at_ms: number;
}

export interface NoteSummary {
  id: string;
  parent_id: string | null;
  title: string;
  sort_order: number;
  revision: number;
  created_at_ms: number;
  updated_at_ms: number;
}

export interface NoteDocument extends NoteSummary {
  markdown: string;
}

export interface NoteTreePayload {
  folders: NoteFolder[];
  notes: NoteSummary[];
}

export interface NotesChangedEvent {
  kind: "created" | "updated" | "renamed" | "moved" | "deleted" | "replaced";
  nodeKind?: NoteNodeKind;
  ids: string[];
  folders?: NoteFolder[];
  notes?: NoteSummary[];
  treeChanged?: boolean;
}

export interface DeleteNoteNodeResult {
  folder_count: number;
  note_count: number;
  ids: string[];
}

export interface NoteTreeNode {
  id: string;
  kind: NoteNodeKind;
  parentId: string | null;
  name: string;
  sortOrder: number;
  revision?: number;
  updatedAtMs: number;
  children: NoteTreeNode[];
}
