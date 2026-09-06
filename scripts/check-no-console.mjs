import { readdir, readFile } from "node:fs/promises";
import path from "node:path";

const ROOT = process.cwd();
const SRC_DIR = path.join(ROOT, "src");
const ALLOWED_FILE = path.normalize(path.join(SRC_DIR, "lib", "logger.ts"));
// The web shims must not import the app logger (it would re-enter the Tauri
// module graph), so they are allowed to use console directly.
const ALLOWED_DIR = path.normalize(path.join(SRC_DIR, "lib", "web"));
const FILE_EXTENSIONS = new Set([".ts", ".tsx"]);
const CONSOLE_PATTERN = /\bconsole\.(debug|info|warn|error|log)\s*\(/g;

const violations = [];

async function walk(directory) {
  const entries = await readdir(directory, { withFileTypes: true });

  for (const entry of entries) {
    const resolved = path.join(directory, entry.name);
    if (entry.isDirectory()) {
      await walk(resolved);
      continue;
    }

    if (!FILE_EXTENSIONS.has(path.extname(entry.name))) {
      continue;
    }

    if (path.normalize(resolved) === ALLOWED_FILE) {
      continue;
    }

    if (path.normalize(resolved).startsWith(`${ALLOWED_DIR}${path.sep}`)) {
      continue;
    }

    const content = await readFile(resolved, "utf8");
    const lines = content.split(/\r?\n/);

    lines.forEach((line, index) => {
      if (CONSOLE_PATTERN.test(line)) {
        violations.push(`${path.relative(ROOT, resolved)}:${index + 1}: ${line.trim()}`);
      }
      CONSOLE_PATTERN.lastIndex = 0;
    });
  }
}

await walk(SRC_DIR);

if (violations.length > 0) {
  console.error("Unexpected console.* usage outside src/lib/logger.ts:");
  for (const violation of violations) {
    console.error(`  ${violation}`);
  }
  process.exitCode = 1;
}
