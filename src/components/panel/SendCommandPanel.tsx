import {
  type ChangeEvent,
  Fragment,
  type KeyboardEvent,
  type ReactNode,
  type UIEvent,
  useCallback,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import { MdAdd, MdCheck, MdKeyboardArrowDown, MdRemove, MdSend, MdStop } from "react-icons/md";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Progress } from "@/components/ui/progress";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import { invoke } from "@/lib/invoke";
import type {
  SendCommandCount,
  SendCommandDataType,
  SendCommandMode,
  SendCommandPanelDraft,
  SendCommandTarget,
} from "@/lib/sendCommandPanelEvents";
import { buildTerminalCommandInput, sendSessionInput } from "@/lib/sessionInput";
import type { SessionType, SyncGroup } from "@/types/global";

interface SendCommandPanelProps {
  serialSessionId: string | null;
  currentShellSessionId: string | null;
  shellSessionIds: string[];
  syncGroups: SyncGroup[];
  currentWindowLabel: string;
  sessionTargets: SendCommandSessionTarget[];
  clearAfterSend: boolean;
  draft?: SendCommandPanelDraft | null;
  onDraftConsumed?: () => void;
  onSendingChange?: (isSending: boolean) => void;
  onClearAfterSendChange: (enabled: boolean) => void;
}

interface SendProgress {
  completedUnits: number;
  totalUnits: number | null;
  unitsPerRound: number;
  totalRounds: number | null;
}

type TargetKind = "serial" | "shell";
type TargetSelectValue = SendCommandTarget | `group:${string}` | `session:${string}`;
type LineEnding = "none" | "cr" | "lf" | "crlf";

interface SendCommandSessionTarget {
  id: string;
  name: string;
  tabName: string;
  type: SessionType;
  ownerWindowLabel?: string | null;
}

interface SendUnit {
  data: string;
  registerSubmission?: string | null;
  resetPreview?: boolean;
}

interface HexGuideRow {
  lineIndex: number;
  guidePositions: number[];
}

interface TargetMenuItemProps {
  checked: boolean;
  children: ReactNode;
  disabled?: boolean;
  onSelect: () => void;
}

const LINE_INTERVAL_SECONDS = 1;
const CHARACTER_INTERVAL_SECONDS = 0.02;
const BYTE_INTERVAL_SECONDS = 0.02;
const HEX_INPUT_INVALID_PATTERN = /[^0-9A-F\s]/u;
const HEX_CHAR_PATTERN = /[0-9A-F]/u;

function formatIntervalSeconds(value: number): string {
  if (!Number.isFinite(value) || value < 0) return "0";
  if (value === LINE_INTERVAL_SECONDS) return "1.00";
  if (value === CHARACTER_INTERVAL_SECONDS) return "0.02";
  return String(value);
}

function normalizeTextNewlines(value: string): string {
  return value.replace(/\r\n|\r/gu, "\n");
}

function buildTextSendUnits(command: string, mode: SendCommandMode): string[] {
  const normalized = normalizeTextNewlines(command);
  return mode === "line" ? normalized.split("\n") : Array.from(normalized);
}

function getShellTextInput(unit: string, mode: SendCommandMode): string {
  if (mode === "line") {
    return buildTerminalCommandInput(unit);
  }
  return unit === "\n" ? "\r" : unit;
}

function getLineEndingValue(lineEnding: LineEnding): string {
  switch (lineEnding) {
    case "cr":
      return "\r";
    case "lf":
      return "\n";
    case "crlf":
      return "\r\n";
    default:
      return "";
  }
}

interface HexParseResult {
  bytes: number[];
  error: boolean;
}

function parseHexText(text: string): HexParseResult {
  const cleaned = text.replace(/\s+/gu, "");
  if (!cleaned) return { bytes: [], error: false };
  if (/[^0-9a-fA-F]/u.test(cleaned) || cleaned.length % 2 !== 0) {
    return { bytes: [], error: true };
  }

  const bytes: number[] = [];
  for (let i = 0; i < cleaned.length; i += 2) {
    bytes.push(Number.parseInt(cleaned.slice(i, i + 2), 16));
  }
  return { bytes, error: false };
}

function formatHexText(text: string): string {
  const normalized = normalizeTextNewlines(text).toUpperCase();
  if (HEX_INPUT_INVALID_PATTERN.test(normalized)) return normalized;

  return normalized
    .split("\n")
    .map((line) => {
      const cleaned = line.replace(/\s+/gu, "");
      let formatted = "";
      for (let i = 0; i < cleaned.length; i += 2) {
        const byte = cleaned.slice(i, i + 2);
        formatted += byte;
        if (byte.length === 2) {
          const byteIndex = i / 2 + 1;
          formatted += byteIndex % 4 === 0 ? "  " : " ";
        }
      }
      return formatted;
    })
    .join("\n");
}

function countHexCharsBefore(text: string, index: number): number {
  let count = 0;
  for (let i = 0; i < Math.min(index, text.length); i += 1) {
    if (/[0-9a-fA-F]/u.test(text[i])) count += 1;
  }
  return count;
}

function getHexCaretPosition(
  text: string,
  hexCharCount: number,
  skipFollowingWhitespace: boolean,
): number {
  if (hexCharCount <= 0 && skipFollowingWhitespace) {
    let index = 0;
    while (index < text.length && /\s/u.test(text[index])) index += 1;
    return index;
  }
  if (hexCharCount <= 0) return 0;

  let count = 0;
  for (let i = 0; i < text.length; i += 1) {
    if (!HEX_CHAR_PATTERN.test(text[i])) continue;
    count += 1;
    if (count === hexCharCount) {
      let nextIndex = i + 1;
      if (skipFollowingWhitespace) {
        while (nextIndex < text.length && /\s/u.test(text[nextIndex])) nextIndex += 1;
      }
      return nextIndex;
    }
  }

  return text.length;
}

