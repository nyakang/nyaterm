import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { setStoredToken, webInvoke } from "./rpcClient";

const fetchMock = vi.fn();

function jsonResponse(body: unknown, status = 200): Response {
  return {
    ok: status >= 200 && status < 300,
    status,
    json: async () => body,
  } as unknown as Response;
}

describe("web RPC client", () => {
  beforeEach(() => {
    vi.stubGlobal("fetch", fetchMock);
    window.localStorage.clear();
    setStoredToken("test-token");
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    window.localStorage.clear();
  });

  it("sends the command envelope and unwraps successful data", async () => {
    fetchMock.mockResolvedValueOnce(jsonResponse({ ok: true, data: [1, 2, 3] }));

    const result = await webInvoke<number[]>("list_sessions");

    expect(result).toEqual([1, 2, 3]);
    const [url, init] = fetchMock.mock.calls[0] as [string, RequestInit];
    expect(url).toContain("/api/rpc");
    expect(init.method).toBe("POST");
    expect(JSON.parse(String(init.body))).toEqual({ cmd: "list_sessions", args: {} });
    expect((init.headers as Record<string, string>).Authorization).toBe(
      "Bearer test-token",
    );
  });

  it("rejects with the server error string, matching desktop invoke semantics", async () => {
    fetchMock.mockResolvedValueOnce(
      jsonResponse({ ok: false, error: "SessionNotFound: nope" }),
    );
    await expect(webInvoke("attach_session", { sessionId: "x" })).rejects.toBe(
      "SessionNotFound: nope",
    );
  });

  it("triggers a browser download for __webDownload markers and returns undefined", async () => {
    fetchMock
      .mockResolvedValueOnce(
        jsonResponse({ ok: true, data: { __webDownload: "abc_report.txt" } }),
      )
      .mockResolvedValueOnce(
        new Response(new Blob(["file-bytes"]), { status: 200 }),
      );
    const createObjectURL = vi.fn(() => "blob:fake");
    const revokeObjectURL = vi.fn();
    vi.stubGlobal("URL", Object.assign(URL, {
      createObjectURL,
      revokeObjectURL,
    }));
    const click = vi.fn();
    const anchor = { href: "", download: "", click, remove: vi.fn() };
    const createElementSpy = vi
      .spyOn(document, "createElement")
      .mockReturnValue(anchor as unknown as HTMLElement);
    const appendSpy = vi.spyOn(document.body, "append").mockImplementation(() => {});

    const result = await webInvoke("download_remote_file", { localPath: "__web_downloads__/report.txt" });

    expect(result).toBeUndefined();
    expect(click).toHaveBeenCalled();
    expect(anchor.download).toBe("abc_report.txt");
    createElementSpy.mockRestore();
    appendSpy.mockRestore();
    vi.unstubAllGlobals();
  });
});
