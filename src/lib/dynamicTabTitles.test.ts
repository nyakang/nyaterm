import { beforeEach, describe, expect, it, vi } from "vitest";
import type {
  SessionCwdPresentation,
  SessionInfo,
} from "@/types/global";

const mocks = vi.hoisted(() => {
  const eventListeners = new Map<string, Set<(event: { payload: unknown }) => void>>();
  const invoke = vi.fn();
  const listen = vi.fn(
    async (eventName: string, callback: (event: { payload: unknown }) => void) => {
      const callbacks = eventListeners.get(eventName) ?? new Set();
      callbacks.add(callback);
      eventListeners.set(eventName, callbacks);
      return () => {
        callbacks.delete(callback);
      };
    },
  );
  return { eventListeners, invoke, listen };
});

vi.mock("@/lib/invoke", () => ({ invoke: mocks.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: mocks.listen }));

import {
  getDynamicTitle,
  getDynamicTitleStoreStatsForTests,
  getDynamicTitlesSnapshot,
  pauseDynamicTitlePublication,
  publishApplicationTitle,
  publishSessionCwdPresentation,
  publishSessionMetadata,
  resetDynamicTitlesForTests,
  resumeDynamicTitlePublication,
  sanitizeDynamicTitle,
  startDynamicTitles,
} from "@/lib/dynamicTabTitles";

const cwd = (
  title = "~/project",
  displayPath = "/home/alice/project",
): SessionCwdPresentation => ({
  title,
  displayPath,
  copyValue: displayPath,
  copyAsUri: false,
});

const localSession = (
  enabled = true,
  overrides: Partial<SessionInfo> = {},
): SessionInfo => ({
  id: "local-1",
  name: "Local Terminal",
  session_type: "Local",
  started_at: "",
  connection_id: "conn-1",
  connected: true,
  owner_window_label: null,
  ai_execution_profile: "auto",
  injection_active: false,
  dynamic_title_enabled: enabled,
  dynamic_title_integration_active: enabled,
  trusted_initial_title: null,
  remote_file_browser_enabled: false,
  remote_stats_enabled: false,
  ssh_profile: null,
  ...overrides,
});

const sshSession = (id = "ssh-1"): SessionInfo => ({
  ...localSession(true),
  id,
  name: "Production SSH",
  session_type: "SSH",
  injection_active: false,
  dynamic_title_integration_active: false,
  trusted_initial_title: null,
  remote_file_browser_enabled: true,
  remote_stats_enabled: true,
});

function emit(eventName: string, payload?: unknown) {
  for (const callback of mocks.eventListeners.get(eventName) ?? []) {
    callback({ payload });
  }
}

function installDefaultListenMock() {
  mocks.listen.mockImplementation(async (eventName: string, callback) => {
    const callbacks = mocks.eventListeners.get(eventName) ?? new Set();
    callbacks.add(callback);
    mocks.eventListeners.set(eventName, callbacks);
    return () => {
      callbacks.delete(callback);
    };
  });
}

async function settle() {
  await Promise.resolve();
  await Promise.resolve();
  await vi.runAllTimersAsync();
}

