#!/usr/bin/env node
/* global process, console */
/**
 * Generate `public/fonts/nyaterm-symbols.ttf`.
 *
 * Why this font exists
 * --------------------
 * Terminal graphics characters must ink the *whole* character cell. TUI art
 * (sparklines, chafa/catimg output, braille plots, mosaic graphics) is drawn as
 * a grid of sub-cells, so any mismatch between the glyph's ink box and the cell
 * shows up as gaps between rows - the "phantom line spacing" that makes such art
 * look broken.
 *
 *   - WezTerm gets this right by synthesizing the glyphs itself
 *     (`custom_block_glyphs`, default on) instead of using a font glyph.
 *   - The WebGL renderer of xterm.js does the same: `@xterm/addon-webgl`
 *     rasterizes these ranges with its own `CustomGlyphDefinitions`.
 *   - The DOM renderer of xterm.js has no custom glyph support at all, so it
 *     falls back to the font. Ordinary monospace fonts only ink ~58% of the line
 *     height for braille (Maple Mono NF CN: 0.376em x 0.772em inside a
 *     0.6em x 1.32em cell) and cover no Legacy Computing characters at all.
 *
 * This script emits a symbols-only TrueType font covering the two ranges that
 * fonts do not handle, with the same geometry the WebGL addon rasterizes so both
 * renderers look identical:
 *
 *   1. Braille Patterns (U+2800-U+28FF)                 - 2x4 dot grid
 *   2. Symbols for Legacy Computing (U+1FB00.., subset)  - sextants, one-eighth
 *      blocks, triangular quarters/three-quarters
 *
 * It is intentionally dependency free (no fontTools/opentype.js):
 *
 *   node scripts/generate-symbol-font.mjs
 *
 * Geometry: 1em = 1000 units, advance 600 (0.6em), ascender 1020, descender -300
 * (a 0.6em x 1.32em cell). Sub-cell grid coordinates are expressed as fractions
 * of that cell and rounded to whole units, matching `@xterm/addon-webgl`:
 *   - braille  : dot columns x=1/4,3/4, dot rows y=1/8,3/8,5/8,7/8, radius 1/8
 *                of the cell width
 *   - sextants : 2 columns (1/2 each) x 3 rows (3/8, 2/8, 3/8)
 *   - eighths  : 1320/8 = 165 tall, 600/8 = 75 wide
 *   - triangles: base on a cell edge, apex at the cell centre (a quarter of the
 *                cell area), and their three-quarter complements
 *
 * Not covered (their exact geometry is not derivable offline without the official
 * code chart, and shipping guessed shapes is worse than a fallback): diagonal
 * blocks U+1FB3C-U+1FB67, medium-shade halves U+1FB8C-U+1FB94, checker boards
 * U+1FB95-U+1FB96 and diagonal strokes/arrows/pictographs U+1FB97-U+1FBCA. Those
 * fall back to the system font (which normally has no glyph either), while the
 * WebGL path still synthesizes U+1FB00-U+1FB3B and U+1FBF0-U+1FBF9.
 *
 * See `src/lib/terminalSymbolFont.ts` for the runtime wiring.
 */

"use strict";

import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const UNITS_PER_EM = 1000;
const ADVANCE_WIDTH = 600;
const ASCENDER = 1020;
const DESCENDER = -300;
const CELL_WIDTH = ADVANCE_WIDTH;
const CELL_HEIGHT = ASCENDER - DESCENDER;

const BRAILLE_FIRST = 0x2800;
const BRAILLE_COUNT = 256;
const BRAILLE_LAST = BRAILLE_FIRST + BRAILLE_COUNT - 1;

// Glyph order: 0 = .notdef, 1 = space, 2 = no-break space, then the symbol ranges.
const SPACE_GLYPH_INDEX = 1;
const NBSP_GLYPH_INDEX = 2;
const FIRST_SYMBOL_GLYPH_INDEX = 3;
const CIRCLE_K = 0.9142135623730951;

/** Maps a cell fraction (0..1, y measured from the cell top) to font units. */
const cellX = (fraction) => Math.round(CELL_WIDTH * fraction);
const cellY = (fraction) => Math.round(ASCENDER - CELL_HEIGHT * fraction);
/** Same, but for legacy-computing eighths of the cell (2 = 2/8 of the cell). */
const eighthX = (eighths) => cellX(eighths / 8);
const eighthY = (eighths) => cellY(eighths / 8);

// Braille bit -> [columnIndex, rowIndex]; bit order follows Unicode (bit 0 = dot 1).
const BRAILLE_DOT_GRID = [
  [0, 0],
  [0, 1],
  [0, 2],
  [1, 0],
  [1, 1],
  [1, 2],
  [0, 3],
  [1, 3],
];

