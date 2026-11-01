// Dev-only design-direction switcher (proposal tool, not shipped in release builds).
//
// Sets <html data-direction="...">; src/styles/directions.css does the rest. The choice is kept
// in localStorage so a reload in the browser preview stays on the same variant.

import { useState } from "react";

export const DIRECTION_KEY = "roalt.direction";

/** id, label shown on the button, what it is. */
export const DIRECTIONS = [
  { id: "soft", label: "Soft Deck", hint: "как сейчас" },
  { id: "slate", label: "Slate", hint: "плоский инструментальный" },
  { id: "studio", label: "Studio", hint: "мягкий bento, крупные радиусы" },
  { id: "grid", label: "Grid", hint: "тактический HUD, моно" },
] as const;

export type DirectionId = (typeof DIRECTIONS)[number]["id"];

export function applyDirection(id: string) {
  if (id && id !== "soft") document.documentElement.dataset.direction = id;
  else delete document.documentElement.dataset.direction;
}

export function DirectionSwitcher() {
  const [current, setCurrent] = useState<string>(() => document.documentElement.dataset.direction ?? "soft");
  const pick = (id: string) => {
    setCurrent(id);
    applyDirection(id);
    try {
      localStorage.setItem(DIRECTION_KEY, id);
    } catch {
      // No storage: the choice lasts for this page.
    }
  };
  return (
    <div className="dir-switch" role="group" aria-label="Вариант дизайна">
      <span>Дизайн</span>
      {DIRECTIONS.map((d) => (
        <button key={d.id} type="button" aria-pressed={current === d.id} title={d.hint} onClick={() => pick(d.id)}>
          {d.label}
        </button>
      ))}
    </div>
  );
}