describe("dynamic title sanitizer", () => {
  beforeEach(() => {
    vi.useRealTimers();
    resetDynamicTitlesForTests();
    mocks.eventListeners.clear();
    vi.clearAllMocks();
    installDefaultListenMock();
  });

  it("normalizes Unicode, removes control/bidi characters, and folds whitespace", () => {
    expect(
      sanitizeDynamicTitle("e\u0301\u0000\tA\u0085\u001b\u007f\u202E B\nC"),
    ).toBe("é A B C");
  });

  it("removes deprecated bidi formatting controls", () => {
    expect(sanitizeDynamicTitle("prod\u206Aadmin\u206Bops\u206C")).toBe(
      "prodadminops",
    );
  });

  it("preserves normal emoji and bounds huge input at grapheme boundaries", () => {
    expect(sanitizeDynamicTitle("👩‍💻 · 项目")).toBe("👩‍💻 · 项目");
    const result = sanitizeDynamicTitle("👩‍💻".repeat(200));
    expect(Array.from(result ?? "").length).toBeLessThanOrEqual(256);
    expect(result?.endsWith("…")).toBe(true);
    expect(sanitizeDynamicTitle("x".repeat(1_000_000))?.length).toBeLessThanOrEqual(
      256,
    );
  });

  it("does not split a combining sequence when Intl.Segmenter is unavailable", () => {
    const intl = globalThis.Intl as typeof Intl & {
      Segmenter?: unknown;
    };
    const original = intl.Segmenter;
    Object.defineProperty(intl, "Segmenter", {
      configurable: true,
      value: undefined,
    });
    try {
      const value = `${"x".repeat(254)}q\u0327\u0301tail`;
      const result = sanitizeDynamicTitle(value);
      expect(result).toBe(`${"x".repeat(254)}…`);
      expect(Array.from(result ?? "").length).toBeLessThanOrEqual(256);
    } finally {
      Object.defineProperty(intl, "Segmenter", {
        configurable: true,
        value: original,
      });
    }
  });

  it("keeps ZWJ families and regional-indicator flags intact without Intl.Segmenter", () => {
    const intl = globalThis.Intl as typeof Intl & {
      Segmenter?: unknown;
    };
    const original = intl.Segmenter;
    Object.defineProperty(intl, "Segmenter", {
      configurable: true,
      value: undefined,
    });
    try {
      const joined = sanitizeDynamicTitle(
        `${"x".repeat(252)}👨‍👩‍👧‍👦tail`,
      );
      expect(joined).toBe(`${"x".repeat(252)}…`);
      expect(joined).not.toContain("\u200D…");

      const flag = sanitizeDynamicTitle(`${"x".repeat(254)}🇺🇸tail`);
      expect(flag).toBe(`${"x".repeat(254)}…`);
      expect(flag).not.toContain("🇺…");

      const conjunct = sanitizeDynamicTitle(`${"x".repeat(253)}क्षtail`);
      expect(conjunct).toBe(`${"x".repeat(253)}…`);
      expect(conjunct).not.toContain("क्…");
    } finally {
      Object.defineProperty(intl, "Segmenter", {
        configurable: true,
        value: original,
      });
    }
  });

  it("does not claim heuristic secret redaction", () => {
    expect(sanitizeDynamicTitle("token=secret-value")).toBe(
      "token=secret-value",
    );
  });
});

