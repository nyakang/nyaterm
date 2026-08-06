import { describe, expect, it } from "vitest";
import type { QuickCommand, QuickCommandCategory } from "@/types/global";
import {
  buildQuickCommandCategoryPath,
  buildQuickCommandCategoryTree,
  collectQuickCommandCategoryDescendantIds,
  deleteQuickCommandCategoryTree,
  getNextQuickCommandCategorySortOrder,
  getQuickCommandCategoryMoveState,
  hasQuickCommandCategorySiblingName,
  moveQuickCommandCategory,
} from "./quickCommandCategories";

describe("quickCommandCategories", () => {
  it("builds a tree with aggregate counts and orphan fallback", () => {
    const categories = [
      category("root", "Root"),
      category("child", "Child", "root"),
      category("orphan", "Orphan", "missing"),
    ];
    const commands = [
      command("cmd-root", "root"),
      command("cmd-child", "child"),
      command("cmd-orphan", "orphan"),
      command("cmd-missing", "missing-command-category"),
    ];

    const tree = buildQuickCommandCategoryTree(categories, commands);

    expect(tree.map((node) => node.category.id)).toEqual([
      "missing-command-category",
      "orphan",
      "root",
    ]);
    expect(tree.find((node) => node.category.id === "root")?.totalCount).toBe(
      2,
    );
    expect(
      tree.find((node) => node.category.id === "root")?.children[0].category.id,
    ).toBe("child");
    expect(tree.find((node) => node.category.id === "orphan")?.totalCount).toBe(
      1,
    );
  });

  it("collects descendants including the selected category", () => {
    const categories = [
      category("root", "Root"),
      category("child", "Child", "root"),
      category("nested", "Nested", "child"),
      category("other", "Other"),
    ];

    expect(
      Array.from(
        collectQuickCommandCategoryDescendantIds(categories, "root"),
      ).sort(),
    ).toEqual(["child", "nested", "root"]);
  });

  it("builds display paths", () => {
    const categories = [
      category("root", "Root"),
      category("child", "Child", "root"),
      category("nested", "Nested", "child"),
    ];

    expect(buildQuickCommandCategoryPath(categories, "nested")).toBe(
      "Root / Child / Nested",
    );
  });

  it("checks duplicate names by sibling scope", () => {
    const categories = [
      category("root", "Root"),
      category("child-a", "Deploy", "root"),
      category("child-b", "Deploy"),
    ];

    expect(
      hasQuickCommandCategorySiblingName(categories, "root", "deploy"),
    ).toBe(true);
    expect(hasQuickCommandCategorySiblingName(categories, null, "deploy")).toBe(
      true,
    );
    expect(
      hasQuickCommandCategorySiblingName(categories, "child-a", "deploy"),
    ).toBe(false);
  });

  it("deletes a category subtree with commands", () => {
    const categories = [
      category("root", "Root"),
      category("child", "Child", "root"),
      category("other", "Other"),
    ];
    const commands = [
      command("cmd-root", "root"),
      command("cmd-child", "child"),
      command("cmd-other", "other"),
      command("cmd-none"),
    ];

    const result = deleteQuickCommandCategoryTree(categories, commands, "root");

    expect(Array.from(result.deleteIds).sort()).toEqual(["child", "root"]);
    expect(result.categories.map((item) => item.id)).toEqual(["other"]);
    expect(result.commands.map((item) => item.id)).toEqual([
      "cmd-other",
      "cmd-none",
    ]);
  });

  it("sorts sibling categories by sort order with name fallback", () => {
    const categories = [
      category("later", "Beta", undefined, 2),
      category("fallback-b", "Zoo"),
      category("first", "Alpha", undefined, 1),
      category("fallback-a", "Apple"),
      category("parent", "Parent", undefined, 9),
      category("child-b", "Child B", "parent", 1),
      category("child-a", "Child A", "parent", 0),
    ];

    const tree = buildQuickCommandCategoryTree(categories, []);

    expect(tree.map((node) => node.category.id)).toEqual([
      "fallback-a",
      "fallback-b",
      "first",
      "later",
      "parent",
    ]);
    expect(
      tree
        .find((node) => node.category.id === "parent")
        ?.children.map((node) => node.category.id),
    ).toEqual(["child-a", "child-b"]);
  });

  it("moves categories only within the same parent and normalizes sibling order", () => {
    const categories = [
      category("root-a", "Root A", undefined, 0),
      category("root-b", "Root B", undefined, 1),
      category("parent", "Parent", undefined, 2),
      category("child-a", "Child A", "parent", 0),
      category("child-b", "Child B", "parent", 1),
      category("other-child", "Other Child", "root-a", 0),
    ];

    const result = moveQuickCommandCategory(categories, "child-b", "up");

    expect(result.find((item) => item.id === "child-b")?.sort_order).toBe(0);
    expect(result.find((item) => item.id === "child-a")?.sort_order).toBe(1);
    expect(result.find((item) => item.id === "root-a")?.sort_order).toBe(0);
    expect(result.find((item) => item.id === "other-child")?.sort_order).toBe(
      0,
    );
  });

  it("reports move boundaries for first and last siblings", () => {
    const categories = [
      category("first", "First", undefined, 0),
      category("second", "Second", undefined, 1),
    ];

    expect(getQuickCommandCategoryMoveState(categories, "first")).toEqual({
      canMoveUp: false,
      canMoveDown: true,
    });
    expect(getQuickCommandCategoryMoveState(categories, "second")).toEqual({
      canMoveUp: true,
      canMoveDown: false,
    });
  });

  it("computes the next sort order within the selected parent", () => {
    const categories = [
      category("root", "Root", undefined, 4),
      category("parent", "Parent", undefined, 5),
      category("child-a", "Child A", "parent", 2),
      category("child-b", "Child B", "parent", 8),
    ];

    expect(getNextQuickCommandCategorySortOrder(categories, null)).toBe(6);
    expect(getNextQuickCommandCategorySortOrder(categories, "parent")).toBe(9);
  });

  it("moves orphan categories as root siblings", () => {
    const categories = [
      category("root", "Root", undefined, 0),
      category("orphan", "Orphan", "missing", 1),
    ];

    const result = moveQuickCommandCategory(categories, "orphan", "up");

    expect(result.find((item) => item.id === "orphan")?.sort_order).toBe(0);
    expect(result.find((item) => item.id === "root")?.sort_order).toBe(1);
  });
});

function category(
  id: string,
  name: string,
  parentId?: string,
  sortOrder?: number,
): QuickCommandCategory {
  return { id, name, parent_id: parentId, sort_order: sortOrder };
}

function command(id: string, categoryId?: string): QuickCommand {
  return {
    id,
    label: id,
    command: "echo test",
    category_id: categoryId,
  };
}
