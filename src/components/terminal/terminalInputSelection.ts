import type { IBufferCell, IBufferLine, Terminal } from "@xterm/xterm";
import type { TerminalInputState } from "@/lib/terminalInputTracker";

/** Read the current cursor line from the terminal buffer up to the cursor. */
export function readCurrentInputLine(terminal: Terminal): string {
  const buffer = terminal.buffer.active;
  const y = buffer.baseY + buffer.cursorY;
  const line = buffer.getLine(y);
  if (!line) return "";
  const fullLine = line.translateToString(true, 0, buffer.cursorX);
  return fullLine;
}

export function readRecentOutput(terminal: Terminal, lineLimit: number) {
  const buffer = terminal.buffer.active;
  const total = buffer.length;
  const start = Math.max(0, total - lineLimit);
  const lines: string[] = [];
  for (let y = start; y < total; y += 1) {
    const line = buffer.getLine(y);
    if (line) lines.push(line.translateToString(true));
  }
  return lines.join("\n").replace(/\s+$/u, "");
}

export function hasErrorKeyword(output: string) {
  return /\b(error|failed|permission denied|no space left on device|connection refused|segmentation fault|out of memory|cannot allocate memory|command not found|module not found|port already in use)\b/i.test(
    output,
  );
}

export function isMultiLineText(text: string): boolean {
  return /[\r\n]/u.test(text);
}

export function isShiftInsertPasteEvent(e: KeyboardEvent): boolean {
  return (
    e.shiftKey &&
    !e.ctrlKey &&
    !e.metaKey &&
    !e.altKey &&
    (e.code === "Insert" || e.key === "Insert")
  );
}

interface LogicalInputLineSnapshot {
  startY: number;
  endY: number;
  text: string;
  stringIndexToCellOffset: number[];
}

interface InputCellSpan {
  startStringIndex: number;
  startCellOffset: number;
  endCellOffset: number;
}

interface InputSpan {
  startCellOffset: number;
  endCellOffset: number;
  startY: number;
  endY: number;
  indexToCellOffset: number[];
}

interface CachedInputSpan {
  key: string;
  span: InputSpan | null;
}

export interface InputSelectionRange {
  start: number;
  end: number;
}

interface InputClickPosition {
  x: number;
  y: number;
}

interface XTermCoreWithRenderDimensions {
  _core?: {
    _renderService?: {
      dimensions?: {
        css?: {
          cell?: {
            height: number;
            width: number;
          };
        };
      };
    };
  };
}

const MAX_GEOMETRY_INPUT_CHARS = 64 * 1024;
const MAX_GEOMETRY_INPUT_LINES = 512;
const inputSpanCache = new WeakMap<Terminal, CachedInputSpan>();

function buildLineStringToCellMap(
  line: IBufferLine,
  stringLength: number,
  maxCols: number,
  scratchCell: IBufferCell,
): number[] {
  const map: number[] = [];
  let col = 0;
  let cellEndCol = 0;

  while (col < maxCols && map.length < stringLength) {
    const cell = line.getCell(col, scratchCell);
    if (!cell) break;

    const chars = cell.getChars();
    const width = cell.getWidth();
    const stride = width || 1;

    if (chars.length === 0) {
      map.push(col);
    } else {
      for (let i = 0; i < chars.length; i += 1) {
        map.push(col);
      }
    }

    cellEndCol = col + stride;
    col += stride;
  }

  map.push(cellEndCol);
  return map;
}

function readLogicalLineSnapshotAt(
  terminal: Terminal,
  anchorY: number,
): LogicalInputLineSnapshot | null {
  const buffer = terminal.buffer.active;
  let startY = anchorY;
  while (startY > 0 && buffer.getLine(startY)?.isWrapped) {
    startY -= 1;
  }

  let endY = anchorY;
  while (endY + 1 < buffer.length && buffer.getLine(endY + 1)?.isWrapped) {
    endY += 1;
  }

  const scratchCell = buffer.getNullCell();
  const parts: string[] = [];
  const stringIndexToCellOffset: number[] = [];
  let lastCellOffset = 0;

  for (let y = startY; y <= endY; y += 1) {
    const line = buffer.getLine(y);
    if (!line) return null;

    const rowOffset = (y - startY) * terminal.cols;
    const maxCols = Math.min(line.length, terminal.cols);
    const text = line.translateToString(false, 0, maxCols);
    const lineMap = buildLineStringToCellMap(line, text.length, maxCols, scratchCell);

    for (let i = 0; i < text.length; i += 1) {
      stringIndexToCellOffset.push(rowOffset + (lineMap[i] ?? i));
    }

    lastCellOffset = rowOffset + (lineMap[text.length] ?? text.length);
    parts.push(text);
  }

  stringIndexToCellOffset.push(lastCellOffset);
  return { startY, endY, text: parts.join(""), stringIndexToCellOffset };
}

