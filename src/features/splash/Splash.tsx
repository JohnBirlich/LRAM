import { useEffect, useState } from "react";
import { api } from "../../api";
import "./splash.css";

const GROW_MS = 3000 + 2000;

const KEY = "lram.splash";

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
  if ((window as { __LRAM_SKIP_SPLASH?: boolean }).__LRAM_SKIP_SPLASH) return true;
  try {
    return sessionStorage.getItem(KEY) !== null;
  } catch {
    return false;
  }
}

// -- Main func
export function Splash() {
  const [state, setState] = useState<"play" | "out" | "gone">(() => (seen() ? "gone" : "play"));

  useEffect(() => {
    if (state !== "play") return;
    const grow = () => {
      try {
        sessionStorage.setItem(KEY, "1");
      } catch {
      }
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
