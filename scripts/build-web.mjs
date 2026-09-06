// Cross-platform web frontend build: sets NYATERM_WEB_BUILD so vite swaps
// the Tauri API packages for the web shims, then runs `vite build`.
// Skips tsc on purpose: `pnpm build` type-checks the desktop code paths, and
// the web build compiles the same sources with module-resolution swapped.
import { spawnSync } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
process.env.NYATERM_WEB_BUILD = "1";

const viteBin = path.join(repoRoot, "node_modules", ".bin", "vite");
const result = spawnSync(viteBin, ["build"], {
  stdio: "inherit",
  cwd: repoRoot,
  shell: process.platform === "win32",
  env: process.env,
});

process.exit(result.status ?? 1);
