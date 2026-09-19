#!/usr/bin/env node
/* global process, console */
/**
 * Make @xterm/addon-webgl rasterize Braille Patterns (U+2800-U+28FF) on a grid
 * that stays uniform across cell boundaries.
 *
 * The addon draws braille itself (`drawBrailleCharacter`) instead of using a
 * font glyph, which is what NyaTerm wants: a font's braille ink only covers
 * ~58% of the line height, so TUI art looks like it has extra line spacing.
 * Upstream insets the dot grid by 10% of the cell height though:
 *
 *   paddingY    = cellHeight * 0.1
 *   yEighth     = cellHeight * 0.8 / 8
 *   dot row y   = paddingY + (dotY + 1) * yEighth   -> 20% / 40% / 60% / 80%
 *
 * That leaves a 10% margin inside every single line, so the gap between the
 * last dot row of one line and the first dot row of the next is larger than the
 * gap between dot rows inside a line - the eye reads it as a line gap.
 *
 * Fix: drop the vertical padding so the four dot rows sit at 1/8, 3/8, 5/8 and
 * 7/8 of the cell. Neighbouring lines then continue the same grid (pitch =
 * cellHeight / 4) and art joins seamlessly, matching WezTerm's own braille
 * rasterization. The dot radius is unchanged (min(cellWidth/8, cellHeight/8)).
 *
 * `scripts/generate-symbol-font.mjs` uses the same grid for the font that
 * covers the DOM renderer, which has no custom glyph support at all.
 *
 * This script is intended for:
 *   @xterm/addon-webgl@0.20.0-beta.287
 *
 * It is idempotent and fails fast when the installed package or the minified
 * bundles no longer match the expected version, so dependency upgrades cannot
 * silently reintroduce the gap.
 */

"use strict";

const fs = require("node:fs");
const path = require("node:path");

const PACKAGE_NAME = "@xterm/addon-webgl";
const EXPECTED_VERSION = "0.20.0-beta.287";
const MARKER = "/*nyaterm:braille-grid*/";

const PACKAGE_DIR = path.resolve(
  process.cwd(),
  "node_modules",
  "@xterm",
  "addon-webgl",
);

const PACKAGE_JSON = path.join(PACKAGE_DIR, "package.json");

const TARGETS = [
  {
    file: path.join(PACKAGE_DIR, "lib", "addon-webgl.mjs"),
    // @xterm/addon-webgl@0.20.0-beta.287 ESM bundle, inside drawBrailleCharacter
    search: "let o=s/8,n=a*.1,T=a*.8/8,h=Math.min(o,T);",
    replace: "let o=s/8,n=0,T=a/8,h=Math.min(o,T);" + MARKER,
  },
  {
    file: path.join(PACKAGE_DIR, "lib", "addon-webgl.js"),
    // @xterm/addon-webgl@0.20.0-beta.287 CJS bundle, inside the braille case
    search: "const n=r/8,L=.1*a,h=.8*a/8,T=Math.min(n,h);",
    replace: "const n=r/8,L=0,h=a/8,T=Math.min(n,h);" + MARKER,
  },
];

function fail(message) {
  throw new Error(`[patch-xterm-braille-grid] ${message}`);
}

function writeFileAtomically(filename, content) {
  const temporary = `${filename}.nyaterm-patch-${process.pid}.tmp`;
  fs.writeFileSync(temporary, content, "utf8");
  fs.renameSync(temporary, filename);
}

if (!fs.existsSync(PACKAGE_JSON)) {
  fail(`${PACKAGE_NAME} is not installed: ${PACKAGE_JSON}`);
}

let packageJson;
try {
  packageJson = JSON.parse(fs.readFileSync(PACKAGE_JSON, "utf8"));
} catch (error) {
  fail(
    `cannot read ${PACKAGE_JSON}: ${
      error instanceof Error ? error.message : String(error)
    }`,
  );
}

if (packageJson.version !== EXPECTED_VERSION) {
  fail(
    `unsupported ${PACKAGE_NAME} version ${JSON.stringify(packageJson.version)}; ` +
      `expected ${EXPECTED_VERSION}. Review and refresh this patch before upgrading.`,
  );
}

let patched = 0;
let already = 0;

for (const { file, search, replace } of TARGETS) {
  const relative = path.relative(process.cwd(), file);

  if (!fs.existsSync(file)) {
    fail(`runtime bundle not found: ${relative}`);
  }

  let source;
  try {
    source = fs.readFileSync(file, "utf8");
  } catch (error) {
    fail(
      `cannot read ${relative}: ${
        error instanceof Error ? error.message : String(error)
      }`,
    );
  }

  if (source.includes(MARKER)) {
    already += 1;
    continue;
  }

  const occurrences = source.split(search).length - 1;
  if (occurrences !== 1) {
    fail(
      `expected exactly one braille draw call in ${relative}, found ${occurrences}. ` +
        "The minified addon bundle may have changed.",
    );
  }

  const patchedSource = source.replace(search, replace);

  if (!patchedSource.includes(MARKER)) {
    fail(`patch verification failed for ${relative}`);
  }

  try {
    writeFileAtomically(file, patchedSource);
  } catch (error) {
    fail(
      `cannot write ${relative}: ${
        error instanceof Error ? error.message : String(error)
      }`,
    );
  }

  patched += 1;
}

console.log(
  `[patch-xterm-braille-grid] uniform braille grid: ` +
    `patched=${patched} already=${already} total=${TARGETS.length}`,
);
