import { useState } from "react";

export const DIRECTION_KEY = "lram.direction";

export const DIRECTIONS = [
  { id: "lram", label: "LRAM", hint: "the new look, from the reference" },
  { id: "soft", label: "Soft Deck", hint: "as it was before the rework" },
] as const;

export type DirectionId = (typeof DIRECTIONS)[number]["id"];

export const DEFAULT_DIRECTION = "lram";

export function applyDirection(id: string) {
  if (id && id !== "soft") document.documentElement.dataset.direction = id;
  else delete document.documentElement.dataset.direction;
}

// -- Main func
export function DirectionSwitcher() {
  const [current, setCurrent] = useState<string>(() => document.documentElement.dataset.direction ?? DEFAULT_DIRECTION);
  const pick = (id: string) => {
    setCurrent(id);
    applyDirection(id);
    try {
      localStorage.setItem(DIRECTION_KEY, id);
    } catch {
    }
  };
  return (
    <div className="dir-switch" role="group" aria-label="Design variant">
      <span>Design</span>
      {DIRECTIONS.map((d) => (
        <button key={d.id} type="button" aria-pressed={current === d.id} title={d.hint} onClick={() => pick(d.id)}>
          {d.label}
        </button>
      ))}
    </div>
  );
}
