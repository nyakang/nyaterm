import { describe, expect, it } from "vitest";
import {
  buildReconnectCwdCommand,
  buildReconnectCwdStartupCommand,
  carryOverSessionCwd,
  getSessionCwd,
  recordSessionCwd,
} from "./terminalSessionCwd";

describe("buildReconnectCwdCommand", () => {
  it("returns null for an empty string", () => {
    expect(buildReconnectCwdCommand("")).toBeNull();
  });

  it("returns null for a blank string", () => {
    expect(buildReconnectCwdCommand("   ")).toBeNull();
  });

  it("returns null for paths with control characters", () => {
    expect(buildReconnectCwdCommand("/tmp/a\nb")).toBeNull();
    expect(buildReconnectCwdCommand("/tmp/a\rb")).toBeNull();
    expect(buildReconnectCwdCommand("/tmp/a\0b")).toBeNull();
  });

  it("returns null for C0 controls beyond \\r\\n\\0", () => {
    expect(buildReconnectCwdCommand("/tmp/\x15touch /tmp/pwn #")).toBeNull();
    expect(buildReconnectCwdCommand("/tmp/a\x01b")).toBeNull();
    expect(buildReconnectCwdCommand("/tmp/a\x1bb")).toBeNull();
    expect(buildReconnectCwdCommand("/tmp/a\tb")).toBeNull();
    expect(buildReconnectCwdCommand("/tmp/a\x1f b")).toBeNull();
  });

  it("returns null for DEL and C1 control characters", () => {
    expect(buildReconnectCwdCommand("/tmp/a\x7fb")).toBeNull();
    expect(buildReconnectCwdCommand("/tmp/a\x9fb")).toBeNull();
    expect(buildReconnectCwdCommand("/tmp/a\x85b")).toBeNull();
  });

  it("wraps a plain path in single quotes", () => {
    expect(buildReconnectCwdCommand("/home/user")).toBe("cd '/home/user'");
  });

  it("keeps spaces inside the quotes", () => {
    expect(buildReconnectCwdCommand("/home/my dir")).toBe("cd '/home/my dir'");
  });

  it("escapes single quotes the POSIX way", () => {
    expect(buildReconnectCwdCommand("/home/o'brien")).toBe("cd '/home/o'\\''brien'");
  });

  it("keeps dollar signs as literals", () => {
    expect(buildReconnectCwdCommand("/home/a$b")).toBe("cd '/home/a$b'");
  });

  it("keeps backslashes as literals", () => {
    expect(buildReconnectCwdCommand("/home/a\\b")).toBe("cd '/home/a\\b'");
  });

  it("supports unicode paths", () => {
    expect(buildReconnectCwdCommand("/home/用户")).toBe("cd '/home/用户'");
  });

  it("decodes percent-encoded paths before replay", () => {
    expect(buildReconnectCwdCommand("/opt/my%20dir")).toBe("cd '/opt/my dir'");
  });

  it("decodes percent-encoded unicode paths before replay", () => {
    expect(buildReconnectCwdCommand("/home/%E7%94%A8%E6%88%B7")).toBe(
      "cd '/home/用户'",
    );
  });

  it("keeps a raw path when a percent sequence is malformed", () => {
    expect(buildReconnectCwdCommand("/opt/100%")).toBe("cd '/opt/100%'");
    expect(buildReconnectCwdCommand("/opt/a%zzb")).toBe("cd '/opt/a%zzb'");
  });

  it("preserves literal percent sequences from the NyaTerm emitter", () => {
    expect(buildReconnectCwdCommand("/opt/100%25")).toBe("cd '/opt/100%'");
    expect(buildReconnectCwdCommand("/opt/my%2520dir")).toBe(
      "cd '/opt/my%20dir'",
    );
  });

  it("returns null when decoding leaves a blank value", () => {
    expect(buildReconnectCwdCommand("%20")).toBeNull();
  });

  it("escapes quotes revealed by percent-decoding", () => {
    expect(buildReconnectCwdCommand("/home/o%27brien")).toBe(
      "cd '/home/o'\\''brien'",
    );
  });

  it("returns null for control characters revealed by percent-decoding", () => {
    expect(buildReconnectCwdCommand("/tmp/a%00b")).toBeNull();
    expect(buildReconnectCwdCommand("/tmp/a%0ab")).toBeNull();
    expect(buildReconnectCwdCommand("/tmp/a%0db")).toBeNull();
    expect(buildReconnectCwdCommand("/tmp/a%15b")).toBeNull();
    expect(buildReconnectCwdCommand("/tmp/a%7fb")).toBeNull();
    expect(buildReconnectCwdCommand("/tmp/a%C2%85b")).toBeNull();
  });
});

