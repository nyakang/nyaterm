import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useSyncExternalStore } from "react";
import { invoke } from "@/lib/invoke";
import type {
  SessionCwdPresentation,
  SessionInfo,
  SessionTitleSnapshot,
  WorkspaceSessionType,
} from "@/types/global";

const TITLE_UPDATE_DEBOUNCE_MS = 75;
const MAX_TITLE_CODE_POINTS = 256;
const MAX_RAW_TITLE_UTF16_UNITS = 4096;
const MAX_EXECUTABLE_IDENTITY_UTF16_UNITS = 32 * 1024;
const MAX_CWD_PRESENTATION_UTF16_UNITS = 16 * 1024;
const MAX_SESSION_TOMBSTONES = 1024;
const MAX_LISTENER_SETUP_ATTEMPTS = 3;
const LISTENER_RETRY_BASE_MS = 100;
const LISTENER_RECOVERY_MS = 5_000;
const SSH_TITLE_DELIMITER = " · ";
const ELLIPSIS = "…";

const BIDI_FORMATTING_CONTROLS =
  /[\u061C\u200E\u200F\u202A-\u202E\u2066-\u206F]/gu;
const TITLE_WHITESPACE_CONTROLS = /[\u0009\u000A\u000D]/gu;
const NON_PRINTING_CONTROLS =
  /[\u0000-\u0008\u000B\u000C\u000E-\u001F\u007F-\u009F]/gu;
const UNSAFE_PATH_PRESENTATION =
  /[\u0000-\u001F\u007F-\u009F\u061C\u200E\u200F\u2028\u2029\u202A-\u202E\u2066-\u206F]/u;

type GraphemeSegmenter = new (
  locales?: string | string[],
  options?: { granularity: "grapheme" },
) => { segment: (value: string) => Iterable<{ segment: string }> };

interface DynamicTitleState {
  generation: number;
  sessionType?: WorkspaceSessionType;
  connectionName: string | null;
  enabled: boolean;
  metadataReady: boolean;
  trustedInitialTitle: string | null;
  initialApplicationTitleHandled: boolean;
  preMetadataTitleSeen: boolean;
  preMetadataFirstIdentity: string | null;
  preMetadataCurrentIdentity: string | null;
  preMetadataSawDistinctIdentity: boolean;
  applicationTitle: string | null;
  cwd: SessionCwdPresentation | null;
  effectiveTitle: string | null;
  timer: ReturnType<typeof setTimeout> | null;
  publicationPaused: boolean;
}

type TombstoneReason = "closed" | "missing";
interface SessionTombstone {
  reason: TombstoneReason;
  generation: number;
}

const states = new Map<string, DynamicTitleState>();
const snapshots = new Map<string, SessionTitleSnapshot>();
const tombstones = new Map<string, SessionTombstone>();
const tombstoneQueue: Array<{ id: string; generation: number }> = [];
const listeners = new Set<() => void>();
const sessionListenerPromises = new Map<string, Promise<() => void>>();
const sessionListenerRetryTimers = new Map<
  string,
  ReturnType<typeof setTimeout>
>();
const sessionListenerRetryAttempts = new Map<string, number>();
let sessionsChangedListener: Promise<UnlistenFn> | null = null;
let globalListenerRetryTimer: ReturnType<typeof setTimeout> | null = null;
let sessionsRefreshRetryTimer: ReturnType<typeof setTimeout> | null = null;
let sessionsRefreshRetryAttempt = 0;
let globalListenerGeneration = 0;
let refreshGeneration = 0;
let stateGeneration = 0;
let tombstoneGeneration = 0;
let started = false;
let snapshotMap: ReadonlyMap<string, SessionTitleSnapshot> = new Map();

function notify() {
  snapshotMap = new Map(snapshots);
  for (const listener of listeners) listener();
}

function codePointCount(value: string): number {
  return Array.from(value).length;
}

