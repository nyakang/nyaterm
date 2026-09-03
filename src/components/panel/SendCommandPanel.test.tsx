import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { SyncGroup } from "@/types/global";
import SendCommandPanel from "./SendCommandPanel";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (_key: string, fallback?: string, options?: Record<string, string | number>) => {
      if (!fallback) return _key;
      return Object.entries(options ?? {}).reduce(
        (label, [name, value]) => label.replace(`{{${name}}}`, String(value)),
        fallback,
      );
    },
  }),
}));

const sessionA = {
  id: "session-a",
  name: "Session A",
  tabName: "Session A",
  type: "SSH" as const,
  ownerWindowLabel: "main",
};

const sessionB = {
  id: "session-b",
  name: "Session B",
  tabName: "Session B",
  type: "SSH" as const,
  ownerWindowLabel: "main",
};

const syncGroup: SyncGroup = {
  id: "group-a",
  name: "Group A",
  color: "blue",
  sessionIds: [sessionA.id],
  pausedSessionIds: [],
  enabled: true,
};

interface PanelProps {
  currentShellSessionId: string | null;
  shellSessionIds: string[];
  sessionTargets: Array<typeof sessionA>;
  syncGroups: SyncGroup[];
  draft?: {
    text: string;
    sourceSessionId: string | null;
    sourceSessionType: "SSH";
    dataType: "text";
    sendMode: "line";
    count: number;
    intervalSeconds: number;
    target: "current" | "all" | "allWindows";
  };
}

function panel(overrides: Partial<PanelProps> = {}) {
  const props: PanelProps = {
    currentShellSessionId: sessionA.id,
    shellSessionIds: [sessionA.id],
    sessionTargets: [sessionA],
    syncGroups: [],
    ...overrides,
  };

  return (
    <SendCommandPanel
      serialSessionId={null}
      currentShellSessionId={props.currentShellSessionId}
      shellSessionIds={props.shellSessionIds}
      syncGroups={props.syncGroups}
      currentWindowLabel="main"
      sessionTargets={props.sessionTargets}
      clearAfterSend={false}
      draft={props.draft}
      onClearAfterSendChange={vi.fn()}
    />
  );
}

function targetTrigger(): HTMLButtonElement {
  return screen.getByRole("button", { name: "Target", hidden: true });
}

async function expectCurrentTarget() {
  await waitFor(() => expect(targetTrigger().textContent).toContain("Current session"));
}

describe("SendCommandPanel target selection", () => {
  it("keeps Current session selected while the next active session is connecting", async () => {
    const view = render(panel());
    const commandInput = screen.getByPlaceholderText(/Enter text to send/u);
    fireEvent.change(commandInput, { target: { value: "pwd" } });

    view.rerender(
      panel({
        currentShellSessionId: null,
        shellSessionIds: [sessionA.id],
      }),
    );

    await expectCurrentTarget();
    expect((screen.getByTitle("Send") as HTMLButtonElement).disabled).toBe(true);

    view.rerender(
      panel({
        currentShellSessionId: sessionB.id,
        shellSessionIds: [sessionA.id, sessionB.id],
        sessionTargets: [sessionA, sessionB],
      }),
    );

    await expectCurrentTarget();
    expect((screen.getByTitle("Send") as HTMLButtonElement).disabled).toBe(false);
  });

  it("falls back from an unavailable all-window target to Current session", async () => {
    const draft = {
      text: "pwd",
      sourceSessionId: sessionA.id,
      sourceSessionType: "SSH" as const,
      dataType: "text" as const,
      sendMode: "line" as const,
      count: 1,
      intervalSeconds: 1,
      target: "allWindows" as const,
    };
    const view = render(panel({ draft }));

    await waitFor(() => expect(targetTrigger().textContent).toContain("All window sessions"));
    view.rerender(panel({ draft, sessionTargets: [] }));

    await expectCurrentTarget();
  });

  it("falls back from an unavailable sync group to Current session", async () => {
    const user = userEvent.setup();
    const view = render(panel({ syncGroups: [syncGroup] }));

    await user.click(targetTrigger());
    await user.click(await screen.findByText("Group: Group A (1)"));
    await waitFor(() => expect(targetTrigger().textContent).toContain("Group: Group A (1)"));

    view.rerender(panel({ syncGroups: [] }));

    await expectCurrentTarget();
  });

  it("falls back from an unavailable explicit session to Current session", async () => {
    const user = userEvent.setup();
    const view = render(panel());

    await user.click(targetTrigger());
    await user.keyboard("{End}{ArrowRight}{Enter}");
    await waitFor(() => expect(targetTrigger().textContent).toContain("Session A · SSH"));

    view.rerender(panel({ sessionTargets: [] }));

    await expectCurrentTarget();
  });
});
