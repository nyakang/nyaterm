// xterm parser bridge for the Windows ConPTY regression test. The Rust test
// feeds the same filtered stream that the desktop renderer receives.
import readline from "node:readline";
import xterm from "@xterm/xterm";

const terminal = new xterm.Terminal({ cols: 80, rows: 24, scrollback: 1000 });
let responses = [];
terminal.onData((data) => responses.push(data));

for await (const line of readline.createInterface({ input: process.stdin })) {
  const request = JSON.parse(line);
  if (request.cols) terminal.resize(request.cols, 24);
  await new Promise((resolve) => terminal.write(request.data ?? "", resolve));
  const buffer = terminal.buffer.active;
  const lines = [];
  for (let i = 0; i < buffer.length; i++) {
    lines.push(buffer.getLine(i)?.translateToString(true) ?? "");
  }
  process.stdout.write(
    `${JSON.stringify({ responses, lines, x: buffer.cursorX, y: buffer.cursorY, baseY: buffer.baseY })}\n`,
  );
  responses = [];
}
