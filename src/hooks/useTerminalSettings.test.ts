import { renderHook } from "@testing-library/react";
import type { Terminal } from "@xterm/xterm";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { TerminalFitScheduler } from "@/components/terminal/terminalFitScheduler";
import type { TerminalColors } from "@/lib/themes";
import type { AppSettings } from "@/types/global";
import { useTerminalSettings } from "./useTerminalSettings";

const webglMocks = vi.hoisted(() => ({
  instances: [] as Array<{
    dispose: ReturnType<typeof vi.fn>;
    contextLoss?: () => void;
  }>,
}));

vi.mock("@xterm/addon-webgl", () => ({
  WebglAddon: class {
    dispose = vi.fn();

    constructor() {
      webglMocks.instances.push(this);
    }

    onContextLoss(callback: () => void) {
      webglMocks.instances[webglMocks.instances.length - 1].contextLoss =
        callback;
      return { dispose: vi.fn() };
    }
  },
}));

vi.mock("@/lib/xtermImeCompatibility", () => ({
  installImeCompatibilityPatch: () => ({ dispose: vi.fn() }),
}));

const theme = (background: string): TerminalColors =>
  ({ background, foreground: "#ffffff" }) as TerminalColors;

const appearance = (fontSize: number): AppSettings["appearance"] =>
  ({
    font_family: "JetBrains Mono",
    font_size: fontSize,
    font_weight: "normal",
    font_weight_bold: "bold",
    cursor_blink: true,
    cursor_style: "block",
    minimum_contrast_ratio: 1,
  }) as unknown as AppSettings["appearance"];

const terminalSettings = {
  hardware_acceleration: true,
  font_size_delta: 0,
  scrollback_lines: 5_000,
} as AppSettings["terminal"];

const interaction = {
  word_separators: " ()[]{}'\"",
  alt_as_meta: false,
  ime_compatibility: false,
} as AppSettings["interaction"];

describe("useTerminalSettings renderer refresh", () => {
  let rafCallbacks: Map<number, FrameRequestCallback>;
  let rafRequests: ReturnType<typeof vi.fn>;
  let nextRafId: number;

  beforeEach(() => {
    vi.useFakeTimers();
    webglMocks.instances.length = 0;
    rafCallbacks = new Map();
    nextRafId = 1;
    rafRequests = vi.fn((callback: FrameRequestCallback) => {
      const id = nextRafId++;
      rafCallbacks.set(id, callback);
      return id;
    });
    vi.stubGlobal("requestAnimationFrame", rafRequests);
    vi.stubGlobal("cancelAnimationFrame", (id: number) =>
      rafCallbacks.delete(id),
    );
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  const flushAnimationFrames = () => {
    while (rafCallbacks.size > 0) {
      const callbacks = [...rafCallbacks.values()];
      rafCallbacks.clear();
      for (const callback of callbacks) callback(performance.now());
    }
  };

  function createHookHarness(
    rendererVisible = true,
    snapshotRestoring = false,
  ) {
    const terminal = {
      rows: 24,
      options: {},
      clearTextureAtlas: vi.fn(),
      refresh: vi.fn(),
      loadAddon: vi.fn(),
    } as unknown as Terminal;
    const terminalRef = { current: terminal };
    const fitSchedulerRef = {
      current: { schedule: vi.fn() } as unknown as TerminalFitScheduler,
    };
    const snapshotRestoringRef = { current: snapshotRestoring };
    const initialProps = {
      visible: rendererVisible,
      colors: theme("#000000"),
      ui: appearance(14),
    };
    const hook = renderHook(
      (props: {
        visible: boolean;
        colors: TerminalColors;
        ui: AppSettings["appearance"];
      }) =>
        useTerminalSettings(
          terminalRef,
          fitSchedulerRef,
          props.colors,
          props.ui,
          terminalSettings,
          interaction,
          props.visible,
          terminal,
          "session-1",
          snapshotRestoringRef,
        ),
      {
        initialProps,
      },
    );
    return {
      ...hook,
      terminal,
      terminalRef,
      fitSchedulerRef,
      initialProps,
      snapshotRestoringRef,
    };
  }

  it("installs WebGL and schedules only one reveal chain", () => {
    const { terminal } = createHookHarness();
    flushAnimationFrames();

    expect(terminal.loadAddon).toHaveBeenCalledTimes(1);
    expect(terminal.refresh).toHaveBeenCalledTimes(3);
    expect(terminal.clearTextureAtlas).toHaveBeenCalledTimes(1);
    expect(rafRequests).toHaveBeenCalledTimes(3);
  });

  it("repaints hidden-to-visible WebGL without clearing the texture atlas", () => {
    const harness = createHookHarness();
    const stableColors = theme("#000000");
    const stableAppearance = appearance(14);
    flushAnimationFrames();
    vi.mocked(harness.terminal.refresh).mockClear();
    vi.mocked(harness.terminal.clearTextureAtlas).mockClear();

    harness.rerender({
      visible: false,
      colors: stableColors,
      ui: stableAppearance,
    });
    flushAnimationFrames();
    vi.mocked(harness.terminal.refresh).mockClear();
    vi.mocked(harness.terminal.clearTextureAtlas).mockClear();
    harness.rerender({
      visible: true,
      colors: stableColors,
      ui: stableAppearance,
    });
    flushAnimationFrames();

    expect(harness.terminal.refresh).toHaveBeenCalledTimes(2);
    expect(harness.terminal.clearTextureAtlas).not.toHaveBeenCalled();
  });

  it("does not add WebGL or settings refreshes during snapshot restore", () => {
    const harness = createHookHarness(true, true);
    flushAnimationFrames();

    expect(harness.terminal.loadAddon).toHaveBeenCalledTimes(1);
    expect(harness.terminal.refresh).not.toHaveBeenCalled();
    expect(harness.fitSchedulerRef.current?.schedule).not.toHaveBeenCalled();

    harness.snapshotRestoringRef.current = false;
    harness.rerender(harness.initialProps);
    flushAnimationFrames();

    expect(harness.terminal.refresh).not.toHaveBeenCalled();
  });

  it("still clears the texture atlas for theme and font changes", () => {
    const harness = createHookHarness();
    flushAnimationFrames();
    vi.mocked(harness.terminal.clearTextureAtlas).mockClear();

    harness.rerender({
      visible: true,
      colors: theme("#101010"),
      ui: appearance(14),
    });
    flushAnimationFrames();
    expect(harness.terminal.clearTextureAtlas).toHaveBeenCalledTimes(1);

    vi.mocked(harness.terminal.clearTextureAtlas).mockClear();
    harness.rerender({
      visible: true,
      colors: theme("#101010"),
      ui: appearance(16),
    });
    flushAnimationFrames();
    expect(harness.terminal.clearTextureAtlas).toHaveBeenCalledTimes(1);
  });
});
