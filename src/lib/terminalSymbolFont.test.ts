import fs from "node:fs";
import path from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  composeTerminalFontFamily,
  ensureTerminalSymbolFontReady,
  TERMINAL_SYMBOL_FONT_FAMILY,
  TERMINAL_SYMBOL_FONT_UNICODE_RANGE,
} from "./terminalSymbolFont";

const FONT_PATH = path.resolve(process.cwd(), "public", "fonts", "nyaterm-symbols.ttf");

interface TableRecord {
  offset: number;
  length: number;
}

function readTableDirectory(buffer: Buffer): Map<string, TableRecord> {
  const tableCount = buffer.readUInt16BE(4);
  const tables = new Map<string, TableRecord>();
  for (let index = 0; index < tableCount; index += 1) {
    const record = 12 + index * 16;
    tables.set(buffer.toString("latin1", record, record + 4), {
      offset: buffer.readUInt32BE(record + 8),
      length: buffer.readUInt32BE(record + 12),
    });
  }
  return tables;
}

/** Maps codepoints to glyph ids, using the format 12 subtable when present. */
function readCoverage(buffer: Buffer, cmap: TableRecord): Map<number, number> {
  const tableCount = buffer.readUInt16BE(cmap.offset + 2);
  const subtables: Array<{ encoding: number; format: number; offset: number }> = [];
  for (let index = 0; index < tableCount; index += 1) {
    const record = cmap.offset + 4 + index * 8;
    const encoding = buffer.readUInt16BE(record + 2);
    const offset = cmap.offset + buffer.readUInt32BE(record + 4);
    subtables.push({ encoding, format: buffer.readUInt16BE(offset), offset });
  }
  const preferred =
    subtables.find((entry) => entry.encoding === 10 && entry.format === 12) ??
    subtables.find((entry) => entry.encoding === 1 && entry.format === 4);
  if (!preferred) {
    throw new Error("no usable cmap subtable");
  }

  const coverage = new Map<number, number>();
  if (preferred.format === 12) {
    const groupCount = buffer.readUInt32BE(preferred.offset + 12);
    for (let index = 0; index < groupCount; index += 1) {
      const group = preferred.offset + 16 + index * 12;
      const start = buffer.readUInt32BE(group);
      const end = buffer.readUInt32BE(group + 4);
      const glyph = buffer.readUInt32BE(group + 8);
      for (let codepoint = start; codepoint <= end; codepoint += 1) {
        coverage.set(codepoint, glyph + (codepoint - start));
      }
    }
    return coverage;
  }

  const segmentCountX2 = buffer.readUInt16BE(preferred.offset + 6);
  const segmentCount = segmentCountX2 / 2;
  const endOffset = preferred.offset + 14;
  const startOffset = endOffset + segmentCountX2 + 2;
  const deltaOffset = startOffset + segmentCountX2;
  for (let index = 0; index < segmentCount; index += 1) {
    const start = buffer.readUInt16BE(startOffset + index * 2);
    const end = buffer.readUInt16BE(endOffset + index * 2);
    const delta = buffer.readInt16BE(deltaOffset + index * 2);
    for (let codepoint = start; codepoint <= end; codepoint += 1) {
      if (codepoint === 0xffff) continue;
      coverage.set(codepoint, (codepoint + delta) & 0xffff);
    }
  }
  return coverage;
}

function glyphHeader(buffer: Buffer, tables: Map<string, TableRecord>, glyph: number) {
  const loca = tables.get("loca");
  const glyf = tables.get("glyf");
  if (!loca || !glyf) throw new Error("missing loca/glyf");
  const start = buffer.readUInt16BE(loca.offset + glyph * 2) * 2;
  const end = buffer.readUInt16BE(loca.offset + glyph * 2 + 2) * 2;
  const offset = glyf.offset + start;
  return {
    empty: end <= start,
    contours: buffer.readInt16BE(offset),
    xMin: buffer.readInt16BE(offset + 2),
    yMin: buffer.readInt16BE(offset + 4),
    xMax: buffer.readInt16BE(offset + 6),
    yMax: buffer.readInt16BE(offset + 8),
  };
}

