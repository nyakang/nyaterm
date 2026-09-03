import { execFileSync } from "node:child_process";
import { copyFileSync, mkdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const manifest = join(root, "src-tauri", "crates", "nyaterm-mcp", "Cargo.toml");
const targetDir = join(root, "src-tauri", "crates", "nyaterm-mcp", "target");
const rustcInfo = execFileSync("rustc", ["-vV"], { encoding: "utf8" });
const host = rustcInfo.match(/^host:\s*(.+)$/m)?.[1]?.trim();
const target =
  process.env.NYATERM_MCP_TARGET ||
  process.env.TAURI_ENV_TARGET_TRIPLE ||
  process.env.CARGO_BUILD_TARGET ||
  host;
if (!target)
  throw new Error("Unable to determine the Rust target triple for nyaterm-mcp");

const buildStd = process.env.NYATERM_MCP_BUILD_STD;

const args = [
  "build",
  "--release",
  "--manifest-path",
  manifest,
  "--target-dir",
  targetDir,
];
if (target !== host) args.push("--target", target);
if (buildStd) args.push("-Z", `build-std=${buildStd}`);
execFileSync("cargo", args, { cwd: root, stdio: "inherit" });

const windowsTarget = target.includes("windows");
const executable = windowsTarget ? "nyaterm-mcp.exe" : "nyaterm-mcp";
const source = join(
  targetDir,
  target !== host ? target : "",
  "release",
  executable,
);
const suffix = windowsTarget ? ".exe" : "";
const destination = join(
  root,
  "src-tauri",
  "binaries",
  `nyaterm-mcp-${target}${suffix}`,
);
mkdirSync(dirname(destination), { recursive: true });
copyFileSync(source, destination);
