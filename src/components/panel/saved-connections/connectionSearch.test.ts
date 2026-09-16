import { describe, expect, it } from "vitest";
import type { SavedConnection } from "@/types/global";
import { matchesConnectionSearch } from "./connectionSearch";

const connection: SavedConnection = {
  id: "conn-1",
  name: "Database Server",
  type: "ssh",
  host: "db.example.com",
  username: "admin",
  tags: ["Production", "gpu"],
};

describe("matchesConnectionSearch", () => {
  it.each([
    "database",
    "example",
    "ADMIN",
    "production",
    "GPU",
  ])("matches connection metadata for %s", (keyword) => {
    expect(matchesConnectionSearch(connection, keyword)).toBe(true);
  });

  it("returns all connections for an empty keyword and rejects unrelated text", () => {
    expect(matchesConnectionSearch(connection, "  ")).toBe(true);
    expect(matchesConnectionSearch(connection, "staging")).toBe(false);
  });
});
