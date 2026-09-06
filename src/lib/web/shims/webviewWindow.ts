/**
 * Web shim for `@tauri-apps/api/webviewWindow`.
 *
 * Web mode has no OS child windows; `getByLabel`/`getAll` surface the fake
 * window handles registered by the modal host (`modalHost.tsx`) so existing
 * window-management flows keep working. Handles carry a `fireDestroyed()`
 * hook the modal host triggers on close, replaying Tauri's
 * `tauri://destroyed` lifecycle.
 */

import { getModal, getOpenModals } from "../windowContext";

export class WebviewWindow {
  readonly label: string;

  private destroyedCallbacks = new Set<() => void>();

  constructor(
    label: string,
    // eslint-disable-next-line @typescript-eslint/no-unused-vars
    _options?: Record<string, unknown>,
  ) {
    this.label = label;
  }

  static async getByLabel(label: string): Promise<WebviewWindow | null> {
    return getModal(label)?.window ?? null;
  }

  static async getAll(): Promise<WebviewWindow[]> {
    return getOpenModals().map((handle) => handle.window);
  }

  async close(): Promise<void> {
    this.fireDestroyed();
  }

  async destroy(): Promise<void> {
    this.fireDestroyed();
  }

  async hide(): Promise<void> {
    this.fireDestroyed();
  }

  async show(): Promise<void> {}

  async setFocus(): Promise<void> {}

  async setEnabled(_enabled: boolean): Promise<void> {}

  async setFocusable(_focusable: boolean): Promise<void> {}

  async setAlwaysOnTop(_alwaysOnTop: boolean): Promise<void> {}

  async setTitle(_title: string): Promise<void> {}

  async setResizable(_resizable: boolean): Promise<void> {}

  async setPosition(_position: unknown): Promise<void> {}

  async setSize(_size: unknown): Promise<void> {}

  async center(): Promise<void> {}

  async isVisible(): Promise<boolean> {
    return true;
  }

  async isMinimized(): Promise<boolean> {
    return false;
  }

  async isFocused(): Promise<boolean> {
    return document.hasFocus();
  }

  async outerPosition(): Promise<{ x: number; y: number }> {
    return { x: 0, y: 0 };
  }

  /** Registers a `tauri://destroyed` callback (see `attachChildWindowDestroyedHandler`). */
  once(event: string, handler: () => void): Promise<() => void> {
    if (event === "tauri://destroyed") {
      this.destroyedCallbacks.add(handler);
      return Promise.resolve(() => {
        this.destroyedCallbacks.delete(handler);
      });
    }
    return Promise.resolve(() => {});
  }

  onCloseRequested(
    _handler: (event: { preventDefault: () => void }) => void,
  ): Promise<() => void> {
    return Promise.resolve(() => {});
  }

  onFocusChanged(_handler: (event: { payload: boolean }) => void): Promise<() => void> {
    return Promise.resolve(() => {});
  }

  /** Invoked by the modal host when the modal is removed from the DOM. */
  fireDestroyed(): void {
    const callbacks = [...this.destroyedCallbacks];
    this.destroyedCallbacks.clear();
    for (const callback of callbacks) {
      try {
        callback();
      } catch (error) {
        console.error("[nyaterm-web] destroyed callback failed", error);
      }
    }
  }
}
