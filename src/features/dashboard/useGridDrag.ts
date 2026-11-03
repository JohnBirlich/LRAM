import { useLayoutEffect, useRef, useState, type DragEvent, type KeyboardEvent, type MouseEvent, type PointerEvent } from "react";
import { isRtl } from "../../i18n";

type Rect = Pick<DOMRect, "left" | "top" | "width">;

export function overAt<T extends { getBoundingClientRect(): Rect; offsetTop: number; offsetHeight: number }>(tiles: T[], dragged: T, x: number, y: number) {
  for (let n = 0; n < tiles.length; n++) {
    if (tiles[n] === dragged) continue;
    const r = tiles[n].getBoundingClientRect();
    const height = Math.max(...tiles.filter((t) => t.offsetTop === tiles[n].offsetTop).map((t) => t.offsetHeight));
    if (y < r.top || (y <= r.top + height && isRtl() !== x < r.left + r.width / 2)) return n;
  }
  return tiles.length;
}

export function overRow<T extends { getBoundingClientRect(): Pick<DOMRect, "top" | "height"> }>(tiles: T[], dragged: T, y: number) {
  for (let n = 0; n < tiles.length; n++) {
    if (tiles[n] === dragged) continue;
    const r = tiles[n].getBoundingClientRect();
    if (y < r.top + r.height / 2) return n;
  }
  return tiles.length;
}

type Dragging = { tile: HTMLElement; x: number; y: number; scroller: Element | null; scroll: number; marked: HTMLElement | null };

type Pressed = { x: number; y: number };

const DRAG_FROM = 5;