describe("dynamic title state", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    resetDynamicTitlesForTests();
    mocks.eventListeners.clear();
    vi.clearAllMocks();
    installDefaultListenMock();
    mocks.invoke.mockImplementation(async (command: string) => {
      if (command === "list_sessions") return [];
      if (command === "get_session_cwd_presentation") return null;
      return null;
    });
  });

  it("coalesces updates and falls back to backend cwd on empty OSC title", () => {
    publishSessionMetadata(localSession());
    publishSessionCwdPresentation("local-1", cwd());
    publishApplicationTitle("local-1", "Build");
    publishApplicationTitle("local-1", "");
    vi.advanceTimersByTime(75);

    const snapshot = getDynamicTitlesSnapshot().get("local-1");
    expect(snapshot?.effectiveTitle).toBe("~/project");
    expect(snapshot?.applicationTitle).toBeNull();
    expect(snapshot?.cwd?.displayPath).toBe("/home/alice/project");
  });

  it("uses compatible cwd reports and explicit titles while Local integration is passive", () => {
    publishSessionMetadata(
      localSession(true, { dynamic_title_integration_active: false }),
    );
    publishSessionCwdPresentation("local-1", cwd());
    vi.advanceTimersByTime(75);

    expect(getDynamicTitle("local-1")).toBe("~/project");
    expect(
      getDynamicTitlesSnapshot().get("local-1")?.cwd?.displayPath,
    ).toBe("/home/alice/project");

    publishApplicationTitle("local-1", "Editor");
    vi.advanceTimersByTime(75);
    expect(getDynamicTitle("local-1")).toBe("Editor");

    publishApplicationTitle("local-1", "");
    vi.advanceTimersByTime(75);
    expect(getDynamicTitle("local-1")).toBe("~/project");
  });

  it("keeps a trusted local prefix on opt-in SSH titles and ignores cwd", () => {
    publishSessionMetadata(sshSession());
    publishSessionCwdPresentation("ssh-1", cwd("remote", "/remote"));
    publishApplicationTitle("ssh-1", "  htop\u202E  ");
    vi.advanceTimersByTime(75);

    const snapshot = getDynamicTitlesSnapshot().get("ssh-1");
    expect(snapshot?.effectiveTitle).toBe("Production SSH · htop");
    expect(snapshot?.cwd).toBeNull();
  });

  it("reserves room for a remote title after a maximum-length SSH prefix", () => {
    publishSessionMetadata({
      ...sshSession(),
      name: "P".repeat(256),
    });
    publishApplicationTitle("ssh-1", "REMOTE");
    vi.advanceTimersByTime(75);

    const title = getDynamicTitle("ssh-1") ?? "";
    expect(title).toContain(" · REMOTE");
    expect(title.startsWith("P")).toBe(true);
    expect(Array.from(title).length).toBeLessThanOrEqual(256);
  });

  it("uses the remaining SSH title budget for a long remote title", () => {
    publishSessionMetadata({ ...sshSession(), name: "Prod" });
    publishApplicationTitle("ssh-1", "R".repeat(512));
    vi.advanceTimersByTime(75);

    const title = getDynamicTitle("ssh-1") ?? "";
    expect(title.startsWith("Prod · R")).toBe(true);
    expect(title.endsWith("…")).toBe(true);
    expect(Array.from(title).length).toBeLessThanOrEqual(256);
  });

  it("budgets both SSH title sides without splitting ZWJ graphemes", () => {
    const prefixGrapheme = "👨‍👩‍👧‍👦";
    const remoteGrapheme = "👩‍💻";
    publishSessionMetadata({
      ...sshSession(),
      name: prefixGrapheme.repeat(80),
    });
    publishApplicationTitle("ssh-1", remoteGrapheme.repeat(100));
    vi.advanceTimersByTime(75);

    const title = getDynamicTitle("ssh-1") ?? "";
    const [prefixPart = "", remotePart = ""] = title.split(" · ");
    expect(prefixPart.endsWith("…")).toBe(true);
    expect(remotePart.endsWith("…")).toBe(true);
    expect(Array.from(prefixPart.slice(0, -1)).length % 7).toBe(0);
    expect(Array.from(remotePart.slice(0, -1)).length % 3).toBe(0);
    expect(Array.from(title).length).toBeLessThanOrEqual(256);
  });

  it("does not count an empty pre-metadata reset as a distinct identity", () => {
    const executable = "C:\\Windows\\System32\\cmd.exe";
    publishApplicationTitle("local-1", "");
    publishApplicationTitle("local-1", executable);
    publishSessionMetadata(
      localSession(true, { trusted_initial_title: executable }),
    );
    publishSessionCwdPresentation("local-1", cwd());
    vi.advanceTimersByTime(75);

    expect(getDynamicTitle("local-1")).toBe("~/project");
    expect(
      getDynamicTitlesSnapshot().get("local-1")?.applicationTitle,
    ).toBeNull();
  });

  it("suppresses only the initial exact ConPTY executable identity", () => {
    const executable =
      "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe";
    publishApplicationTitle("local-1", executable);
    publishSessionMetadata(
      localSession(true, { trusted_initial_title: executable }),
    );
    publishSessionCwdPresentation("local-1", cwd());
    vi.advanceTimersByTime(75);
    expect(getDynamicTitle("local-1")).toBe("~/project");

    // The same string is accepted after the one-time startup decision.
    publishApplicationTitle("local-1", executable.replace(/\\/g, "/"));
    vi.advanceTimersByTime(75);
    expect(getDynamicTitle("local-1")).toBe(executable.replace(/\\/g, "/"));

    publishApplicationTitle("local-1", "Editor");
    vi.advanceTimersByTime(75);
    expect(getDynamicTitle("local-1")).toBe("Editor");
  });

  it("does not filter a later executable title after a distinct pre-metadata title", () => {
    const executable = "C:\\Windows\\System32\\cmd.exe";
    publishApplicationTitle("local-1", "Editor");
    publishApplicationTitle("local-1", executable);
    publishSessionMetadata(
      localSession(true, { trusted_initial_title: executable }),
    );
    vi.advanceTimersByTime(75);

    expect(getDynamicTitle("local-1")).toBe(executable);
  });

  it("requires normalized identity equality without trimming whitespace", () => {
    const executable = "C:\\Windows\\System32\\cmd.exe";
    publishApplicationTitle("local-1", ` ${executable} `);
    publishSessionMetadata(
      localSession(true, { trusted_initial_title: executable }),
    );
    vi.advanceTimersByTime(75);

    expect(getDynamicTitle("local-1")).toBe(executable);
  });

  it("does not confuse distinct long executable identities after display truncation", () => {
    const prefix = `C:\\${"a".repeat(300)}`;
    const trusted = `${prefix}\\powershell.exe`;
    const distinct = `${prefix}\\different.exe`;
    publishApplicationTitle("local-1", distinct);
    publishSessionMetadata(
      localSession(true, { trusted_initial_title: trusted }),
    );
    publishSessionCwdPresentation("local-1", cwd());
    vi.advanceTimersByTime(75);

    expect(
      getDynamicTitlesSnapshot().get("local-1")?.applicationTitle,
    ).not.toBeNull();
    expect(getDynamicTitle("local-1")).not.toBe("~/project");
  });

  it("holds title and cwd together until renderer replay resumes", () => {
    publishSessionMetadata(localSession());
    publishSessionCwdPresentation("local-1", cwd());
    vi.advanceTimersByTime(75);
    expect(getDynamicTitle("local-1")).toBe("~/project");

    pauseDynamicTitlePublication("local-1");
    publishSessionCwdPresentation(
      "local-1",
      cwd("~/next", "/home/alice/next"),
    );
    publishApplicationTitle("local-1", "Editor");
    vi.advanceTimersByTime(500);
    expect(getDynamicTitle("local-1")).toBe("~/project");

    resumeDynamicTitlePublication("local-1");
    vi.advanceTimersByTime(75);
    expect(getDynamicTitle("local-1")).toBe("Editor");
    expect(
      getDynamicTitlesSnapshot().get("local-1")?.cwd?.displayPath,
    ).toBe("/home/alice/next");
  });

  it("does not promote title events while policy is disabled", () => {
    publishSessionMetadata(localSession(false));
    publishApplicationTitle("local-1", "Ignored for tab title");
    vi.advanceTimersByTime(75);
    expect(getDynamicTitle("local-1")).toBeNull();
  });

  it("rejects unsafe or oversized frontend cwd payloads defensively", () => {
    publishSessionMetadata(localSession());
    publishSessionCwdPresentation("local-1", {
      title: "bad",
      displayPath: "/tmp/a\u001bb",
      copyValue: "/tmp/a\u001bb",
      copyAsUri: false,
    });
    vi.advanceTimersByTime(75);
    expect(getDynamicTitlesSnapshot().get("local-1")?.cwd).toBeNull();
  });
});

