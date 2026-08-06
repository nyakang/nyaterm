import type { QuickCommand, QuickCommandCategory } from "@/types/global";

export interface QuickCommandCategoryNode {
  category: QuickCommandCategory;
  children: QuickCommandCategoryNode[];
  count: number;
  totalCount: number;
}

export interface QuickCommandCategoryTreeRow {
  node: QuickCommandCategoryNode;
  depth: number;
}

export type QuickCommandCategoryMoveDirection = "up" | "down";

export interface QuickCommandCategoryMoveState {
  canMoveUp: boolean;
  canMoveDown: boolean;
}

export function buildQuickCommandCategoryList(
  categories: QuickCommandCategory[],
  commands: QuickCommand[],
): QuickCommandCategory[] {
  const byId = new Map<string, QuickCommandCategory>();

  for (const category of categories) {
    if (!category.id || byId.has(category.id)) continue;
    byId.set(category.id, {
      ...category,
      parent_id: normalizeParentId(category.parent_id),
    });
  }

  for (const command of commands) {
    const categoryId = command.category_id?.trim();
    if (!categoryId || byId.has(categoryId)) continue;
    byId.set(categoryId, { id: categoryId, name: categoryId });
  }

  return Array.from(byId.values()).sort(compareQuickCommandCategories);
}

export function buildQuickCommandCategoryTree(
  categories: QuickCommandCategory[],
  commands: QuickCommand[],
): QuickCommandCategoryNode[] {
  const normalizedCategories = buildQuickCommandCategoryList(
    categories,
    commands,
  );
  const categoryById = new Map(
    normalizedCategories.map((category) => [category.id, category]),
  );
  const nodes = new Map<string, QuickCommandCategoryNode>();
  const directCounts = getQuickCommandCategoryDirectCounts(commands);

  for (const category of normalizedCategories) {
    nodes.set(category.id, {
      category,
      children: [],
      count: directCounts.get(category.id) ?? 0,
      totalCount: 0,
    });
  }

  const roots: QuickCommandCategoryNode[] = [];
  for (const category of normalizedCategories) {
    const node = nodes.get(category.id);
    if (!node) continue;
    const parentId = getUsableParentId(category, categoryById);
    if (parentId) {
      nodes.get(parentId)?.children.push(node);
    } else {
      roots.push(node);
    }
  }

  const fillTotalCount = (node: QuickCommandCategoryNode): number => {
    node.totalCount =
      node.count +
      node.children.reduce((sum, child) => sum + fillTotalCount(child), 0);
    return node.totalCount;
  };
  roots.forEach(fillTotalCount);
  return roots;
}

export function flattenVisibleQuickCommandCategoryTree(
  nodes: QuickCommandCategoryNode[],
  expandedCategoryIds: Set<string>,
): QuickCommandCategoryTreeRow[] {
  const rows: QuickCommandCategoryTreeRow[] = [];
  const visit = (items: QuickCommandCategoryNode[], depth: number) => {
    for (const node of items) {
      rows.push({ node, depth });
      if (expandedCategoryIds.has(node.category.id)) {
        visit(node.children, depth + 1);
      }
    }
  };
  visit(nodes, 0);
  return rows;
}

export function collectQuickCommandCategoryDescendantIds(
  categories: QuickCommandCategory[],
  categoryId: string | null | undefined,
): Set<string> {
  const result = new Set<string>();
  if (!categoryId) return result;

  const categoryById = new Map(
    categories.map((category) => [category.id, category]),
  );
  if (!categoryById.has(categoryId)) return result;

  const childrenByParent = new Map<string, string[]>();
  for (const category of categories) {
    const parentId = getUsableParentId(category, categoryById);
    if (!parentId) continue;
    const children = childrenByParent.get(parentId) ?? [];
    children.push(category.id);
    childrenByParent.set(parentId, children);
  }

  const queue = [categoryId];
  while (queue.length > 0) {
    const id = queue.shift();
    if (!id || result.has(id)) continue;
    result.add(id);
    queue.push(...(childrenByParent.get(id) ?? []));
  }

  return result;
}

export function collectQuickCommandCategoryAncestorIds(
  categories: QuickCommandCategory[],
  categoryId: string | null | undefined,
): string[] {
  if (!categoryId) return [];
  const categoryById = new Map(
    categories.map((category) => [category.id, category]),
  );
  const ancestors: string[] = [];
  const seen = new Set<string>();
  let currentId = categoryById.get(categoryId)?.parent_id;

  while (currentId && categoryById.has(currentId) && !seen.has(currentId)) {
    seen.add(currentId);
    ancestors.push(currentId);
    currentId = categoryById.get(currentId)?.parent_id;
  }

  return ancestors;
}

export function buildQuickCommandCategoryPath(
  categories: QuickCommandCategory[],
  categoryId: string | null | undefined,
): string {
  if (!categoryId) return "";
  const categoryById = new Map(
    categories.map((category) => [category.id, category]),
  );
  const segments: string[] = [];
  const seen = new Set<string>();
  let currentId: string | undefined = categoryId;

  while (currentId && categoryById.has(currentId) && !seen.has(currentId)) {
    seen.add(currentId);
    const category = categoryById.get(currentId);
    if (!category) break;
    segments.push(category.name);
    currentId = category.parent_id;
  }

  return segments.reverse().join(" / ");
}

