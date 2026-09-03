import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { describe, expect, it } from "vitest";
import { codeMirrorFileViewExtensions } from "./codeMirrorFileView";

function findRules(fragment: string): string[] {
  const rules: string[] = [];
  for (const sheet of Array.from(document.styleSheets)) {
    const owner = sheet.ownerNode as HTMLStyleElement | null;
    // jsdom's cssText drops !important inside var() values, so read the raw
    // rule text from the mounted style tag instead.
    const text = owner?.textContent ?? "";
    for (const line of text.split("\n")) {
      if (line.includes(fragment)) {
        rules.push(line);
      }
    }
  }
  return rules;
}

function mountEditor(): HTMLDivElement {
  const host = document.createElement("div");
  document.body.appendChild(host);
  new EditorView({
    state: EditorState.create({
      doc: "hello",
      extensions: codeMirrorFileViewExtensions("plaintext"),
    }),
    parent: host,
  });
  return host;
}

describe("codeMirrorFileView selection styling", () => {
  it("renders the editor selection with the terminal selection color", () => {
    const host = mountEditor();

    try {
      const rules = findRules("cm-selectionBackground").join("\n");
      expect(rules).toContain("var(--df-terminal-selection");
      expect(rules).toContain("!important");
    } finally {
      host.remove();
    }
  });

  it("keeps the active line highlight below the selection layer", () => {
    const host = mountEditor();

    try {
      // CodeMirror draws its selection layer below in-flow line backgrounds,
      // so the highlight must live on a ::before with z-index under the
      // selection layer (-2), not on the line element itself.
      const activeLineRules = findRules("cm-activeLine").filter(
        (rule) => !rule.includes("cm-activeLineGutter"),
      );
      const beforeRule = activeLineRules.find((rule) => rule.includes(":before"));

      expect(beforeRule).toBeDefined();
      expect(beforeRule).toContain("z-index: -3");
      expect(beforeRule).toContain("color-mix(in srgb, var(--muted) 22%, transparent)");

      const lineRules = activeLineRules.filter((rule) => !rule.includes(":before"));
      // The CodeMirror base theme also styles .cm-activeLine (#cceeff44);
      // our theme's rule is the one that clears the element background.
      const ownLineRule = lineRules.find((rule) => rule.includes("background-color: transparent"));
      expect(ownLineRule).toBeDefined();
      expect(ownLineRule).toContain("position: relative");
    } finally {
      host.remove();
    }
  });
});

describe("codeMirrorFileView search panel styling", () => {
  it("removes CodeMirror's light button gradient and uses theme colors", () => {
    const host = mountEditor();

    try {
      const buttonRules = findRules("cm-search").filter((rule) => rule.includes("cm-button"));
      const baseRule = buttonRules.find(
        (rule) => rule.includes("background-image: none") && rule.includes("var(--secondary)"),
      );

      expect(baseRule).toBeDefined();
      expect(baseRule).toContain("color: var(--foreground)");
      expect(baseRule).toContain("border: 1px solid var(--border)");

      const activeRule = buttonRules.find((rule) => rule.includes("cm-button:active"));
      expect(activeRule).toContain("background-image: none");
    } finally {
      host.remove();
    }
  });

  it("uses the theme focus ring and checkbox accent", () => {
    const host = mountEditor();

    try {
      const searchRules = findRules("cm-search").join("\n");
      expect(searchRules).toContain("accent-color: var(--primary)");
      expect(searchRules).toContain("outline: 2px solid var(--ring)");
      expect(searchRules).toContain("color: var(--muted-foreground)");
    } finally {
      host.remove();
    }
  });
});