describe("bundled NyaTerm Symbols font", () => {
  const buffer = fs.readFileSync(FONT_PATH);
  const tables = readTableDirectory(buffer);
  const coverage = readCoverage(buffer, tables.get("cmap") as TableRecord);

  it("is a TrueType font with the tables browsers need", () => {
    expect(buffer.readUInt32BE(0)).toBe(0x00010000);
    expect([...tables.keys()].sort()).toEqual([
      "OS/2",
      "cmap",
      "glyf",
      "head",
      "hhea",
      "hmtx",
      "loca",
      "maxp",
      "name",
      "post",
    ]);
  });

  it("uses the monospace cell metrics the terminal grid expects", () => {
    const head = tables.get("head") as TableRecord;
    const hhea = tables.get("hhea") as TableRecord;
    expect(buffer.readUInt16BE(head.offset + 18)).toBe(1000); // unitsPerEm
    expect(buffer.readInt16BE(head.offset + 50)).toBe(0); // short loca
    expect(buffer.readInt16BE(hhea.offset + 4)).toBe(1020); // ascender
    expect(buffer.readInt16BE(hhea.offset + 6)).toBe(-300); // descender
  });

  it("maps every braille codepoint to a distinct glyph", () => {
    const braille: number[] = [];
    for (let code = 0x2800; code <= 0x28ff; code += 1) {
      const glyph = coverage.get(code);
      expect(glyph, `U+${code.toString(16)}`).toBeGreaterThan(0);
      braille.push(glyph as number);
    }
    expect(new Set(braille).size).toBe(256);
  });

  it("covers the Legacy Computing ranges it ships", () => {
    const sextants: number[] = [];
    for (let code = 0x1fb00; code <= 0x1fb3b; code += 1) {
      const glyph = coverage.get(code);
      expect(glyph, `U+${code.toString(16)}`).toBeGreaterThan(0);
      sextants.push(glyph as number);
    }
    expect(new Set(sextants).size).toBe(60);

    const blocks: number[] = [];
    for (let code = 0x1fb68; code <= 0x1fb8b; code += 1) {
      const glyph = coverage.get(code);
      expect(glyph, `U+${code.toString(16)}`).toBeGreaterThan(0);
      blocks.push(glyph as number);
    }
    expect(new Set(blocks).size).toBe(0x1fb8b - 0x1fb68 + 1);

    const digits: number[] = [];
    for (let code = 0x1fbf0; code <= 0x1fbf9; code += 1) {
      const glyph = coverage.get(code);
      expect(glyph, `U+${code.toString(16)}`).toBeGreaterThan(0);
      digits.push(glyph as number);
    }
    expect(new Set(digits).size).toBe(10);
  });

  it("draws the segmented digits from the expected segments", () => {
    // Number of drawn segments per digit (a..g), matching the WebGL addon.
    const segmentCounts = [6, 2, 5, 5, 4, 5, 6, 4, 7, 6];
    segmentCounts.forEach((expected, digit) => {
      const header = glyphHeader(buffer, tables, coverage.get(0x1fbf0 + digit) as number);
      expect(header.contours, `digit ${digit}`).toBe(expected);
    });
    const one = glyphHeader(buffer, tables, coverage.get(0x1fbf1) as number);
    expect([one.xMin, one.yMin, one.xMax, one.yMax]).toEqual([480, 50, 570, 670]);
  });

  it("advances every glyph by exactly one cell width", () => {
    const hmtx = tables.get("hmtx") as TableRecord;
    for (const code of [0x0020, 0x2801, 0x28ff, 0x1fb00, 0x1fb6c, 0x1fb8b]) {
      const glyph = coverage.get(code) as number;
      expect(buffer.readUInt16BE(hmtx.offset + glyph * 4), `U+${code.toString(16)}`).toBe(600);
    }
  });

  it("places braille dots on 1/8..7/8 of the cell", () => {
    const allDots = coverage.get(0x28ff) as number;
    const header = glyphHeader(buffer, tables, allDots);
    expect(header.contours).toBe(8);
    expect([header.xMin, header.yMin, header.xMax, header.yMax]).toEqual([75, -210, 525, 930]);
  });

  it("draws the eighth blocks on the cell eighth grid", () => {
    const cases: Array<[number, [number, number, number, number]]> = [
      [0x1fb70, [75, -300, 150, 1020]], // vertical one eighth block-2
      [0x1fb76, [0, 690, 600, 855]], // horizontal one eighth block-2
      [0x1fb7c, [0, -300, 600, 1020]], // left and lower corner
      [0x1fb82, [0, 690, 600, 1020]], // upper one quarter
      [0x1fb86, [0, -135, 600, 1020]], // upper seven eighths
      [0x1fb87, [450, -300, 600, 1020]], // right one quarter
      [0x1fb8b, [75, -300, 600, 1020]], // right seven eighths
    ];
    for (const [code, expected] of cases) {
      const header = glyphHeader(buffer, tables, coverage.get(code) as number);
      expect(
        [header.xMin, header.yMin, header.xMax, header.yMax],
        `U+${code.toString(16)}`,
      ).toEqual(expected);
    }
  });

  it("draws sextants and triangles on the cell grid", () => {
    const topLeft = glyphHeader(buffer, tables, coverage.get(0x1fb00) as number);
    expect([topLeft.xMin, topLeft.yMin, topLeft.xMax, topLeft.yMax]).toEqual([0, 525, 300, 1020]);

    const allButTopLeft = glyphHeader(buffer, tables, coverage.get(0x1fb3b) as number);
    expect(allButTopLeft.contours).toBe(3);
    expect([
      allButTopLeft.xMin,
      allButTopLeft.yMin,
      allButTopLeft.xMax,
      allButTopLeft.yMax,
    ]).toEqual([0, -300, 600, 1020]);

    const leftQuarter = glyphHeader(buffer, tables, coverage.get(0x1fb6c) as number);
    expect(leftQuarter.contours).toBe(1);
    expect([leftQuarter.xMin, leftQuarter.yMin, leftQuarter.xMax, leftQuarter.yMax]).toEqual([
      0, -300, 300, 1020,
    ]);

    const leftThreeQuarters = glyphHeader(buffer, tables, coverage.get(0x1fb68) as number);
    expect(leftThreeQuarters.contours).toBe(1);
    expect([
      leftThreeQuarters.xMin,
      leftThreeQuarters.yMin,
      leftThreeQuarters.xMax,
      leftThreeQuarters.yMax,
    ]).toEqual([0, -300, 600, 1020]);
  });
});