function truncateTitle(
  value: string,
  maxCodePoints = MAX_TITLE_CODE_POINTS,
): string {
  if (maxCodePoints <= 0) return "";
  if (codePointCount(value) <= maxCodePoints) return value;

  const ellipsisLength = codePointCount(ELLIPSIS);
  if (maxCodePoints <= ellipsisLength) return ELLIPSIS;
  const maxContent = maxCodePoints - ellipsisLength;
  const Segmenter = (
    globalThis.Intl as typeof Intl & { Segmenter?: GraphemeSegmenter }
  ).Segmenter;
  if (Segmenter) {
    const segmenter = new Segmenter(undefined, { granularity: "grapheme" });
    let result = "";
    for (const { segment } of segmenter.segment(value)) {
      if (codePointCount(result) + codePointCount(segment) > maxContent) break;
      result += segment;
    }
    return `${result}${ELLIPSIS}`;
  }

  const codePoints = Array.from(value);
  const end = fallbackTruncationEnd(codePoints, maxContent);
  return `${codePoints.slice(0, end).join("")}${ELLIPSIS}`;
}

function isGraphemeContinuation(value: string | undefined): boolean {
  if (!value) return false;
  return (
    value === "\u200D" ||
    /\p{M}/u.test(value) ||
    /[\uFE00-\uFE0F\u{E0100}-\u{E01EF}\u{1F3FB}-\u{1F3FF}]/u.test(
      value,
    ) ||
    /[\u{E0020}-\u{E007F}]/u.test(value)
  );
}

function isRegionalIndicator(value: string | undefined): boolean {
  return value != null && /[\u{1F1E6}-\u{1F1FF}]/u.test(value);
}

function fallbackTruncationEnd(
  codePoints: string[],
  proposedEnd: number,
): number {
  let end = Math.min(proposedEnd, codePoints.length);
  if (end >= codePoints.length) return end;

  // Regional indicators form pairs. If the proposed boundary divides a pair,
  // drop the first half rather than render a misleading lone flag letter.
  if (isRegionalIndicator(codePoints[end])) {
    let precedingIndicators = 0;
    for (let index = end - 1; index >= 0; index -= 1) {
      if (!isRegionalIndicator(codePoints[index])) break;
      precedingIndicators += 1;
    }
    if (precedingIndicators % 2 === 1) end -= 1;
  }

  // Conservatively remove the whole joined/extended cluster around the cut.
  // This intentionally over-truncates when Intl.Segmenter is unavailable.
  while (
    end > 0 &&
    (isGraphemeContinuation(codePoints[end]) ||
      isGraphemeContinuation(codePoints[end - 1]) ||
      codePoints[end - 1] === "\u200D" ||
      (codePoints[end - 1] === "\r" && codePoints[end] === "\n"))
  ) {
    end -= 1;
  }
  return end;
}

function boundUtf16(value: string, limit: number): string {
  if (value.length <= limit) return value;
  let end = limit;
  const last = value.charCodeAt(end - 1);
  if (last >= 0xd800 && last <= 0xdbff) end -= 1;
  return value.slice(0, end);
}

function normalizeTitleCharacters(value: string): string {
  return boundUtf16(value, MAX_RAW_TITLE_UTF16_UNITS)
    .normalize("NFC")
    .replace(BIDI_FORMATTING_CONTROLS, "")
    .replace(TITLE_WHITESPACE_CONTROLS, " ")
    .replace(NON_PRINTING_CONTROLS, "")
    .replace(/\s+/gu, " ")
    .trim();
}

/** Normalize untrusted OSC 0/2 text before any tab/native-title sink. */
export function sanitizeDynamicTitle(
  value: string | null | undefined,
): string | null {
  if (value == null) return null;
  const normalized = normalizeTitleCharacters(value);
  if (!normalized) return null;
  return truncateTitle(normalized) || null;
}