/**
 * Symbols for Legacy Computing, block sextants U+1FB00-U+1FB3B.
 * Bit order (as in @xterm/addon-webgl): bit 0 = top-left, bit 1 = top-right,
 * bit 2 = middle-left, bit 3 = middle-right, bit 4 = bottom-left,
 * bit 5 = bottom-right. Patterns transcribed from
 * @xterm/addon-webgl CustomGlyphDefinitions.ts (MIT, (c) the xterm.js authors).
 */
const SEXTANT_PATTERNS = {
  0x1fb00: 0b000001,
  0x1fb01: 0b000010,
  0x1fb02: 0b000011,
  0x1fb03: 0b000100,
  0x1fb04: 0b000101,
  0x1fb05: 0b000110,
  0x1fb06: 0b000111,
  0x1fb07: 0b001000,
  0x1fb08: 0b001001,
  0x1fb09: 0b001010,
  0x1fb0a: 0b001011,
  0x1fb0b: 0b001100,
  0x1fb0c: 0b001101,
  0x1fb0d: 0b001110,
  0x1fb0e: 0b001111,
  0x1fb0f: 0b010000,
  0x1fb10: 0b010001,
  0x1fb11: 0b010010,
  0x1fb12: 0b010011,
  0x1fb13: 0b010100,
  0x1fb14: 0b010110,
  0x1fb15: 0b010111,
  0x1fb16: 0b011000,
  0x1fb17: 0b011001,
  0x1fb18: 0b011010,
  0x1fb19: 0b011011,
  0x1fb1a: 0b011100,
  0x1fb1b: 0b011101,
  0x1fb1c: 0b011110,
  0x1fb1d: 0b011111,
  0x1fb1e: 0b100000,
  0x1fb1f: 0b100001,
  0x1fb20: 0b100010,
  0x1fb21: 0b100011,
  0x1fb22: 0b100100,
  0x1fb23: 0b100101,
  0x1fb24: 0b100110,
  0x1fb25: 0b100111,
  0x1fb26: 0b101000,
  0x1fb27: 0b101001,
  0x1fb28: 0b101011,
  0x1fb29: 0b101100,
  0x1fb2a: 0b101101,
  0x1fb2b: 0b101110,
  0x1fb2c: 0b101111,
  0x1fb2d: 0b110000,
  0x1fb2e: 0b110001,
  0x1fb2f: 0b110010,
  0x1fb30: 0b110011,
  0x1fb31: 0b110100,
  0x1fb32: 0b110101,
  0x1fb33: 0b110110,
  0x1fb34: 0b110111,
  0x1fb35: 0b111000,
  0x1fb36: 0b111001,
  0x1fb37: 0b111010,
  0x1fb38: 0b111011,
  0x1fb39: 0b111100,
  0x1fb3a: 0b111101,
  0x1fb3b: 0b111110,
};

/* ---------------------------------------------------------------- contours */

/**
 * Quadratic circle approximation; control points sit at k*r on the diagonal.
 * Contour order is counter clockwise in font coordinates.
 */
function circleContour(cx, cy, r) {
  const k = r * CIRCLE_K;
  const points = [];
  const push = (x, y, on) =>
    points.push({ x: Math.round(x), y: Math.round(y), on });
  push(cx + r, cy, true);
  push(cx + k, cy + k, false);
  push(cx, cy + r, true);
  push(cx - k, cy + k, false);
  push(cx - r, cy, true);
  push(cx - k, cy - k, false);
  push(cx, cy - r, true);
  push(cx + k, cy - k, false);
  return points;
}

/** Straight-on-curve-only contour from [x, y] pairs (font units). */
function polygonContour(vertices) {
  return vertices.map(([x, y]) => ({ x: Math.round(x), y: Math.round(y), on: true }));
}

function rectContour(left, top, right, bottom) {
  return polygonContour([
    [left, top],
    [right, top],
    [right, bottom],
    [left, bottom],
  ]);
}

/** A rectangle in cell fractions: `x0/y0/x1/y1` are 0..1 with y from the top. */
function cellRect(x0, y0, x1, y1) {
  return rectContour(cellX(x0), cellY(y0), cellX(x1), cellY(y1));
}

function glyphFromContours(contours) {
  const points = contours.flat();
  if (points.length === 0) {
    return { contours: [], points: [], xMin: 0, yMin: 0, xMax: 0, yMax: 0 };
  }
  const xs = points.map((point) => point.x);
  const ys = points.map((point) => point.y);
  return {
    contours,
    points,
    xMin: Math.min(...xs),
    yMin: Math.min(...ys),
    xMax: Math.max(...xs),
    yMax: Math.max(...ys),
  };
}

function notdefGlyph() {
  // Hollow box: outer contour counter clockwise, inner contour clockwise.
  return glyphFromContours([
    rectContour(60, 780, 540, -40),
    polygonContour([
      [120, 720],
      [120, 20],
      [480, 20],
      [480, 720],
    ]),
  ]);
}

/* ------------------------------------------------------------ glyph builders */