function readLogicalLineSnapshot(terminal: Terminal): LogicalInputLineSnapshot | null {
  const buffer = terminal.buffer.active;
  return readLogicalLineSnapshotAt(terminal, buffer.baseY + buffer.cursorY);
}

function findTrackedInputCellSpans(
  snapshot: LogicalInputLineSnapshot,
  state: TerminalInputState,
): InputCellSpan[] {
  if (!state.value) return [];

  const { text, stringIndexToCellOffset } = snapshot;
  let searchFrom = 0;
  let matchIndex = text.indexOf(state.value, searchFrom);
  const spans: InputCellSpan[] = [];

  while (matchIndex >= 0) {
    const endIndex = matchIndex + state.value.length;
    const startCellOffset = stringIndexToCellOffset[matchIndex];
    const endCellOffset = stringIndexToCellOffset[endIndex];

    if (startCellOffset !== undefined && endCellOffset !== undefined) {
      spans.push({
        startStringIndex: matchIndex,
        startCellOffset,
        endCellOffset,
      });
    }

    searchFrom = matchIndex + 1;
    matchIndex = text.indexOf(state.value, searchFrom);
  }

  return spans;
}

function toGlobalCellOffset(snapshot: LogicalInputLineSnapshot, cellOffset: number, cols: number) {
  return snapshot.startY * cols + cellOffset;
}

function inputCellSpanToInputSpan(
  snapshot: LogicalInputLineSnapshot,
  span: InputCellSpan,
  valueLength: number,
  cols: number,
): InputSpan | null {
  const indexToCellOffset: number[] = [];

  for (let i = 0; i <= valueLength; i += 1) {
    const cellOffset = snapshot.stringIndexToCellOffset[span.startStringIndex + i];
    if (cellOffset === undefined) return null;
    indexToCellOffset.push(toGlobalCellOffset(snapshot, cellOffset, cols));
  }

  return {
    startCellOffset: toGlobalCellOffset(snapshot, span.startCellOffset, cols),
    endCellOffset: toGlobalCellOffset(snapshot, span.endCellOffset, cols),
    startY: snapshot.startY,
    endY: snapshot.endY,
    indexToCellOffset,
  };
}

function findSingleLineInputSpans(
  snapshot: LogicalInputLineSnapshot,
  state: TerminalInputState,
  cols: number,
): InputSpan[] {
  return findTrackedInputCellSpans(snapshot, state)
    .map((span) => inputCellSpanToInputSpan(snapshot, span, state.value.length, cols))
    .filter((span): span is InputSpan => span !== null);
}

function findLineSegmentIndex(lineText: string, segment: string): number {
  if (!segment) {
    return lineText.length;
  }

  return lineText.lastIndexOf(segment);
}

function cellOffsetToInputIndex(indexToCellOffset: number[], cellOffset: number): number {
  for (let i = 0; i < indexToCellOffset.length; i += 1) {
    if ((indexToCellOffset[i] ?? 0) >= cellOffset) {
      return i;
    }
  }
  return indexToCellOffset.length - 1;
}

function getMultilineSegmentOffsets(value: string): number[] {
  const offsets = [0];
  for (let i = 0; i < value.length; i += 1) {
    if (value[i] === "\n") {
      offsets.push(i + 1);
    }
  }
  return offsets;
}

function getSegmentIndexAtCursor(offsets: number[], cursor: number): number {
  let index = 0;
  for (let i = 0; i < offsets.length; i += 1) {
    if ((offsets[i] ?? 0) <= cursor) {
      index = i;
    } else {
      break;
    }
  }
  return index;
}

