import {
  autocompletion,
  closeBrackets,
  closeBracketsKeymap,
  completionKeymap,
} from "@codemirror/autocomplete";
import { defaultKeymap, history, historyKeymap, indentLess, insertTab } from "@codemirror/commands";
import { cpp } from "@codemirror/lang-cpp";
import { css } from "@codemirror/lang-css";
import { go } from "@codemirror/lang-go";
import { html } from "@codemirror/lang-html";
import { java } from "@codemirror/lang-java";
import { javascript } from "@codemirror/lang-javascript";
import { json } from "@codemirror/lang-json";
import { markdown } from "@codemirror/lang-markdown";
import { php } from "@codemirror/lang-php";
import { python } from "@codemirror/lang-python";
import { rust } from "@codemirror/lang-rust";
import { sass as sassLanguage } from "@codemirror/lang-sass";
import { sql } from "@codemirror/lang-sql";
import { xml } from "@codemirror/lang-xml";
import { yaml } from "@codemirror/lang-yaml";
import {
  bracketMatching,
  foldGutter,
  foldKeymap,
  HighlightStyle,
  indentOnInput,
  StreamLanguage,
  syntaxHighlighting,
} from "@codemirror/language";
import { csharp, dart } from "@codemirror/legacy-modes/mode/clike";
import { cmake } from "@codemirror/legacy-modes/mode/cmake";
import { diff } from "@codemirror/legacy-modes/mode/diff";
import { dockerFile } from "@codemirror/legacy-modes/mode/dockerfile";
import { lua } from "@codemirror/legacy-modes/mode/lua";
import { fSharp } from "@codemirror/legacy-modes/mode/mllike";
import { nginx } from "@codemirror/legacy-modes/mode/nginx";
import { perl } from "@codemirror/legacy-modes/mode/perl";
import { powerShell } from "@codemirror/legacy-modes/mode/powershell";
import { properties } from "@codemirror/legacy-modes/mode/properties";
import { protobuf } from "@codemirror/legacy-modes/mode/protobuf";
import { r } from "@codemirror/legacy-modes/mode/r";
import { ruby } from "@codemirror/legacy-modes/mode/ruby";
import { shell } from "@codemirror/legacy-modes/mode/shell";
import { swift } from "@codemirror/legacy-modes/mode/swift";
import { toml } from "@codemirror/legacy-modes/mode/toml";
import { vb } from "@codemirror/legacy-modes/mode/vb";
import { search, searchKeymap } from "@codemirror/search";
import { EditorState, type Extension } from "@codemirror/state";
import {
  crosshairCursor,
  drawSelection,
  dropCursor,
  EditorView,
  highlightActiveLine,
  highlightActiveLineGutter,
  highlightSpecialChars,
  keymap,
  lineNumbers,
  rectangularSelection,
  scrollPastEnd,
} from "@codemirror/view";
import { tags } from "@lezer/highlight";

export interface CursorPosition {
  line: number;
  column: number;
}

interface FileViewExtensionOptions {
  editable?: boolean;
  updateListener?: Extension;
}

const fileViewHighlightStyle = HighlightStyle.define([
  {
    tag: [tags.comment, tags.lineComment, tags.blockComment, tags.docComment, tags.meta],
    color: "var(--df-text-muted)",
    fontStyle: "italic",
  },
  {
    tag: [
      tags.keyword,
      tags.controlKeyword,
      tags.definitionKeyword,
      tags.moduleKeyword,
      tags.operatorKeyword,
    ],
    color: "var(--df-primary)",
  },
  {
    tag: [tags.operator, tags.definitionOperator, tags.punctuation, tags.separator],
    color: "var(--df-text-dimmed)",
  },
  {
    tag: [tags.string, tags.docString, tags.character, tags.attributeValue],
    color: "var(--df-success)",
  },
  {
    tag: [tags.number, tags.integer, tags.float, tags.bool, tags.null, tags.atom],
    color: "var(--df-warning)",
  },
  {
    tag: [tags.regexp, tags.escape, tags.url],
    color: "var(--df-accent)",
  },
  {
    tag: [tags.className, tags.typeName, tags.namespace, tags.tagName],
    color: "color-mix(in srgb, var(--df-link) 72%, var(--df-success))",
  },
  {
    tag: [
      tags.function(tags.variableName),
      tags.function(tags.propertyName),
      tags.definition(tags.variableName),
      tags.definition(tags.propertyName),
    ],
    color: "var(--df-link)",
  },
  {
    tag: [tags.propertyName, tags.attributeName, tags.labelName],
    color: "color-mix(in srgb, var(--df-link) 78%, var(--df-text))",
  },
  {
    tag: [tags.constant(tags.variableName), tags.standard(tags.variableName), tags.macroName],
    color: "color-mix(in srgb, var(--df-warning) 85%, var(--df-text))",
  },
  {
    tag: [tags.deleted, tags.invalid],
    color: "var(--df-danger)",
  },
  {
    tag: [tags.inserted, tags.changed],
    color: "var(--df-success)",
  },
  {
    tag: tags.heading,
    color: "var(--df-primary)",
    fontWeight: "600",
  },
  {
    tag: [tags.emphasis],
    fontStyle: "italic",
  },
  {
    tag: [tags.strong],
    fontWeight: "600",
  },
]);