function brailleGlyph(pattern) {
  const contours = [];
  for (let bit = 0; bit < 8; bit += 1) {
    if ((pattern & (1 << bit)) === 0) continue;
    const [columnIndex, rowIndex] = BRAILLE_DOT_GRID[bit];
    const cx = cellX(0.25 + columnIndex * 0.5);
    const cy = cellY(0.125 + rowIndex * 0.25);
    contours.push(circleContour(cx, cy, cellX(0.125)));
  }
  return glyphFromContours(contours);
}

/** Block sextant: 2 columns, 3 rows at 3/8, 2/8, 3/8 of the cell height. */
function sextantGlyph(pattern) {
  const rows = [
    [0, 3 / 8],
    [3 / 8, 5 / 8],
    [5 / 8, 1],
  ];
  const contours = [];
  for (let row = 0; row < 3; row += 1) {
    const left = (pattern >> (row * 2)) & 1;
    const right = (pattern >> (row * 2 + 1)) & 1;
    if (!left && !right) continue;
    const [top, bottom] = rows[row];
    const x0 = left ? 0 : 0.5;
    const x1 = right ? 1 : 0.5;
    contours.push(cellRect(x0, top, x1, bottom));
  }
  return glyphFromContours(contours);
}

/** Horizontal one-eighth block at position `n` (1..8), counted from the top. */
function horizontalEighthGlyph(positions) {
  return glyphFromContours(
    positions.map((n) => cellRect(0, (n - 1) / 8, 1, n / 8)),
  );
}

/** Vertical one-eighth block at position `n` (1..8), counted from the left. */
function verticalEighthGlyph(positions) {
  return glyphFromContours(
    positions.map((n) => cellRect((n - 1) / 8, 0, n / 8, 1)),
  );
}

/** Corner pieces: a one-eighth column/row pair (e.g. left + lower). */
function cornerGlyph(half, horizontal) {
  const column = half === "left" ? [0, 1 / 8] : [7 / 8, 1];
  const row = horizontal === "upper" ? [0, 1 / 8] : [7 / 8, 1];
  return glyphFromContours([
    cellRect(column[0], 0, column[1], 1),
    cellRect(0, row[0], 1, row[1]),
  ]);
}

/** Upper `n`/8 block (counted from the top). */
function upperBlockGlyph(eighths) {
  return glyphFromContours([cellRect(0, 0, 1, eighths / 8)]);
}

/** Right `n`/8 block (counted from the right edge). */
function rightBlockGlyph(eighths) {
  return glyphFromContours([cellRect(1 - eighths / 8, 0, 1, 1)]);
}

/**
 * Triangular quarter block: base on the given cell edge, apex at the cell
 * centre, i.e. a quarter of the cell area. With `threeQuarter` the complement is
 * emitted instead (a pentagon with a triangular notch, per the Unicode names).
 */
const TRIANGLE_QUARTERS = {
  upper: [
    [0, 0],
    [1, 0],
    [0.5, 0.5],
  ],
  right: [
    [1, 0],
    [1, 1],
    [0.5, 0.5],
  ],
  lower: [
    [1, 1],
    [0, 1],
    [0.5, 0.5],
  ],
  left: [
    [0, 1],
    [0, 0],
    [0.5, 0.5],
  ],
};

const TRIANGLE_THREE_QUARTERS = {
  upper: [
    [0, 0],
    [0.5, 0.5],
    [1, 0],
    [1, 1],
    [0, 1],
  ],
  right: [
    [1, 0],
    [0.5, 0.5],
    [1, 1],
    [0, 1],
    [0, 0],
  ],
  lower: [
    [1, 1],
    [0.5, 0.5],
    [0, 1],
    [0, 0],
    [1, 0],
  ],
  left: [
    [0, 0],
    [1, 0],
    [1, 1],
    [0, 1],
    [0.5, 0.5],
  ],
};

function triangularGlyph(edge, threeQuarter = false) {
  const vertices = threeQuarter ? TRIANGLE_THREE_QUARTERS[edge] : TRIANGLE_QUARTERS[edge];
  return glyphFromContours([
    polygonContour(vertices.map(([x, y]) => [cellX(x), cellY(y)])),
  ]);
}

/**
 * Segmented (seven-segment) digits, U+1FBF0-U+1FBF9. Bit 6..0 = segments a..g,
 * geometry transcribed from @xterm/addon-webgl CustomGlyphDefinitions.ts
 * (MIT, (c) the xterm.js authors) so the DOM renderer matches the WebGL one.
 */
const SEGMENTED_DIGIT_PATTERNS = [
  0b1111110, 0b0110000, 0b1101101, 0b1111001, 0b0110011, 0b1011011, 0b1011111,
  0b1110010, 0b1111111, 0b1111011,
];

