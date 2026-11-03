import { Component, StrictMode, type ReactNode } from "react";
import { createRoot } from "react-dom/client";
import "@fontsource-variable/geist";
import "@fontsource-variable/geist-mono";
import "./theme.css";
import "./styles/directions.css";
import App, { THEME_KEY } from "./App";
import { applyDirection, DEFAULT_DIRECTION, DIRECTION_KEY } from "./ui/DirectionSwitcher";
import { debugLog } from "./api";
import { t } from "./i18n";

let forwarding = false;
// -- Main func
function report(level: "error" | "warn", message: string) {
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
  if (t === "light") document.documentElement.dataset.theme = t;
  applyDirection(localStorage.getItem(DIRECTION_KEY) ?? DEFAULT_DIRECTION);
} catch {
}

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