export function languageExtension(language: string) {
  switch (language) {
    case "batch":
    case "shell":
      return StreamLanguage.define(shell);
    case "c":
    case "cpp":
      return cpp();
    case "cmake":
      return StreamLanguage.define(cmake);
    case "csharp":
      return StreamLanguage.define(csharp);
    case "css":
    case "less":
      return css();
    case "dart":
      return StreamLanguage.define(dart);
    case "diff":
      return StreamLanguage.define(diff);
    case "dockerfile":
      return StreamLanguage.define(dockerFile);
    case "fsharp":
      return StreamLanguage.define(fSharp);
    case "go":
      return go();
    case "graphql":
      return javascript({ jsx: true, typescript: true });
    case "html":
      return html();
    case "ini":
    case "makefile":
    case "properties":
      return StreamLanguage.define(properties);
    case "java":
    case "kotlin":
      return java();
    case "javascript":
      return javascript({ jsx: true });
    case "json":
    case "json5":
    case "jsonc":
      return json();
    case "lua":
      return StreamLanguage.define(lua);
    case "markdown":
      return markdown();
    case "nginx":
      return StreamLanguage.define(nginx);
    case "perl":
      return StreamLanguage.define(perl);
    case "php":
      return php();
    case "powershell":
      return StreamLanguage.define(powerShell);
    case "protobuf":
      return StreamLanguage.define(protobuf);
    case "python":
      return python();
    case "r":
      return StreamLanguage.define(r);
    case "ruby":
      return StreamLanguage.define(ruby);
    case "rust":
      return rust();
    case "sass":
      return sassLanguage({ indented: true });
    case "scss":
      return sassLanguage();
    case "sql":
      return sql();
    case "svelte":
    case "vue":
      return html();
    case "swift":
      return StreamLanguage.define(swift);
    case "toml":
      return StreamLanguage.define(toml);
    case "typescript":
      return javascript({ jsx: true, typescript: true });
    case "vb":
      return StreamLanguage.define(vb);
    case "xml":
      return xml();
    case "yaml":
      return yaml();
    default:
      return [];
  }
}

export function getDisplayLanguage(language: string) {
  return language === "plaintext" ? "Plain Text" : language.toLocaleUpperCase();
}

export function getCursorPosition(state: EditorState): CursorPosition {
  const head = state.selection.main.head;
  const line = state.doc.lineAt(head);
  return { line: line.number, column: head - line.from + 1 };
}

