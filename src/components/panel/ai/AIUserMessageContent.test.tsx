import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { AIMessage } from "@/types/global";
import { AIUserMessageContent } from "./AIUserMessageContent";

function message(content: string): AIMessage {
  return {
    id: "message",
    sessionId: "session",
    role: "user",
    content,
    createdAt: "2026-01-01T00:00:00Z",
  };
}

describe("AyaAgent history reference rendering", () => {
  it("renders file, host and session tokens with retained source metadata", () => {
    const content = "@file.py @jump.example.test @terminal";
    const item = message(content);
    item.attachments = [
      {
        id: "file",
        kind: "file",
        name: "file.py",
        title: "root@source.example.test:2222:/home/file.py",
        start: 0,
        end: 8,
      },
      {
        id: "host",
        kind: "host",
        name: "jump.example.test",
        title: "root@jump.example.test:2200",
        start: 9,
        end: 27,
      },
      {
        id: "session",
        kind: "session",
        name: "terminal",
        title: "root@source.example.test:2222",
        start: 28,
        end: 37,
      },
    ];
    const view = render(<AIUserMessageContent message={item} />);
    expect(view.container.querySelectorAll("[data-ai-reference-kind]")).toHaveLength(3);
    expect(
      view.container.querySelector('[data-ai-reference-kind="file"]')?.getAttribute("aria-label"),
    ).toBe(item.attachments[0].title);
    expect(view.container.querySelector('[data-ai-reference-kind="host"]')?.textContent).toBe(
      "@jump.example.test",
    );
  });

  it("does not attach a reference to a different occurrence of the same filename", () => {
    const content = "😀 typed @note.txt; selected @note.txt and @note.txt";
    const start = content.indexOf("@note.txt", content.indexOf("selected"));
    const second = content.lastIndexOf("@note.txt");
    const item = message(content);
    item.attachments = [
      { id: "a", kind: "file", name: "note.txt", title: "host-a:/note.txt", start, end: start + 9 },
      {
        id: "b",
        kind: "file",
        name: "note.txt",
        title: "host-b:/note.txt",
        start: second,
        end: second + 9,
      },
    ];
    const view = render(<AIUserMessageContent message={item} />);
    expect(view.container.querySelectorAll("[data-ai-reference-kind]")).toHaveLength(2);
    expect(view.container.querySelector('[aria-label="host-a:/note.txt"]')).not.toBeNull();
    expect(view.container.querySelector("[title]")).toBeNull();
    expect(view.container.textContent).toBe(content);
  });

  it("keeps legacy attachments visible and refuses stale new spans", () => {
    const item = message("@old.txt @host");
    item.attachments = [
      { id: "old", name: "old.txt", host: "host-a", path: "/old.txt" },
      { id: "stale", kind: "host", name: "host", title: "host-b", start: 0, end: 5 },
    ];
    const view = render(<AIUserMessageContent message={item} />);
    expect(view.container.querySelectorAll("[data-ai-reference-kind]")).toHaveLength(1);
    expect(
      view.container.querySelector("[data-ai-reference-kind]")?.getAttribute("aria-label"),
    ).toBe("host-a:/old.txt");
  });
});