export function hasQuickCommandCategorySiblingName(
  categories: QuickCommandCategory[],
  parentId: string | null,
  name: string,
  excludeId?: string,
) {
  const normalizedName = name.trim().toLowerCase();
  return categories.some(
    (category) =>
      category.id !== excludeId &&
      (category.parent_id ?? null) === parentId &&
      category.name.trim().toLowerCase() === normalizedName,
  );
}

export function deleteQuickCommandCategoryTree(
  categories: QuickCommandCategory[],
  commands: QuickCommand[],
  categoryId: string,
) {
  const deleteIds = collectQuickCommandCategoryDescendantIds(
    categories,
    categoryId,
  );
  return {
    deleteIds,
    categories: categories.filter((category) => !deleteIds.has(category.id)),
    commands: commands.filter(
      (command) => !command.category_id || !deleteIds.has(command.category_id),
    ),
  };
}

export function getNextQuickCommandCategorySortOrder(
  categories: QuickCommandCategory[],
  parentId: string | null | undefined,
) {
  const normalizedCategories = buildQuickCommandCategoryList(categories, []);
  const categoryById = new Map(
    normalizedCategories.map((category) => [category.id, category]),
  );
  const normalizedParentId = normalizeParentId(parentId);
  const usableParentId =
    normalizedParentId && categoryById.has(normalizedParentId)
      ? normalizedParentId
      : undefined;

  return (
    normalizedCategories
      .filter(
        (category) =>
          getUsableParentId(category, categoryById) === usableParentId,
      )
      .reduce(
        (max, category) => Math.max(max, category.sort_order ?? 0),
        -1,
      ) + 1
  );
}

export function getQuickCommandCategoryMoveState(
  categories: QuickCommandCategory[],
  categoryId: string,
): QuickCommandCategoryMoveState {
  const { siblings, index } = getQuickCommandCategorySiblingOrder(
    categories,
    categoryId,
  );

  return {
    canMoveUp: index > 0,
    canMoveDown: index >= 0 && index < siblings.length - 1,
  };
}

export function moveQuickCommandCategory(
  categories: QuickCommandCategory[],
  categoryId: string,
  direction: QuickCommandCategoryMoveDirection,
) {
  const { siblings, index } = getQuickCommandCategorySiblingOrder(
    categories,
    categoryId,
  );
  if (index < 0) return categories;

  const swapIndex = direction === "up" ? index - 1 : index + 1;
  if (swapIndex < 0 || swapIndex >= siblings.length) return categories;

  const reorderedSiblings = [...siblings];
  [reorderedSiblings[index], reorderedSiblings[swapIndex]] = [
    reorderedSiblings[swapIndex],
    reorderedSiblings[index],
  ];

  const sortOrderById = new Map(
    reorderedSiblings.map((category, sortOrder) => [category.id, sortOrder]),
  );

  return categories.map((category) =>
    sortOrderById.has(category.id)
      ? { ...category, sort_order: sortOrderById.get(category.id) }
      : category,
  );
}

export function getQuickCommandCategoryDirectCounts(commands: QuickCommand[]) {
  const counts = new Map<string, number>();
  for (const command of commands) {
    const categoryId = command.category_id?.trim();
    if (!categoryId) continue;
    counts.set(categoryId, (counts.get(categoryId) ?? 0) + 1);
  }
  return counts;
}

export function getQuickCommandUncategorizedCount(commands: QuickCommand[]) {
  return commands.filter((command) => !command.category_id).length;
}

function normalizeParentId(value: string | null | undefined) {
  const trimmed = value?.trim();
  return trimmed || undefined;
}

function getUsableParentId(
  category: QuickCommandCategory,
  categoryById: Map<string, QuickCommandCategory>,
) {
  const parentId = normalizeParentId(category.parent_id);
  if (!parentId || parentId === category.id || !categoryById.has(parentId))
    return undefined;

  const seen = new Set<string>([category.id]);
  let currentId: string | undefined = parentId;
  while (currentId) {
    if (seen.has(currentId)) return undefined;
    seen.add(currentId);
    currentId = categoryById.get(currentId)?.parent_id;
  }

  return parentId;
}

function getQuickCommandCategorySiblingOrder(
  categories: QuickCommandCategory[],
  categoryId: string,
) {
  const normalizedCategories = buildQuickCommandCategoryList(categories, []);
  const categoryById = new Map(
    normalizedCategories.map((category) => [category.id, category]),
  );
  const target = categoryById.get(categoryId);
  if (!target) return { siblings: [], index: -1 };

  const parentId = getUsableParentId(target, categoryById);
  const siblings = normalizedCategories.filter(
    (category) => getUsableParentId(category, categoryById) === parentId,
  );

  return {
    siblings,
    index: siblings.findIndex((category) => category.id === categoryId),
  };
}

function compareQuickCommandCategories(
  left: QuickCommandCategory,
  right: QuickCommandCategory,
) {
  return (
    (left.sort_order ?? 0) - (right.sort_order ?? 0) ||
    left.name.localeCompare(right.name, undefined, { sensitivity: "base" }) ||
    left.id.localeCompare(right.id)
  );
}
