import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";
import { ConnectionTagsField } from "./ConnectionTagsField";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, options?: { tag?: string }) =>
      key === "dialog.removeTag" ? `Remove ${options?.tag}` : key,
  }),
}));

function TestField({ onKeyDown }: { onKeyDown?: () => void }) {
  const [tags, setTags] = useState(["production"]);
  return (
    <div onKeyDown={onKeyDown}>
      <ConnectionTagsField
        value={tags}
        suggestions={["production", "staging", "gpu"]}
        onChange={setTags}
      />
    </div>
  );
}

describe("ConnectionTagsField", () => {
  it("adds trimmed tags with Enter and comma while preventing exact duplicates", async () => {
    const user = userEvent.setup();
    render(<TestField />);
    const input = screen.getByRole("combobox");

    await user.type(input, "  database  {Enter}");
    expect(screen.queryByText("database")).not.toBeNull();

    await user.type(input, "router,");
    expect(screen.queryByText("router")).not.toBeNull();

    await user.type(input, "production{Enter}");
    expect(screen.getAllByText("production")).toHaveLength(1);

    await user.type(input, "Production{Enter}");
    expect(screen.queryByText("Production")).not.toBeNull();
  });

  it("removes tags with the remove button and empty Backspace", async () => {
    const user = userEvent.setup();
    render(<TestField />);
    const input = screen.getByRole("combobox");

    await user.click(screen.getByRole("button", { name: "Remove production" }));
    expect(screen.queryByRole("button", { name: "Remove production" })).toBeNull();

    await user.type(input, "gpu{Enter}");
    await user.type(input, "{Backspace}");
    expect(screen.queryByRole("button", { name: "Remove gpu" })).toBeNull();
  });

  it("reuses filtered existing tags and keeps Enter from reaching the parent form", async () => {
    const user = userEvent.setup();
    const parentKeyDown = vi.fn();
    render(<TestField onKeyDown={parentKeyDown} />);
    const input = screen.getByRole("combobox");

    await user.type(input, "stag");
    await user.click(screen.getByRole("option", { name: "staging" }));
    expect(screen.queryByText("staging")).not.toBeNull();

    parentKeyDown.mockClear();
    fireEvent.keyDown(input, { key: "Enter" });
    expect(parentKeyDown).not.toHaveBeenCalled();
  });
});
