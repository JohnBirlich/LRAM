import { useEffect, useMemo, useRef, useState } from "react";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { api, events } from "../../api";
import { Icon } from "../../ui/Icon";
import { toast, toastError } from "../../ui/Toast";
import { t } from "../../i18n";
import { saveReport as saveReportFile } from "./report";

const KEEP = 5000;

const HISTORY = 2000;

const LEVELS = ["error", "warn", "info", "debug"] as const;
type Level = (typeof LEVELS)[number];
type Line = { id: number; level: Level; text: string };

const BY_NUMBER: Record<number, Level> = { 1: "debug", 2: "debug", 3: "info", 4: "warn", 5: "error" };

function levelOf(text: string, prev: Level): Level {
  const m = /\[(TRACE|DEBUG|INFO|WARN|ERROR)\]/.exec(text);
  if (!m) return text.startsWith("[fatal]") ? "error" : prev;
  return m[1] === "TRACE" ? "debug" : (m[1].toLowerCase() as Level);
}

const COLOR: Record<Level, string> = { error: "text-err", warn: "text-warn", info: "text-text", debug: "text-dim" };

let nextId = 0;

const CLEARED_KEY = "roalt.debug.clearedAfter";
function clearedAfter(): string | null {
  try {
    return localStorage.getItem(CLEARED_KEY);
  } catch {
    return null;
  }
}
function setClearedAfter(line: string) {
  try {
    localStorage.setItem(CLEARED_KEY, line);
  } catch {
  }
}

function afterMarker(lines: string[], marker: string | null): string[] {
  const at = marker === null ? -1 : lines.lastIndexOf(marker);
  return at < 0 ? lines : lines.slice(at + 1);
}

// -- Main func
export function DebugView() {
  const [lines, setLines] = useState<Line[]>([]);
  const [shown, setShown] = useState<Record<Level, boolean>>({ error: true, warn: true, info: true, debug: false });
  const [query, setQuery] = useState("");
  const [paused, setPaused] = useState(false);
  const box = useRef<HTMLDivElement>(null);
  const held = useRef<Line[]>([]);
  const pausedRef = useRef(paused);
  pausedRef.current = paused;

  useEffect(() => {
    let live = true;
    const add = (more: Line[]) => setLines((l) => [...l, ...more].slice(-KEEP));
    const early: Line[] = [];
    let loaded = false;
    const un = events.onLog((p) => {
      const line = { id: nextId++, level: BY_NUMBER[p.level] ?? levelOf(p.message, "info"), text: p.message };
      if (!loaded) early.push(line);
      else if (pausedRef.current) held.current.push(line);
      else add([line]);
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
        const kept = afterMarker(text, clearedAfter()).length;
        setLines([...history.slice(history.length - kept), ...early].slice(-KEEP));
      })
      .catch(toastError)
      .finally(() => {
        loaded = true;
      });
    return () => {
      live = false;
      un.then((f) => f());
    };
  }, []);

  useEffect(() => {
    if (!paused && held.current.length) {
      const more = held.current;
      held.current = [];
      setLines((l) => [...l, ...more].slice(-KEEP));
    }
  }, [paused]);

  const visible = useMemo(() => {
    const q = query.trim().toLowerCase();
    return lines.filter((l) => shown[l.level] && (!q || l.text.toLowerCase().includes(q)));
  }, [lines, shown, query]);

  const counts = useMemo(() => {
    const c: Record<Level, number> = { error: 0, warn: 0, info: 0, debug: 0 };
    for (const l of lines) c[l.level]++;
    return c;
  }, [lines]);

  useEffect(() => {
    if (!paused && box.current) box.current.scrollTop = box.current.scrollHeight;
  }, [visible, paused]);

  const clear = () => {
    const newest = held.current.at(-1) ?? lines.at(-1);
    if (newest) setClearedAfter(newest.text.split("\n").at(-1) ?? newest.text);
    held.current = [];
    setLines([]);
  };
  const copy = () =>
    writeText(visible.map((l) => l.text).join("\n")).then(() => toast(t("debug.copied")), toastError);
  const copyReport = () =>
    api
      .diagnosticsText()
      .then(writeText)
      .then(() => toast(t("debug.reportCopied")), toastError);

  return (
    <main className="flex-1 min-w-0 flex flex-col px-8 pt-5 pb-6 min-h-0">
      <h1 className="page-title">{t("debug.title")}</h1>
      <p className="text-dim mt-1">{t("debug.intro")}</p>

      <div className="flex flex-wrap items-center gap-2 mt-4">
        {LEVELS.map((lv) => (
          <button
            key={lv}
            className="btn btn-sm mono"
            aria-pressed={shown[lv]}
            style={shown[lv] ? undefined : { opacity: 0.6 }}
            onClick={() => setShown((s) => ({ ...s, [lv]: !s[lv] }))}
          >
            <span className={COLOR[lv]}>{t(`debug.level.${lv}`)}</span>
            <span className="text-dim">{counts[lv]}</span>
          </button>
        ))}
        <input
          className="field min-h-9 flex-1 min-w-[160px]"
          type="search"
          placeholder={t("debug.filter")}
          aria-label={t("debug.filter")}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        <button className="btn btn-sm" aria-pressed={paused} onClick={() => setPaused((p) => !p)}>
          {paused ? t("debug.resume") : t("debug.pause")}
        </button>
        <button className="btn btn-sm" onClick={copy} disabled={!visible.length}>
          <Icon name="copy" />
          {t("debug.copy")}
        </button>
        <button className="btn btn-sm" onClick={clear}>
          <Icon name="trash" />
          {t("debug.clear")}
        </button>
        <button className="btn btn-sm" onClick={() => api.logsOpen().catch(toastError)}>
          <Icon name="external" />
          {t("debug.openFolder")}
        </button>
        <button className="btn btn-sm" title={t("debug.reportHint")} onClick={copyReport}>
          <Icon name="copy" />
          {t("debug.copyReport")}
        </button>
        <button className="btn btn-sm" title={t("debug.reportHint")} onClick={() => void saveReportFile()}>
          <Icon name="download" />
          {t("debug.saveReport")}
        </button>
      </div>

      <div
        ref={box}
        role="log"
        aria-label={t("debug.title")}
        className="selectable mono scroll flex-1 min-h-0 mt-3 p-3 border border-line bg-panel text-xs leading-5"
        style={{ scrollBehavior: "auto" }}
      >
        {visible.length === 0 ? (
          <p className="text-dim">{t("debug.empty")}</p>
        ) : (
          visible.map((l) => (
            <div key={l.id} className={`whitespace-pre-wrap break-all ${COLOR[l.level]}`}>
              {l.text}
            </div>
          ))
        )}
      </div>
    </main>
  );
}