function validateCwdPresentation(
  value: SessionCwdPresentation | null | undefined,
): SessionCwdPresentation | null {
  if (value == null) return null;
  if (
    typeof value.title !== "string" ||
    typeof value.displayPath !== "string" ||
    typeof value.copyValue !== "string" ||
    (value.operationalPath != null &&
      typeof value.operationalPath !== "string") ||
    typeof value.copyAsUri !== "boolean" ||
    !value.displayPath ||
    !value.copyValue ||
    value.displayPath.length > MAX_CWD_PRESENTATION_UTF16_UNITS ||
    value.copyValue.length > MAX_CWD_PRESENTATION_UTF16_UNITS ||
    (value.operationalPath?.length ?? 0) >
      MAX_CWD_PRESENTATION_UTF16_UNITS ||
    UNSAFE_PATH_PRESENTATION.test(value.displayPath) ||
    UNSAFE_PATH_PRESENTATION.test(value.copyValue) ||
    (value.operationalPath != null &&
      UNSAFE_PATH_PRESENTATION.test(value.operationalPath))
  ) {
    return null;
  }
  const title = sanitizeDynamicTitle(value.title);
  if (!title) return null;
  return {
    title,
    displayPath: value.displayPath,
    copyValue: value.copyValue,
    operationalPath: value.operationalPath ?? null,
    copyAsUri: value.copyAsUri,
  };
}

function sameCwdPresentation(
  left: SessionCwdPresentation | null,
  right: SessionCwdPresentation | null,
): boolean {
  return (
    left === right ||
    (left !== null &&
      right !== null &&
      left.title === right.title &&
      left.displayPath === right.displayPath &&
      left.copyValue === right.copyValue &&
      left.operationalPath === right.operationalPath &&
      left.copyAsUri === right.copyAsUri)
  );
}

function stateFor(sessionId: string): DynamicTitleState {
  const existing = states.get(sessionId);
  if (existing) return existing;

  const created: DynamicTitleState = {
    generation: ++stateGeneration,
    connectionName: null,
    enabled: false,
    metadataReady: false,
    trustedInitialTitle: null,
    initialApplicationTitleHandled: false,
    preMetadataTitleSeen: false,
    preMetadataFirstIdentity: null,
    preMetadataCurrentIdentity: null,
    preMetadataSawDistinctIdentity: false,
    applicationTitle: null,
    cwd: null,
    effectiveTitle: null,
    timer: null,
    publicationPaused: false,
  };
  states.set(sessionId, created);
  snapshots.set(sessionId, {
    applicationTitle: null,
    cwd: null,
    effectiveTitle: null,
    enabled: false,
  });
  return created;
}

function composeSshDynamicTitle(
  connectionName: string | null,
  applicationTitle: string,
): string | null {
  const prefix = sanitizeDynamicTitle(connectionName) ?? "SSH";
  const remote = sanitizeDynamicTitle(applicationTitle);
  if (!remote) return null;

  const delimiterLength = codePointCount(SSH_TITLE_DELIMITER);
  const contentBudget = MAX_TITLE_CODE_POINTS - delimiterLength;
  const prefixLength = codePointCount(prefix);
  const remoteLength = codePointCount(remote);
  if (prefixLength + remoteLength <= contentBudget) {
    return sanitizeDynamicTitle(`${prefix}${SSH_TITLE_DELIMITER}${remote}`);
  }

  // A short side keeps its complete identity; when both are long, split the
  // remaining budget so untrusted remote text can neither erase the trusted
  // connection prefix nor disappear behind it.
  const balancedPrefixBudget = Math.ceil(contentBudget / 2);
  const balancedRemoteBudget = contentBudget - balancedPrefixBudget;
  let prefixBudget: number;
  let remoteBudget: number;
  if (remoteLength <= balancedRemoteBudget) {
    remoteBudget = remoteLength;
    prefixBudget = contentBudget - remoteBudget;
  } else if (prefixLength <= balancedPrefixBudget) {
    prefixBudget = prefixLength;
    remoteBudget = contentBudget - prefixBudget;
  } else {
    prefixBudget = balancedPrefixBudget;
    remoteBudget = balancedRemoteBudget;
  }

  const composed = `${truncateTitle(prefix, prefixBudget)}${SSH_TITLE_DELIMITER}${truncateTitle(remote, remoteBudget)}`;
  return sanitizeDynamicTitle(composed);
}

