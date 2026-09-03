import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn().mockResolvedValue(undefined),
}));
vi.mock("./invoke", () => ({ invoke: mocks.invoke }));

import { sendSessionInput, sendSessionInputWithSync } from "./sessionInput";

describe("sendSessionInputWithSync command confirmation", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("forwards terminal responses without previews or sync fan-out", async () => {
    await sendSessionInput("primary", "\x1b[?1;2c", {
      preview: null,
      origin: "terminal_response",
    });

    expect(mocks.invoke).toHaveBeenCalledTimes(1);
    expect(mocks.invoke).toHaveBeenCalledWith("write_to_session", {
      sessionId: "primary",
      data: "\x1b[?1;2c",
      origin: "terminal_response",
      sensitivity: undefined,
    });
  });

  it("registers each peer candidate before synchronized command input", async () => {
    await sendSessionInputWithSync(
      "primary",
      "uptime\r",
      ["peer-a", "peer-b"],
      {
        registerSubmission: "uptime",
        origin: "keyboard",
      },
    );

    for (const sessionId of ["primary", "peer-a", "peer-b"]) {
      const expectedRegistration =
        sessionId === "primary"
          ? "register_command_submission"
          : "register_command_confirmation_candidate";
      const registerIndex = mocks.invoke.mock.calls.findIndex(
        ([command, payload]) =>
          command === expectedRegistration && payload.sessionId === sessionId,
      );
      const writeIndex = mocks.invoke.mock.calls.findIndex(
        ([command, payload]) =>
          command === "write_to_session" && payload.sessionId === sessionId,
      );
      expect(registerIndex).toBeGreaterThanOrEqual(0);
      expect(writeIndex).toBeGreaterThan(registerIndex);
    }

    expect(mocks.invoke).toHaveBeenCalledWith("write_to_session", {
      sessionId: "peer-a",
      data: "uptime\r",
      origin: "sync_input",
      sensitivity: undefined,
    });
  });

  it("does not block peer input when auxiliary registration fails", async () => {
    mocks.invoke.mockImplementation(async (command: string, payload) => {
      if (
        command === "register_command_confirmation_candidate" &&
        payload.sessionId === "peer"
      ) {
        throw new Error("registration unavailable");
      }
    });

    await sendSessionInputWithSync("primary", "uptime\r", ["peer"], {
      registerSubmission: "uptime",
    });

    expect(mocks.invoke).toHaveBeenCalledWith("write_to_session", {
      sessionId: "peer",
      data: "uptime\r",
      origin: "sync_input",
      sensitivity: undefined,
    });
  });

  it("keeps non-command synchronized input registration-free", async () => {
    await sendSessionInputWithSync("primary", "x", ["peer"], {
      origin: "keyboard",
    });

    expect(mocks.invoke).not.toHaveBeenCalledWith(
      "register_command_submission",
      expect.anything(),
    );
    expect(mocks.invoke).not.toHaveBeenCalledWith(
      "register_command_confirmation_candidate",
      expect.anything(),
    );
  });
});