function segmentedDigitGlyph(pattern) {
  const segW = 0.15; // width of vertical segments (fraction of the cell width)
  const segH = 0.075; // height of horizontal segments (fraction of the cell height)
  const padX = 0.05;
  const padY = 0.175;
  const gap = 0.015;
  const taperX = segW / 2;
  const taperY = segH / 2;
  const left = padX;
  const right = 1 - padX;
  const top = padY;
  const bottom = 1 - padY;
  const midY = 0.5;

  const contours = [];
  const hexagon = (vertices) => {
    contours.push(
      polygonContour(vertices.map(([x, y]) => [cellX(x), cellY(y)])),
    );
  };

  // a: top horizontal
  if (pattern & 0b1000000) {
    const y1 = top;
    const y2 = top + segH / 2;
    const y3 = top + segH;
    const x1 = left + segW + gap;
    const x2 = right - segW - gap;
    hexagon([
      [x1, y2],
      [x1 + taperX, y1],
      [x2 - taperX, y1],
      [x2, y2],
      [x2 - taperX, y3],
      [x1 + taperX, y3],
    ]);
  }
  // b: upper right vertical
  if (pattern & 0b0100000) {
    const x1 = right - segW;
    const x2 = right - segW / 2;
    const x3 = right;
    const y1 = top + segH + gap;
    const y2 = midY - gap;
    hexagon([
      [x2, y1],
      [x3, y1 + taperY],
      [x3, y2 - taperY],
      [x2, y2],
      [x1, y2 - taperY],
      [x1, y1 + taperY],
    ]);
  }
  // c: lower right vertical
  if (pattern & 0b0010000) {
    const x1 = right - segW;
    const x2 = right - segW / 2;
    const x3 = right;
    const y1 = midY + gap;
    const y2 = bottom - segH - gap;
    hexagon([
      [x2, y1],
      [x3, y1 + taperY],
      [x3, y2 - taperY],
      [x2, y2],
      [x1, y2 - taperY],
      [x1, y1 + taperY],
    ]);
  }
  // d: bottom horizontal
  if (pattern & 0b0001000) {
    const y1 = bottom - segH;
    const y2 = bottom - segH / 2;
    const y3 = bottom;
    const x1 = left + segW + gap;
    const x2 = right - segW - gap;
    hexagon([
      [x1, y2],
      [x1 + taperX, y1],
      [x2 - taperX, y1],
      [x2, y2],
      [x2 - taperX, y3],
      [x1 + taperX, y3],
    ]);
  }
  // e: lower left vertical
  if (pattern & 0b0000100) {
    const x1 = left;
    const x2 = left + segW / 2;
    const x3 = left + segW;
    const y1 = midY + gap;
    const y2 = bottom - segH - gap;
    hexagon([
      [x2, y1],
      [x3, y1 + taperY],
      [x3, y2 - taperY],
      [x2, y2],
      [x1, y2 - taperY],
      [x1, y1 + taperY],
    ]);
  }
  // f: upper left vertical
  if (pattern & 0b0000010) {
    const x1 = left;
    const x2 = left + segW / 2;
    const x3 = left + segW;
    const y1 = top + segH + gap;
    const y2 = midY - gap;
    hexagon([
      [x2, y1],
      [x3, y1 + taperY],
      [x3, y2 - taperY],
      [x2, y2],
      [x1, y2 - taperY],
      [x1, y1 + taperY],
    ]);
  }
  // g: middle horizontal
  if (pattern & 0b0000001) {
    const y1 = midY - segH / 2;
    const y2 = midY;
    const y3 = midY + segH / 2;
    const x1 = left + segW + gap;
    const x2 = right - segW - gap;
    hexagon([
      [x1, y2],
      [x1 + taperX, y1],
      [x2 - taperX, y1],
      [x2, y2],
      [x2 - taperX, y3],
      [x1 + taperX, y3],
    ]);
  }

  return glyphFromContours(contours);
}

/** Builds the codepoint -> glyph list for the Symbols for Legacy Computing block. */
function legacyComputingGlyphs() {
  const entries = [];
  const add = (codepoint, glyph) => entries.push({ codepoint, glyph });

  for (const [key, pattern] of Object.entries(SEXTANT_PATTERNS)) {
    add(Number(key), sextantGlyph(pattern));
  }

  // Triangular quarters (U+1FB6C-U+1FB6F) and their three-quarter complements
  // (U+1FB68-U+1FB6B), in the order the codepoints are assigned.
  add(0x1fb68, triangularGlyph("left", true));
  add(0x1fb69, triangularGlyph("upper", true));
  add(0x1fb6a, triangularGlyph("right", true));
  add(0x1fb6b, triangularGlyph("lower", true));
  add(0x1fb6c, triangularGlyph("left"));
  add(0x1fb6d, triangularGlyph("upper"));
  add(0x1fb6e, triangularGlyph("right"));
  add(0x1fb6f, triangularGlyph("lower"));

  for (let position = 2; position <= 7; position += 1) {
    add(0x1fb70 + (position - 2), verticalEighthGlyph([position]));
  }
  for (let position = 2; position <= 7; position += 1) {
    add(0x1fb76 + (position - 2), horizontalEighthGlyph([position]));
  }
  add(0x1fb7c, cornerGlyph("left", "lower"));
  add(0x1fb7d, cornerGlyph("left", "upper"));
  add(0x1fb7e, cornerGlyph("right", "upper"));
  add(0x1fb7f, cornerGlyph("right", "lower"));
  add(0x1fb80, horizontalEighthGlyph([1, 8]));
  add(0x1fb81, horizontalEighthGlyph([1, 3, 5, 8]));
  for (const [offset, eighths] of [2, 3, 5, 6, 7].entries()) {
    add(0x1fb82 + offset, upperBlockGlyph(eighths));
  }
  for (const [offset, eighths] of [2, 3, 5, 6, 7].entries()) {
    add(0x1fb87 + offset, rightBlockGlyph(eighths));
  }

  // Segmented digits U+1FBF0-U+1FBF9 (order matches the digit values).
  SEGMENTED_DIGIT_PATTERNS.forEach((pattern, digit) => {
    add(0x1fbf0 + digit, segmentedDigitGlyph(pattern));
  });

  return entries.sort((a, b) => a.codepoint - b.codepoint);
}

