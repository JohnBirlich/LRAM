import { useEffect, useRef, useState } from "react";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { api, events } from "../api";
import { Icon } from "./Icon";
import { toast, toastError } from "./Toast";
import { t } from "../i18n";

const KEEP = 300;
const HISTORY = 120;

type Level = "error" | "warn" | "info" | "debug";
type Line = { id: number; level: Level; text: string };

const BY_NUMBER: Record<number, Level> = { 1: "debug", 2: "debug", 3: "info", 4: "warn", 5: "error" };

function levelOf(text: string, prev: Level): Level {
  const m = /\[(TRACE|DEBUG|INFO|WARN|ERROR)\]/.exec(text);
  if (!m) return text.startsWith("[fatal]") ? "error" : prev;
  return m[1] === "TRACE" ? "debug" : (m[1].toLowerCase() as Level);
}

let nextId = 0;

// -- Main func
export function LogDock() {
  const [open, setOpen] = useState(false);
  const [lines, setLines] = useState<Line[]>([]);
  const box = useRef<HTMLDivElement>(null);

  useEffect(() => {
    let live = true;
    const add = (more: Line[]) => setLines((l) => [...l, ...more].slice(-KEEP));
    const early: Line[] = [];
    let loaded = false;
    const un = events.onLog((p) => {
      const line = { id: nextId++, level: BY_NUMBER[p.level] ?? levelOf(p.message, "info"), text: p.message };
      if (loaded) add([line]);
      else early.push(line);
    });
    api
      .debugTail(HISTORY)
      .then((text) => {
        if (!live) return;
        let prev: Level = "info";
        const history = text.map((s) => {
          prev = levelOf(s, prev);
          return { id: nextId++, level: prev, text: s };
        });
        setLines([...history, ...early].slice(-KEEP));
      })
      .catch(() => {})
      .finally(() => {
        loaded = true;
      });
    return () => {
      live = false;
      un.then((f) => f());
    };
  }, []);

  useEffect(() => {
    if (open && box.current) box.current.scrollTop = box.current.scrollHeight;
  }, [lines, open]);

  const copy = () =>
    writeText(lines.map((l) => l.text).join("\n")).then(() => toast(t("debug.copied")), toastError);

  return (
    <div className="logdock">
      <button
        className="logdock-head"
        aria-expanded={open}
        onClick={() => setOpen((o) => !o)}
        title={t("debug.title")}
      >
        <Icon name="chevron" size={12} className={open ? "" : "-rotate-90"} />
        <span>{t("debug.title")}</span>
        <span className="ms-auto flex items-center gap-3 normal-case tracking-normal">
          {lines.length > 0 && <span className="mono tabular-nums">{lines.length}</span>}
          {lines.at(-1) && (
            <span className="max-w-[46ch] truncate text-dim">{lines.at(-1)!.text}</span>
          )}
        </span>
      </button>
      {open && (
        <>
          <div className="logdock-body" ref={box} role="log" aria-live="off">
            {lines.length === 0 ? (
              <p className="text-dim">{t("debug.empty")}</p>
            ) : (
              lines.map((l) => (
                <div key={l.id} className="log-line" data-level={l.level}>
                  {l.text}
                </div>
              ))
            )}
          </div>
          <div className="logdock-foot">
            <button className="btn btn-sm" onClick={() => void copy()} disabled={!lines.length}>
              {t("debug.copy")}
            </button>
            <button className="btn btn-sm" onClick={() => setLines([])} disabled={!lines.length}>
              {t("debug.clear")}
            </button>
            <button className="btn btn-sm ms-auto" onClick={() => void api.logsOpen().catch(toastError)}>
              {t("debug.openFolder")}
            </button>
          </div>
        </>
      )}
    </div>
  );
}
