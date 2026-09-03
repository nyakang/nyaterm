import {
  createContext,
  type ReactNode,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
} from "react";
import {
  DEFAULT_THEME_ID,
  getAvailableThemes,
  resolveTheme,
  type Theme,
  type ThemeColors,
} from "@/lib/themes";
import { useApp } from "./AppContext";

interface ThemeContextType {
  theme: Theme;
  themeName: string;
  setTheme: (id: string) => void;
  terminalTheme: Theme;
  terminalThemeName: string | null;
  setTerminalTheme: (id: string | null) => void;
  themeNames: Theme[];
}

/**
 * Theme state: current theme object and name. setTheme updates local state and
 * persists via updateAppSettings. CSS vars applied to :root on theme change.
 */
const ThemeContext = createContext<ThemeContextType | null>(null);

export const THEME_CACHE_KEY = "df-theme-id";
export const THEME_SNAPSHOT_CACHE_KEY = "df-theme-snapshot";

/** Inject all theme colors as CSS custom properties on :root */
export function applyThemeToDOM(colors: ThemeColors) {
  const root = document.documentElement.style;
  root.setProperty("--df-bg", colors.bg);
  root.setProperty("--df-bg-panel", colors.bgPanel);
  root.setProperty("--df-bg-panel-solid", colors.bgPanel);
  root.setProperty("--df-bg-terminal", colors.bgTerminal);
  root.setProperty("--df-bg-hover", colors.bgHover);
  root.setProperty("--df-bg-input", colors.bgInput);
  root.setProperty("--df-bg-section-header", colors.bgSectionHeader);
  root.setProperty("--df-border", colors.border);
  root.setProperty("--df-text", colors.text);
  root.setProperty("--df-text-muted", colors.textMuted);
  root.setProperty("--df-text-dimmed", colors.textDimmed);
  root.setProperty("--df-primary", colors.primary);
  root.setProperty("--df-primary-hover", colors.primaryHover);
  root.setProperty("--df-on-primary", colors.onPrimary);
  root.setProperty("--df-focus-ring", colors.focusRing);
  root.setProperty("--df-danger", colors.danger);
  root.setProperty("--df-danger-hover", colors.dangerHover);
  root.setProperty("--df-success", colors.success);
  root.setProperty("--df-warning", colors.warning);
  root.setProperty("--df-link", colors.link);
  root.setProperty("--df-shadow", colors.shadow);
  root.setProperty("--df-scroll-thumb", colors.scrollThumb);
  root.setProperty("--df-accent", colors.accent);
}

/** Inject terminal theme colors used by terminal-adjacent UI. */
export function applyTerminalThemeToDOM(colors: ThemeColors["terminal"]) {
  const root = document.documentElement.style;
  root.setProperty("--df-terminal-bg", colors.background);
  root.setProperty("--df-terminal-fg", colors.foreground);
  root.setProperty("--df-terminal-selection", colors.selectionBackground);
}

/** Provides theme, themeName, setTheme. Syncs with appSettings.appearance.theme from backend. */
export function ThemeProvider({ children }: { children: ReactNode }) {
  const { appSettings, updateAppSettings } = useApp();
  const customThemes = appSettings.appearance.custom_themes ?? [];
  const availableThemes = useMemo(() => getAvailableThemes(customThemes), [customThemes]);
  const initialId = appSettings.appearance.theme || DEFAULT_THEME_ID;
  const [themeName, setThemeName] = useState(initialId);

  const initialTerminalId = appSettings.appearance.terminal_theme || null;
  const [terminalThemeName, setTerminalThemeName] = useState<string | null>(initialTerminalId);

  const current = resolveTheme(themeName, customThemes);

  const resolvedTerminalTheme = useMemo(() => {
    if (terminalThemeName && availableThemes.some((theme) => theme.id === terminalThemeName)) {
      return resolveTheme(terminalThemeName, customThemes);
    }
    return current;
  }, [availableThemes, customThemes, terminalThemeName, current]);

  // Apply CSS vars whenever UI theme changes and cache the ID
  useEffect(() => {
    applyThemeToDOM(current.colors);
    try {
      localStorage.setItem(THEME_CACHE_KEY, current.id);
      localStorage.setItem(
        THEME_SNAPSHOT_CACHE_KEY,
        JSON.stringify({ id: current.id, colors: current.colors }),
      );
    } catch {}
  }, [current]);

  useEffect(() => {
    applyTerminalThemeToDOM(resolvedTerminalTheme.colors.terminal);
  }, [resolvedTerminalTheme]);

  // Sync UI theme from backend
  useEffect(() => {
    const configTheme = appSettings.appearance.theme;
    if (
      configTheme &&
      configTheme !== themeName &&
      availableThemes.some((theme) => theme.id === configTheme)
    ) {
      setThemeName(configTheme);
    }
  }, [appSettings.appearance.theme, availableThemes, themeName]);

  // Sync terminal theme from backend
  useEffect(() => {
    const configTerminalTheme = appSettings.appearance.terminal_theme;
    const normalized = configTerminalTheme || null;
    if (normalized !== terminalThemeName) {
      setTerminalThemeName(normalized);
    }
  }, [appSettings.appearance.terminal_theme, terminalThemeName]);

  const setTheme = useCallback(
    (id: string) => {
      if (availableThemes.some((theme) => theme.id === id)) {
        setThemeName(id);
        updateAppSettings({ appearance: { ...appSettings.appearance, theme: id } });
      }
    },
    [appSettings.appearance, availableThemes, updateAppSettings],
  );

  const setTerminalTheme = useCallback(
    (id: string | null) => {
      const validId = id && availableThemes.some((theme) => theme.id === id) ? id : null;
      setTerminalThemeName(validId);
      updateAppSettings({
        appearance: { ...appSettings.appearance, terminal_theme: validId },
      });
    },
    [appSettings.appearance, availableThemes, updateAppSettings],
  );

  const contextValue = useMemo(
    () => ({
      theme: current,
      themeName,
      setTheme,
      terminalTheme: resolvedTerminalTheme,
      terminalThemeName,
      setTerminalTheme,
      themeNames: availableThemes,
    }),
    [
      current,
      themeName,
      setTheme,
      resolvedTerminalTheme,
      terminalThemeName,
      setTerminalTheme,
      availableThemes,
    ],
  );

  return <ThemeContext.Provider value={contextValue}>{children}</ThemeContext.Provider>;
}

/** Hook to access ThemeContext. Throws if used outside ThemeProvider. */
export function useTheme(): ThemeContextType {
  const ctx = useContext(ThemeContext);
  if (!ctx) throw new Error("useTheme must be used within ThemeProvider");
  return ctx;
}