/* ------------------------------------------------------------------ helpers */

function pad4(buffer) {
  const remainder = buffer.length % 4;
  return remainder === 0
    ? buffer
    : Buffer.concat([buffer, Buffer.alloc(4 - remainder)]);
}

function checksum(buffer) {
  let sum = 0;
  for (let offset = 0; offset < buffer.length; offset += 4) {
    sum = (sum + buffer.readUInt32BE(offset)) >>> 0;
  }
  return sum;
}

/* ------------------------------------------------------------------- tables */

function buildGlyf(glyphs) {
  const chunks = [];
  for (const glyph of glyphs) {
    if (glyph.points.length === 0) {
      chunks.push(Buffer.alloc(0));
      continue;
    }

    const bytes = [];
    const write16 = (value) => bytes.push((value >> 8) & 0xff, value & 0xff);

    write16(glyph.contours.length);
    write16(glyph.xMin);
    write16(glyph.yMin);
    write16(glyph.xMax);
    write16(glyph.yMax);

    let endPointIndex = -1;
    for (const contour of glyph.contours) {
      endPointIndex += contour.length;
      write16(endPointIndex);
    }
    write16(0); // instructionLength

    // Flags with explicit int16 deltas; X_SAME/Y_SAME when the delta is zero.
    let previousX = 0;
    let previousY = 0;
    for (const point of glyph.points) {
      let flag = point.on ? 0x01 : 0x00;
      if (point.x === previousX) flag |= 0x10;
      if (point.y === previousY) flag |= 0x20;
      bytes.push(flag);
      previousX = point.x;
      previousY = point.y;
    }

    // Deltas are only stored for points that are not flagged X_SAME/Y_SAME.
    previousX = 0;
    for (const point of glyph.points) {
      const delta = point.x - previousX;
      previousX = point.x;
      if (delta !== 0) write16(delta);
    }
    previousY = 0;
    for (const point of glyph.points) {
      const delta = point.y - previousY;
      previousY = point.y;
      if (delta !== 0) write16(delta);
    }

    chunks.push(Buffer.from(bytes));
  }
  return chunks;
}

function buildLoca(glyphChunks) {
  const offsets = [0];
  let offset = 0;
  for (const chunk of glyphChunks) {
    offset += chunk.length;
    offsets.push(offset);
  }
  const buffer = Buffer.alloc(offsets.length * 2);
  offsets.forEach((value, index) => buffer.writeUInt16BE(value / 2, index * 2));
  return buffer;
}

function buildHmtx(glyphs) {
  const buffer = Buffer.alloc(glyphs.length * 4);
  glyphs.forEach((glyph, index) => {
    buffer.writeUInt16BE(ADVANCE_WIDTH, index * 4);
    buffer.writeInt16BE(glyph.xMin, index * 4 + 2);
  });
  return buffer;
}

function buildMaxp(glyphCount, maxPoints, maxContours) {
  const buffer = Buffer.alloc(32);
  buffer.writeUInt32BE(0x00010000, 0);
  buffer.writeUInt16BE(glyphCount, 4);
  buffer.writeUInt16BE(maxPoints, 6);
  buffer.writeUInt16BE(maxContours, 8);
  buffer.writeUInt16BE(0, 10); // maxCompositePoints
  buffer.writeUInt16BE(0, 12); // maxCompositeContours
  buffer.writeUInt16BE(1, 14); // maxZones
  for (let offset = 16; offset < 32; offset += 2) {
    buffer.writeUInt16BE(0, offset);
  }
  return buffer;
}

