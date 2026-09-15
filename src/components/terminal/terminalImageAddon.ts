import { type IImageAddonOptions, ImageAddon } from "@xterm/addon-image";
import type { Terminal } from "@xterm/xterm";
import { logger } from "@/lib/logger";

export const TERMINAL_IMAGE_ADDON_OPTIONS = {
  enableSizeReports: false,
  sixelSupport: true,
  iipSupport: true,
  kittySupport: false,
  storageLimit: 32,
  pixelLimit: 8_388_608,
  showPlaceholder: true,
} satisfies IImageAddonOptions;

const installedImageAddons = new WeakMap<Terminal, ImageAddon>();

export interface TerminalImageAddonInstallOptions {
  sessionId?: string;
  sessionType?: string;
}

export function installTerminalImageAddon(
  terminal: Terminal,
  options: TerminalImageAddonInstallOptions = {},
): ImageAddon | null {
  const existingAddon = installedImageAddons.get(terminal);
  if (existingAddon) return existingAddon;

  const imageAddon = new ImageAddon(TERMINAL_IMAGE_ADDON_OPTIONS);

  try {
    terminal.loadAddon(imageAddon);
    installedImageAddons.set(terminal, imageAddon);
    return imageAddon;
  } catch (error) {
    try {
      imageAddon.dispose();
    } catch {
      /* ignore cleanup failures while keeping terminal startup alive */
    }

    logger.warn({
      domain: "terminal.input",
      event: "terminal.image_addon_failed",
      message: "Failed to initialize terminal image addon",
      ids: options.sessionId ? { session_id: options.sessionId } : undefined,
      data: {
        session_type: options.sessionType,
        sixel_support: TERMINAL_IMAGE_ADDON_OPTIONS.sixelSupport,
        iip_support: TERMINAL_IMAGE_ADDON_OPTIONS.iipSupport,
        kitty_support: TERMINAL_IMAGE_ADDON_OPTIONS.kittySupport,
        storage_limit_mb: TERMINAL_IMAGE_ADDON_OPTIONS.storageLimit,
        pixel_limit: TERMINAL_IMAGE_ADDON_OPTIONS.pixelLimit,
      },
      error,
    });
    return null;
  }
}