function canResolveEnhancedGeometry(state: TerminalInputState): boolean {
  if (!state.value) return false;
  if (state.value.length > MAX_GEOMETRY_INPUT_CHARS) return false;
  if (state.multiline && state.value.split("\n").length > MAX_GEOMETRY_INPUT_LINES) return false;
  return true;
}

function getPreviousLogicalLineSnapshot(
  terminal: Terminal,
  current: LogicalInputLineSnapshot,
): LogicalInputLineSnapshot | null {
  if (current.startY <= 0) return null;
  return readLogicalLineSnapshotAt(terminal, current.startY - 1);
}

function getNextLogicalLineSnapshot(
  terminal: Terminal,
  current: LogicalInputLineSnapshot,
): LogicalInputLineSnapshot | null {
  const buffer = terminal.buffer.active;
  if (current.endY + 1 >= buffer.length) return null;
  return readLogicalLineSnapshotAt(terminal, current.endY + 1);
}

function findSegmentStartIndex(lineText: string, segment: string): number | null {
  if (!segment) {
    return lineText.length;
  }

  const index = findLineSegmentIndex(lineText, segment);
  return index >= 0 ? index : null;
}

function buildMultilineInputSpan(terminal: Terminal, state: TerminalInputState): InputSpan | null {
  const segments = state.value.split("\n");
  if (segments.length <= 1) return null;

  const cursorSnapshot = readLogicalLineSnapshot(terminal);
  if (!cursorSnapshot) return null;

  const segmentOffsets = getMultilineSegmentOffsets(state.value);
  const cursorSegmentIndex = getSegmentIndexAtCursor(segmentOffsets, state.cursor);
  const segmentSnapshots: Array<LogicalInputLineSnapshot | null> = new Array(segments.length).fill(
    null,
  );
  segmentSnapshots[cursorSegmentIndex] = cursorSnapshot;

  for (let i = cursorSegmentIndex - 1; i >= 0; i -= 1) {
    const next = segmentSnapshots[i + 1];
    if (!next) return null;
    segmentSnapshots[i] = getPreviousLogicalLineSnapshot(terminal, next);
  }

  for (let i = cursorSegmentIndex + 1; i < segments.length; i += 1) {
    const previous = segmentSnapshots[i - 1];
    if (!previous) return null;
    segmentSnapshots[i] = getNextLogicalLineSnapshot(terminal, previous);
  }

  const indexToCellOffset: number[] = [];

  for (let segmentIndex = 0; segmentIndex < segments.length; segmentIndex += 1) {
    const segment = segments[segmentIndex] ?? "";
    const snapshot = segmentSnapshots[segmentIndex];
    const valueOffset = segmentOffsets[segmentIndex] ?? 0;
    if (!snapshot) return null;

    const matchIndex = findSegmentStartIndex(snapshot.text, segment);
    if (matchIndex === null) return null;

    for (let i = 0; i <= segment.length; i += 1) {
      const cellOffset = snapshot.stringIndexToCellOffset[matchIndex + i];
      if (cellOffset === undefined) return null;
      indexToCellOffset[valueOffset + i] = toGlobalCellOffset(snapshot, cellOffset, terminal.cols);
    }
  }

  if (indexToCellOffset.length !== state.value.length + 1) return null;

  const firstSnapshot = segmentSnapshots[0];
  const lastSnapshot = segmentSnapshots[segmentSnapshots.length - 1];
  const startCellOffset = indexToCellOffset[0];
  const endCellOffset = indexToCellOffset[state.value.length];
  if (
    !firstSnapshot ||
    !lastSnapshot ||
    startCellOffset === undefined ||
    endCellOffset === undefined
  ) {
    return null;
  }

  return {
    startCellOffset,
    endCellOffset,
    startY: firstSnapshot.startY,
    endY: lastSnapshot.endY,
    indexToCellOffset,
  };
}

function buildInputSpanCacheKey(terminal: Terminal, state: TerminalInputState): string {
  const buffer = terminal.buffer.active;
  return [
    state.value,
    state.cursor,
    terminal.cols,
    buffer.baseY,
    buffer.cursorX,
    buffer.cursorY,
  ].join("\u0000");
}

