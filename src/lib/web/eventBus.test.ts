import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { resetWebEventBusForTests, webEmit, webListen } from "./eventBus";

type MessageHandler = (event: { data: string }) => void;

let sendHandler: ((url: string) => FakeWebSocket) | null = null;
let lastSocket: FakeWebSocket | null = null;

class FakeWebSocket {
  static instances: FakeWebSocket[] = [];
  onopen: (() => void) | null = null;
  onmessage: MessageHandler | null = null;
  onclose: (() => void) | null = null;
  onerror: (() => void) | null = null;
  sent: string[] = [];
  closed = false;

  constructor(public url: string) {
    lastSocket = this;
    FakeWebSocket.instances.push(this);
    sendHandler?.(url);
  }

  send(data: string): void {
    this.sent.push(data);
  }

  close(): void {
    this.closed = true;
    this.onclose?.();
  }

  emitOpen(): void {
    this.onopen?.();
  }

  emitMessage(data: unknown): void {
    this.onmessage?.({ data: JSON.stringify(data) });
  }
}

describe("web event bus", () => {
  beforeEach(() => {
    vi.stubGlobal("WebSocket", FakeWebSocket as unknown as typeof WebSocket);
    FakeWebSocket.instances = [];
    sendHandler = null;
    lastSocket = null;
    window.localStorage.clear();
    // Seed a token so the bus connects without showing the login overlay.
    window.localStorage.setItem("nyaterm_web_token", "test-token");
  });

  afterEach(() => {
    resetWebEventBusForTests();
    vi.unstubAllGlobals();
  });

  it("dispatches server frames to exact-name listeners", async () => {
    const seen: unknown[] = [];
    const unlisten = await webListen<{ value: number }>("transfer-event", ({ payload }) => {
      seen.push(payload);
    });

    lastSocket?.emitOpen();
    lastSocket?.emitMessage({ event: "transfer-event", payload: { value: 7 } });
    lastSocket?.emitMessage({ event: "other-event", payload: { value: 1 } });

    expect(seen).toEqual([{ value: 7 }]);
    unlisten();
    lastSocket?.emitMessage({ event: "transfer-event", payload: { value: 8 } });
    expect(seen).toEqual([{ value: 7 }]);
  });

  it("emit dispatches to local listeners (in-document modal semantics)", async () => {
    const payloads: unknown[] = [];
    await webListen("proxy-saved", ({ payload }) => payloads.push(payload));
    await webEmit("proxy-saved", { id: "p1" });
    expect(payloads).toEqual([{ id: "p1" }]);
  });

  it("connects lazily only while listeners exist", async () => {
    expect(lastSocket).toBeNull();
    const unlisten = await webListen("settings-changed", () => {});
    // Connection is asynchronous (waits for the token).
    await vi.waitFor(() => expect(lastSocket).not.toBeNull());
    unlisten();
    resetWebEventBusForTests();
  });
});
