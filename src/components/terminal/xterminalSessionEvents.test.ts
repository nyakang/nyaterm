import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn().mockResolvedValue(undefined),
  listen: vi.fn(),
}));

vi.mock("@/lib/invoke", () => ({ invoke: mocks.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: mocks.listen }));
vi.mock("sonner", () => ({ toast: { error: vi.fn() } }));

import {
  getDynamicTitle,
  pauseDynamicTitlePublication,
  publishApplicationTitle,
  publishSessionMetadata,
  resetDynamicTitlesForTests,
  resumeDynamicTitlePublication,
} from "@/lib/dynamicTabTitles";
import {
  createXTerminalSessionEvents,
  replaySnapshotBeforeAttach,
} from "./xterminalSessionEvents";

function params(overrides: Record<string, unknown> = {}) {
  return {
    sessionId: "ssh-1",
    terminal: { focus: vi.fn() },
    frameGate: { enqueue: vi.fn() },
    sessionIdRef: { current: "ssh-1" },
    visibleRef: { current: false },
    lastErrorNoticeAtRef: { current: 0 },
    aiCapturingRef: { current: false },
    zmodemActiveRef: { current: false },
    inputStateRef: { current: {} },
    alternateScreenTrackerRef: { current: { ingest: vi.fn() } },
    hibernationPhaseRef: { current: "waking" },
    detachedHibernateEpochRef: { current: 1 },
    onConnectionErrorRef: { current: undefined },
    tRef: { current: (key: string) => key },
    isTerminalAlive: () => true,
    requestWake: vi.fn(),
    enterDisconnectedState: vi.fn(),
    enterDisconnectedStateIfAttachSessionMissing: vi.fn(() => false),
    noteSkippedOutput: vi.fn(),
    noteOutputActivity: vi.fn(),
    updateCredentialPromptInputMode: vi.fn(),
    feedCredentialOutput: vi.fn(),
    maybeRecoverPerformanceMode: vi.fn(),
    refreshOutputPressureMode: vi.fn(),
    noteShellCommand: vi.fn(),
    clearCredentialPromptInputMode: vi.fn(),
    dismissSuggestions: vi.fn(),
    writeTerminalTextAfterOutputQueue: vi.fn().mockResolvedValue(undefined),
    initialReplayPromise: Promise.resolve(),
    updateOutputDrainMode: vi.fn(),
    logHibernation: vi.fn(),
    zmodemHandler: { handle: vi.fn() },
    replayPendingWakeEvents: vi.fn(),
    settleOutputAfterAttach: vi.fn().mockResolvedValue(true),
    flushPendingDynamicTitle: vi.fn(),
    ...overrides,
  };
}

