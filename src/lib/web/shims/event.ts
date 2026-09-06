/**
 * Web shim for `@tauri-apps/api/event`.
 *
 * Backend events arrive over the multiplexed WebSocket (`eventBus.ts`);
 * frontend-emitted events stay in-document (web "child windows" are modals
 * in this same document, so cross-window events remain local).
 */

import { webEmit, webListen, type UnlistenFn, type WebEvent } from "../eventBus";

export type { UnlistenFn };

export function listen<T>(
  eventName: string,
  handler: (event: WebEvent<T>) => void,
): Promise<UnlistenFn> {
  return webListen<T>(eventName, handler);
}

export function once<T>(
  eventName: string,
  handler: (event: WebEvent<T>) => void,
): Promise<UnlistenFn> {
  let unlisten: UnlistenFn | undefined;
  const promise = webListen<T>(eventName, (event) => {
    unlisten?.();
    handler(event);
  });
  unlisten = undefined;
  return promise.then((un) => {
    unlisten = un;
    return un;
  });
}

export function emit(eventName: string, payload?: unknown): Promise<void> {
  return webEmit(eventName, payload);
}

export function emitTo(
  _target: unknown,
  eventName: string,
  payload?: unknown,
): Promise<void> {
  // Web mode has no distinct event targets; fall back to local dispatch.
  return webEmit(eventName, payload);
}