function buildHexGuideRows(text: string): HexGuideRow[] {
  return normalizeTextNewlines(text)
    .split("\n")
    .map((line, lineIndex) => {
      const hexCharCount = line.replace(/[^0-9A-F]/gu, "").length;
      const guideCount = Math.floor(Math.floor(hexCharCount / 2) / 4);
      return {
        lineIndex,
        guidePositions: Array.from({ length: guideCount }, (_, index) => index + 1),
      };
    });
}

function bytesToRawString(bytes: number[]): string {
  return String.fromCharCode(...bytes);
}

function buildHexPreview(bytes: number[]): string {
  return bytes
    .map((byte) => (byte >= 0x20 && byte <= 0x7e ? String.fromCharCode(byte) : "."))
    .join("");
}

function isGroupTarget(value: TargetSelectValue): value is `group:${string}` {
  return value.startsWith("group:");
}

function isSessionTarget(value: TargetSelectValue): value is `session:${string}` {
  return value.startsWith("session:");
}

function getSessionTargetId(value: `session:${string}`): string {
  return value.slice("session:".length);
}

function TargetMenuItem({ checked, children, disabled, onSelect }: TargetMenuItemProps) {
  return (
    <DropdownMenuItem disabled={disabled} onSelect={onSelect} className="min-w-0">
      <span className="flex h-4 w-4 shrink-0 items-center justify-center">
        {checked ? <MdCheck className="text-sm" /> : null}
      </span>
      <span className="truncate">{children}</span>
    </DropdownMenuItem>
  );
}

