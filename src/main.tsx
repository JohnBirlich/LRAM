import { Component, StrictMode, type ReactNode } from "react";
import { createRoot } from "react-dom/client";
import "@fontsource-variable/geist";
import "@fontsource-variable/geist-mono";
import "./theme.css";
import "./styles/directions.css";
import App, { THEME_KEY } from "./App";
import { applyDirection, DIRECTION_KEY } from "./ui/DirectionSwitcher";
import { debugLog } from "./api";
import { t } from "./i18n";

// Everything that goes wrong in the UI reaches roalt.log (the Debug tab).
let forwarding = false;
function report(level: "error" | "warn", message: string) {
  // A failure while forwarding must not forward itself again.
  if (forwarding) return;
  forwarding = true;
  try {
    debugLog(level, message);
  } finally {
    forwarding = false;
  }
}
const describe = (e: unknown) => (e instanceof Error ? `${e.name}: ${e.message}${e.stack ? `\n${e.stack}` : ""}` : String(e));
window.addEventListener("error", (e) => report("error", `uncaught ${describe(e.error ?? e.message)} (${e.filename}:${e.lineno}:${e.colno})`));
window.addEventListener("unhandledrejection", (e) => report("error", `unhandled rejection ${describe(e.reason)}`));
for (const level of ["error", "warn"] as const) {
  const original = console[level].bind(console);
  console[level] = (...args: unknown[]) => {
    original(...args);
    report(level, `console.${level}: ${args.map(describe).join(" ")}`);
  };
}

/** A render error logs and shows a way back instead of a blank window. */
class Boundary extends Component<{ children: ReactNode }, { failed: boolean }> {
  state = { failed: false };
  static getDerivedStateFromError() {
    return { failed: true };
  }
  componentDidCatch(e: unknown, info: { componentStack?: string | null }) {
    report("error", `render failed: ${describe(e)}${info.componentStack ?? ""}`);
  }
  render() {
    if (!this.state.failed) return this.props.children;
    return (
      <div className="flex-1 flex flex-col items-center justify-center gap-3 p-10 text-center">
        <p>{t("app.crashed")}</p>
        <button className="btn" onClick={() => this.setState({ failed: false })}>
          {t("app.tryAgain")}
        </button>
      </div>
    );
  }
}

try {
  const t = localStorage.getItem(THEME_KEY);
  // The resolved theme (never "system").
  if (t === "light") document.documentElement.dataset.theme = t;
  // Design direction (dev-only proposals, src/styles/directions.css).
  const d = localStorage.getItem(DIRECTION_KEY);
  if (d) applyDirection(d);
} catch {
  // No storage: the default (dark) applies until settings load.
}

// Plain-browser preview (`npm run dev` outside Tauri). Dead code in production builds.
if (import.meta.env.DEV && !("__TAURI_INTERNALS__" in window)) {
  await import("./dev/mockIpc");
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <Boundary>
      <App />
    </Boundary>
  </StrictMode>,
);
