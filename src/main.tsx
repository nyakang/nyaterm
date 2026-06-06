import React from "react";
import ReactDOM from "react-dom/client";
import "@fontsource/jetbrains-mono/400.css";
import "@fontsource/jetbrains-mono/500.css";
import "@fontsource/jetbrains-mono/700.css";
import "@fontsource/inter/400.css";
import "@fontsource/inter/500.css";
import "@fontsource/inter/600.css";
import "@fontsource-variable/noto-sans-sc";
import "./i18n";
import ErrorBoundary from "./components/ErrorBoundary";
import { Toaster } from "./components/ui/sonner";
import "./index.css";
import { applyThemeToDOM, THEME_CACHE_KEY, ThemeProvider } from "./context/ThemeContext";
import { DEFAULT_THEME_ID, themes } from "./lib/themes";

// Apply cached theme synchronously before React renders to avoid flash
try {
  const cachedId = localStorage.getItem(THEME_CACHE_KEY);
  const theme = (cachedId && themes[cachedId]) || themes[DEFAULT_THEME_ID];
  applyThemeToDOM(theme.colors);
} catch {}

document.addEventListener("contextmenu", (e) => e.preventDefault());

const params = new URLSearchParams(window.location.search);
const windowType = params.get("window");

if (windowType) {
  // Child window: lightweight provider stack, no full App
  const { ChildAppProvider } = await import("./context/ChildAppProvider");
  const { default: ChildWindowRouter } = await import("./ChildWindowRouter");

  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode>
      <ErrorBoundary>
        <ChildAppProvider>
          <ThemeProvider>
            <ChildWindowRouter windowType={windowType} />
            <Toaster />
          </ThemeProvider>
        </ChildAppProvider>
      </ErrorBoundary>
    </React.StrictMode>,
  );
} else {
  // Main window: full app with all providers
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  const { setOwnerMainWindowLabel } = await import("./lib/windowManager");
  const { AppProvider } = await import("./context/AppContext");
  const { default: App } = await import("./App");
  setOwnerMainWindowLabel(getCurrentWindow().label);

  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode>
      <ErrorBoundary>
        <AppProvider>
          <ThemeProvider>
            <App />
            <Toaster />
          </ThemeProvider>
        </AppProvider>
      </ErrorBoundary>
    </React.StrictMode>,
  );
}
