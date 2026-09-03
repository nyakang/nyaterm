import type { IWindowOptions } from "@xterm/xterm";

/**
 * Do not reflect application-controlled icon/window titles back into the PTY.
 * Keep this explicit instead of depending on xterm.js defaults.
 */
export const XTERM_SECURE_WINDOW_OPTIONS: IWindowOptions = Object.freeze({
  getIconTitle: false,
  getWinTitle: false,
});
