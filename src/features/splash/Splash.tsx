import { useEffect, useState } from "react";
import { api } from "../../api";
import "./splash.css";

/** When the window grows: the animation settles at 3 s (79% of 3.8 s), then holds 2 s. */
const GROW_MS = 3000 + 2000;
/** Set once the splash has run: a webview reload doesn't play it again. */
const KEY = "lram.splash";

/** The LRAM mark in whatever colours a layer of the glitch needs. */
function Tile({ className, blue, light }: { className: string; blue: string; light: string }) {
  return (
    <svg className={className} width="120" height="120" viewBox="0 0 18 18" aria-hidden="true">
      <rect x="2" y="2" width="14" height="14" fill={light} />
      <rect x="2" y="2" width="5.04" height="13.16" fill={blue} />
      <rect x="7.04" y="9" width="7.84" height="6.16" fill={blue} />
    </svg>
  );
}

function seen() {
  // Settings.skip_splash, from `main_window`: the window is already full size.
  if ((window as { __LRAM_SKIP_SPLASH?: boolean }).__LRAM_SKIP_SPLASH) return true;
  try {
    return sessionStorage.getItem(KEY) !== null;
  } catch {
    return false;
  }
}

/** The start-up splash over the app: the tile glitches into the LRAM mark, then the window
 * (opened small by `splash::start`) grows into the app and this fades away. */
export function Splash() {
  const [state, setState] = useState<"play" | "out" | "gone">(() => (seen() ? "gone" : "play"));

  useEffect(() => {
    if (state !== "play") return;
    // Plays even with Windows' animation effects off (theme.css leaves the splash out).
    const grow = () => {
      try {
        sessionStorage.setItem(KEY, "1");
      } catch {
        // No storage: a reload plays it again.
      }
      // Fades once the window has grown, even if growing failed.
      api.splashDone().finally(() => setState("out"));
    };
    const t = setTimeout(grow, GROW_MS);
    return () => clearTimeout(t);
  }, [state]);

  if (state === "gone") return null;
  return (
    <div
      className={`splash ${state === "out" ? "out" : ""}`}
      data-tauri-drag-region
      onTransitionEnd={(e) => e.target === e.currentTarget && setState("gone")}
      aria-hidden="true"
    >
      <div className="mark" data-tauri-drag-region>
        <div className="logo">
          <div className="cube" />
          <div className="tile">
            <Tile className="red" blue="#ff2a4d" light="#ff2a4d" />
            <Tile className="cyan" blue="#22e8ff" light="#22e8ff" />
            <Tile className="core" blue="#3E79C9" light="#98C0FB" />
            <Tile className="slice-a" blue="#eceef1" light="#eceef1" />
            <Tile className="slice-b" blue="#22e8ff" light="#22e8ff" />
          </div>
        </div>
      </div>
      <div className="mark">
        <div className="word">
          <b>LRAM</b>
          <span>v{import.meta.env.APP_VERSION}</span>
        </div>
      </div>
      <div className="scan" />
    </div>
  );
}
