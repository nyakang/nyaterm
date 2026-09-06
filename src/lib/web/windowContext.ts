/**
 * Shared state for web-mode "windows".
 *
 * Web mode renders desktop child windows as in-document modals. The window
 * shims (`shims/window.ts`, `shims/webviewWindow.ts`) and the modal host
 * (`modalHost.tsx`) coordinate through this module so that existing window
 * management code (which expects Tauri window handles and lifecycle events)
 * keeps working unchanged.
 */

import type { WebviewWindow } from "./shims/webviewWindow";

export interface OpenModalHandle {
  label: string;
  window: WebviewWindow;
}

/** Innermost-first stack of modal labels; the top is the "current" window. */
const modalLabelStack: string[] = [];

const openModals = new Map<string, OpenModalHandle>();

/** Labels with at least one registered `onCloseRequested` handler. */
const closeSubscribers = new Map<string, number>();

export function pushWindowLabel(label: string): void {
  modalLabelStack.push(label);
}

export function popWindowLabel(label: string): void {
  const index = modalLabelStack.lastIndexOf(label);
  if (index >= 0) modalLabelStack.splice(index, 1);
}

/** Label reported by `getCurrentWindow()`; falls back outside modals. */
export function currentWindowLabel(fallback: string): string {
  return modalLabelStack[modalLabelStack.length - 1] ?? fallback;
}

export function registerModal(handle: OpenModalHandle): void {
  openModals.set(handle.label, handle);
  pushWindowLabel(handle.label);
}

export function unregisterModal(label: string): void {
  openModals.delete(label);
  popWindowLabel(label);
}

export function getModal(label: string): OpenModalHandle | null {
  return openModals.get(label) ?? null;
}

export function getOpenModals(): OpenModalHandle[] {
  return [...openModals.values()];
}

export function incrementCloseSubscriber(label: string): void {
  closeSubscribers.set(label, (closeSubscribers.get(label) ?? 0) + 1);
}

export function decrementCloseSubscriber(label: string): void {
  const next = (closeSubscribers.get(label) ?? 1) - 1;
  if (next <= 0) closeSubscribers.delete(label);
  else closeSubscribers.set(label, next);
}

export function hasCloseSubscriber(label: string): boolean {
  return (closeSubscribers.get(label) ?? 0) > 0;
}
