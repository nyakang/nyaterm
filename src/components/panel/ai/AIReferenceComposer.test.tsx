import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { type AIInlineMention, AIReferenceComposer } from "./AIReferenceComposer";

const mention: AIInlineMention = {
  id: "file:one",
  kind: "file",
  label: "nginx.conf",
  title: "root@prod.example.test:22:/etc/nginx/nginx.conf",
  start: 5,
  end: 16,
};

describe("AIReferenceComposer", () => {
  it("renders an atomic inline reference with the complete source path on hover", () => {
    render(
      <AIReferenceComposer
        value="Read @nginx.conf now"
        mentions={[mention]}
        placeholder="Ask"
        disabled={false}
        onChange={vi.fn()}
        onMentionQuery={vi.fn()}
        onKeyDown={vi.fn()}
        onCompositionStart={vi.fn()}
        onCompositionEnd={vi.fn()}
      />,
    );

    const editor = screen.getByRole("textbox");
    const token = editor.querySelector('[data-ai-reference-id="file:one"]');
    expect(token?.textContent).toBe("@nginx.conf");
    expect(token?.getAttribute("contenteditable")).toBe("false");
    expect(token?.getAttribute("aria-label")).toBe(
      "root@prod.example.test:22:/etc/nginx/nginx.conf",
    );
    expect(token?.hasAttribute("title")).toBe(false);
  });

  it("shows a themed tooltip on editor tokens without changing their text or deletion behavior", async () => {
    const onChange = vi.fn();
    const view = render(
      <AIReferenceComposer
        value="Read @nginx.conf now"
        mentions={[mention]}
        placeholder="Ask"
        disabled={false}
        onChange={onChange}
        onMentionQuery={vi.fn()}
        onKeyDown={vi.fn()}
        onCompositionStart={vi.fn()}
        onCompositionEnd={vi.fn()}
      />,
    );
    const editor = view.getByRole("textbox");
    const token = editor.querySelector('[data-ai-reference-id="file:one"]');
    if (!token) throw new Error("Missing file token");
    fireEvent.pointerMove(token, { pointerType: "mouse" });
    await waitFor(() => {
      const tooltip = document.querySelector('[data-slot="tooltip-content"]');
      expect(tooltip?.textContent).toContain(mention.title);
      expect(tooltip?.className).toContain("bg-popover");
    });
    expect(token.textContent).toBe("@nginx.conf");
    expect(onChange).not.toHaveBeenCalled();
    const range = document.createRange();
    range.setStartAfter(token);
    range.collapse(true);
    window.getSelection()?.removeAllRanges();
    window.getSelection()?.addRange(range);
    fireEvent.keyDown(editor, { key: "Backspace" });
    expect(onChange).toHaveBeenLastCalledWith("Read  now", []);
    await waitFor(() => expect(document.querySelector('[data-slot="tooltip-content"]')).toBeNull());
  });

  it("deletes a whole mention when Backspace is pressed directly after it", () => {
    const onChange = vi.fn();
    render(
      <AIReferenceComposer
        value="Read @nginx.conf now"
        mentions={[mention]}
        placeholder="Ask"
        disabled={false}
        onChange={onChange}
        onMentionQuery={vi.fn()}
        onKeyDown={vi.fn()}
        onCompositionStart={vi.fn()}
        onCompositionEnd={vi.fn()}
      />,
    );

    const editor = screen.getByRole("textbox");
    const token = editor.querySelector('[data-ai-reference-id="file:one"]');
    if (!token) throw new Error("Inline file reference was not rendered");
    const range = document.createRange();
    range.setStart(editor, Array.prototype.indexOf.call(editor.childNodes, token) + 1);
    range.collapse(true);
    const selection = window.getSelection();
    selection?.removeAllRanges();
    selection?.addRange(range);

    fireEvent.keyDown(editor, { key: "Backspace" });

    expect(onChange).toHaveBeenLastCalledWith("Read  now", []);
    expect(editor.querySelector('[data-ai-reference-id="file:one"]')).toBeNull();
  });
});
