import { fireEvent, render, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { ReferenceTooltip } from "./ReferenceTooltip";

function visibleTooltip() {
  const tooltip = document.querySelector<HTMLElement>('[data-slot="tooltip-content"]');
  if (!tooltip) throw new Error("Themed tooltip was not opened");
  return tooltip;
}

describe("reference tooltips", () => {
  it("uses project theme tokens and wraps complete paths and scope descriptions", async () => {
    const text =
      "root@source.example.test:2222 (via root@jump.example.test:2200)\nReference all 2 sessions; files are not included.";
    const view = render(
      <ReferenceTooltip text={text}>
        <button type="button">Host</button>
      </ReferenceTooltip>,
    );
    const trigger = view.getByRole("button");
    expect(trigger.hasAttribute("title")).toBe(false);
    fireEvent.focus(trigger);
    const tooltip = await waitFor(visibleTooltip);
    expect(tooltip.className).toContain("bg-popover");
    expect(tooltip.className).toContain("text-popover-foreground");
    expect(tooltip.className).toContain("border-border");
    expect(tooltip.className).toContain("whitespace-pre-wrap");
    expect(tooltip.textContent).toContain("via root@jump.example.test:2200");
    expect(tooltip.textContent).toContain("files are not included");
  });

  it("keeps unavailable references readable without enabling selection", async () => {
    const view = render(
      <ReferenceTooltip text="File exceeds the size limit" disabled triggerClassName="block w-full">
        <button type="button" disabled>
          Large file
        </button>
      </ReferenceTooltip>,
    );
    const button = view.getByRole("button");
    expect(button.hasAttribute("disabled")).toBe(true);
    const trigger = button.parentElement;
    if (!trigger) throw new Error("Missing disabled tooltip trigger");
    expect(trigger.tabIndex).toBe(0);
    fireEvent.focus(trigger);
    expect((await waitFor(visibleTooltip)).textContent).toContain("File exceeds the size limit");
  });
});
