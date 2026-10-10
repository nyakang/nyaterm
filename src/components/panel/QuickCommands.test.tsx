import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type {
  QuickCommand,
  QuickCommandCategory,
  QuickCommandsConfig,
  QuickCommandViewMode,
} from "@/types/global";
import QuickCommands from "./QuickCommands";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  listen: vi.fn(async () => vi.fn()),
  openQuickCommand: vi.fn(),
  writeClipboardText: vi.fn(async () => undefined),
}));

let appState: {
  appSettings: {
    ui: {
      quick_cmd_category_width: number;
      quick_cmd_view_mode: QuickCommandViewMode;
      quick_cmd_sort_mode: "created";
      quick_cmd_selected_category: string;
    };
  };
  updateUi: ReturnType<typeof vi.fn>;
};

let config: QuickCommandsConfig;

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    i18n: { language: "en" },
    t: (key: string, options?: Record<string, unknown>) => {
      const labels: Record<string, string> = {
        "quickCommands.addCategory": "Add Category",
        "quickCommands.addCommand": "Add Command",
        "quickCommands.allCategories": "All Categories",
        "quickCommands.uncategorized": "Uncategorized",
        "quickCommands.edit": "Edit",
        "quickCommands.copyCommand": "Copy command",
        "quickCommands.sendToAll": "Send to all",
        "quickCommands.delete": "Delete",
        "quickCommands.noCommandsFound": "No commands found",
        "quickCommands.newCategoryRootHint": "Create at root",
        "quickCommands.newCategoryParentHint": `Create under ${String(options?.category ?? "")}`,
        "quickCommands.categoryName": "Category name",
        "quickCommands.categoryPlaceholder": "Category name",
        "common.cancel": "Cancel",
        "common.confirm": "Confirm",
      };
      return labels[key] ?? key;
    },
  }),
}));

vi.mock("@/context/AppContext", () => ({
  useApp: () => appState,
}));

vi.mock("@/lib/invoke", () => ({
  invoke: mocks.invoke,
}));

vi.mock("@/lib/backend/runtime", () => ({ runtime: "desktop" }));
vi.mock("@/lib/platform", () => ({ isWindows: true }));
vi.mock("@/lib/backend/api", () => ({ listen: mocks.listen }));

vi.mock("@/lib/windowManager", () => ({
  openQuickCommand: mocks.openQuickCommand,
}));

vi.mock("@/lib/clipboard", () => ({
  writeClipboardText: mocks.writeClipboardText,
}));

vi.mock("@/lib/aiEvents", () => ({
  openAIAssistant: vi.fn(),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: mocks.listen,
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(),
  save: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: vi.fn(),
}));

const dockerCategory: QuickCommandCategory = {
  id: "docker",
  name: "Docker",
};

const deployCommand: QuickCommand = {
  id: "deploy",
  label: "Deploy",
  command: "docker compose up -d",
  category_id: dockerCategory.id,
  execution_mode: "execute",
};

function setUi(viewMode: QuickCommandViewMode, selectedCategory: string) {
  appState = {
    appSettings: {
      ui: {
        quick_cmd_category_width: 176,
        quick_cmd_view_mode: viewMode,
        quick_cmd_sort_mode: "created",
        quick_cmd_selected_category: selectedCategory,
      },
    },
    updateUi: vi.fn(),
  };
}

function renderQuickCommands(options?: {
  viewMode?: QuickCommandViewMode;
  selectedCategory?: string;
  commands?: QuickCommand[];
  categories?: QuickCommandCategory[];
}) {
  setUi(options?.viewMode ?? "list", options?.selectedCategory ?? "all");
  config = {
    commands: options?.commands ?? [deployCommand],
    categories: options?.categories ?? [dockerCategory],
  };
  const onSend = vi.fn();
  const onSendToAll = vi.fn();
  const view = render(
    <QuickCommands
      onSend={onSend}
      onSendToAll={onSendToAll}
      currentSessionId="session-1"
    />,
  );
  return { ...view, onSend, onSendToAll };
}

async function waitForLoadedCommand() {
  await screen.findByText(deployCommand.label);
}

async function waitForLoadedCategories() {
  await screen.findByText(dockerCategory.name);
}

