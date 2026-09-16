import type { SavedConnection } from "@/types/global";

export function matchesConnectionSearch(connection: SavedConnection, keyword: string): boolean {
  const normalizedKeyword = keyword.trim().toLowerCase();
  if (!normalizedKeyword) return true;

  return [connection.name, connection.host, connection.username, ...(connection.tags ?? [])].some(
    (value) => value?.toLowerCase().includes(normalizedKeyword),
  );
}