describe("dynamic title listener lifecycle", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    resetDynamicTitlesForTests();
    mocks.eventListeners.clear();
    vi.clearAllMocks();
    installDefaultListenMock();
  });

  it("retries the global listener with a bounded backoff", async () => {
    let globalAttempts = 0;
    mocks.listen.mockImplementation(async (eventName: string, callback) => {
      if (eventName === "sessions-changed") {
        globalAttempts += 1;
        if (globalAttempts < 3) throw new Error("not ready");
      }
      const callbacks = mocks.eventListeners.get(eventName) ?? new Set();
      callbacks.add(callback);
      mocks.eventListeners.set(eventName, callbacks);
      return () => callbacks.delete(callback);
    });
    mocks.invoke.mockResolvedValueOnce([]);

    startDynamicTitles();
    await settle();

    expect(globalAttempts).toBe(3);
    expect(mocks.invoke).toHaveBeenCalledWith("list_sessions");
    expect(
      getDynamicTitleStoreStatsForTests().globalListenerRetrying,
    ).toBe(false);
  });

  it("keeps a slow recovery retry after the first bounded global-listener burst", async () => {
    let globalAttempts = 0;
    mocks.listen.mockImplementation(async (eventName: string, callback) => {
      if (eventName === "sessions-changed") {
        globalAttempts += 1;
        if (globalAttempts <= 3) throw new Error("bridge unavailable");
      }
      const callbacks = mocks.eventListeners.get(eventName) ?? new Set();
      callbacks.add(callback);
      mocks.eventListeners.set(eventName, callbacks);
      return () => callbacks.delete(callback);
    });
    mocks.invoke.mockResolvedValueOnce([]);

    startDynamicTitles();
    await settle();

    expect(globalAttempts).toBe(4);
    expect(mocks.invoke).toHaveBeenCalledWith("list_sessions");
    expect(
      getDynamicTitleStoreStatsForTests().globalListenerRetrying,
    ).toBe(false);
  });

  it("retries a failed initial session snapshot without another lifecycle event", async () => {
    let snapshots = 0;
    mocks.invoke.mockImplementation(async (command: string) => {
      if (command === "list_sessions") {
        snapshots += 1;
        if (snapshots <= 3) throw new Error("backend not ready");
        return [localSession()];
      }
      if (command === "get_session_cwd_presentation") return cwd();
      return null;
    });

    startDynamicTitles();
    await settle();

    expect(snapshots).toBe(4);
    expect(getDynamicTitleStoreStatsForTests().sessionsRefreshRetrying).toBe(
      false,
    );
    expect(getDynamicTitlesSnapshot().get("local-1")?.enabled).toBe(true);
  });

  it("registers sessions-changed before taking the initial snapshot", async () => {
    let resolveGlobalListen!: (dispose: () => void) => void;
    mocks.listen.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveGlobalListen = resolve;
        }),
    );
    mocks.invoke.mockResolvedValueOnce([localSession()]);

    startDynamicTitles();
    await Promise.resolve();
    expect(mocks.invoke).not.toHaveBeenCalled();

    resolveGlobalListen(() => {});
    await settle();
    expect(mocks.invoke).toHaveBeenCalledWith("list_sessions");
  });

  it("takes a passive structured cwd snapshot only after listeners are installed", async () => {
    mocks.invoke
      .mockResolvedValueOnce([
        localSession(true, { dynamic_title_integration_active: false }),
      ])
      .mockResolvedValueOnce(cwd());

    startDynamicTitles();
    await settle();

    expect(mocks.listen).toHaveBeenCalledWith(
      "cwd-presentation-changed-local-1",
      expect.any(Function),
    );
    expect(getDynamicTitle("local-1")).toBe("~/project");
  });

  it("slowly recovers initial cwd snapshot failures without treating them as null", async () => {
    let cwdAttempts = 0;
    mocks.invoke.mockImplementation(async (command: string) => {
      if (command === "list_sessions") return [localSession()];
      if (command === "get_session_cwd_presentation") {
        cwdAttempts += 1;
        if (cwdAttempts <= 3) throw new Error("snapshot unavailable");
        return cwd("~/recovered", "/home/alice/recovered");
      }
      return null;
    });

    startDynamicTitles();
    await settle();

    expect(cwdAttempts).toBe(4);
    expect(getDynamicTitle("local-1")).toBe("~/recovered");
  });

  it("does not let a stale startup snapshot overwrite a newer cwd event", async () => {
    let resolveSnapshot!: (value: SessionCwdPresentation) => void;
    const snapshotPromise = new Promise<SessionCwdPresentation>((resolve) => {
      resolveSnapshot = resolve;
    });
    mocks.invoke.mockImplementation(async (command: string) => {
      if (command === "list_sessions") return [localSession()];
      if (command === "get_session_cwd_presentation") return snapshotPromise;
      return null;
    });

    startDynamicTitles();
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
    emit(
      "cwd-presentation-changed-local-1",
      cwd("~/new", "/home/alice/new"),
    );
    resolveSnapshot(cwd("~/stale", "/home/alice/stale"));
    await settle();

    expect(
      getDynamicTitlesSnapshot().get("local-1")?.cwd?.displayPath,
    ).toBe("/home/alice/new");
  });

  it("does not add cwd listeners or snapshots for default-off Local sessions", async () => {
    mocks.invoke.mockResolvedValueOnce([localSession(false)]);

    startDynamicTitles();
    await settle();

    expect(mocks.listen).not.toHaveBeenCalledWith(
      "cwd-presentation-changed-local-1",
      expect.any(Function),
    );
    expect(mocks.invoke).not.toHaveBeenCalledWith(
      "get_session_cwd_presentation",
      expect.anything(),
    );
  });

  it("retries a failed session listener without an external lifecycle event", async () => {
    let cwdAttempts = 0;
    mocks.invoke
      .mockResolvedValueOnce([localSession()])
      .mockResolvedValue(cwd());
    mocks.listen.mockImplementation(async (eventName: string, callback) => {
      if (eventName === "cwd-presentation-changed-local-1") {
        cwdAttempts += 1;
        if (cwdAttempts === 1) throw new Error("temporary failure");
      }
      const callbacks = mocks.eventListeners.get(eventName) ?? new Set();
      callbacks.add(callback);
      mocks.eventListeners.set(eventName, callbacks);
      return () => callbacks.delete(callback);
    });

    startDynamicTitles();
    await settle();

    expect(cwdAttempts).toBe(2);
    expect(getDynamicTitleStoreStatsForTests().sessionListenerRetries).toBe(0);
    expect(
      mocks.eventListeners.get("cwd-presentation-changed-local-1")?.size,
    ).toBe(1);
  });

  it("disposes partial registrations and slowly recovers after a failed burst", async () => {
    const cwdDispose = vi.fn();
    let closedAttempts = 0;
    mocks.invoke.mockResolvedValueOnce([localSession()]);
    mocks.listen.mockImplementation(async (eventName: string, callback) => {
      if (eventName === "cwd-presentation-changed-local-1") {
        const callbacks = mocks.eventListeners.get(eventName) ?? new Set();
        callbacks.add(callback);
        mocks.eventListeners.set(eventName, callbacks);
        return cwdDispose;
      }
      if (eventName === "session-closed-local-1") {
        closedAttempts += 1;
        if (closedAttempts <= 3) throw new Error("closed listener failed");
      }
      return () => {};
    });

    startDynamicTitles();
    await settle();

    expect(closedAttempts).toBe(4);
    expect(cwdDispose).toHaveBeenCalledTimes(3);
    expect(getDynamicTitleStoreStatsForTests().sessionListeners).toBe(1);
    expect(getDynamicTitleStoreStatsForTests().sessionListenerRetries).toBe(0);
  });

  it("an old listener rejection cannot delete a newer same-id setup", async () => {
    let liveSessions: SessionInfo[] = [localSession()];
    let rejectFirstCwd!: (error: unknown) => void;
    const firstCwdPromise = new Promise<() => void>((_, reject) => {
      rejectFirstCwd = reject;
    });
    let cwdRegistrations = 0;

    mocks.invoke.mockImplementation(async (command: string) => {
      if (command === "list_sessions") return liveSessions;
      if (command === "get_session_cwd_presentation") return cwd();
      return null;
    });
    mocks.listen.mockImplementation((eventName: string, callback) => {
      const callbacks = mocks.eventListeners.get(eventName) ?? new Set();
      callbacks.add(callback);
      mocks.eventListeners.set(eventName, callbacks);
      const dispose = () => callbacks.delete(callback);
      if (eventName === "cwd-presentation-changed-local-1") {
        cwdRegistrations += 1;
        if (cwdRegistrations === 1) return firstCwdPromise;
      }
      return Promise.resolve(() => {
        dispose();
      });
    });

    startDynamicTitles();
    await Promise.resolve();
    await Promise.resolve();

    liveSessions = [];
    emit("sessions-changed");
    await settle();
    liveSessions = [localSession()];
    emit("sessions-changed");
    await settle();
    expect(cwdRegistrations).toBe(2);

    rejectFirstCwd(new Error("stale setup failed"));
    await settle();
    expect(getDynamicTitleStoreStatsForTests().sessionListeners).toBe(1);
  });

  it("ignores a late close callback from an older same-id generation", async () => {
    let liveSessions: SessionInfo[] = [localSession()];
    mocks.invoke.mockImplementation(async (command: string) => {
      if (command === "list_sessions") return liveSessions;
      if (command === "get_session_cwd_presentation") return cwd();
      return null;
    });

    startDynamicTitles();
    await settle();
    const oldClosed = [
      ...(mocks.eventListeners.get("session-closed-local-1") ?? []),
    ][0];
    expect(oldClosed).toBeTypeOf("function");

    liveSessions = [];
    emit("sessions-changed");
    await settle();
    liveSessions = [localSession()];
    emit("sessions-changed");
    await settle();
    expect(getDynamicTitle("local-1")).toBe("~/project");

    oldClosed?.({ payload: undefined });
    await settle();
    expect(getDynamicTitle("local-1")).toBe("~/project");
  });

  it("bounds tombstones during sustained session churn", async () => {
    const sessions = Array.from({ length: 1100 }, (_, index) =>
      sshSession(`ssh-${index}`),
    );
    mocks.invoke.mockResolvedValueOnce(sessions);

    startDynamicTitles();
    await settle();
    for (const session of sessions) {
      emit(`session-closed-${session.id}`);
    }
    await settle();

    const stats = getDynamicTitleStoreStatsForTests();
    expect(stats.tombstones).toBeLessThanOrEqual(1024);
    expect(stats.tombstoneQueue).toBeLessThanOrEqual(2048);
    expect(stats.states).toBe(0);
    expect(stats.snapshots).toBe(0);
  });
});