// -- Main func
export function useGridDrag(onMove: (from: number, over: number) => void, column = false) {
  const grid = useRef<HTMLElement | null>(null);
  const dragging = useRef<Dragging | null>(null);
  const flip = useRef<Map<Element, DOMRect> | null>(null);
  const pressed = useRef<Pressed | null>(null);
  const dragged = useRef(false);
  const [moved, setMoved] = useState("");

  const tiles = () => [...(grid.current?.children ?? [])] as HTMLElement[];
  const over = (all: HTMLElement[], tile: HTMLElement, e: PointerEvent) => (column ? overRow(all, tile, e.clientY) : overAt(all, tile, e.clientX, e.clientY));
  const snapshot = () => void (flip.current = new Map(tiles().map((el) => [el, el.getBoundingClientRect()])));

  const mark = (d: Dragging, target: HTMLElement | null, side?: "before" | "after") => {
    d.marked?.removeAttribute("data-drop");
    if (target) target.dataset.drop = side;
    d.marked = target;
  };

  useLayoutEffect(() => {
    const d = dragging.current;
    if (d && !d.tile.isConnected) {
      mark(d, null);
      dragging.current = null;
    }
    const before = flip.current;
    if (!before) return;
    flip.current = null;
    const els = tiles();
    for (const el of els) {
      el.style.transition = "none";
      el.style.transform = "";
    }
    const deltas = els.map((el) => {
      const [was, r] = [before.get(el), el.getBoundingClientRect()];
      return was ? [was.left - r.left, was.top - r.top] : [0, 0];
    });
    els.forEach((el, n) => {
      const [dx, dy] = deltas[n];
      if (Math.abs(dx) >= 1 || Math.abs(dy) >= 1) el.style.transform = `translate(${dx}px, ${dy}px)`;
    });
    void grid.current?.offsetHeight;
    for (const el of els) {
      el.style.transition = "";
      el.style.transform = "";
    }
  });

  function begin(e: PointerEvent<HTMLButtonElement>, x: number, y: number) {
    const tile = tiles().find((el) => el.contains(e.currentTarget));
    if (!tile) return false;
    e.currentTarget.setPointerCapture(e.pointerId);
    tile.style.transition = "none";
    tile.classList.add("tile-drag");
    const scroller = tile.closest(".scroll");
    dragging.current = { tile, x, y, scroller, scroll: scroller?.scrollTop ?? 0, marked: null };
    return true;
  }

  function down(e: PointerEvent<HTMLButtonElement>) {
    if (e.button !== 0) return;
    if (begin(e, e.clientX, e.clientY)) e.preventDefault();
  }

  function move(e: PointerEvent) {
    const d = dragging.current;
    if (!d) return;
    d.tile.style.transform = `translate(${e.clientX - d.x}px, ${e.clientY - d.y + (d.scroller?.scrollTop ?? 0) - d.scroll}px)`;
    const all = tiles();
    const from = all.indexOf(d.tile);
    const to = over(all, d.tile, e);
    if (to === from || to === from + 1) mark(d, null);
    else if (to < all.length) mark(d, all[to], "before");
    else mark(d, all[all.length - 1], "after");
  }

  function end(e: PointerEvent | null) {
    const d = dragging.current;
    if (!d) return;
    dragging.current = null;
    mark(d, null);
    d.tile.classList.remove("tile-drag");
    const all = tiles();
    const from = all.indexOf(d.tile);
    const to = e ? over(all, d.tile, e) : from;
    if (to === from || to === from + 1) {
      d.tile.style.transition = "";
      d.tile.style.transform = "";
      return;
    }
    snapshot();
    onMove(from, to);
  }

  function key(e: KeyboardEvent<HTMLButtonElement>, name: string) {
    const all = tiles();
    const back = isRtl() ? 1 : -1;
    const step = ({ ArrowLeft: back, ArrowRight: -back, ArrowUp: -1, ArrowDown: 1 } as Record<string, number>)[e.key];
    if (!step) return;
    e.preventDefault();
    const grip = e.currentTarget;
    const from = all.findIndex((el) => el.contains(grip));
    const same = (n: number, m: number) => !!all[n] && all[n].dataset.group !== undefined && all[n].dataset.group === all[m].dataset.group;
    let to = from + step;
    if (e.key === "ArrowUp" || e.key === "ArrowDown") {
      const tops = [...new Set(all.map((el) => el.offsetTop))].sort((a, b) => a - b);
      const row = tops[tops.indexOf(all[from].offsetTop) + step];
      if (row === undefined) return;
      const off = (el: HTMLElement) => Math.abs(el.offsetLeft + el.offsetWidth / 2 - (all[from].offsetLeft + all[from].offsetWidth / 2));
      to = all.reduce((best, el, n) => (el.offsetTop === row && (best < 0 || off(el) < off(all[best])) ? n : best), -1);
    }
    while (same(to, from)) to += step;
    if (to < 0 || to >= all.length) return;
    let over = step < 0 ? to : to + 1;
    if (step > 0) while (same(over, to)) over++;
    snapshot();
    onMove(from, over);
    requestAnimationFrame(() => {
      const now = tiles();
      setMoved(`${name}: ${now.findIndex((el) => el.contains(grip)) + 1} / ${now.length}`);
      grip.focus();
    });
  }

  const grip = (name: string) => ({
    onPointerDown: down,
    onPointerMove: move,
    onPointerUp: (e: PointerEvent) => end(e),
    onPointerCancel: () => end(null),
    onKeyDown: (e: KeyboardEvent<HTMLButtonElement>) => key(e, name),
  });

  const item = (name: string) => ({
    "aria-keyshortcuts": "Alt+ArrowLeft Alt+ArrowRight Alt+ArrowUp Alt+ArrowDown",
    onDragStart: (e: DragEvent) => e.preventDefault(),
    onPointerDown: (e: PointerEvent<HTMLButtonElement>) => {
      dragged.current = false;
      pressed.current = e.button === 0 ? { x: e.clientX, y: e.clientY } : null;
      if (pressed.current) e.currentTarget.setPointerCapture(e.pointerId);
    },
    onPointerMove: (e: PointerEvent<HTMLButtonElement>) => {
      const p = pressed.current;
      if (p && Math.hypot(e.clientX - p.x, e.clientY - p.y) >= DRAG_FROM) {
        pressed.current = null;
        dragged.current = begin(e, p.x, p.y);
      }
      move(e);
    },
    onPointerUp: (e: PointerEvent) => {
      pressed.current = null;
      end(e);
    },
    onPointerCancel: () => {
      pressed.current = null;
      end(null);
    },
    onClickCapture: (e: MouseEvent) => {
      if (!dragged.current) return;
      dragged.current = false;
      e.preventDefault();
      e.stopPropagation();
    },
    onKeyDown: (e: KeyboardEvent<HTMLButtonElement>) => {
      if (!e.altKey) return;
      key(e, name);
      if (e.key.startsWith("Arrow")) e.stopPropagation();
    },
  });

  return { grid, grip, item, moved };
}