function displayableLocalCwd(
  state: DynamicTitleState,
): SessionCwdPresentation | null {
  if (!state.enabled || state.sessionType !== "Local") return null;

  // Managed integration identifies who installed the producer, not whether a
  // validated OSC 7 report is displayable. Passive shells and profiles may
  // provide compatible cwd reports themselves.
  return state.cwd;
}

function effectiveTitleFor(state: DynamicTitleState): string | null {
  if (!state.enabled) return null;

  const applicationTitle = state.applicationTitle;
  if (state.sessionType === "SSH") {
    return applicationTitle
      ? composeSshDynamicTitle(state.connectionName, applicationTitle)
      : null;
  }

  return sanitizeDynamicTitle(
    applicationTitle ?? displayableLocalCwd(state)?.title,
  );
}

function publishNow(sessionId: string) {
  const state = states.get(sessionId);
  if (!state) return;
  state.timer = null;
  if (state.publicationPaused) return;

  const nextEffective = effectiveTitleFor(state);
  const nextCwd = displayableLocalCwd(state);
  const previous = snapshots.get(sessionId);
  if (
    nextEffective === state.effectiveTitle &&
    previous?.applicationTitle === state.applicationTitle &&
    sameCwdPresentation(previous?.cwd ?? null, nextCwd) &&
    previous?.enabled === state.enabled
  ) {
    return;
  }
  state.effectiveTitle = nextEffective;

  snapshots.set(sessionId, {
    applicationTitle: state.applicationTitle,
    cwd: nextCwd,
    effectiveTitle: nextEffective,
    enabled: state.enabled,
  });
  // Disabled sessions keep bounded lifecycle state but do not repaint for
  // every ignored application title.
  if (state.enabled || previous?.enabled !== state.enabled) notify();
}

function schedulePublish(sessionId: string) {
  const state = stateFor(sessionId);
  if (state.publicationPaused || state.timer !== null) return;
  state.timer = setTimeout(
    () => publishNow(sessionId),
    TITLE_UPDATE_DEBOUNCE_MS,
  );
}

function normalizeExecutableIdentity(value: string): string | null {
  if (value.length > MAX_EXECUTABLE_IDENTITY_UTF16_UNITS) return null;
  return value
    .replace(/^\\\\\?\\/u, "")
    .replace(/\\/gu, "/")
    .toLocaleLowerCase("en-US");
}

function isTrustedInitialTitle(
  state: DynamicTitleState,
  rawTitle: string,
): boolean {
  if (
    state.sessionType !== "Local" ||
    state.initialApplicationTitleHandled ||
    !state.trustedInitialTitle
  ) {
    return false;
  }
  const incomingIdentity = normalizeExecutableIdentity(rawTitle);
  const trustedIdentity = normalizeExecutableIdentity(
    state.trustedInitialTitle,
  );
  return incomingIdentity !== null && incomingIdentity === trustedIdentity;
}

function addTombstone(sessionId: string, reason: TombstoneReason) {
  const generation = ++tombstoneGeneration;
  tombstones.set(sessionId, { reason, generation });
  tombstoneQueue.push({ id: sessionId, generation });

  while (
    tombstones.size > MAX_SESSION_TOMBSTONES ||
    tombstoneQueue.length > MAX_SESSION_TOMBSTONES * 2
  ) {
    const stale = tombstoneQueue.shift();
    if (!stale) break;
    if (tombstones.get(stale.id)?.generation === stale.generation) {
      tombstones.delete(stale.id);
    }
  }
}

