import type { Group, SavedConnection } from "@/types/global";

export type StartWorkspaceMode = "workbench" | "assets";
export type AssetViewMode = "list" | "cards";
export type AssetSortKey =
  | "name"
  | "address"
  | "connectionTime"
  | "cpu"
  | "memory"
  | "storage"
  | "accelerators";
export type AssetSortDirection = "asc" | "desc";

export interface AssetSortState {
  key: AssetSortKey;
  direction: AssetSortDirection;
}

export interface AssetRecord {
  connection: SavedConnection;
  groupPath: string;
  groupSortOrder: number;
  connectionTimeMs: number | null;
  searchText: string;
}

export interface AssetGroupOption {
  group: Group;
  path: string;
  searchText: string;
}
