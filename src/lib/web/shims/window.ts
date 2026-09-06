/**
 * Web shim for `@tauri-apps/api/window`.
 *
 * The browser has no OS windows: `getCurrentWindow()` returns a logical
 * window whose label is the topmost open modal (from `windowContext`) while
 * a modal page is running, or "web" otherwise. Window management calls are
 * accepted and ignored; closing a modal routes through the modal host.
 */

import {
  currentWindowLabel,
  decrementCloseSubscriber,
  incrementCloseSubscriber,
} from "../windowContext";
import type { UnlistenFn } from "../eventBus";

export type { UnlistenFn };

export interface MonitorLike {
  name: string | null;
  position: { x: number; y: number };
  size: { width: number; height: number };
  workArea: {
    position: { x: number; y: number };
    size: { width: number; height: number };
  };
  scaleFactor: number;
}

export const UserAttentionType = {
  Critical: 1,
  Informational: 2,
} as const;
export type UserAttentionType = (typeof UserAttentionType)[keyof typeof UserAttentionType];

export class PhysicalPosition {
  constructor(
    public x: number,
    public y: number,
  ) {}
}

export class PhysicalSize {
  constructor(
    public width: number,
    public height: number,
  ) {}
}

class WebWindow {
  /** Dynamic: reflects the topmost modal while modal code is running.
   *  The fallback "main" matches the server's bridge-window label, so
   *  owner-scoped payloads (auth prompts, MCP) target this window exactly
   *  like they target the desktop main window. */
  get label(): string {
    return currentWindowLabel("main");
  }

  async close(): Promise<void> {
    window.dispatchEvent(
      new CustomEvent("nyaterm:web-modal-close", { detail: { label: this.label } }),
    );
  }

  async destroy(): Promise<void> {
    return this.close();
  }

  async hide(): Promise<void> {
    return this.close();
  }

  async show(): Promise<void> {}

  async setFocus(): Promise<void> {}

  async setAlwaysOnTop(_alwaysOnTop: boolean): Promise<void> {}

  async setEnabled(_enabled: boolean): Promise<void> {}

  async setFocusable(_focusable: boolean): Promise<void> {}

  async setResizable(_resizable: boolean): Promise<void> {}

  async setTitle(_title: string): Promise<void> {}

  async center(): Promise<void> {}

  async setPosition(_position: unknown): Promise<void> {}

  async setSize(_size: unknown): Promise<void> {}

  async unminimize(): Promise<void> {}

  async minimize(): Promise<void> {}

  async toggleMaximize(): Promise<void> {}

  async internalToggleMaximize(): Promise<void> {}

  async isAlwaysOnTop(): Promise<boolean> {
    return false;
  }

  async startDragging(): Promise<void> {}

  onResized(handler: (event: { payload: unknown }) => void): Promise<UnlistenFn> {
    const resizeListener = () => handler({ payload: null });
    window.addEventListener("resize", resizeListener);
    return Promise.resolve(() => {
      window.removeEventListener("resize", resizeListener);
    });
  }

  onMoved(handler: (event: { payload: unknown }) => void): Promise<UnlistenFn> {
    // Browser windows have no move event for the page; treat resize as the
    // closest signal so terminal fit refreshes keep working.
    const resizeListener = () => handler({ payload: null });
    window.addEventListener("resize", resizeListener);
    return Promise.resolve(() => {
      window.removeEventListener("resize", resizeListener);
    });
  }

  onScaleChanged(handler: (event: { payload: number }) => void): Promise<UnlistenFn> {
    const mq = window.matchMedia(`(resolution: ${window.devicePixelRatio}dppx)`);
    const listener = () => handler({ payload: window.devicePixelRatio || 1 });
    mq.addEventListener("change", listener, { once: true });
    return Promise.resolve(() => mq.removeEventListener("change", listener));
  }

  async isMinimized(): Promise<boolean> {
    return false;
  }

  async isMaximized(): Promise<boolean> {
    return false;
  }

  async isFocused(): Promise<boolean> {
    return document.hasFocus();
  }

  async isVisible(): Promise<boolean> {
    return true;
  }

  async outerPosition(): Promise<PhysicalPosition> {
    return new PhysicalPosition(0, 0);
  }

  async outerSize(): Promise<PhysicalSize> {
    return new PhysicalSize(window.innerWidth, window.innerHeight);
  }

  async innerSize(): Promise<PhysicalSize> {
    return this.outerSize();
  }

  async scaleFactor(): Promise<number> {
    return window.devicePixelRatio || 1;
  }

  onCloseRequested(handler: (event: { preventDefault: () => void }) => void): Promise<UnlistenFn> {
    const closeListener = (event: Event) => {
      const detail = (event as CustomEvent<{ label: string }>).detail ?? { label: "" };
      if (detail.label !== this.label) return;
      let prevented = false;
      handler({ preventDefault: () => (prevented = true) });
      if (!prevented) {
        window.dispatchEvent(
          new CustomEvent("nyaterm:web-modal-close", { detail: { label: this.label } }),
        );
      }
    };
    window.addEventListener("nyaterm:web-modal-close-requested", closeListener);
    incrementCloseSubscriber(this.label);
    return Promise.resolve(() => {
      window.removeEventListener("nyaterm:web-modal-close-requested", closeListener);
      decrementCloseSubscriber(this.label);
    });
  }

  onFocusChanged(handler: (event: { payload: boolean }) => void): Promise<UnlistenFn> {
    const focusListener = () => handler({ payload: document.hasFocus() });
    const blurListener = () => handler({ payload: false });
    window.addEventListener("focus", focusListener);
    window.addEventListener("blur", blurListener);
    return Promise.resolve(() => {
      window.removeEventListener("focus", focusListener);
      window.removeEventListener("blur", blurListener);
    });
  }

  once(_event: string, _handler: () => void): Promise<UnlistenFn> {
    return Promise.resolve(() => {});
  }

  async requestUserAttention(_type?: UserAttentionType): Promise<void> {
    const original = document.title;
    document.title = `• ${original}`;
    setTimeout(() => {
      document.title = original;
    }, 1500);
  }
}

let currentWindow: WebWindow | null = null;

export function getCurrentWindow(): WebWindow {
  currentWindow ??= new WebWindow();
  return currentWindow;
}

export async function availableMonitors(): Promise<MonitorLike[]> {
  return [await primaryMonitor()];
}

export async function primaryMonitor(): Promise<MonitorLike> {
  const width = window.screen?.width ?? 1920;
  const height = window.screen?.height ?? 1080;
  return {
    name: "primary",
    position: { x: 0, y: 0 },
    size: { width, height },
    workArea: {
      position: { x: 0, y: 0 },
      size: { width, height },
    },
    scaleFactor: window.devicePixelRatio || 1,
  };
}

export async function currentMonitor(): Promise<MonitorLike | null> {
  return primaryMonitor();
}

// Type-only export so `import { Window } from "@tauri-apps/api/window"` keeps
// compiling in web builds (the class above is a behavioral stand-in).
export { WebWindow as Window };