export function publishSessionMetadata(session: SessionInfo): void {
  const tombstone = tombstones.get(session.id);
  if (tombstone?.reason === "closed") return;
  if (tombstone?.reason === "missing") {
    tombstones.delete(session.id);
    const staleState = states.get(session.id);
    if (staleState?.timer != null) clearTimeout(staleState.timer);
    states.delete(session.id);
    if (snapshots.delete(session.id)) notify();
  }

  const state = stateFor(session.id);
  state.sessionType = session.session_type;
  state.connectionName = sanitizeDynamicTitle(session.name) ?? "Session";
  state.enabled =
    session.dynamic_title_enabled === true &&
    (session.session_type === "Local" || session.session_type === "SSH");
  state.trustedInitialTitle =
    typeof session.trusted_initial_title === "string" &&
    session.trusted_initial_title.length <=
      MAX_EXECUTABLE_IDENTITY_UTF16_UNITS
      ? session.trusted_initial_title
      : null;
  state.metadataReady = true;

  if (!state.enabled) {
    state.applicationTitle = null;
    state.cwd = null;
    state.initialApplicationTitleHandled = false;
    state.preMetadataTitleSeen = false;
    state.preMetadataFirstIdentity = null;
    state.preMetadataCurrentIdentity = null;
    state.preMetadataSawDistinctIdentity = false;
  } else if (state.preMetadataTitleSeen) {
    if (
      state.applicationTitle &&
      !state.preMetadataSawDistinctIdentity &&
      state.preMetadataCurrentIdentity !== null &&
      state.trustedInitialTitle !== null &&
      state.preMetadataCurrentIdentity ===
        normalizeExecutableIdentity(state.trustedInitialTitle)
    ) {
      state.applicationTitle = null;
    }
    state.initialApplicationTitleHandled = true;
    state.preMetadataTitleSeen = false;
    state.preMetadataFirstIdentity = null;
    state.preMetadataCurrentIdentity = null;
    state.preMetadataSawDistinctIdentity = false;
  }
  schedulePublish(session.id);
}

function removeSession(sessionId: string, reason: TombstoneReason) {
  addTombstone(sessionId, reason);

  const state = states.get(sessionId);
  if (state?.timer != null) clearTimeout(state.timer);
  states.delete(sessionId);
  if (snapshots.delete(sessionId)) notify();

  const listenerPromise = sessionListenerPromises.get(sessionId);
  sessionListenerPromises.delete(sessionId);
  void listenerPromise?.then((dispose) => dispose());
  const retryTimer = sessionListenerRetryTimers.get(sessionId);
  if (retryTimer !== undefined) clearTimeout(retryTimer);
  sessionListenerRetryTimers.delete(sessionId);
  sessionListenerRetryAttempts.delete(sessionId);
}

export function subscribeDynamicTitles(callback: () => void): () => void {
  listeners.add(callback);
  return () => listeners.delete(callback);
}

export function getDynamicTitlesSnapshot(): ReadonlyMap<
  string,
  SessionTitleSnapshot
> {
  return snapshotMap;
}

export function getDynamicTitle(
  sessionId: string | null | undefined,
): string | null {
  if (!sessionId) return null;
  return snapshots.get(sessionId)?.effectiveTitle ?? null;
}

export function getDynamicTitleSnapshot(
  sessionId: string | null | undefined,
): SessionTitleSnapshot | null {
  if (!sessionId) return null;
  return snapshots.get(sessionId) ?? null;
}

function rememberPreMetadataTitle(
  state: DynamicTitleState,
  rawTitle: string,
) {
  const identity = normalizeExecutableIdentity(rawTitle);
  if (!state.preMetadataTitleSeen) {
    state.preMetadataTitleSeen = true;
    state.preMetadataFirstIdentity = identity;
  } else if (identity !== state.preMetadataFirstIdentity) {
    state.preMetadataSawDistinctIdentity = true;
  }
  state.preMetadataCurrentIdentity = identity;
}

