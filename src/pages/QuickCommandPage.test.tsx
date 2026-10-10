import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { QuickCommand } from "@/types/global";
import QuickCommandPage from "./QuickCommandPage";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  emit: vi.fn(async () => undefined),
  close: vi.fn(),
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) => {
      const labels: Record<string, string> = {
        "quickCommands.addCommand": "Add Command",
        "quickCommands.editCommand": "Edit Command",
        "quickCommands.labelName": "Label",
        "quickCommands.uncategorized": "Uncategorized",
        "quickCommands.executionMode": "Mode",
        "quickCommands.executeImmediately": "Run now",
        "quickCommands.appendOnly": "Append",
        "quickCommands.nyascript": "NyaScript",
        "quickCommands.nyascriptHint": "Run a native terminal automation script.",
        "dialog.cancel": "Cancel",
        "dialog.save": "Save",
        "dialog.saving": "Saving",
      };
      return labels[key] ?? key;
    },
  }),
}));

vi.mock("@/lib/invoke", () => ({ invoke: mocks.invoke }));
vi.mock("@/lib/backend/api", () => ({ emit: mocks.emit }));
vi.mock("@/lib/backend/runtime", () => ({ runtime: "desktop" }));
vi.mock("@/lib/platform", () => ({ isWindows: true }));
vi.mock("@/lib/backend/platform/window", () => ({
  getCurrentWindow: () => ({ close: mocks.close }),
}));
vi.mock("@/components/layout/ChildWindowHeader", () => ({
  default: ({ title }: { title: string }) => <div>{title}</div>,
}));
vi.mock("@/components/quick-commands/QuickCommandEditor", () => ({
  default: ({ value, onChange }: { value: string; onChange: (value: string) => void }) => (
    <textarea
      aria-label="Script"
      value={value}
      onChange={(event) => onChange(event.target.value)}
    />
  ),
}));

describe("QuickCommandPage NyaScript mode", () => {
  beforeEach(() => {
    window.history.replaceState({}, "", "/");
    mocks.invoke.mockReset();
    mocks.emit.mockClear();
    mocks.close.mockClear();
    mocks.invoke.mockImplementation((command: string) => {
      if (command === "get_quick_commands") {
        return Promise.resolve({ commands: [], categories: [] });
      }
      return Promise.resolve(undefined);
    });
  });

  it("saves a multiline NyaScript quick command without changing its script text", async () => {
    const user = userEvent.setup();
    const script = 'sendln "show status"\nwait_regex "ready\\s*>"';
    render(<QuickCommandPage />);

    fireEvent.change(screen.getByLabelText("Label"), { target: { value: "Bootstrap" } });
    fireEvent.change(screen.getByLabelText("Script"), { target: { value: script } });
    await user.click(screen.getByRole("tab", { name: "NyaScript" }));
    expect(screen.getByText("Run a native terminal automation script.")).not.toBeNull();
    await user.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => {
      expect(mocks.invoke).toHaveBeenCalledWith(
        "upsert_quick_command",
        expect.objectContaining({
          command: expect.objectContaining({
            label: "Bootstrap",
            command: script,
            execution_mode: "nyascript",
          }),
        }),
      );
    });
    expect(mocks.emit).toHaveBeenCalledWith(
      "quick-command-saved",
      expect.objectContaining({
        command: expect.objectContaining({ command: script, execution_mode: "nyascript" }),
      }),
    );
  });

  it("preserves NyaScript mode when editing a synced command", async () => {
    const command: QuickCommand = {
      id: "script-1",
      label: "Bootstrap",
      command: 'use current\nwait "ready>"',
      execution_mode: "nyascript",
    };
    window.history.replaceState(
      {},
      "",
      `/?data=${encodeURIComponent(JSON.stringify(command))}`,
    );

    render(<QuickCommandPage />);
    expect((screen.getByLabelText("Script") as HTMLTextAreaElement).value).toBe(command.command);
    expect(screen.getByText("Run a native terminal automation script.")).not.toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => {
      expect(mocks.invoke).toHaveBeenCalledWith(
        "upsert_quick_command",
        expect.objectContaining({
          command: expect.objectContaining({
            id: "script-1",
            command: command.command,
            execution_mode: "nyascript",
          }),
        }),
      );
    });
  });
});