/** Windows BMP format 4 subtable covering `segments` (idDelta only). */
function buildCmapFormat4(segments) {
  const segmentCount = segments.length;
  const segX2 = segmentCount * 2;
  const subtableLength = 14 + segX2 * 4 + 2;

  const sub = Buffer.alloc(subtableLength);
  sub.writeUInt16BE(4, 0);
  sub.writeUInt16BE(subtableLength, 2);
  sub.writeUInt16BE(0, 4); // language
  sub.writeUInt16BE(segX2, 6);
  const entrySelector = Math.floor(Math.log2(segmentCount));
  const searchRange = 2 ** entrySelector * 2;
  sub.writeUInt16BE(searchRange, 8);
  sub.writeUInt16BE(entrySelector, 10);
  sub.writeUInt16BE(segX2 - searchRange, 12);

  const endOffset = 14;
  const startOffset = endOffset + segX2 + 2;
  const deltaOffset = startOffset + segX2;
  const rangeOffset = deltaOffset + segX2;
  segments.forEach((segment, index) => {
    sub.writeUInt16BE(segment.end, endOffset + index * 2);
    sub.writeUInt16BE(segment.start, startOffset + index * 2);
    sub.writeUInt16BE((segment.delta - segment.start) & 0xffff, deltaOffset + index * 2);
    sub.writeUInt16BE(0, rangeOffset + index * 2);
  });
  return sub;
}

/** Format 12 subtable for the full codepoint space (needed for U+1FBxx). */
function buildCmapFormat12(groups) {
  const subtableLength = 16 + groups.length * 12;
  const sub = Buffer.alloc(subtableLength);
  sub.writeUInt16BE(12, 0);
  sub.writeUInt16BE(0, 2); // reserved
  sub.writeUInt32BE(subtableLength, 4);
  sub.writeUInt32BE(0, 8); // language
  sub.writeUInt32BE(groups.length, 12);
  groups.forEach((group, index) => {
    const offset = 16 + index * 12;
    sub.writeUInt32BE(group.start, offset);
    sub.writeUInt32BE(group.end, offset + 4);
    sub.writeUInt32BE(group.glyph, offset + 8);
  });
  return sub;
}

function buildCmap(coverage) {
  // coverage: ascending list of {codepoint, glyphIndex}
  const groups = [];
  for (const entry of coverage) {
    const last = groups[groups.length - 1];
    if (
      last &&
      entry.codepoint === last.end + 1 &&
      entry.glyphIndex === last.glyph + (last.end - last.start) + 1
    ) {
      last.end = entry.codepoint;
    } else {
      groups.push({
        start: entry.codepoint,
        end: entry.codepoint,
        glyph: entry.glyphIndex,
      });
    }
  }

  const bmpSegments = [];
  for (const entry of coverage) {
    if (entry.codepoint > 0xffff) continue;
    const last = bmpSegments[bmpSegments.length - 1];
    if (last && entry.codepoint === last.end + 1) {
      last.end = entry.codepoint;
      continue;
    }
    bmpSegments.push({
      start: entry.codepoint,
      end: entry.codepoint,
      delta: entry.glyphIndex,
    });
  }
  bmpSegments.push({ start: 0xffff, end: 0xffff, delta: 1 });

  const sub4 = buildCmapFormat4(bmpSegments);
  const sub12 = buildCmapFormat12(groups);
  const headerLength = 4 + 8 * 2;
  const table = Buffer.alloc(headerLength + sub4.length + sub12.length);
  table.writeUInt16BE(0, 0);
  table.writeUInt16BE(2, 2);
  // encoding record 1: Windows BMP (format 4)
  table.writeUInt16BE(3, 4);
  table.writeUInt16BE(1, 6);
  table.writeUInt32BE(headerLength, 8);
  // encoding record 2: Windows full repertoire (format 12)
  table.writeUInt16BE(3, 12);
  table.writeUInt16BE(10, 14);
  table.writeUInt32BE(headerLength + sub4.length, 16);
  sub4.copy(table, headerLength);
  sub12.copy(table, headerLength + sub4.length);
  return table;
}

function buildName(family, style, version) {
  const records = [
    [1, family],
    [2, style],
    [3, `${family} ${style}`],
    [4, `${family} ${style}`],
    [5, version],
    [6, `${family.replace(/\s+/g, "")}-${style}`],
    [16, family],
    [17, style],
  ];
  const strings = records.map(([, value]) => {
    const utf16 = Buffer.from(value, "utf16le");
    for (let index = 0; index < utf16.length; index += 2) {
      const low = utf16[index];
      utf16[index] = utf16[index + 1];
      utf16[index + 1] = low;
    }
    return utf16;
  });
  const headerLength = 6 + records.length * 12;
  const table = Buffer.alloc(
    headerLength + strings.reduce((sum, string) => sum + string.length, 0),
  );
  table.writeUInt16BE(0, 0);
  table.writeUInt16BE(records.length, 2);
  table.writeUInt16BE(headerLength, 4);
  let stringOffset = 0;
  records.forEach(([nameId], index) => {
    const recordOffset = 6 + index * 12;
    table.writeUInt16BE(3, recordOffset); // Windows
    table.writeUInt16BE(1, recordOffset + 2); // Unicode BMP
    table.writeUInt16BE(0x0409, recordOffset + 4); // en-US
    table.writeUInt16BE(nameId, recordOffset + 6);
    table.writeUInt16BE(strings[index].length, recordOffset + 8);
    table.writeUInt16BE(stringOffset, recordOffset + 10);
    strings[index].copy(table, headerLength + stringOffset);
    stringOffset += strings[index].length;
  });
  return table;
}