describe("QuickCommands context actions", () => {
  beforeEach(() => {
    mocks.invoke.mockReset();
    mocks.openQuickCommand.mockReset();
    mocks.writeClipboardText.mockReset();
    mocks.writeClipboardText.mockResolvedValue(undefined);
    mocks.invoke.mockImplementation((command: string) => {
      if (command === "get_quick_commands") return Promise.resolve(config);
      return Promise.resolve(undefined);
    });
  });

  it.each(["list", "compact", "tile"] as const)(
    "distinguishes command and blank context menus in %s view",
    async (viewMode) => {
      const { onSend, onSendToAll } = renderQuickCommands({ viewMode });
      await waitForLoadedCommand();

      fireEvent.contextMenu(screen.getByText(deployCommand.label));
      const edit = await screen.findByText("Edit");
      const itemMenu = edit.closest('[data-slot="context-menu-content"]');
      expect(itemMenu).not.toBeNull();
      expect(
        Array.from(itemMenu?.querySelectorAll('[role="menuitem"]') ?? []).map(
          (item) => item.textContent?.trim(),
        ),
      ).toEqual(["Edit", "Copy command", "Send to all", "Delete"]);

      fireEvent.click(screen.getByText("Copy command"));
      await waitFor(() => {
        expect(mocks.writeClipboardText).toHaveBeenCalledWith(
          deployCommand.command,
        );
      });
      expect(onSend).not.toHaveBeenCalled();
      expect(onSendToAll).not.toHaveBeenCalled();

      fireEvent.contextMenu(screen.getByTestId("quick-command-pane"));
      const addCommand = await screen.findByText("Add Command");
      expect(screen.queryByText("Edit")).toBeNull();
      fireEvent.click(addCommand);
      expect(mocks.openQuickCommand).toHaveBeenCalledWith(undefined, {
        categoryId: null,
      });
    },
  );

  it("resolves command context from an SVG descendant", async () => {
    renderQuickCommands();
    await waitForLoadedCommand();

    const commandItem = screen
      .getByText(deployCommand.label)
      .closest("[data-quick-command-id]");
    const icon = commandItem?.querySelector("svg");
    expect(icon).not.toBeNull();

    fireEvent.contextMenu(icon!);
    const edit = await screen.findByText("Edit");
    const itemMenu = edit.closest('[data-slot="context-menu-content"]');
    expect(itemMenu).not.toBeNull();
    expect(
      Array.from(itemMenu?.querySelectorAll('[role="menuitem"]') ?? []).map(
        (item) => item.textContent?.trim(),
      ),
    ).toEqual(["Edit", "Copy command", "Send to all", "Delete"]);
  });

  it("inherits the selected saved category from toolbar and empty-state creation", async () => {
    renderQuickCommands({
      selectedCategory: dockerCategory.id,
      commands: [],
    });
    await waitForLoadedCategories();

    const addButtons = screen.getAllByRole("button", { name: "Add Command" });
    fireEvent.click(addButtons[0]);
    expect(mocks.openQuickCommand).toHaveBeenLastCalledWith(undefined, {
      categoryId: dockerCategory.id,
    });

    fireEvent.click(addButtons[addButtons.length - 1]);
    expect(mocks.openQuickCommand).toHaveBeenLastCalledWith(undefined, {
      categoryId: dockerCategory.id,
    });

    fireEvent.contextMenu(screen.getByTestId("quick-command-pane"));
    fireEvent.click(
      await screen.findByRole("menuitem", { name: "Add Command" }),
    );
    expect(mocks.openQuickCommand).toHaveBeenLastCalledWith(undefined, {
      categoryId: dockerCategory.id,
    });
  });

  it.each(["all", "uncategorized", "missing-category"])(
    "does not force a category for %s selection",
    async (selectedCategory) => {
      renderQuickCommands({ selectedCategory, commands: [] });
      await waitForLoadedCategories();

      fireEvent.click(
        screen.getAllByRole("button", { name: "Add Command" })[0],
      );
      expect(mocks.openQuickCommand).toHaveBeenLastCalledWith(undefined, {
        categoryId: null,
      });
    },
  );

  it("creates a root category from the sidebar blank area", async () => {
    renderQuickCommands();
    await waitForLoadedCommand();

    fireEvent.contextMenu(
      screen.getByTestId("quick-command-category-empty-area"),
    );
    fireEvent.click(await screen.findByText("Add Category"));
    expect(await screen.findByText("Create at root")).not.toBeNull();
  });

  it("preserves Add Command on a saved category", async () => {
    renderQuickCommands();
    await waitForLoadedCommand();

    fireEvent.contextMenu(screen.getByText(dockerCategory.name));
    fireEvent.click(await screen.findByText("Add Command"));
    expect(mocks.openQuickCommand).toHaveBeenCalledWith(undefined, {
      categoryId: dockerCategory.id,
    });
  });

  it("starts and cancels NyaScript without sending script text to the terminal", async () => {
    const scriptCommand: QuickCommand = {
      id: "bootstrap",
      label: "Bootstrap",
      command: 'sendln "show status"\nwait "ready>"',
      execution_mode: "nyascript",
    };
    mocks.invoke.mockImplementation((command: string, args?: Record<string, unknown>) => {
      if (command === "get_quick_commands") {
        return Promise.resolve({ commands: [scriptCommand], categories: [] });
      }
      if (command === "start_nyascript") {
        expect(args).toEqual({
          source: scriptCommand.command,
          currentSessionId: "session-1",
        });
        return Promise.resolve("run-1");
      }
      if (command === "get_nyascript_status") {
        return Promise.resolve({
          runId: "run-1",
          state: "running",
          currentLine: 2,
          activeAlias: "current",
          sessionId: "session-1",
          logs: ["waiting"],
          error: null,
        });
      }
      return Promise.resolve(undefined);
    });

    const { onSend, onSendToAll } = renderQuickCommands({
      commands: [scriptCommand],
      categories: [],
    });
    await screen.findByText("Bootstrap");

    fireEvent.click(screen.getByText("Bootstrap"));
    await waitFor(() => {
      expect(mocks.invoke).toHaveBeenCalledWith("start_nyascript", {
        source: scriptCommand.command,
        currentSessionId: "session-1",
      });
    });
    expect(onSend).not.toHaveBeenCalled();
    expect(onSendToAll).not.toHaveBeenCalled();
    expect((await screen.findByTestId("nyascript-run-status")).textContent).toContain("waiting");

    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await waitFor(() => {
      expect(mocks.invoke).toHaveBeenCalledWith("cancel_nyascript", { runId: "run-1" });
    });

    fireEvent.contextMenu(screen.getByText("Bootstrap"));
    expect(screen.queryByText("Send to all")).toBeNull();
  });
});