export default function SendCommandPanel({
  serialSessionId,
  currentShellSessionId,
  shellSessionIds,
  syncGroups,
  currentWindowLabel,
  sessionTargets,
  clearAfterSend,
  draft,
  onDraftConsumed,
  onSendingChange,
  onClearAfterSendChange,
}: SendCommandPanelProps) {
  const { t } = useTranslation();
  const clearAfterSendId = useId();
  const [dataType, setDataType] = useState<SendCommandDataType>("text");
  const [commandText, setCommandText] = useState("");
  const [hexText, setHexText] = useState("");
  const [sendMode, setSendMode] = useState<SendCommandMode>("line");
  const [count, setCount] = useState<SendCommandCount>(1);
  const [intervalInput, setIntervalInput] = useState("1.00");
  const [target, setTarget] = useState<TargetSelectValue>("current");
  const [draftCurrentSessionId, setDraftCurrentSessionId] = useState<string | null>(null);
  const [draftTargetKind, setDraftTargetKind] = useState<TargetKind | null>(null);
  const [isSending, setIsSending] = useState(false);
  const [progress, setProgress] = useState<SendProgress | null>(null);
  const [lineEnding, setLineEnding] = useState<LineEnding>("crlf");
  const [hexScroll, setHexScroll] = useState({ left: 0, top: 0 });
  const textInputRef = useRef<HTMLTextAreaElement>(null);
  const hexInputRef = useRef<HTMLTextAreaElement>(null);
  const cancelRef = useRef(false);
  const sendingRef = useRef(false);
  const timerRef = useRef<number | null>(null);
  const timerResolveRef = useRef<(() => void) | null>(null);

  const activeTargetKind: TargetKind = serialSessionId ? "serial" : "shell";
  const targetKind =
    draftTargetKind === "serial" && draftCurrentSessionId === serialSessionId
      ? "serial"
      : draftTargetKind === "shell" &&
          draftCurrentSessionId !== null &&
          shellSessionIds.includes(draftCurrentSessionId)
        ? "shell"
        : activeTargetKind;

  const currentTargetSessionId =
    targetKind === "serial"
      ? serialSessionId
      : draftCurrentSessionId && shellSessionIds.includes(draftCurrentSessionId)
        ? draftCurrentSessionId
        : currentShellSessionId;

  const sessionTargetById = useMemo(
    () => new Map(sessionTargets.map((session) => [session.id, session])),
    [sessionTargets],
  );

  const isCompatibleTarget = useCallback(
    (session: SendCommandSessionTarget) =>
      targetKind === "serial" ? session.type === "Serial" : session.type !== "Serial",
    [targetKind],
  );

  const sessionTargetOptions = useMemo(
    () =>
      sessionTargets
        .filter(isCompatibleTarget)
        .map((session) => ({
          session,
          value: `session:${session.id}` as const,
        })),
    [isCompatibleTarget, sessionTargets],
  );

  const sessionTargetByValue = useMemo(
    () => new Map(sessionTargetOptions.map((option) => [option.value, option])),
    [sessionTargetOptions],
  );

  const allWindowSessionIds = useMemo(
    () => sessionTargetOptions.map((option) => option.session.id),
    [sessionTargetOptions],
  );

  const sessionTargetGroups = useMemo(() => {
    const groups = new Map<
      string,
      {
        key: string;
        ownerWindowLabel: string | null;
        label: string;
        options: typeof sessionTargetOptions;
      }
    >();

    for (const option of sessionTargetOptions) {
      const ownerWindowLabel = option.session.ownerWindowLabel ?? null;
      const isCurrentWindow = ownerWindowLabel === currentWindowLabel;
      const key = isCurrentWindow ? "__current__" : (ownerWindowLabel ?? "__unknown__");
      const label = isCurrentWindow
        ? t("serialSend.currentWindowSessions", "Current window sessions")
        : t("serialSend.windowGroup", "Window: {{label}}", {
            label: ownerWindowLabel ?? t("serialSend.unknownWindow", "Unknown window"),
          });

      const group = groups.get(key);
      if (group) {
        group.options.push(option);
      } else {
        groups.set(key, { key, ownerWindowLabel, label, options: [option] });
      }
    }

    return [...groups.values()].sort((a, b) => {
      if (a.ownerWindowLabel === currentWindowLabel) return -1;
      if (b.ownerWindowLabel === currentWindowLabel) return 1;
      return a.label.localeCompare(b.label);
    });
  }, [currentWindowLabel, sessionTargetOptions, t]);

  const groupTargetOptions = useMemo(() => {
    return syncGroups
      .filter((group) => group.enabled)
      .map((group) => {
        const pausedSessionIds = new Set(group.pausedSessionIds);
        const sessionIds = group.sessionIds.filter((sessionId, index) => {
          if (group.sessionIds.indexOf(sessionId) !== index) return false;
          if (pausedSessionIds.has(sessionId)) return false;
          const session = sessionTargetById.get(sessionId);
          return session ? isCompatibleTarget(session) : false;
        });

        return {
          group,
          value: `group:${group.id}` as const,
          sessionIds,
        };
      })
      .filter((option) => option.sessionIds.length > 0);
  }, [isCompatibleTarget, sessionTargetById, syncGroups]);

  const groupTargetByValue = useMemo(
    () => new Map(groupTargetOptions.map((option) => [option.value, option])),
    [groupTargetOptions],
  );

  const getSessionTargetLabel = useCallback(
    (session: SendCommandSessionTarget) => {
      const name = session.name.trim() || session.id;
      const tabName = session.tabName.trim();
      const tabSuffix = tabName && tabName !== name ? ` · ${tabName}` : "";

      return t("serialSend.singleSession", "{{name}} · {{type}}{{tabSuffix}}", {
        name,
        type: session.type,
        tabSuffix,
      });
    },
    [t],
  );

  const targetLabel = useMemo(() => {
    if (isGroupTarget(target)) {
      const option = groupTargetByValue.get(target);
      return option
        ? t("serialSend.groupSession", "Group: {{name}} ({{count}})", {
            name: option.group.name,
            count: option.sessionIds.length,
          })
        : t("serialSend.target", "Target");
    }

    if (isSessionTarget(target)) {
      const session = sessionTargetByValue.get(target)?.session;
      return session ? getSessionTargetLabel(session) : t("serialSend.targetSessions", "Sessions");
    }

    if (target === "allWindows") {
      return t("serialSend.allWindowSessions", "All window sessions");
    }

    return target === "all"
      ? t("serialSend.allSessions", "All sessions")
      : t("serialSend.currentSession", "Current session");
  }, [getSessionTargetLabel, groupTargetByValue, sessionTargetByValue, t, target]);

  const targetSessionIds = useMemo(() => {
    if (isGroupTarget(target)) {
      return groupTargetByValue.get(target)?.sessionIds ?? [];
    }

    if (isSessionTarget(target)) {
      return sessionTargetByValue.has(target) ? [getSessionTargetId(target)] : [];
    }

    if (target === "allWindows") {
      return allWindowSessionIds;
    }

    if (targetKind === "serial") {
      return serialSessionId ? [serialSessionId] : [];
    }

    return target === "current"
      ? currentTargetSessionId
        ? [currentTargetSessionId]
        : []
      : shellSessionIds;
  }, [
    allWindowSessionIds,
    currentTargetSessionId,
    groupTargetByValue,
    serialSessionId,
    sessionTargetByValue,
    shellSessionIds,
    target,
    targetKind,
  ]);

  const parsedHex = useMemo(() => parseHexText(hexText), [hexText]);
  const parsedHexBytes = parsedHex.error ? null : parsedHex.bytes;
  const hasInvalidHex = parsedHex.error;
  const hasPayload =
    dataType === "hex"
      ? parsedHexBytes !== null && parsedHexBytes.length > 0
      : commandText.length > 0;
  const cancelSend = useCallback(() => {
    cancelRef.current = true;
    if (timerRef.current !== null) {
      window.clearTimeout(timerRef.current);
      timerRef.current = null;
    }
    timerResolveRef.current?.();
    timerResolveRef.current = null;
  }, []);

  useEffect(() => {
    return () => cancelSend();
  }, [cancelSend]);

  useEffect(() => {
    if (isGroupTarget(target) && !groupTargetByValue.has(target)) {
      setTarget("current");
      return;
    }

    if (isSessionTarget(target) && !sessionTargetByValue.has(target)) {
      setTarget("current");
      return;
    }

    if (target === "allWindows" && allWindowSessionIds.length === 0) {
      setTarget("current");
      return;
    }

    if (targetKind === "serial" && target === "all") {
      setTarget("current");
    }
  }, [
    allWindowSessionIds.length,
    groupTargetByValue,
    sessionTargetByValue,
    target,
    targetKind,
  ]);

  useEffect(() => {
    if (
      draftCurrentSessionId &&
      targetKind === "shell" &&
      !shellSessionIds.includes(draftCurrentSessionId)
    ) {
      setDraftCurrentSessionId(null);
      setDraftTargetKind(null);
    }
  }, [draftCurrentSessionId, shellSessionIds, targetKind]);

  useEffect(() => {
    if (!draft) return;

    const nextDataType = draft.dataType ?? "text";
    setDataType(nextDataType);
    setCommandText(draft.text);
    if (nextDataType === "hex") {
      setHexText(formatHexText(draft.text));
      setSendMode(draft.sendMode === "packet" ? "packet" : "byte");
    } else {
      setSendMode(draft.sendMode === "character" ? "character" : "line");
    }
    setCount(draft.count);
    setIntervalInput(formatIntervalSeconds(draft.intervalSeconds));
    setTarget(draft.target);
    setDraftCurrentSessionId(draft.sourceSessionId);
    setDraftTargetKind(draft.sourceSessionType === "Serial" ? "serial" : "shell");
    onDraftConsumed?.();
    requestAnimationFrame(() => {
      if (nextDataType === "hex") {
        hexInputRef.current?.focus();
      } else {
        textInputRef.current?.focus();
      }
    });
  }, [draft, onDraftConsumed]);

  const waitInterval = useCallback(async (seconds: number) => {
    if (seconds <= 0 || cancelRef.current) return;

    await new Promise<void>((resolve) => {
      timerResolveRef.current = resolve;
      timerRef.current = window.setTimeout(() => {
        timerRef.current = null;
        timerResolveRef.current = null;
        resolve();
      }, seconds * 1000);
    });
  }, []);

  const buildSendUnits = useCallback((): SendUnit[] => {
    if (dataType === "hex") {
      if (!parsedHexBytes || parsedHexBytes.length === 0) return [];
      if (sendMode === "packet") {
        return [{ data: bytesToRawString(parsedHexBytes) }];
      }
      return parsedHexBytes.map((byte) => ({ data: String.fromCharCode(byte) }));
    }

    if (targetKind === "serial") {
      const lineEndingValue = getLineEndingValue(lineEnding);
      return buildTextSendUnits(commandText, sendMode).map((unit) => ({
        data: sendMode === "line" ? `${unit}${lineEndingValue}` : unit,
      }));
    }

    return buildTextSendUnits(commandText, sendMode).map((unit) => ({
      data: getShellTextInput(unit, sendMode),
      registerSubmission: sendMode === "line" && unit.trim() ? unit : null,
      resetPreview: sendMode === "line",
    }));
  }, [commandText, dataType, lineEnding, parsedHexBytes, sendMode, targetKind]);

  const getDefaultInterval = useCallback(() => {
    if (dataType === "hex") return sendMode === "byte" ? BYTE_INTERVAL_SECONDS : 0;
    return sendMode === "line" ? LINE_INTERVAL_SECONDS : CHARACTER_INTERVAL_SECONDS;
  }, [dataType, sendMode]);

  const sendCommand = useCallback(async () => {
    if (sendingRef.current) return;

    const units = buildSendUnits();
    const intervalSeconds = Number.parseFloat(intervalInput);
    const effectiveInterval = Number.isFinite(intervalSeconds)
      ? Math.max(0, intervalSeconds)
      : getDefaultInterval();
    const targets = [...targetSessionIds];
    if (units.length === 0 || targets.length === 0) return;

    cancelRef.current = false;
    sendingRef.current = true;
    setIsSending(true);
    onSendingChange?.(true);
    setProgress(
      count === null || count > 1 || units.length > 1
        ? {
            completedUnits: 0,
            totalUnits: count === null ? null : units.length * count,
            unitsPerRound: units.length,
            totalRounds: count,
          }
        : null,
    );

    let failedCount = 0;
    let sendCount = 0;
    let completedUnits = 0;
    let firstUnit = true;
    let cancelled = false;
    let completedSuccessfully = false;

    try {
      let round = 0;
      while ((count === null || round < count) && !cancelRef.current) {
        for (const unit of units) {
          if (cancelRef.current) break;
          if (!firstUnit) {
            await waitInterval(effectiveInterval);
          }
          if (cancelRef.current) break;

          const results = await Promise.allSettled(
            targets.map((sessionId) =>
              targetKind === "shell" && dataType === "text"
                ? sendSessionInput(sessionId, unit.data, {
                    preview: unit.resetPreview ? { kind: "reset" } : undefined,
                    registerSubmission: unit.registerSubmission,
                  })
                : invoke("write_to_session", { sessionId, data: unit.data }),
            ),
          );

          failedCount += results.filter((result) => result.status === "rejected").length;
          sendCount += results.length;
          completedUnits += 1;
          setProgress((current) => (current ? { ...current, completedUnits } : current));
          firstUnit = false;
        }
        round += 1;
      }

      cancelled = cancelRef.current;
      if (!cancelled) {
        if (sendCount > 0 && failedCount === sendCount) {
          toast.error(t("serialSend.sendFailed", "Send failed"));
          return;
        }
        if (failedCount > 0) {
          toast.error(t("serialSend.sendPartial", "Some sessions did not receive the data"));
          return;
        }
        completedSuccessfully = sendCount > 0;
      }
    } finally {
      if (timerRef.current !== null) {
        window.clearTimeout(timerRef.current);
        timerRef.current = null;
      }
      timerResolveRef.current = null;
      cancelRef.current = false;
      sendingRef.current = false;
      setIsSending(false);
      onSendingChange?.(false);
      setProgress(null);
      if (completedSuccessfully && clearAfterSend) {
        if (dataType === "hex") {
          setHexText("");
        } else {
          setCommandText("");
        }
      }
      requestAnimationFrame(() => {
        if (dataType === "hex") {
          hexInputRef.current?.focus();
        } else {
          textInputRef.current?.focus();
        }
      });
    }
  }, [
    buildSendUnits,
    clearAfterSend,
    count,
    dataType,
    getDefaultInterval,
    intervalInput,
    onSendingChange,
    t,
    targetKind,
    targetSessionIds,
    waitInterval,
  ]);

  const handleDataTypeChange = useCallback((value: SendCommandDataType) => {
    setDataType(value);
    if (value === "hex") {
      setSendMode("byte");
      setIntervalInput(formatIntervalSeconds(BYTE_INTERVAL_SECONDS));
    } else {
      setSendMode("line");
      setIntervalInput(formatIntervalSeconds(LINE_INTERVAL_SECONDS));
    }
  }, []);

  const handleSendModeChange = useCallback(
    (value: SendCommandMode) => {
      setSendMode(value);
      if (dataType === "hex") {
        setIntervalInput(value === "byte" ? "0.02" : "0");
      } else {
        setIntervalInput(value === "line" ? "1.00" : "0.02");
      }
    },
    [dataType],
  );

  const handleHexTextChange = useCallback((event: ChangeEvent<HTMLTextAreaElement>) => {
    const rawValue = event.target.value;
    const rawCaret = event.target.selectionStart ?? rawValue.length;
    const formattedValue = formatHexText(rawValue);
    const normalizedRawValue = normalizeTextNewlines(rawValue).toUpperCase();
    const hasInvalidInput = HEX_INPUT_INVALID_PATTERN.test(normalizedRawValue);
    const hexCharsBeforeCaret = countHexCharsBefore(rawValue, rawCaret);
    const nextCaret = hasInvalidInput
      ? rawCaret
      : getHexCaretPosition(
          formattedValue,
          hexCharsBeforeCaret,
          (hexCharsBeforeCaret > 0 && hexCharsBeforeCaret % 2 === 0) ||
            (rawCaret > 0 && /\s/u.test(rawValue[rawCaret - 1])),
        );

    setHexText(formattedValue);
    requestAnimationFrame(() => {
      hexInputRef.current?.setSelectionRange(nextCaret, nextCaret);
    });
  }, []);

  const handleHexScroll = useCallback((event: UIEvent<HTMLTextAreaElement>) => {
    setHexScroll({
      left: event.currentTarget.scrollLeft,
      top: event.currentTarget.scrollTop,
    });
  }, []);

  const handleHexKeyDown = useCallback(
    (event: KeyboardEvent<HTMLTextAreaElement>) => {
      if ((event.ctrlKey || event.metaKey) && event.key === "Enter") {
        event.preventDefault();
        if (isSending) {
          cancelSend();
        } else {
          void sendCommand();
        }
        return;
      }

      if (event.key !== "Backspace" || event.altKey || event.ctrlKey || event.metaKey) return;

      const input = event.currentTarget;
      const selectionStart = input.selectionStart ?? 0;
      const selectionEnd = input.selectionEnd ?? selectionStart;
      if (selectionStart !== selectionEnd || selectionStart <= 0) return;
      if (!/\s/u.test(hexText[selectionStart - 1])) return;

      let byteEnd = selectionStart - 1;
      while (byteEnd >= 0 && /\s/u.test(hexText[byteEnd])) byteEnd -= 1;
      if (byteEnd < 0 || !HEX_CHAR_PATTERN.test(hexText[byteEnd])) return;

      let byteStart = byteEnd - 1;
      while (byteStart >= 0 && /\s/u.test(hexText[byteStart])) byteStart -= 1;
      if (byteStart < 0 || !HEX_CHAR_PATTERN.test(hexText[byteStart])) return;

      event.preventDefault();
      const nextRawValue = `${hexText.slice(0, byteStart)}${hexText.slice(selectionStart)}`;
      const nextValue = formatHexText(nextRawValue);
      const nextCaret = getHexCaretPosition(
        nextValue,
        countHexCharsBefore(hexText, byteStart),
        true,
      );

      setHexText(nextValue);
      requestAnimationFrame(() => {
        hexInputRef.current?.setSelectionRange(nextCaret, nextCaret);
      });
    },
    [cancelSend, hexText, isSending, sendCommand],
  );

  const handleCountInputChange = useCallback((value: string) => {
    const trimmed = value.trim();
    if (trimmed === "∞" || trimmed.toLowerCase() === "inf") {
      setCount(null);
      return;
    }

    const parsed = Number.parseInt(trimmed, 10);
    if (Number.isFinite(parsed)) {
      setCount(Math.max(1, parsed));
    }
  }, []);

  const decrementCount = useCallback(() => {
    setCount((current) => {
      if (current === null) return null;
      if (current <= 1) return null;
      return current - 1;
    });
  }, []);

  const incrementCount = useCallback(() => {
    setCount((current) => {
      if (current === null) return 1;
      return current + 1;
    });
  }, []);

  const progressPercent =
    progress?.totalUnits && progress.totalUnits > 0
      ? Math.min(100, Math.round((progress.completedUnits / progress.totalUnits) * 100))
      : null;
  const completedRounds = progress
    ? Math.floor(progress.completedUnits / progress.unitsPerRound)
    : 0;
  const currentRound = progress
    ? progress.totalRounds === null
      ? completedRounds + 1
      : Math.min(progress.totalRounds, completedRounds + 1)
    : 0;

  const previewBytes = parsedHexBytes ?? [];
  const hexPreview = buildHexPreview(previewBytes);
  const hexGuideRows = useMemo(() => buildHexGuideRows(hexText), [hexText]);
  const hasHexGuides = hexGuideRows.some((row) => row.guidePositions.length > 0);

  return (
    <div className="h-full flex flex-col overflow-hidden px-2 py-1.5 gap-2">
      <div className="flex shrink-0 flex-wrap items-center gap-1.5">
        <div className="flex h-8 min-w-[8.5rem] flex-[1_1_10rem] items-center overflow-hidden rounded-md border border-border/70 bg-background/60">
          <Label className="shrink-0 px-2 text-[0.625rem] text-muted-foreground">
            {t("serialSend.dataType", "Data Type")}
          </Label>
          <Select
            value={dataType}
            onValueChange={(value) => handleDataTypeChange(value as SendCommandDataType)}
            disabled={isSending}
          >
            <SelectTrigger className="h-8 min-w-0 flex-1 border-0 bg-transparent px-2 text-[0.6875rem] shadow-none focus-visible:ring-0">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="text" className="text-xs">
                {t("serialSend.text", "Text")}
              </SelectItem>
              <SelectItem value="hex" className="text-xs">
                {t("serialSend.hex", "Hex")}
              </SelectItem>
            </SelectContent>
          </Select>
        </div>

        <div className="flex h-8 min-w-[10rem] flex-[1.2_1_12rem] items-center overflow-hidden rounded-md border border-border/70 bg-background/60">
          <Label className="shrink-0 px-2 text-[0.625rem] text-muted-foreground">
            {t("serialSend.sendMode", "Send Mode")}
          </Label>
          <Select
            value={sendMode}
            onValueChange={(value) => handleSendModeChange(value as SendCommandMode)}
            disabled={isSending}
          >
            <SelectTrigger className="h-8 min-w-0 flex-1 border-0 bg-transparent px-2 text-[0.6875rem] shadow-none focus-visible:ring-0">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {dataType === "hex" ? (
                <>
                  <SelectItem value="byte" className="text-xs">
                    {t("serialSend.byteByByte", "Byte by byte")}
                  </SelectItem>
                  <SelectItem value="packet" className="text-xs">
                    {t("serialSend.packet", "Packet")}
                  </SelectItem>
                </>
              ) : (
                <>
                  <SelectItem value="line" className="text-xs">
                    {t("serialSend.lineByLine", "Line by line")}
                  </SelectItem>
                  <SelectItem value="character" className="text-xs">
                    {t("serialSend.characterByCharacter", "Character by character")}
                  </SelectItem>
                </>
              )}
            </SelectContent>
          </Select>
        </div>

        <div className="flex h-8 min-w-[10rem] flex-[1.2_1_12rem] items-center overflow-hidden rounded-md border border-border/70 bg-background/60">
          <Label className="shrink-0 px-2 text-[0.625rem] text-muted-foreground">
            {t("serialSend.target", "Target")}
          </Label>
          <DropdownMenu>
            <DropdownMenuTrigger asChild disabled={isSending}>
              <Button
                type="button"
                variant="ghost"
                className="h-8 min-w-0 flex-1 justify-between rounded-none border-0 bg-transparent px-2 text-[0.6875rem] font-normal shadow-none hover:bg-transparent focus:bg-transparent active:bg-transparent dark:bg-input/30 dark:hover:bg-input/50 data-[state=open]:dark:bg-input/30 focus-visible:ring-0"
                disabled={isSending}
                aria-label={t("serialSend.target", "Target")}
              >
                <span className="truncate">{targetLabel}</span>
                <MdKeyboardArrowDown className="ml-1 shrink-0 text-sm text-muted-foreground" />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="start" className="min-w-[13rem] max-w-[22rem]">
              <TargetMenuItem
                checked={target === "current"}
                disabled={!currentTargetSessionId}
                onSelect={() => setTarget("current")}
              >
                {t("serialSend.currentSession", "Current session")}
              </TargetMenuItem>
              {targetKind === "shell" && (
                <TargetMenuItem checked={target === "all"} onSelect={() => setTarget("all")}>
                  {t("serialSend.allSessions", "All sessions")}
                </TargetMenuItem>
              )}
              <TargetMenuItem
                checked={target === "allWindows"}
                disabled={allWindowSessionIds.length === 0}
                onSelect={() => setTarget("allWindows")}
              >
                {t("serialSend.allWindowSessions", "All window sessions")}
              </TargetMenuItem>
              <DropdownMenuSub>
                <DropdownMenuSubTrigger
                  disabled={sessionTargetOptions.length === 0}
                  className="min-w-0"
                >
                  <span className="truncate">{t("serialSend.targetSessions", "Sessions")}</span>
                </DropdownMenuSubTrigger>
                <DropdownMenuSubContent className="max-h-[70vh] min-w-[14rem] max-w-[22rem] overflow-y-auto">
                  {sessionTargetGroups.map((group, groupIndex) => (
                    <Fragment key={group.key}>
                      {groupIndex > 0 && <DropdownMenuSeparator />}
                      <DropdownMenuLabel className="max-w-[20rem] truncate text-[0.625rem] text-muted-foreground">
                        {group.label}
                      </DropdownMenuLabel>
                      {group.options.map((option) => (
                        <TargetMenuItem
                          key={option.session.id}
                          checked={target === option.value}
                          onSelect={() => setTarget(option.value)}
                        >
                          {getSessionTargetLabel(option.session)}
                        </TargetMenuItem>
                      ))}
                    </Fragment>
                  ))}
                </DropdownMenuSubContent>
              </DropdownMenuSub>
              {groupTargetOptions.map((option) => (
                <TargetMenuItem
                  key={option.group.id}
                  checked={target === option.value}
                  onSelect={() => setTarget(option.value)}
                >
                  {t("serialSend.groupSession", "Group: {{name}} ({{count}})", {
                    name: option.group.name,
                    count: option.sessionIds.length,
                  })}
                </TargetMenuItem>
              ))}
            </DropdownMenuContent>
          </DropdownMenu>
        </div>

        <div className="flex h-8 min-w-[8.5rem] flex-[1_1_9.5rem] items-center overflow-hidden rounded-md border border-border/70 bg-background/60">
          <Label className="shrink-0 px-2 text-[0.625rem] text-muted-foreground">
            {t("serialSend.count", "Count")}
          </Label>
          <div className="flex min-w-0 flex-1 items-center border-l border-border/60">
            <Button
              type="button"
              variant="ghost"
              size="icon-xs"
              className="h-8 w-8 rounded-none text-muted-foreground"
              onClick={decrementCount}
              disabled={isSending}
            >
              <MdRemove className="text-sm" />
            </Button>
            <Input
              className="h-8 min-w-10 rounded-none border-0 bg-transparent px-1 text-center text-[0.75rem] font-medium shadow-none focus-visible:ring-0"
              value={count === null ? "∞" : String(count)}
              inputMode="numeric"
              disabled={isSending}
              aria-label={t("serialSend.count", "Count")}
              onChange={(e) => handleCountInputChange(e.target.value)}
              onBlur={() => {
                if (count !== null && count < 1) setCount(1);
              }}
            />
            <Button
              type="button"
              variant="ghost"
              size="icon-xs"
              className="h-8 w-8 rounded-none text-muted-foreground"
              onClick={incrementCount}
              disabled={isSending}
            >
              <MdAdd className="text-sm" />
            </Button>
          </div>
        </div>

        <div className="flex h-8 min-w-[9rem] flex-[1_1_10rem] items-center overflow-hidden rounded-md border border-border/70 bg-background/60">
          <Label className="shrink-0 px-2 text-[0.625rem] text-muted-foreground">
            {t("serialSend.interval", "Interval")}
          </Label>
          <div className="flex min-w-0 flex-1 items-center border-l border-border/60">
            <Input
              className="h-8 min-w-14 rounded-none border-0 bg-transparent px-2 text-right text-[0.75rem] font-medium shadow-none focus-visible:ring-0"
              value={intervalInput}
              inputMode="decimal"
              disabled={isSending}
              aria-label={t("serialSend.interval", "Interval")}
              onChange={(e) => setIntervalInput(e.target.value)}
              onBlur={() => {
                const parsed = Number.parseFloat(intervalInput);
                if (!Number.isFinite(parsed) || parsed < 0) {
                  setIntervalInput(formatIntervalSeconds(getDefaultInterval()));
                }
              }}
            />
            <span className="shrink-0 pr-2 text-[0.625rem] text-muted-foreground">
              {t("serialSend.seconds", "s")}
            </span>
          </div>
        </div>
        {targetKind === "serial" && dataType === "text" && sendMode === "line" && (
          <div className="flex h-8 min-w-[7.5rem] flex-[0.8_1_8.5rem] items-center overflow-hidden rounded-md border border-border/70 bg-background/60">
            <Label className="shrink-0 px-2 text-[0.625rem] text-muted-foreground">
              {t("serialSend.lineEnding", "Line Ending")}
            </Label>
            <Select
              value={lineEnding}
              onValueChange={(value) => setLineEnding(value as LineEnding)}
              disabled={isSending}
            >
              <SelectTrigger className="h-8 min-w-0 flex-1 border-0 bg-transparent px-2 text-[0.6875rem] shadow-none focus-visible:ring-0">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="none" className="text-xs">
                  {t("serialSend.noLineEnding", "None")}
                </SelectItem>
                <SelectItem value="cr" className="text-xs">
                  CR
                </SelectItem>
                <SelectItem value="lf" className="text-xs">
                  LF
                </SelectItem>
                <SelectItem value="crlf" className="text-xs">
                  CR+LF
                </SelectItem>
              </SelectContent>
            </Select>
          </div>
        )}
      </div>

      <div className="relative min-h-0 flex-1">
        {dataType === "text" ? (
          <Textarea
            ref={textInputRef}
            className="min-h-0 h-full resize-none pr-36 pb-10 text-xs leading-5 md:text-xs"
            placeholder={t(
              "serialSend.shellPlaceholder",
              "Enter text to send...\nCtrl/Cmd + Enter to send",
            )}
            value={commandText}
            disabled={isSending}
            onChange={(e) => setCommandText(e.target.value)}
            onKeyDown={(e) => {
              if ((e.ctrlKey || e.metaKey) && e.key === "Enter") {
                e.preventDefault();
                if (isSending) {
                  cancelSend();
                } else {
                  void sendCommand();
                }
              }
            }}
          />
        ) : (
          <div className="grid h-full min-h-0 grid-cols-[minmax(0,1fr)_minmax(0,0.85fr)] gap-1.5 pr-36 pb-10">
            <div className="flex min-h-0 flex-col overflow-hidden rounded-md border border-border bg-background">
              <div className="flex h-8 shrink-0 items-center border-b border-border/70 px-2">
                <span className="text-[0.625rem] font-medium text-muted-foreground">
                  {t("serialSend.hexEditor", "HEX Editor")}
                </span>
                {hasInvalidHex && (
                  <span className="ml-auto truncate text-[0.625rem] text-destructive">
                    {t(
                      "serialSend.hexError",
                      "Invalid hex input. Use hex characters (0-9, A-F) separated by spaces.",
                    )}
                  </span>
                )}
              </div>
              <div className="relative min-h-0 flex-1 overflow-hidden">
                {hasHexGuides && (
                  <div
                    className="pointer-events-none absolute inset-0 overflow-hidden px-3 py-2 font-mono text-xs leading-5 md:text-xs"
                    aria-hidden="true"
                  >
                    <div
                      className="min-w-max"
                      style={{
                        transform: `translate(${-hexScroll.left}px, ${-hexScroll.top}px)`,
                      }}
                    >
                      {hexGuideRows.map((row) => (
                        <div key={row.lineIndex} className="relative h-5 min-w-max">
                          {row.guidePositions.map((groupNumber) => (
                            <span
                              key={groupNumber}
                              className="absolute -top-0.5 h-6 border-l-2 border-dashed border-primary/80"
                              style={{ left: `calc(${groupNumber * 13 - 1}ch)` }}
                            />
                          ))}
                        </div>
                      ))}
                    </div>
                  </div>
                )}
                <Textarea
                  ref={hexInputRef}
                  className={`relative z-10 min-h-0 h-full resize-none overflow-auto rounded-none border-0 bg-transparent font-mono text-xs leading-5 shadow-none focus-visible:ring-0 md:text-xs ${
                    hasInvalidHex ? "text-destructive" : ""
                  }`}
                  placeholder={t("serialSend.hexPlaceholder", "e.g. 48 65 6C 6C 6F")}
                  value={hexText}
                  wrap="off"
                  disabled={isSending}
                  onChange={handleHexTextChange}
                  onKeyDown={handleHexKeyDown}
                  onScroll={handleHexScroll}
                />
              </div>
            </div>
            <div className="flex min-h-0 flex-col overflow-hidden rounded-md border border-border/70 bg-muted/30">
              <div className="flex h-8 shrink-0 items-center border-b border-border/70 px-2">
                <span className="text-[0.625rem] font-medium text-muted-foreground">
                  {t("serialSend.hexPreview", "Preview")}
                </span>
                <span className="ml-auto text-[0.625rem] tabular-nums text-muted-foreground">
                  {t("serialSend.hexByteCount", "{{count}} bytes", {
                    count: previewBytes.length,
                  })}
                </span>
              </div>
              <div className="min-h-0 flex-1 overflow-auto p-2">
                <pre className="whitespace-pre-wrap break-all font-mono text-xs leading-5">
                  {hasInvalidHex ? "" : hexPreview}
                </pre>
              </div>
            </div>
          </div>
        )}

        <div className="absolute top-2 right-2 z-20 flex h-6 items-center gap-1.5 rounded-md border border-border/70 bg-background/95 px-2 shadow-sm backdrop-blur">
          <Label
            htmlFor={clearAfterSendId}
            className="cursor-pointer whitespace-nowrap text-[0.625rem] text-muted-foreground"
          >
            {t("serialSend.clearAfterSend", "Clear after send")}
          </Label>
          <Switch
            id={clearAfterSendId}
            size="sm"
            checked={clearAfterSend}
            disabled={isSending}
            aria-label={t("serialSend.clearAfterSend", "Clear after send")}
            onCheckedChange={onClearAfterSendChange}
          />
        </div>

        {progress && (
          <div className="pointer-events-none absolute inset-x-2 top-10 z-10">
            <div className="rounded-md border border-primary/25 bg-background/95 px-2.5 py-2 shadow-sm backdrop-blur">
              <div className="mb-1.5 flex min-w-0 items-center gap-2">
                <span className="truncate text-[0.6875rem] font-medium text-foreground">
                  {progress.totalRounds === null
                    ? t("serialSend.shellProgressInfinite", "Sending round {{current}}", {
                        current: currentRound,
                      })
                    : t("serialSend.shellProgressRound", "Sending {{current}} / {{total}}", {
                        current: currentRound,
                        total: progress.totalRounds,
                      })}
                </span>
                <span className="ml-auto shrink-0 text-[0.625rem] tabular-nums text-muted-foreground">
                  {progress.totalUnits === null
                    ? t("serialSend.shellProgressCompleted", "{{count}} sent", {
                        count: progress.completedUnits,
                      })
                    : t("serialSend.shellProgressUnits", "{{completed}} / {{total}} units", {
                        completed: progress.completedUnits,
                        total: progress.totalUnits,
                      })}
                </span>
              </div>
              {progressPercent !== null ? (
                <Progress value={progressPercent} className="h-1.5" />
              ) : (
                <div className="h-1.5 overflow-hidden rounded-full bg-primary/20">
                  <div className="h-full w-1/3 rounded-full bg-primary/70" />
                </div>
              )}
            </div>
          </div>
        )}

        <Button
          size="icon-xs"
          variant={isSending ? "destructive" : "default"}
          className="absolute bottom-2 right-2 h-7 w-7 shadow-sm"
          title={isSending ? t("serialSend.stop", "Stop") : t("serialSend.send", "Send")}
          onClick={() => {
            if (isSending) {
              cancelSend();
            } else {
              void sendCommand();
            }
          }}
          disabled={!isSending && (!hasPayload || targetSessionIds.length === 0)}
        >
          {isSending ? <MdStop className="text-sm" /> : <MdSend className="text-sm" />}
        </Button>
      </div>
    </div>
  );
}