export function codeMirrorFileViewExtensions(
  language: string,
  { editable = true, updateListener }: FileViewExtensionOptions = {},
) {
  const extensions: Extension[] = [
    lineNumbers(),
    foldGutter(),
    highlightSpecialChars(),
    drawSelection(),
    dropCursor(),
    EditorState.allowMultipleSelections.of(true),
    syntaxHighlighting(fileViewHighlightStyle),
    bracketMatching(),
    highlightActiveLine(),
    highlightActiveLineGutter(),
    search({ top: true }),
    languageExtension(language),
    rectangularSelection(),
    crosshairCursor(),
    scrollPastEnd(),
    keymap.of([...defaultKeymap, ...searchKeymap, ...foldKeymap]),
    EditorView.lineWrapping,
    EditorView.theme({
      "&": {
        height: "100%",
        backgroundColor: "var(--background)",
        color: "var(--foreground)",
      },
      "&.cm-focused": {
        outline: "none",
      },
      ".cm-content": {
        minHeight: "100%",
        caretColor: "var(--foreground)",
        cursor: "text",
        userSelect: "text",
      },
      ".cm-line": {
        cursor: "text",
      },
      ".cm-cursor, .cm-dropCursor": {
        borderLeftColor: "var(--foreground)",
      },
      ".cm-selectionBackground, &.cm-focused .cm-selectionBackground": {
        backgroundColor: "var(--df-terminal-selection, var(--df-primary)) !important",
      },
      ".cm-scroller": {
        fontFamily: "var(--font-mono), 'JetBrains Mono', monospace",
        overflow: "auto",
      },
      ".cm-gutters": {
        backgroundColor: "color-mix(in srgb, var(--muted) 18%, transparent)",
        color: "var(--muted-foreground)",
        borderRightColor: "color-mix(in srgb, var(--border) 70%, transparent)",
      },
      ".cm-foldGutter": {
        width: "1.1rem",
      },
      ".cm-foldGutter span": {
        cursor: "pointer",
        color: "var(--muted-foreground)",
        opacity: "0.45",
        transition: "opacity 120ms ease, color 120ms ease",
      },
      ".cm-foldGutter:hover span": {
        opacity: "0.8",
      },
      ".cm-activeLine": {
        "&::before": {
          content: '""',
          position: "absolute",
          inset: "0",
          zIndex: "-3",
          backgroundColor: "color-mix(in srgb, var(--muted) 22%, transparent)",
        },
        position: "relative",
        backgroundColor: "transparent",
      },
      ".cm-activeLineGutter": {
        backgroundColor: "color-mix(in srgb, var(--muted) 32%, transparent)",
      },
      ".cm-tooltip": {
        borderColor: "var(--border)",
        backgroundColor: "var(--popover)",
        color: "var(--popover-foreground)",
        fontSize: "12px",
        boxShadow: "0 10px 30px rgb(0 0 0 / 0.22)",
      },
      ".cm-tooltip-autocomplete ul li[aria-selected]": {
        backgroundColor: "color-mix(in srgb, var(--primary) 18%, transparent)",
        color: "var(--foreground)",
      },
      ".cm-search": {
        backgroundColor: "var(--popover)",
        color: "var(--popover-foreground)",
        borderBottomColor: "var(--border)",
        gap: "0.375rem",
        padding: "0.375rem",
      },
      ".cm-search input": {
        backgroundColor: "var(--background)",
        color: "var(--foreground)",
        border: "1px solid var(--border)",
        borderRadius: "0.25rem",
        padding: "0.125rem 0.375rem",
      },
      ".cm-search input::placeholder": {
        color: "var(--muted-foreground)",
      },
      ".cm-search input[type=checkbox]": {
        accentColor: "var(--primary)",
      },
      ".cm-search .cm-button": {
        backgroundColor: "var(--secondary)",
        backgroundImage: "none",
        color: "var(--foreground)",
        border: "1px solid var(--border)",
        borderRadius: "0.25rem",
        padding: "0.125rem 0.375rem",
      },
      ".cm-search .cm-button:hover": {
        backgroundColor: "color-mix(in srgb, var(--primary) 14%, var(--secondary))",
        borderColor: "color-mix(in srgb, var(--primary) 45%, var(--border))",
      },
      ".cm-search .cm-button:active": {
        backgroundColor: "color-mix(in srgb, var(--primary) 22%, var(--secondary))",
        backgroundImage: "none",
      },
      ".cm-search input:focus-visible, .cm-search button:focus-visible": {
        outline: "2px solid var(--ring)",
        outlineOffset: "1px",
      },
      ".cm-search button[name=close]": {
        color: "var(--muted-foreground)",
      },
      ".cm-search button[name=close]:hover": {
        color: "var(--foreground)",
      },
    }),
  ];

  if (editable) {
    extensions.splice(
      3,
      0,
      history(),
      indentOnInput(),
      autocompletion(),
      closeBrackets(),
      keymap.of([
        { key: "Tab", run: insertTab },
        { key: "Shift-Tab", run: indentLess },
      ]),
      keymap.of([...closeBracketsKeymap, ...historyKeymap, ...completionKeymap]),
    );
  } else {
    extensions.push(EditorState.readOnly.of(true), EditorView.editable.of(false));
  }

  if (updateListener) {
    extensions.push(updateListener);
  }

  return extensions;
}