function buildOs2(firstChar, lastChar) {
  const buffer = Buffer.alloc(96);
  const write32 = (value, offset) => buffer.writeUInt32BE(value >>> 0, offset);
  const write16 = (value, offset) => buffer.writeUInt16BE(value & 0xffff, offset);
  const writeS16 = (value, offset) => buffer.writeInt16BE(value, offset);

  write16(4, 0); // version
  writeS16(ADVANCE_WIDTH, 2);
  write16(400, 4); // usWeightClass
  write16(5, 6); // usWidthClass
  write16(0, 8); // fsType: installable embedding
  writeS16(650, 10);
  writeS16(600, 12);
  writeS16(0, 14);
  writeS16(75, 16);
  writeS16(650, 18);
  writeS16(600, 20);
  writeS16(0, 22);
  writeS16(350, 24);
  writeS16(50, 26);
  writeS16(0, 28);
  writeS16(0, 30); // sFamilyClass
  Buffer.from([2, 11, 5, 2, 2, 2, 2, 2, 2, 4]).copy(buffer, 32); // panose
  write32(0x00000001, 42); // ulUnicodeRange1: bit 0 (Basic Latin), for the space glyph
  write32(0, 46);
  write32(0, 50);
  write32(0, 54);
  buffer.write("NYAT", 58, 4, "latin1"); // achVendID
  write16(0x0040, 62); // fsSelection: REGULAR
  write16(Math.min(firstChar, 0xffff), 64);
  write16(Math.min(Math.max(lastChar, 0xffff), 0xffff), 66);
  writeS16(ASCENDER, 68);
  writeS16(DESCENDER, 70);
  writeS16(0, 72);
  write16(ASCENDER, 74);
  write16(-DESCENDER, 76);
  write32(0, 78); // ulCodePageRange1
  write32(0, 82);
  writeS16(500, 86); // sxHeight
  writeS16(700, 88); // sCapHeight
  write16(0x0020, 90); // usDefaultChar
  write16(0x0020, 92); // usBreakChar
  write16(1, 94); // usMaxContext
  return buffer;
}

function buildHhea(numberOfHMetrics) {
  const buffer = Buffer.alloc(36);
  buffer.writeUInt32BE(0x00010000, 0);
  buffer.writeInt16BE(ASCENDER, 4);
  buffer.writeInt16BE(DESCENDER, 6);
  buffer.writeInt16BE(0, 8); // lineGap
  buffer.writeUInt16BE(ADVANCE_WIDTH, 10);
  buffer.writeInt16BE(75, 12);
  buffer.writeInt16BE(-75, 14);
  buffer.writeInt16BE(525, 16);
  buffer.writeInt16BE(1, 18);
  buffer.writeInt16BE(0, 20);
  buffer.writeInt16BE(0, 22);
  for (let offset = 24; offset < 34; offset += 2) {
    buffer.writeInt16BE(0, offset);
  }
  buffer.writeUInt16BE(numberOfHMetrics, 34);
  return buffer;
}

function buildHead(indexToLocFormat, xMin, yMin, xMax, yMax) {
  const buffer = Buffer.alloc(54);
  buffer.writeUInt32BE(0x00010000, 0);
  buffer.writeUInt32BE(0x00010000, 4);
  buffer.writeUInt32BE(0, 8); // checkSumAdjustment, patched later
  buffer.writeUInt32BE(0x5f0f3cf5, 12);
  buffer.writeUInt16BE(0x0001, 16); // flags: baseline at y=0
  buffer.writeUInt16BE(UNITS_PER_EM, 18);
  buffer.writeUInt32BE(0xd1a1a1a1, 20); // created, fixed for reproducible builds
  buffer.writeUInt32BE(0xd1a1a1a1, 24); // modified
  buffer.writeInt16BE(xMin, 36);
  buffer.writeInt16BE(yMin, 38);
  buffer.writeInt16BE(xMax, 40);
  buffer.writeInt16BE(yMax, 42);
  buffer.writeUInt16BE(0, 44); // macStyle
  buffer.writeUInt16BE(8, 46); // lowestRecPPEM
  buffer.writeInt16BE(2, 48);
  buffer.writeInt16BE(indexToLocFormat, 50);
  buffer.writeInt16BE(0, 52);
  return buffer;
}

