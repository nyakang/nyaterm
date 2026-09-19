import fs from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";

const packageDir = path.resolve(process.cwd(), "node_modules", "@xterm", "addon-webgl", "lib");

const cases = [
  {
    name: "ESM",
    file: "addon-webgl.mjs",
    search: "let o=s/8,n=a*.1,T=a*.8/8,h=Math.min(o,T);",
    patched: "let o=s/8,n=0,T=a/8,h=Math.min(o,T);",
  },
  {
    name: "CJS",
    file: "addon-webgl.js",
    search: "const n=r/8,L=.1*a,h=.8*a/8,T=Math.min(n,h);",
    patched: "const n=r/8,L=0,h=a/8,T=Math.min(n,h);",
  },
] as const;

describe("xterm braille grid patch", () => {
  it.each(cases)("$name rasterizes braille on a grid that continues across lines", ({
    file,
    search,
    patched,
  }) => {
    const source = fs.readFileSync(path.join(packageDir, file), "utf8");

    expect(source).toContain("/*nyaterm:braille-grid*/");
    expect(source).toContain(patched);
    // Upstream insets the dot grid by 10% of the cell height, which leaves a
    // larger gap between lines than inside a line.
    expect(source).not.toContain(search);
  });
});
