import type { KeyboardEvent } from "react";

// -- Main func
export function menuKeys(e: KeyboardEvent<HTMLElement>) {
  const root = e.currentTarget;
  if (e.key === "Tab") {
    root.hidePopover();
    return;
  }
  const all = [...root.querySelectorAll<HTMLElement>(".menu-item:not(:disabled), [role^=menuitem]:not(:disabled)")];
  const at = all.indexOf(document.activeElement as HTMLElement);
  const next = e.key === "ArrowDown" ? at + 1 : e.key === "ArrowUp" ? (at < 0 ? -1 : at - 1) : e.key === "Home" ? 0 : e.key === "End" ? all.length - 1 : null;
  if (next === null || !all.length) return;
  e.preventDefault();
  all[(next + all.length) % all.length].focus();
}