function buildPost() {
  const buffer = Buffer.alloc(32);
  buffer.writeUInt32BE(0x00030000, 0); // version 3.0: no glyph names
  buffer.writeInt32BE(0, 4);
  buffer.writeInt16BE(0, 8);
  buffer.writeInt16BE(100, 10);
  return buffer;
}

function buildTableDirectory(tables) {
  const tags = Object.keys(tables).sort();
  const entrySelector = Math.floor(Math.log2(tags.length));
  const searchRange = 2 ** entrySelector * 16;
  const header = Buffer.alloc(12 + tags.length * 16);
  header.writeUInt32BE(0x00010000, 0);
  header.writeUInt16BE(tags.length, 4);
  header.writeUInt16BE(searchRange, 6);
  header.writeUInt16BE(entrySelector, 8);
  header.writeUInt16BE(tags.length * 16 - searchRange, 10);

  let offset = header.length;
  const chunks = [];
  tags.forEach((tag, index) => {
    const data = tables[tag];
    const padded = pad4(data);
    const record = 12 + index * 16;
    header.write(tag, record, 4, "latin1");
    header.writeUInt32BE(checksum(padded), record + 4);
    header.writeUInt32BE(offset, record + 8);
    header.writeUInt32BE(data.length, record + 12);
    offset += padded.length;
    chunks.push(padded);
  });
  return { header, chunks, tags };
}

/* -------------------------------------------------------------------- build */

const FAMILY_NAME = "NyaTerm Symbols";
const STYLE_NAME = "Regular";
const VERSION_STRING = "Version 1.000";

const symbolEntries = legacyComputingGlyphs();
const glyphs = [notdefGlyph(), glyphFromContours([]), glyphFromContours([])];
const coverage = [
  { codepoint: 0x0020, glyphIndex: SPACE_GLYPH_INDEX },
  { codepoint: 0x00a0, glyphIndex: NBSP_GLYPH_INDEX },
];

for (let offset = 0; offset < BRAILLE_COUNT; offset += 1) {
  const codepoint = BRAILLE_FIRST + offset;
  coverage.push({ codepoint, glyphIndex: FIRST_SYMBOL_GLYPH_INDEX + offset });
  glyphs.push(brailleGlyph(offset));
}
for (const entry of symbolEntries) {
  coverage.push({ codepoint: entry.codepoint, glyphIndex: glyphs.length });
  glyphs.push(entry.glyph);
}
coverage.sort((a, b) => a.codepoint - b.codepoint);

const glyphChunks = buildGlyf(glyphs).map(pad4);
const loca = buildLoca(glyphChunks);
const glyf = Buffer.concat(glyphChunks);
const hmtx = buildHmtx(glyphs);
const maxPoints = Math.max(...glyphs.map((glyph) => glyph.points.length));
const maxContours = Math.max(...glyphs.map((glyph) => glyph.contours.length));
const coveredPoints = glyphs.flatMap((glyph) => glyph.points);
const xMin = Math.min(...coveredPoints.map((point) => point.x));
const yMin = Math.min(...coveredPoints.map((point) => point.y));
const xMax = Math.max(...coveredPoints.map((point) => point.x));
const yMax = Math.max(...coveredPoints.map((point) => point.y));

const tables = {
  "OS/2": buildOs2(coverage[0].codepoint, coverage[coverage.length - 1].codepoint),
  cmap: buildCmap(coverage),
  glyf,
  head: buildHead(0, xMin, yMin, xMax, yMax),
  hhea: buildHhea(glyphs.length),
  hmtx,
  loca,
  maxp: buildMaxp(glyphs.length, maxPoints, maxContours),
  name: buildName(FAMILY_NAME, STYLE_NAME, VERSION_STRING),
  post: buildPost(),
};

const { header, chunks, tags } = buildTableDirectory(tables);
const font = Buffer.concat([header, ...chunks]);
const headDirectoryEntry = 12 + tags.indexOf("head") * 16;
const headOffset = header.readUInt32BE(headDirectoryEntry + 8);
const adjustment = (0xb1b0afba - checksum(font)) >>> 0;
font.writeUInt32BE(adjustment, headOffset + 8);

const here = dirname(fileURLToPath(import.meta.url));
const outputPath = resolve(here, "..", "public", "fonts", "nyaterm-symbols.ttf");
mkdirSync(dirname(outputPath), { recursive: true });
writeFileSync(outputPath, font);

console.log(
  `[generate-symbol-font] ${glyphs.length} glyphs (` +
    `${BRAILLE_COUNT} braille + ${symbolEntries.length} legacy computing), ` +
    `${(font.length / 1024).toFixed(1)} KiB -> ${outputPath}`,
);
console.log(
  `[generate-symbol-font] braille dots: columns 1/4,3/4 rows 1/8..7/8 r=1/8; ` +
    `sextants 3/8:2/8:3/8; eighths ${eighthX(1)}x${eighthY(1)}; ` +
    `bbox=[${xMin}, ${yMin}, ${xMax}, ${yMax}]`,
);
