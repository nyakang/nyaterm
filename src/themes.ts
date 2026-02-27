// ── Theme Definitions ──────────────────────────────────────────────────────
// Each theme defines both UI surface colors and a full xterm 16-color palette.

export interface TerminalColors {
  background: string;
  foreground: string;
  cursor: string;
  selectionBackground: string;
  black: string;
  red: string;
  green: string;
  yellow: string;
  blue: string;
  magenta: string;
  cyan: string;
  white: string;
  brightBlack: string;
  brightRed: string;
  brightGreen: string;
  brightYellow: string;
  brightBlue: string;
  brightMagenta: string;
  brightCyan: string;
  brightWhite: string;
}

export interface ThemeColors {
  // UI surface colors
  bg: string;
  bgPanel: string;
  bgTerminal: string;
  bgHover: string;
  bgInput: string;
  bgSectionHeader: string;
  border: string;
  text: string;
  textMuted: string;
  textDimmed: string;
  primary: string;
  primaryHover: string;
  scrollThumb: string;
  accent: string;
  // Terminal 16-color ANSI palette
  terminal: TerminalColors;
}

export interface Theme {
  id: string;
  name: string;
  /** Short label for the switcher preview */
  label: string;
  /** Representative color swatch for the picker */
  swatch: string;
  colors: ThemeColors;
}

// ── GitHub Dark ────────────────────────────────────────────────────────────
const githubDark: Theme = {
  id: "github-dark",
  name: "GitHub Dark",
  label: "GitHub",
  swatch: "#0d1117",
  colors: {
    bg: "#0d1117",
    bgPanel: "#161b22",
    bgTerminal: "#0d1117",
    bgHover: "#1c2128",
    bgInput: "#0d1117",
    bgSectionHeader: "#12171f",
    border: "#30363d",
    text: "#c9d1d9",
    textMuted: "#8b949e",
    textDimmed: "#6e7681",
    primary: "#3b82f6",
    primaryHover: "#2563eb",
    scrollThumb: "#30363d",
    accent: "#58a6ff",
    terminal: {
      background: "#0d1117",
      foreground: "#c9d1d9",
      cursor: "#c9d1d9",
      selectionBackground: "#264f78",
      black: "#484f58",
      red: "#ff7b72",
      green: "#3fb950",
      yellow: "#d29922",
      blue: "#58a6ff",
      magenta: "#bc8cff",
      cyan: "#39d353",
      white: "#b1bac4",
      brightBlack: "#6e7681",
      brightRed: "#ffa198",
      brightGreen: "#56d364",
      brightYellow: "#e3b341",
      brightBlue: "#79c0ff",
      brightMagenta: "#d2a8ff",
      brightCyan: "#56d364",
      brightWhite: "#f0f6fc",
    },
  },
};

// ── Dracula ────────────────────────────────────────────────────────────────
const dracula: Theme = {
  id: "dracula",
  name: "Dracula",
  label: "Dracula",
  swatch: "#282a36",
  colors: {
    bg: "#282a36",
    bgPanel: "#21222c",
    bgTerminal: "#282a36",
    bgHover: "#343746",
    bgInput: "#21222c",
    bgSectionHeader: "#303341",
    border: "#44475a",
    text: "#f8f8f2",
    textMuted: "#bd93f9",
    textDimmed: "#6272a4",
    primary: "#bd93f9",
    primaryHover: "#a77bfa",
    scrollThumb: "#44475a",
    accent: "#ff79c6",
    terminal: {
      background: "#282a36",
      foreground: "#f8f8f2",
      cursor: "#f8f8f2",
      selectionBackground: "#44475a",
      black: "#21222c",
      red: "#ff5555",
      green: "#50fa7b",
      yellow: "#f1fa8c",
      blue: "#bd93f9",
      magenta: "#ff79c6",
      cyan: "#8be9fd",
      white: "#f8f8f2",
      brightBlack: "#6272a4",
      brightRed: "#ff6e6e",
      brightGreen: "#69ff94",
      brightYellow: "#ffffa5",
      brightBlue: "#d6acff",
      brightMagenta: "#ff92df",
      brightCyan: "#a4ffff",
      brightWhite: "#ffffff",
    },
  },
};

