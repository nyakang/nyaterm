import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { TFunction } from "i18next";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Group, SavedConnection, UiConfig } from "@/types/global";
import AssetView from "./AssetView";
import StartWorkspace from "./StartWorkspace";

const refreshConnectionsMock = vi.fn();
const updateUiMock = vi.fn();
const connectConnectionMock = vi.fn();
const editConnectionMock = vi.fn();
const GPU_CONNECTION_TIME_MS = Date.UTC(2026, 7, 13, 1, 15, 0);
const WINDOWS_CONNECTION_TIME_MS = Date.UTC(2026, 7, 13, 2, 30, 0);

let appState: {
  appSettings: { ui: Partial<UiConfig> };
  savedConnections: SavedConnection[];
  savedGroups: Group[];
  refreshConnections: () => Promise<void>;
  updateUi: (updates: Partial<UiConfig> | ((prev: UiConfig) => Partial<UiConfig>)) => void;
};

vi.mock("@/context/AppContext", () => ({
  useApp: () => appState,
}));

describe("start workspace asset view", () => {
  beforeEach(() => {
    refreshConnectionsMock.mockReset();
    refreshConnectionsMock.mockResolvedValue(undefined);
    updateUiMock.mockReset();
    connectConnectionMock.mockReset();
    connectConnectionMock.mockResolvedValue(undefined);
    editConnectionMock.mockReset();
    appState = {
      appSettings: { ui: { start_workspace_mode: "workbench" } },
      savedConnections: sampleConnections(),
      savedGroups: sampleGroups(),
      refreshConnections: refreshConnectionsMock,
      updateUi: (updates) => {
        const nextUpdates =
          typeof updates === "function" ? updates(appState.appSettings.ui as UiConfig) : updates;
        appState.appSettings.ui = { ...appState.appSettings.ui, ...nextUpdates };
        updateUiMock(nextUpdates);
      },
    };
  });

  it("shows the workbench by default and persists asset mode when switching tabs", async () => {
    const user = userEvent.setup();
    const view = renderStartWorkspace();

    expect(screen.getByText("Temporary SSH")).not.toBeNull();
    expect(screen.queryByText("Assets")).toBeNull();

    await user.click(screen.getByRole("button", { name: "Assets Mode" }));
    expect(updateUiMock).toHaveBeenCalledWith({ start_workspace_mode: "assets" });

    view.rerender(startWorkspaceElement());
    expect(document.querySelector("[data-asset-view]")).not.toBeNull();
    expect(screen.getByText("GPU Lab")).not.toBeNull();
    expect(screen.queryByText("Temporary SSH")).toBeNull();
  });

  it("opens the persisted assets tab by default", () => {
    appState.appSettings.ui.start_workspace_mode = "assets";
    renderStartWorkspace();

    expect(document.querySelector("[data-asset-view]")).not.toBeNull();
    expect(screen.queryByText("Temporary SSH")).toBeNull();
  });

  it("keeps the asset surface transparent while sticky table areas stay opaque", () => {
    appState.appSettings.ui.start_workspace_mode = "assets";
    renderStartWorkspace();

    const assetSurface = document.querySelector("[data-asset-view]") as HTMLElement;
    expect(assetSurface.style.backgroundColor).toBe("transparent");
    expect(assetSurface.style.getPropertyValue("--nyaterm-asset-sticky-bg")).toBe(
      "var(--df-bg-terminal-solid)",
    );
  });

  it("does not render status or favorite actions in the asset surface", () => {
    renderAssetView();

    const assetSurface = document.querySelector("[data-asset-view]");
    expect(assetSurface?.textContent?.toLowerCase()).not.toContain("online");
    expect(assetSurface?.textContent?.toLowerCase()).not.toContain("offline");
    expect(assetSurface?.textContent?.toLowerCase()).not.toContain("favorite");
    expect(assetSurface?.textContent).not.toContain("System");
    expect(assetSurface?.textContent).not.toContain("Updated");
    expect(assetSurface?.lastElementChild?.textContent).not.toBe("3 items");
  });

  it("uses compute device wording, connection icons, and compact missing values", () => {
    renderAssetView();

    expect(screen.getByText("Compute Devices")).not.toBeNull();
    expect(screen.queryByRole("button", { name: "Physical" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Virtual" })).toBeNull();
    expect(document.querySelector("[data-asset-table] img, [data-asset-table] svg")).not.toBeNull();
    expect(document.querySelector("[data-asset-view]")?.textContent).not.toContain("Unentered");
    expect(screen.getAllByText("-").length).toBeGreaterThan(0);
  });

  it("shows only normalized user tags in stable order", () => {
    renderAssetView();

    const tagFilters = document.querySelector("[data-asset-tag-filters]");
    const tagLabels = Array.from(tagFilters?.querySelectorAll("button") ?? []).map(
      (button) => button.textContent,
    );

    expect(tagLabels).toEqual(["edge", "office", "production", "Production", "training"]);
    expect(screen.queryByRole("button", { name: "Linux" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Windows" })).toBeNull();
    expect(screen.queryByRole("button", { name: "GPU" })).toBeNull();
    expect(screen.queryByRole("button", { name: "NPU" })).toBeNull();
    expect(tagFilters?.className).toContain("overflow-x-auto");
    expect(screen.getByRole("button", { name: "List" })).not.toBeNull();
    expect(screen.getByRole("button", { name: "Cards" })).not.toBeNull();
  });

  it("filters by exact user tags with OR semantics", async () => {
    const user = userEvent.setup();
    appState.savedConnections = appState.savedConnections.map((connection) =>
      connection.id === "conn-win" ? { ...connection, tags: undefined } : connection,
    );
    renderAssetView();

    await user.click(screen.getByRole("button", { name: "Production" }));
    expect(screen.getByText("GPU Lab")).not.toBeNull();
    expect(screen.queryByText("Windows VM")).toBeNull();
    expect(screen.queryByText("Ascend Edge")).toBeNull();

    await user.click(screen.getByRole("button", { name: "production" }));
    expect(screen.getByText("GPU Lab")).not.toBeNull();
    expect(screen.queryByText("Windows VM")).toBeNull();
    expect(screen.getByText("Ascend Edge")).not.toBeNull();
  });

  it("combines tag filters with search and group selection", async () => {
    const user = userEvent.setup();
    renderAssetView();

    await user.click(screen.getByRole("button", { name: "Production" }));
    await user.type(screen.getByPlaceholderText("Search assets"), "windows");
    expect(screen.queryByText("GPU Lab")).toBeNull();
    expect(screen.getByText("Windows VM")).not.toBeNull();

    await user.clear(screen.getByPlaceholderText("Search assets"));
    await user.click(screen.getByLabelText("Pick group"));
    const aiLabOptions = screen.getAllByText("Assets / Region / AI Lab");
    await user.click(aiLabOptions[aiLabOptions.length - 1]);
    expect(screen.getByText("GPU Lab")).not.toBeNull();
    expect(screen.queryByText("Windows VM")).toBeNull();
    expect(screen.queryByText("Ascend Edge")).toBeNull();
  });

  it("clears tag filters with All", async () => {
    const user = userEvent.setup();
    renderAssetView();

    await user.click(screen.getByRole("button", { name: "edge" }));
    expect(screen.queryByText("GPU Lab")).toBeNull();
    await user.click(screen.getByRole("button", { name: "All" }));
    expect(screen.getByText("GPU Lab")).not.toBeNull();
    expect(screen.getByText("Windows VM")).not.toBeNull();
    expect(screen.getByText("Ascend Edge")).not.toBeNull();
  });

  it("removes selected tags that disappear after connections refresh", async () => {
    const user = userEvent.setup();
    const view = renderAssetView();

    await user.click(screen.getByRole("button", { name: "edge" }));
    appState.savedConnections = appState.savedConnections.map((connection) =>
      connection.id === "conn-npu" ? { ...connection, tags: undefined } : connection,
    );
    rerenderAssetView(view);

    await waitFor(() => expect(screen.queryByRole("button", { name: "edge" })).toBeNull());
    expect(screen.getByText("GPU Lab")).not.toBeNull();
    expect(screen.getByText("Windows VM")).not.toBeNull();
    expect(screen.getByText("Ascend Edge")).not.toBeNull();
  });

  it("shows only All when no connections have user tags", () => {
    appState.savedConnections = appState.savedConnections.map((connection) => ({
      ...connection,
      tags: undefined,
    }));
    renderAssetView();

    expect(document.querySelector("[data-asset-tag-filters]")).toBeNull();
    expect(screen.getByRole("button", { name: "All" })).not.toBeNull();
  });

  it("keeps OS and accelerator properties searchable without built-in tags", async () => {
    const user = userEvent.setup();
    renderAssetView();
    const search = screen.getByPlaceholderText("Search assets");

    await user.type(search, "linux");
    expect(screen.getByText("GPU Lab")).not.toBeNull();
    await user.clear(search);

    await user.type(search, "windows");
    expect(screen.getByText("Windows VM")).not.toBeNull();
    await user.clear(search);

    await user.type(search, "gpu");
    expect(screen.getByText("GPU Lab")).not.toBeNull();
    await user.clear(search);

    await user.type(search, "npu");
    expect(screen.getByText("Ascend Edge")).not.toBeNull();
  });

  it("shows ancestor breadcrumbs after selecting a group", async () => {
    const user = userEvent.setup();
    renderAssetView();

    await user.click(screen.getByLabelText("Pick group"));
    const aiLabOptions = screen.getAllByText("Assets / Region / AI Lab");
    await user.click(aiLabOptions[aiLabOptions.length - 1]);

    expect(screen.getByText("Region")).not.toBeNull();
    expect(screen.getByText("AI Lab")).not.toBeNull();
    expect(screen.getByText("GPU Lab")).not.toBeNull();
    expect(screen.queryByText("Windows VM")).toBeNull();
  });

  it("switches between table and card views", async () => {
    const user = userEvent.setup();
    renderAssetView();

    expect(screen.getByRole("table")).not.toBeNull();
    await user.click(screen.getByRole("button", { name: "Cards" }));

    expect(screen.queryByRole("table")).toBeNull();
    expect(document.querySelector("[data-asset-card-grid]")).not.toBeNull();
    expect(screen.getByText("GPU Lab")).not.toBeNull();
  });

  it("does not expand row details on click or double click", async () => {
    renderAssetView();

    fireEvent.click(screen.getByText("GPU Lab"));
    fireEvent.doubleClick(screen.getByText("GPU Lab"));

    expect(screen.getAllByText("GPU Lab").length).toBe(1);
    expect(screen.queryByText("Basic Info")).toBeNull();
    expect(connectConnectionMock).not.toHaveBeenCalled();
    expect(editConnectionMock).not.toHaveBeenCalled();
  });

  it("connects and edits assets from fixed action buttons", async () => {
    const user = userEvent.setup();
    renderAssetView();

    await user.click(screen.getAllByRole("button", { name: "Connect" })[0]);
    await user.click(screen.getAllByRole("button", { name: "Edit" })[0]);

    expect(connectConnectionMock).toHaveBeenCalledWith(appState.savedConnections[0]);
    expect(editConnectionMock).toHaveBeenCalledWith(appState.savedConnections[0]);
  });

  it("shows disk capacity and sorts from table headers", async () => {
    const user = userEvent.setup();
    const view = renderAssetView();

    expect(screen.getByText("2.0 TB")).not.toBeNull();
    expect(screen.getByText(formatTestDateTime(GPU_CONNECTION_TIME_MS))).not.toBeNull();

    await user.click(screen.getByRole("button", { name: /Name/ }));
    rerenderAssetView(view);
    expect(getRenderedAssetNames()).toEqual(["Ascend Edge", "GPU Lab", "Windows VM"]);

    await user.click(screen.getByRole("button", { name: /Name/ }));
    rerenderAssetView(view);
    expect(getRenderedAssetNames()).toEqual(["Windows VM", "GPU Lab", "Ascend Edge"]);

    await user.click(screen.getByRole("button", { name: /Address/ }));
    rerenderAssetView(view);
    expect(getRenderedAssetNames()).toEqual(["GPU Lab", "Windows VM", "Ascend Edge"]);

    await user.click(screen.getByRole("button", { name: /Connected At/ }));
    rerenderAssetView(view);
    expect(getRenderedAssetNames()).toEqual(["GPU Lab", "Windows VM", "Ascend Edge"]);

    await user.click(screen.getByRole("button", { name: /Connected At/ }));
    rerenderAssetView(view);
    expect(getRenderedAssetNames()).toEqual(["Windows VM", "GPU Lab", "Ascend Edge"]);

    await user.click(screen.getByRole("button", { name: /Memory/ }));
    rerenderAssetView(view);
    expect(getRenderedAssetNames()).toEqual(["GPU Lab", "Windows VM", "Ascend Edge"]);

    await user.click(screen.getByRole("button", { name: /Storage/ }));
    rerenderAssetView(view);
    expect(getRenderedAssetNames()).toEqual(["GPU Lab", "Windows VM", "Ascend Edge"]);
  });

  it("restores persisted asset sort state", () => {
    appState.appSettings.ui.asset_sort_key = "name";
    appState.appSettings.ui.asset_sort_direction = "desc";

    renderAssetView();

    expect(getRenderedAssetNames()).toEqual(["Windows VM", "GPU Lab", "Ascend Edge"]);
  });

  it("persists asset sort changes from table headers", async () => {
    const user = userEvent.setup();
    const view = renderAssetView();

    await user.click(screen.getByRole("button", { name: /Connected At/ }));
    expect(updateUiMock).toHaveBeenLastCalledWith({
      asset_sort_key: "connectionTime",
      asset_sort_direction: "asc",
    });
    expect(appState.appSettings.ui.asset_sort_key).toBe("connectionTime");
    expect(appState.appSettings.ui.asset_sort_direction).toBe("asc");

    view.rerender(
      <AssetView
        t={t}
        onConnectConnection={connectConnectionMock}
        onEditConnection={editConnectionMock}
      />,
    );
    expect(getRenderedAssetNames()).toEqual(["GPU Lab", "Windows VM", "Ascend Edge"]);

    await user.click(screen.getByRole("button", { name: /Connected At/ }));
    expect(updateUiMock).toHaveBeenLastCalledWith({
      asset_sort_key: "connectionTime",
      asset_sort_direction: "desc",
    });
  });

  it("resizes table columns from header drag handles", () => {
    renderAssetView();

    const nameColumn = document.querySelector('[data-asset-column="name"]') as HTMLTableColElement;
    const nameResizer = document.querySelector("[data-asset-column-resizer]") as HTMLElement;

    expect(nameColumn.style.width).toBe("280px");

    fireEvent.pointerDown(nameResizer, { clientX: 300 });
    fireEvent.pointerMove(document, { clientX: 360 });
    fireEvent.pointerUp(document);

    expect(nameColumn.style.width).toBe("340px");
  });

  it("virtualizes table rows and reveals later assets after scrolling", () => {
    appState.savedConnections = manyConnections(120);
    renderAssetView();

    expect(screen.getByText("node-0")).not.toBeNull();
    expect(screen.queryByText("node-119")).toBeNull();

    const scroller = document.querySelector("[data-asset-table]") as HTMLElement;
    setScrollTop(scroller, 119 * 56);
    fireEvent.scroll(scroller);

    expect(screen.getByText("node-119")).not.toBeNull();
    expect(screen.queryByText("node-0")).toBeNull();
  });

  it("virtualizes card rows and reveals later assets after scrolling", async () => {
    const user = userEvent.setup();
    appState.savedConnections = manyConnections(120);
    renderAssetView();

    await user.click(screen.getByRole("button", { name: "Cards" }));
    expect(screen.getByText("node-0")).not.toBeNull();
    expect(screen.queryByText("node-119")).toBeNull();

    const scroller = document.querySelector("[data-asset-card-grid]") as HTMLElement;
    setScrollTop(scroller, 119 * 198);
    fireEvent.scroll(scroller);

    expect(screen.getByText("node-119")).not.toBeNull();
    expect(screen.queryByText("node-0")).toBeNull();
  });
});

function renderAssetView() {
  return render(
    <AssetView
      t={t}
      onConnectConnection={connectConnectionMock}
      onEditConnection={editConnectionMock}
    />,
  );
}

function rerenderAssetView(view: ReturnType<typeof render>) {
  view.rerender(
    <AssetView
      t={t}
      onConnectConnection={connectConnectionMock}
      onEditConnection={editConnectionMock}
    />,
  );
}

function renderStartWorkspace() {
  return render(startWorkspaceElement());
}

function startWorkspaceElement() {
  return (
    <StartWorkspace
      t={t}
      backgroundEnabled
      temporarySshShortcut="Ctrl+Shift+N"
      openChatShortcut="Ctrl+I"
      showCommandsShortcut="Ctrl+Shift+P"
      switchTerminalShortcut="Ctrl+Tab"
      onTemporarySshLink={vi.fn()}
      onOpenChat={vi.fn()}
      onShowCommands={vi.fn()}
      onSwitchTerminal={vi.fn()}
      onConnectConnection={connectConnectionMock}
      onEditConnection={editConnectionMock}
    />
  );
}

function setScrollTop(element: HTMLElement, value: number) {
  Object.defineProperty(element, "scrollTop", {
    configurable: true,
    value,
  });
}

const translations: Record<string, string> = {
  "app.openChat": "Open Chat",
  "app.showAllCommands": "Show Commands",
  "app.switchTerminal": "Switch Terminal",
  "common.add": "Add",
  "common.close": "Close",
  "common.more": "More",
  "common.remove": "Remove",
  "temporarySsh.title": "Temporary SSH",
  "assets.accelerators": "Compute Devices",
  "assets.actions": "Actions",
  "assets.address": "Address",
  "assets.addressAscending": "Address Asc",
  "assets.addressDescending": "Address Desc",
  "assets.all": "All",
  "assets.allComputeDevices": "All",
  "assets.architecture": "Architecture",
  "assets.assets": "Assets Mode",
  "assets.basicInfo": "Basic Info",
  "assets.cards": "Cards",
  "assets.cloud": "Cloud",
  "assets.connectionTime": "Connected At",
  "assets.cpu": "CPU",
  "assets.cpuCores": "CPU Cores",
  "assets.cpuModel": "CPU Model",
  "assets.cpuSockets": "CPU Sockets",
  "assets.cpuSummary": "CPU Summary",
  "assets.cpuThreads": "CPU Threads",
  "assets.defaultSort": "Default",
  "assets.deviceType": "Device Type",
  "assets.disk": "Disk",
  "assets.embedded": "Embedded",
  "assets.group": "Group",
  "assets.groupPickerPlaceholder": "Pick group",
  "assets.hostname": "Hostname",
  "assets.items": "{{count}} items",
  "assets.kernel": "Kernel",
  "assets.list": "List",
  "assets.localMachine": "Local Machine",
  "assets.memory": "Memory",
  "assets.name": "Name",
  "assets.nameAscending": "Name Asc",
  "assets.nameDescending": "Name Desc",
  "assets.network": "Network",
  "assets.nextPage": "Next Page",
  "assets.noResults": "No results",
  "assets.none": "None",
  "assets.notApplicable": "-",
  "assets.notes": "Notes",
  "assets.osVersion": "OS Version",
  "assets.other": "Other",
  "assets.previousPage": "Previous Page",
  "assets.processor": "Processor",
  "assets.searchPlaceholder": "Search assets",
  "assets.storage": "Storage",
  "assets.storageDevice": "Storage Device",
  "assets.system": "System",
  "assets.tags": "Tags",
  "assets.title": "Assets",
  "assets.totalCapacity": "Total Capacity",
  "assets.updatedAt": "Updated At",
  "assets.updatedNewest": "Updated Newest",
  "assets.updatedOldest": "Updated Oldest",
  "assets.updatedOn": "Updated {{date}}",
  "assets.workbench": "Workbench",
  "savedConnections.connect": "Connect",
  "savedConnections.edit": "Edit",
};

const t = ((key: string, options?: { count?: number; date?: string }) => {
  let value = translations[key] ?? key;
  if (options?.count !== undefined) value = value.replace("{{count}}", String(options.count));
  if (options?.date !== undefined) value = value.replace("{{date}}", options.date);
  return value;
}) as TFunction;

function sampleGroups(): Group[] {
  return [
    { id: "region", name: "Region", sort_order: 0 },
    { id: "ai", name: "AI Lab", parent_id: "region", sort_order: 0 },
  ];
}

function sampleConnections(): SavedConnection[] {
  return [
    {
      id: "conn-gpu",
      name: "GPU Lab",
      type: "ssh",
      host: "10.0.0.2",
      port: 22,
      username: "root",
      group_id: "ai",
      tags: [" training ", "Production", "", "training"],
      sort_order: 0,
      last_used_at_ms: GPU_CONNECTION_TIME_MS,
      asset: {
        device_type: "physical",
        os_name: "Ubuntu Linux",
        cpu_model: "EPYC",
        memory_bytes: 64 * 1024 ** 3,
        accelerators: [{ type: "gpu", vendor: "NVIDIA", model: "H100" }],
        disks: [{ capacity_bytes: 2 * 1024 ** 4, count: 1 }],
      },
    },
    {
      id: "conn-win",
      name: "Windows VM",
      type: "ssh",
      host: "10.0.0.10",
      port: 22,
      username: "admin",
      tags: ["office", "Production"],
      sort_order: 1,
      last_used_at_ms: WINDOWS_CONNECTION_TIME_MS,
      asset: {
        device_type: "virtual",
        os_name: "Windows Server",
      },
    },
    {
      id: "conn-npu",
      name: "Ascend Edge",
      type: "ssh",
      host: "edge-01",
      port: 22,
      username: "root",
      tags: ["edge", "production"],
      sort_order: 2,
      asset: {
        device_type: "physical",
        os_name: "openEuler",
        accelerators: [{ type: "npu", vendor: "Huawei", model: "Ascend 910B" }],
      },
    },
  ];
}

function getRenderedAssetNames(): string[] {
  return Array.from(document.querySelectorAll("[data-asset-name]")).map(
    (element) => element.textContent ?? "",
  );
}

function formatTestDateTime(value: number): string {
  return new Date(value).toLocaleString();
}

function manyConnections(count: number): SavedConnection[] {
  return Array.from({ length: count }, (_, index) => ({
    id: `node-${index}`,
    name: `node-${index}`,
    type: "ssh" as const,
    host: `10.0.1.${index}`,
    port: 22,
    username: "root",
    sort_order: index,
    asset: { os_name: "Ubuntu Linux" },
  }));
}