/** Called only after xterm.js parses OSC 0/2. */
export function publishApplicationTitle(
  sessionId: string,
  rawTitle: string,
): void {
  if (tombstones.has(sessionId)) return;
  const state = stateFor(sessionId);
  if (state.metadataReady && !state.enabled) return;
  const title = sanitizeDynamicTitle(rawTitle);
  if (!state.metadataReady && title) {
    rememberPreMetadataTitle(state, rawTitle);
  }
  if (!title) {
    state.applicationTitle = null;
    schedulePublish(sessionId);
    return;
  }

  if (state.metadataReady && !state.initialApplicationTitleHandled) {
    const trustedInitialTitle = isTrustedInitialTitle(state, rawTitle);
    state.initialApplicationTitleHandled = true;
    if (trustedInitialTitle) {
      state.applicationTitle = null;
      schedulePublish(sessionId);
      return;
    }
  }

  state.applicationTitle = title;
  schedulePublish(sessionId);
}

/** Called by the backend-authoritative structured cwd event bridge. */
export function publishSessionCwdPresentation(
  sessionId: string,
  presentation: SessionCwdPresentation | null,
): void {
  if (tombstones.has(sessionId)) return;
  const state = stateFor(sessionId);
  state.cwd = validateCwdPresentation(presentation);
  schedulePublish(sessionId);
}

/** Hold title/cwd publication while renderer replay is unsettled. */
export function pauseDynamicTitlePublication(sessionId: string): void {
  if (tombstones.has(sessionId)) return;
  const state = stateFor(sessionId);
  state.publicationPaused = true;
  if (state.timer !== null) {
    clearTimeout(state.timer);
    state.timer = null;
  }
}

/** Publish only the final cached title/cwd after renderer attach/replay settles. */
export function resumeDynamicTitlePublication(sessionId: string): void {
  const state = states.get(sessionId);
  if (!state || tombstones.has(sessionId)) return;
  if (!state.publicationPaused) return;
  state.publicationPaused = false;
  schedulePublish(sessionId);
}

async function setupSessionListeners(
  session: SessionInfo,
  generation: number,
): Promise<() => void> {
  const sessionId = session.id;
  const isCurrentGeneration = () =>
    states.get(sessionId)?.generation === generation;
  let disposed = false;
  let cwdEventObserved = false;
  let cwdSnapshotRetryTimer: ReturnType<typeof setTimeout> | null = null;
  const disposers: UnlistenFn[] = [];

  const requestCwdSnapshot = (attempt: number) => {
    if (disposed || cwdEventObserved || !isCurrentGeneration()) return;
    void invoke<SessionCwdPresentation | null>(
      "get_session_cwd_presentation",
      { sessionId },
    ).then(
      (currentCwd) => {
        if (!disposed && !cwdEventObserved && isCurrentGeneration()) {
          publishSessionCwdPresentation(sessionId, currentCwd);
        }
      },
      () => {
        if (
          disposed ||
          cwdEventObserved ||
          !isCurrentGeneration()
        ) {
          return;
        }
        const exhaustedBurst = attempt >= MAX_LISTENER_SETUP_ATTEMPTS;
        cwdSnapshotRetryTimer = setTimeout(() => {
          cwdSnapshotRetryTimer = null;
          requestCwdSnapshot(exhaustedBurst ? 1 : attempt + 1);
        }, exhaustedBurst ? LISTENER_RECOVERY_MS : LISTENER_RETRY_BASE_MS * attempt);
      },
    );
  };

  try {
    const trackLocalCwd =
      session.session_type === "Local" && session.dynamic_title_enabled === true;
    if (trackLocalCwd) {
      const cwdUnlisten = await listen<SessionCwdPresentation | null>(
        `cwd-presentation-changed-${sessionId}`,
        (event) => {
          if (!disposed && isCurrentGeneration()) {
            cwdEventObserved = true;
            publishSessionCwdPresentation(sessionId, event.payload);
          }
        },
      );
      if (disposed) cwdUnlisten();
      else disposers.push(cwdUnlisten);
    }

    const closedUnlisten = await listen<void>(
      `session-closed-${sessionId}`,
      () => {
        if (!disposed && isCurrentGeneration()) {
          removeSession(sessionId, "closed");
        }
      },
    );
    if (disposed) closedUnlisten();
    else disposers.push(closedUnlisten);

    if (trackLocalCwd) {
      // Do not keep listener disposal waiting on snapshot IPC. Errors are not
      // authoritative null cwd values: retry at a bounded rate while the
      // already-live event listener prevents stale snapshot overwrites.
      requestCwdSnapshot(1);
    }
  } catch (error) {
    disposed = true;
    if (cwdSnapshotRetryTimer !== null) {
      clearTimeout(cwdSnapshotRetryTimer);
      cwdSnapshotRetryTimer = null;
    }
    for (const dispose of disposers.splice(0)) dispose();
    throw error;
  }

  return () => {
    disposed = true;
    if (cwdSnapshotRetryTimer !== null) {
      clearTimeout(cwdSnapshotRetryTimer);
      cwdSnapshotRetryTimer = null;
    }
    for (const dispose of disposers.splice(0)) dispose();
  };
}