// ── Nord ───────────────────────────────────────────────────────────────────
const nord: Theme = {
  id: "nord",
  name: "Nord",
  label: "Nord",
  swatch: "#2e3440",
  colors: {
    bg: "#2e3440",
    bgPanel: "#3b4252",
    bgTerminal: "#2e3440",
    bgHover: "#434c5e",
    bgInput: "#2e3440",
    bgSectionHeader: "#323845",
    border: "#4c566a",
    text: "#d8dee9",
    textMuted: "#81a1c1",
    textDimmed: "#616e88",
    primary: "#88c0d0",
    primaryHover: "#7bb8c9",
    scrollThumb: "#4c566a",
    accent: "#5e81ac",
    terminal: {
      background: "#2e3440",
      foreground: "#d8dee9",
      cursor: "#d8dee9",
      selectionBackground: "#434c5e",
      black: "#3b4252",
      red: "#bf616a",
      green: "#a3be8c",
      yellow: "#ebcb8b",
      blue: "#81a1c1",
      magenta: "#b48ead",
      cyan: "#88c0d0",
      white: "#e5e9f0",
      brightBlack: "#4c566a",
      brightRed: "#d06f79",
      brightGreen: "#b1cc99",
      brightYellow: "#f1d59d",
      brightBlue: "#93b4d6",
      brightMagenta: "#c59cbd",
      brightCyan: "#9bd3e4",
      brightWhite: "#eceff4",
    },
  },
};

// ── Monokai Pro ────────────────────────────────────────────────────────────
const monokaiPro: Theme = {
  id: "monokai-pro",
  name: "Monokai Pro",
  label: "Monokai",
  swatch: "#2d2a2e",
  colors: {
    bg: "#2d2a2e",
    bgPanel: "#221f22",
    bgTerminal: "#2d2a2e",
    bgHover: "#3a373b",
    bgInput: "#221f22",
    bgSectionHeader: "#302d31",
    border: "#49474c",
    text: "#fcfcfa",
    textMuted: "#c1c0c0",
    textDimmed: "#727072",
    primary: "#ffd866",
    primaryHover: "#e6c25e",
    scrollThumb: "#49474c",
    accent: "#78dce8",
    terminal: {
      background: "#2d2a2e",
      foreground: "#fcfcfa",
      cursor: "#fcfcfa",
      selectionBackground: "#49474c",
      black: "#403e41",
      red: "#ff6188",
      green: "#a9dc76",
      yellow: "#ffd866",
      blue: "#66d9ef",
      magenta: "#ab9df2",
      cyan: "#78dce8",
      white: "#fcfcfa",
      brightBlack: "#727072",
      brightRed: "#ff7b9a",
      brightGreen: "#bdef83",
      brightYellow: "#ffe385",
      brightBlue: "#7ee0f5",
      brightMagenta: "#bfaef7",
      brightCyan: "#8ce7f2",
      brightWhite: "#ffffff",
    },
  },
};

// ── Solarized Light ────────────────────────────────────────────────────────
const solarizedLight: Theme = {
  id: "solarized-light",
  name: "Solarized Light",
  label: "Solarized",
  swatch: "#fdf6e3",
  colors: {
    bg: "#fdf6e3",
    bgPanel: "#eee8d5",
    bgTerminal: "#fdf6e3",
    bgHover: "#e8e1cb",
    bgInput: "#fdf6e3",
    bgSectionHeader: "#f5efdc",
    border: "#d3cbb7",
    text: "#586e75",
    textMuted: "#93a1a1",
    textDimmed: "#b0b8b8",
    primary: "#268bd2",
    primaryHover: "#1e7abc",
    scrollThumb: "#d3cbb7",
    accent: "#2aa198",
    terminal: {
      background: "#fdf6e3",
      foreground: "#586e75",
      cursor: "#586e75",
      selectionBackground: "#eee8d5",
      black: "#073642",
      red: "#dc322f",
      green: "#859900",
      yellow: "#b58900",
      blue: "#268bd2",
      magenta: "#d33682",
      cyan: "#2aa198",
      white: "#eee8d5",
      brightBlack: "#002b36",
      brightRed: "#ea6052",
      brightGreen: "#9eb300",
      brightYellow: "#cfa600",
      brightBlue: "#45a5f1",
      brightMagenta: "#e94b99",
      brightCyan: "#3bc6bb",
      brightWhite: "#fdf6e3",
    },
  },
};

// ── Exports ────────────────────────────────────────────────────────────────

export const themes: Record<string, Theme> = {
  "github-dark": githubDark,
  dracula,
  nord,
  "monokai-pro": monokaiPro,
  "solarized-light": solarizedLight,
};

export const themeList: Theme[] = [githubDark, dracula, nord, monokaiPro, solarizedLight];

export const DEFAULT_THEME_ID = "github-dark";