describe("font wiring", () => {
  it("declares the bundled font in CSS with a matching unicode-range and file", () => {
    const css = fs.readFileSync(path.resolve(process.cwd(), "src", "index.css"), "utf8");
    expect(css).toContain(`font-family: "${TERMINAL_SYMBOL_FONT_FAMILY}"`);
    expect(css).toContain('url("/fonts/nyaterm-symbols.ttf")');
    expect(css).toContain(`unicode-range: ${TERMINAL_SYMBOL_FONT_UNICODE_RANGE};`);
    expect(fs.existsSync(FONT_PATH)).toBe(true);
  });

  it("preloads the bundled font", () => {
    const html = fs.readFileSync(path.resolve(process.cwd(), "index.html"), "utf8");
    expect(html).toContain('rel="preload"');
    expect(html).toContain('href="/fonts/nyaterm-symbols.ttf"');
  });
});

describe("composeTerminalFontFamily", () => {
  it("puts the bundled symbols font first", () => {
    expect(composeTerminalFontFamily('"JetBrainsMono Nerd Font Mono", monospace')).toBe(
      `"${TERMINAL_SYMBOL_FONT_FAMILY}", "JetBrainsMono Nerd Font Mono", monospace`,
    );
  });

  it("falls back to a generic monospace stack when nothing is configured", () => {
    expect(composeTerminalFontFamily("   ")).toBe(`"${TERMINAL_SYMBOL_FONT_FAMILY}", monospace`);
  });
});

describe("ensureTerminalSymbolFontReady", () => {
  const originalFonts = Object.getOwnPropertyDescriptor(document, "fonts");

  const stubFonts = (fonts: unknown) => {
    Object.defineProperty(document, "fonts", {
      value: fonts,
      configurable: true,
    });
  };

  afterEach(() => {
    if (originalFonts) {
      Object.defineProperty(document, "fonts", originalFonts);
    } else {
      Reflect.deleteProperty(document, "fonts");
    }
  });

  it("resolves true when the face loads", async () => {
    const load = vi.fn().mockResolvedValue([{}]);
    stubFonts({ load });
    await expect(ensureTerminalSymbolFontReady()).resolves.toBe(true);
    expect(load).toHaveBeenCalledWith(
      `16px "${TERMINAL_SYMBOL_FONT_FAMILY}"`,
      "\u2800\u28ff\u{1fb00}",
    );
  });

  it("resolves false when no face matches", async () => {
    stubFonts({ load: vi.fn().mockResolvedValue([]) });
    await expect(ensureTerminalSymbolFontReady()).resolves.toBe(false);
  });

  it("resolves false when loading throws", async () => {
    stubFonts({ load: vi.fn().mockRejectedValue(new Error("nope")) });
    await expect(ensureTerminalSymbolFontReady()).resolves.toBe(false);
  });

  it("resolves false without a FontFaceSet", async () => {
    stubFonts(undefined);
    await expect(ensureTerminalSymbolFontReady()).resolves.toBe(false);
  });
});