function getCachedInputSpan(terminal: Terminal, state: TerminalInputState): InputSpan | null {
  const key = buildInputSpanCacheKey(terminal, state);
  const cached = inputSpanCache.get(terminal);
  if (cached?.key === key) {
    return cached.span;
  }

  const span = resolveTrackedInputSpan(terminal, state);
  inputSpanCache.set(terminal, { key, span });
  return span;
}

function resolveTrackedInputSpan(terminal: Terminal, state: TerminalInputState): InputSpan | null {
  if (!canResolveEnhancedGeometry(state)) return null;

  const buffer = terminal.buffer.active;
  const cursorCellOffset = (buffer.baseY + buffer.cursorY) * terminal.cols + buffer.cursorX;

  if (!state.multiline) {
    const snapshot = readLogicalLineSnapshot(terminal);
    if (!snapshot) return null;

    const spans = findSingleLineInputSpans(snapshot, state, terminal.cols);
    return spans.find((span) => span.indexToCellOffset[state.cursor] === cursorCellOffset) ?? null;
  }

  const span = buildMultilineInputSpan(terminal, state);
  if (span?.indexToCellOffset[state.cursor] !== cursorCellOffset) {
    return null;
  }
  return span;
}

export function getSelectedInputRange(
  terminal: Terminal,
  state: TerminalInputState,
): InputSelectionRange | null {
  if (terminal.buffer.active.type === "alternate") return null;
  if (state.desynced || state.lineRewriteRequired) return null;

  const selection = terminal.getSelectionPosition();
  if (!selection) return null;

  const selectionStart = selection.start.y * terminal.cols + selection.start.x;
  const selectionEnd = selection.end.y * terminal.cols + selection.end.x;
  const inputSpan = getCachedInputSpan(terminal, state);
  if (!inputSpan) return null;

  if (
    selection.start.y < inputSpan.startY ||
    selection.end.y > inputSpan.endY ||
    selectionStart < inputSpan.startCellOffset ||
    selectionEnd > inputSpan.endCellOffset ||
    selectionEnd <= selectionStart
  ) {
    return null;
  }

  const start = cellOffsetToInputIndex(inputSpan.indexToCellOffset, selectionStart);
  const end = cellOffsetToInputIndex(inputSpan.indexToCellOffset, selectionEnd);

  if (start < 0 || end > state.value.length || end <= start) {
    return null;
  }

  return { start, end };
}

export function getMouseBufferPosition(
  terminal: Terminal,
  event: MouseEvent,
): InputClickPosition | null {
  const screenEl = terminal.element?.querySelector(".xterm-screen") as HTMLElement | null;
  const core = (terminal as Terminal & XTermCoreWithRenderDimensions)._core;
  const cellWidth = core?._renderService?.dimensions?.css?.cell?.width ?? 0;
  const cellHeight = core?._renderService?.dimensions?.css?.cell?.height ?? 0;
  if (!screenEl || cellWidth <= 0 || cellHeight <= 0) return null;

  const rect = screenEl.getBoundingClientRect();
  const viewportX = Math.floor((event.clientX - rect.left) / cellWidth);
  const viewportY = Math.floor((event.clientY - rect.top) / cellHeight);
  if (viewportX < 0 || viewportY < 0 || viewportY >= terminal.rows) return null;

  return {
    x: Math.min(terminal.cols, viewportX),
    y: terminal.buffer.active.viewportY + viewportY,
  };
}

export function getInputIndexAtBufferPosition(
  terminal: Terminal,
  state: TerminalInputState,
  position: InputClickPosition,
): number | null {
  if (terminal.buffer.active.type === "alternate") return null;
  if (state.desynced || state.lineRewriteRequired) return null;

  const clickedCellOffset = position.y * terminal.cols + position.x;
  const inputSpan = getCachedInputSpan(terminal, state);
  if (!inputSpan) return null;

  if (position.y < inputSpan.startY || position.y > inputSpan.endY) {
    return null;
  }

  if (clickedCellOffset < inputSpan.startCellOffset) {
    return null;
  }

  if (clickedCellOffset > inputSpan.endCellOffset) {
    return position.y === inputSpan.endY ? state.value.length : null;
  }

  const inputIndex = cellOffsetToInputIndex(inputSpan.indexToCellOffset, clickedCellOffset);
  if (inputIndex < 0 || inputIndex > state.value.length) return null;
  return inputIndex;
}