describe("xterminalSessionEvents setup lifecycle", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    resetDynamicTitlesForTests();
    vi.clearAllMocks();
    mocks.invoke.mockResolvedValue(undefined);
  });

  it("keeps backend output detached until a slow listener retry succeeds", async () => {
    const disposeOutput = vi.fn();
    let listenerFailures = 0;
    mocks.listen.mockImplementation(async (eventName: string) => {
      if (eventName === "terminal-output-ssh-1") return disposeOutput;
      if (listenerFailures < 3) {
        listenerFailures += 1;
        throw new Error("listener unavailable");
      }
      return vi.fn();
    });
    const options = params();
    const events = createXTerminalSessionEvents(options as never);

    const setup = events.setup();
    await vi.advanceTimersByTimeAsync(300);
    expect(mocks.invoke).not.toHaveBeenCalledWith(
      "attach_session",
      expect.anything(),
    );
    await vi.advanceTimersByTimeAsync(5_000);
    await setup;

    expect(disposeOutput).toHaveBeenCalledTimes(3);
    expect(mocks.invoke).toHaveBeenCalledWith("attach_session", {
      sessionId: "ssh-1",
    });
    expect(options.detachedHibernateEpochRef.current).toBeNull();
    expect(options.hibernationPhaseRef.current).toBe("idle");
    expect(options.flushPendingDynamicTitle).toHaveBeenCalledTimes(1);
  });

  it("resumes publication when post-attach output drain times out", async () => {
    mocks.listen.mockResolvedValue(vi.fn());
    const options = params({
      settleOutputAfterAttach: vi.fn().mockResolvedValue(false),
    });
    const events = createXTerminalSessionEvents(options as never);

    await events.setup();

    expect(options.flushPendingDynamicTitle).toHaveBeenCalledTimes(1);
    expect(options.updateOutputDrainMode).toHaveBeenCalledTimes(1);
    expect(options.detachedHibernateEpochRef.current).toBeNull();
    expect(options.hibernationPhaseRef.current).toBe("failed");
  });

  it("retries transient attach failures without resuming publication early", async () => {
    mocks.listen.mockResolvedValue(vi.fn());
    mocks.invoke
      .mockRejectedValueOnce(new Error("attach unavailable"))
      .mockResolvedValue(undefined);
    const options = params();
    const events = createXTerminalSessionEvents(options as never);

    const setup = events.setup();
    await vi.advanceTimersByTimeAsync(0);
    expect(options.flushPendingDynamicTitle).not.toHaveBeenCalled();
    expect(options.detachedHibernateEpochRef.current).toBe(1);

    await vi.advanceTimersByTimeAsync(100);
    await setup;

    expect(mocks.invoke).toHaveBeenCalledTimes(2);
    expect(options.settleOutputAfterAttach).toHaveBeenCalledTimes(1);
    expect(options.flushPendingDynamicTitle).toHaveBeenCalledTimes(1);
    expect(options.detachedHibernateEpochRef.current).toBeNull();
    expect(options.hibernationPhaseRef.current).toBe("idle");
  });

  it("resumes publication without retrying when the attached session is missing", async () => {
    mocks.listen.mockResolvedValue(vi.fn());
    mocks.invoke.mockRejectedValue(new Error("session not found"));
    const options = params({
      enterDisconnectedStateIfAttachSessionMissing: vi.fn(() => true),
    });
    const events = createXTerminalSessionEvents(options as never);

    await events.setup();

    expect(mocks.invoke).toHaveBeenCalledTimes(1);
    expect(options.flushPendingDynamicTitle).toHaveBeenCalledTimes(1);
    expect(options.updateOutputDrainMode).toHaveBeenCalledTimes(1);
    expect(options.detachedHibernateEpochRef.current).toBeNull();
    expect(options.hibernationPhaseRef.current).toBe("failed");
  });

  it("cancels retry work without taking ownership of title publication", async () => {
    mocks.listen.mockRejectedValue(new Error("listener unavailable"));
    const options = params();
    const events = createXTerminalSessionEvents(options as never);

    const setup = events.setup();
    await Promise.resolve();
    events.dispose();
    await vi.runAllTimersAsync();
    await setup;

    expect(options.flushPendingDynamicTitle).not.toHaveBeenCalled();
    expect(mocks.invoke).not.toHaveBeenCalledWith(
      "attach_session",
      expect.anything(),
    );
  });

  it("holds titles across hibernated teardown until wake recovery completes", async () => {
    publishSessionMetadata({
      id: "ssh-1",
      name: "Production",
      session_type: "SSH",
      started_at: "",
      connected: true,
      ai_execution_profile: "auto",
      injection_active: false,
      dynamic_title_enabled: true,
      dynamic_title_integration_active: false,
      remote_file_browser_enabled: true,
      remote_stats_enabled: true,
    });
    publishApplicationTitle("ssh-1", "Before hibernation");
    await vi.advanceTimersByTimeAsync(75);
    expect(getDynamicTitle("ssh-1")).toBe(
      "Production · Before hibernation",
    );

    pauseDynamicTitlePublication("ssh-1");
    const teardownEvents = createXTerminalSessionEvents(
      params({
        flushPendingDynamicTitle: () =>
          resumeDynamicTitlePublication("ssh-1"),
      }) as never,
    );
    teardownEvents.dispose();
    publishApplicationTitle("ssh-1", "After wake");
    await vi.advanceTimersByTimeAsync(75);
    expect(getDynamicTitle("ssh-1")).toBe(
      "Production · Before hibernation",
    );

    mocks.listen.mockResolvedValue(vi.fn());
    const wakeEvents = createXTerminalSessionEvents(
      params({
        settleOutputAfterAttach: vi.fn().mockResolvedValue(false),
        flushPendingDynamicTitle: () =>
          resumeDynamicTitlePublication("ssh-1"),
      }) as never,
    );
    await wakeEvents.setup();
    await vi.advanceTimersByTimeAsync(75);

    expect(getDynamicTitle("ssh-1")).toBe("Production · After wake");
    wakeEvents.dispose();
  });
});

function createDeferred() {
  let resolve!: () => void;
  const promise = new Promise<void>((nextResolve) => {
    resolve = nextResolve;
  });
  return { promise, resolve };
}

describe("replaySnapshotBeforeAttach", () => {
  it("replays the snapshot before pending wake events and backend attach", async () => {
    const replay = createDeferred();
    const order: string[] = [];
    const attachSession = vi.fn(async () => {
      order.push("attach");
    });
    const restore = replaySnapshotBeforeAttach({
      initialReplayPromise: replay.promise.then(() => {
        order.push("replay");
      }),
      replayPendingWakeEvents: () => order.push("pending-wake"),
      attachSession,
    });

    await Promise.resolve();
    expect(attachSession).not.toHaveBeenCalled();
    expect(order).toEqual([]);

    replay.resolve();
    await restore;

    expect(order).toEqual(["replay", "pending-wake", "attach"]);
    expect(attachSession).toHaveBeenCalledTimes(1);
  });
});
