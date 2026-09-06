/**
 * Web-mode event bus: a single multiplexed WebSocket replacing Tauri's
 * event system.
 *
 * The server broadcasts every backend event to every connected client
 * (matching the desktop's global broadcast semantics), and `emit` degrades
 * to local in-document dispatch — in web mode "child windows" are in-document
 * modals, so frontend→frontend events stay inside this document, exactly
 * like same-window events on the desktop.
 */

import { apiBase, ensureWebToken, getStoredToken } from "./rpcClient";

export type UnlistenFn = () => void;

export interface WebEvent<T = unknown> {
  event: string;
  eventId: number;
  payload: T;
}

type Handler = (event: WebEvent) => void;

const handlers = new Map<string, Set<Handler>>();
let socket: WebSocket | null = null;
let connecting = false;
let reconnectAttempt = 0;
let reconnectTimer: ReturnType<typeof setTimeout> | null = null;
let eventIdCounter = 0;

function wsUrl(): string {
  const base = apiBase();
  const token = getStoredToken() ?? "";
  if (base.startsWith("http")) {
    const wsBase = base.replace(/^http/, "ws");
    return `${wsBase}/api/ws?token=${encodeURIComponent(token)}`;
  }
  const protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
  return `${protocol}//${window.location.host}/api/ws?token=${encodeURIComponent(token)}`;
}

function dispatch(event: string, payload: unknown): void {
  const listeners = handlers.get(event);
  if (!listeners) return;
  eventIdCounter += 1;
  const wrapped: WebEvent = { event, eventId: eventIdCounter, payload };
  for (const handler of [...listeners]) {
    try {
      handler(wrapped);
    } catch (error) {
      console.error(`[nyaterm-web] listener for "${event}" threw`, error);
    }
  }
}

function scheduleReconnect(): void {
  if (reconnectTimer) return;
  reconnectAttempt = Math.min(reconnectAttempt + 1, 6);
  const delay = Math.min(1000 * 2 ** reconnectAttempt, 30_000);
  reconnectTimer = setTimeout(() => {
    reconnectTimer = null;
    connect();
  }, delay);
}

function connect(): void {
  if (typeof window === "undefined") return;
  if (socket || connecting) return;
  if (handlers.size === 0) return;
  connecting = true;
  ensureWebToken()
    .then(() => {
      const ws = new WebSocket(wsUrl());
      socket = ws;
      ws.onopen = () => {
        reconnectAttempt = 0;
      };
      ws.onmessage = (message) => {
        try {
          const frame = JSON.parse(String(message.data)) as {
            event: string;
            payload: unknown;
          };
          if (typeof frame.event === "string") {
            dispatch(frame.event, frame.payload);
          }
        } catch {
          // Ignore malformed frames.
        }
      };
      ws.onclose = () => {
        if (socket === ws) socket = null;
        connecting = false;
        if (handlers.size > 0) scheduleReconnect();
      };
      ws.onerror = () => {
        ws.close();
      };
    })
    .catch(() => {
      connecting = false;
      if (handlers.size > 0) scheduleReconnect();
    });
}

function ensureConnected(): void {
  connect();
}

/** Tauri-compatible `listen`: registers a handler for an exact event name. */
export function webListen<T = unknown>(
  eventName: string,
  handler: (event: WebEvent<T>) => void,
): Promise<UnlistenFn> {
  let listeners = handlers.get(eventName);
  if (!listeners) {
    listeners = new Set();
    handlers.set(eventName, listeners);
  }
  const wrapped: Handler = (event) => handler(event as WebEvent<T>);
  listeners.add(wrapped);
  ensureConnected();

  return Promise.resolve(() => {
    const current = handlers.get(eventName);
    if (!current) return;
    current.delete(wrapped);
    if (current.size === 0) {
      handlers.delete(eventName);
    }
  });
}

/**
 * Tauri-compatible `emit`: in web mode events are dispatched to local
 * listeners in this document only. Cross-window events do not exist here —
 * "child windows" are in-document modals sharing this bus.
 */
export function webEmit(eventName: string, payload?: unknown): Promise<void> {
  dispatch(eventName, payload);
  return Promise.resolve();
}

/** Test helper: drop all connections and listeners. */
export function resetWebEventBusForTests(): void {
  handlers.clear();
  socket?.close();
  socket = null;
  connecting = false;
  if (reconnectTimer) {
    clearTimeout(reconnectTimer);
    reconnectTimer = null;
  }
  reconnectAttempt = 0;
}