describe("recordSessionCwd / getSessionCwd", () => {
  it("round-trips a recorded cwd without consuming it", () => {
    recordSessionCwd("cwd-roundtrip", "/var/log");
    expect(getSessionCwd("cwd-roundtrip")).toBe("/var/log");
    expect(getSessionCwd("cwd-roundtrip")).toBe("/var/log");
  });

  it("removes the record when the payload is empty", () => {
    recordSessionCwd("cwd-clear", "/var/log");
    recordSessionCwd("cwd-clear", "");
    expect(getSessionCwd("cwd-clear")).toBeNull();
  });

  it("removes the record when the payload is blank", () => {
    recordSessionCwd("cwd-clear-blank", "/var/log");
    recordSessionCwd("cwd-clear-blank", "   ");
    expect(getSessionCwd("cwd-clear-blank")).toBeNull();
  });

  it("returns null for an unknown session", () => {
    expect(getSessionCwd("cwd-unknown")).toBeNull();
  });
});

describe("carryOverSessionCwd", () => {
  it("copies the cwd to a session without a record", () => {
    recordSessionCwd("carry-from", "/opt");
    carryOverSessionCwd("carry-from", "carry-to");
    expect(getSessionCwd("carry-to")).toBe("/opt");
  });

  it("keeps the target session's own cwd", () => {
    recordSessionCwd("carry-from-own", "/opt");
    recordSessionCwd("carry-to-own", "/srv");
    carryOverSessionCwd("carry-from-own", "carry-to-own");
    expect(getSessionCwd("carry-to-own")).toBe("/srv");
  });

  it("does nothing when the source has no record", () => {
    carryOverSessionCwd("carry-from-missing", "carry-to-missing");
    expect(getSessionCwd("carry-to-missing")).toBeNull();
  });

  it("does nothing for identical session ids", () => {
    recordSessionCwd("carry-same", "/opt");
    carryOverSessionCwd("carry-same", "carry-same");
    expect(getSessionCwd("carry-same")).toBe("/opt");
  });
});

describe("buildReconnectCwdStartupCommand", () => {
  it("returns undefined when no cwd was recorded", () => {
    expect(buildReconnectCwdStartupCommand("startup-unknown", 1000)).toBeUndefined();
  });

  it("returns undefined when the recorded cwd cannot be replayed", () => {
    recordSessionCwd("startup-invalid", "   ");
    expect(buildReconnectCwdStartupCommand("startup-invalid", 1000)).toBeUndefined();
  });

  it("builds the startup command with the passed delay", () => {
    recordSessionCwd("startup-ok", "/opt/app");
    expect(buildReconnectCwdStartupCommand("startup-ok", 1000)).toEqual({
      command: "cd '/opt/app'",
      delayMs: 1000,
    });
  });

  it("passes a zero delay through unchanged", () => {
    recordSessionCwd("startup-zero-delay", "/opt/app");
    expect(buildReconnectCwdStartupCommand("startup-zero-delay", 0)).toEqual({
      command: "cd '/opt/app'",
      delayMs: 0,
    });
  });
});