function ensureSessionListeners(session: SessionInfo) {
  if (
    sessionListenerPromises.has(session.id) ||
    sessionListenerRetryTimers.has(session.id)
  ) {
    return;
  }

  const generation = stateFor(session.id).generation;
  const attempt = (sessionListenerRetryAttempts.get(session.id) ?? 0) + 1;
  sessionListenerRetryAttempts.set(session.id, attempt);
  let promise: Promise<() => void>;
  promise = setupSessionListeners(session, generation).then(
    (dispose) => {
      sessionListenerRetryAttempts.delete(session.id);
      return dispose;
    },
    () => {
      if (sessionListenerPromises.get(session.id) === promise) {
        sessionListenerPromises.delete(session.id);
      }
      if (
        states.get(session.id)?.generation === generation &&
        !tombstones.has(session.id)
      ) {
        const exhaustedBurst = attempt >= MAX_LISTENER_SETUP_ATTEMPTS;
        if (exhaustedBurst) sessionListenerRetryAttempts.delete(session.id);
        const timer = setTimeout(() => {
          sessionListenerRetryTimers.delete(session.id);
          if (
            states.get(session.id)?.generation === generation &&
            !tombstones.has(session.id)
          ) {
            ensureSessionListeners(session);
          }
        }, exhaustedBurst ? LISTENER_RECOVERY_MS : LISTENER_RETRY_BASE_MS * attempt);
        sessionListenerRetryTimers.set(session.id, timer);
      } else {
        sessionListenerRetryAttempts.delete(session.id);
      }
      return () => {};
    },
  );
  sessionListenerPromises.set(session.id, promise);
}

function scheduleSessionsRefreshRetry(generation: number) {
  if (
    sessionsRefreshRetryTimer !== null ||
    !started ||
    generation !== refreshGeneration
  ) {
    return;
  }
  const attempt = ++sessionsRefreshRetryAttempt;
  const exhaustedBurst = attempt >= MAX_LISTENER_SETUP_ATTEMPTS;
  if (exhaustedBurst) sessionsRefreshRetryAttempt = 0;
  sessionsRefreshRetryTimer = setTimeout(() => {
    sessionsRefreshRetryTimer = null;
    if (started && generation === refreshGeneration) {
      void refreshSessions();
    }
  }, exhaustedBurst ? LISTENER_RECOVERY_MS : LISTENER_RETRY_BASE_MS * attempt);
}

async function refreshSessions(): Promise<void> {
  const generation = ++refreshGeneration;
  if (sessionsRefreshRetryTimer !== null) {
    clearTimeout(sessionsRefreshRetryTimer);
    sessionsRefreshRetryTimer = null;
  }
  try {
    const sessions = await invoke<SessionInfo[]>("list_sessions");
    if (generation !== refreshGeneration) return;
    sessionsRefreshRetryAttempt = 0;

    const ids = new Set(sessions.map((session) => session.id));
    for (const id of [...states.keys()]) {
      if (!ids.has(id)) removeSession(id, "missing");
    }

    for (const session of sessions) {
      if (tombstones.get(session.id)?.reason === "closed") continue;
      publishSessionMetadata(session);
      ensureSessionListeners(session);
    }
  } catch {
    // Listener registration can succeed before the invoke bridge/backend is
    // ready. Keep one bounded-rate retry chain so an otherwise idle existing
    // session cannot remain metadata-less forever.
    scheduleSessionsRefreshRetry(generation);
  }
}

