export const DEFAULT_FILE_EDITOR_FONT_SIZE = 13;
export const MIN_FILE_EDITOR_FONT_SIZE = 8;
export const MAX_FILE_EDITOR_FONT_SIZE = 72;
export const FILE_EDITOR_FONT_SIZE_STEP = 1;

export function clampFileEditorFontSize(fontSize: number): number {
  if (!Number.isFinite(fontSize)) return DEFAULT_FILE_EDITOR_FONT_SIZE;
  return Math.max(
    MIN_FILE_EDITOR_FONT_SIZE,
    Math.min(MAX_FILE_EDITOR_FONT_SIZE, Math.round(fontSize)),
  );
}

export function increaseFileEditorFontSize(fontSize: number): number {
  return clampFileEditorFontSize(fontSize + FILE_EDITOR_FONT_SIZE_STEP);
}

export function decreaseFileEditorFontSize(fontSize: number): number {
  return clampFileEditorFontSize(fontSize - FILE_EDITOR_FONT_SIZE_STEP);
}