async function startGlobalDynamicTitleListener(
  generation: number,
  attempt: number,
): Promise<void> {
  try {
    const unlisten = await listen("sessions-changed", () => {
      void refreshSessions();
    });
    if (!started || generation !== globalListenerGeneration) {
      unlisten();
      return;
    }
    sessionsChangedListener = Promise.resolve(unlisten);
    globalListenerRetryTimer = null;
    // Install the global listener before the initial snapshot so creation
    // cannot land between list_sessions and listener registration.
    await refreshSessions();
  } catch {
    if (!started || generation !== globalListenerGeneration) return;
    const exhaustedBurst = attempt >= MAX_LISTENER_SETUP_ATTEMPTS;
    globalListenerRetryTimer = setTimeout(() => {
      globalListenerRetryTimer = null;
      void startGlobalDynamicTitleListener(
        generation,
        exhaustedBurst ? 1 : attempt + 1,
      );
    }, exhaustedBurst ? LISTENER_RECOVERY_MS : LISTENER_RETRY_BASE_MS * attempt);
  }
}

/** Start metadata/cwd lifecycle listeners. Safe to call repeatedly. */
export function startDynamicTitles(): void {
  if (started) return;
  started = true;
  const generation = ++globalListenerGeneration;
  void startGlobalDynamicTitleListener(generation, 1);
}

/** Test/HMR cleanup; production keeps this store for app lifetime. */
export function resetDynamicTitlesForTests(): void {
  for (const state of states.values()) {
    if (state.timer !== null) clearTimeout(state.timer);
  }
  for (const promise of sessionListenerPromises.values()) {
    void promise.then((dispose) => dispose());
  }
  sessionListenerPromises.clear();
  for (const timer of sessionListenerRetryTimers.values()) {
    clearTimeout(timer);
  }
  sessionListenerRetryTimers.clear();
  sessionListenerRetryAttempts.clear();
  if (globalListenerRetryTimer !== null) {
    clearTimeout(globalListenerRetryTimer);
    globalListenerRetryTimer = null;
  }
  if (sessionsRefreshRetryTimer !== null) {
    clearTimeout(sessionsRefreshRetryTimer);
    sessionsRefreshRetryTimer = null;
  }
  sessionsRefreshRetryAttempt = 0;
  globalListenerGeneration += 1;
  states.clear();
  snapshots.clear();
  tombstones.clear();
  tombstoneQueue.length = 0;
  tombstoneGeneration = 0;
  stateGeneration = 0;
  snapshotMap = new Map();
  listeners.clear();
  if (sessionsChangedListener) {
    void sessionsChangedListener.then((dispose) => dispose());
    sessionsChangedListener = null;
  }
  started = false;
  refreshGeneration += 1;
}

/** Bounded-state diagnostics used by regression tests only. */
export function getDynamicTitleStoreStatsForTests() {
  return {
    states: states.size,
    snapshots: snapshots.size,
    tombstones: tombstones.size,
    tombstoneQueue: tombstoneQueue.length,
    sessionListeners: sessionListenerPromises.size,
    sessionListenerRetries: sessionListenerRetryTimers.size,
    globalListenerRetrying: globalListenerRetryTimer !== null,
    sessionsRefreshRetrying: sessionsRefreshRetryTimer !== null,
  };
}

export function useDynamicTitles(): ReadonlyMap<string, SessionTitleSnapshot> {
  return useSyncExternalStore(
    subscribeDynamicTitles,
    getDynamicTitlesSnapshot,
    getDynamicTitlesSnapshot,
  );
}
