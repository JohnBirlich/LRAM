import { useEffect, useState, type SetStateAction } from "react";
import { api, type AccountSummary } from "../../api";
import { EMPTY_DRAFT, targetText, type Draft } from "../launch/target";

const DETAILED = "roalt.homeDetailed";
const ORDER = "roalt.homeOrder";
const SIZES = "roalt.homeSizes";
const TABS = "roalt.homeTabs";
const SECTIONS = "roalt.sectionOrder";

let unsavedSections: string[] | null = null;
let unsavedDetailed: boolean | null = null;
let unsavedOrder: number[] | null = null;
let unsavedSizes: Record<number, number> | null = null;
let unsavedTabs: StoredTab[] | null = null;

export type HomeTab = { accountId: number; draft: Draft; status?: "queued" | "launching" | "failed" };
type StoredTab = { accountId: number; text: string };

function storedDetailed(): boolean {
  if (unsavedDetailed !== null) return unsavedDetailed;
  try {
    return localStorage.getItem(DETAILED) !== "0";
  } catch {
    return true;
  }
}

function storedOrder(): number[] {
  if (unsavedOrder) return unsavedOrder;
  try {
    const v: unknown = JSON.parse(localStorage.getItem(ORDER) ?? "null");
    return Array.isArray(v) ? v.filter((n): n is number => typeof n === "number") : [];
  } catch {
    return [];
  }
}

function storedSections(): string[] {
  if (unsavedSections) return unsavedSections;
  try {
    const v: unknown = JSON.parse(localStorage.getItem(SECTIONS) ?? "null");
    return Array.isArray(v) ? v.filter((s): s is string => typeof s === "string") : [];
  } catch {
    return [];
  }
}

function storedSizes(): Record<number, number> {
  if (unsavedSizes) return unsavedSizes;
  try {
    const v: unknown = JSON.parse(localStorage.getItem(SIZES) ?? "null");
    return v && typeof v === "object" && !Array.isArray(v) ? Object.fromEntries(Object.entries(v).filter(([, w]) => typeof w === "number")) : {};
  } catch {
    return {};
  }
}

export function useHomeSizes(): [Record<number, number>, (id: number, width: number | null) => void] {
  const [sizes, setSizes] = useState(storedSizes);
  return [sizes, (id, width) => {
    const next = { ...sizes };
    if (width === null) delete next[id];
    else next[id] = width;
    unsavedSizes = next;
    setSizes(next);
    try {
      localStorage.setItem(SIZES, JSON.stringify(next));
    } catch {
    }
  }];
}

export function useHomeDetailed(): [boolean, (on: boolean) => void] {
  const [on, setOn] = useState(storedDetailed);
  return [on, (next) => {
    unsavedDetailed = next;
    setOn(next);
    try {
      localStorage.setItem(DETAILED, next ? "1" : "0");
    } catch {
    }
  }];
}

export function useHomeOrder(): [number[], (ids: number[]) => void] {
  const [order, setOrder] = useState(storedOrder);
  return [order, (next) => {
    unsavedOrder = next;
    setOrder(next);
    try {
      localStorage.setItem(ORDER, JSON.stringify(next));
    } catch {
    }
  }];
}

export function homeSorted<T extends { accountId: number }>(list: T[], order: number[]): T[] {
  const at = new Map(order.map((id, n) => [id, n]));
  const rank = (i: T) => at.get(i.accountId) ?? order.length;
  return [...list].sort((a, b) => rank(a) - rank(b));
}

export function byOrder<T>(ids: readonly T[], order: T[]): T[] {
  const at = new Map(order.map((id, n) => [id, n]));
  const rank = (id: T) => at.get(id) ?? order.length;
  return [...ids].sort((a, b) => rank(a) - rank(b));
}

export function useSectionOrder(): [string[], (ids: string[]) => void] {
  const [order, setOrder] = useState(storedSections);
  return [order, (next) => {
    unsavedSections = next;
    setOrder(next);
    try {
      localStorage.setItem(SECTIONS, JSON.stringify(next));
    } catch {
    }
  }];
}

export function reordered<T>(order: T[], shown: T[], from: number, over: number): T[] {
  const id = shown[from];
  const ids = [...new Set([...order, ...shown])].filter((x) => x !== id);
  const next = shown.slice(over).find((x) => x !== id);
  const others = shown.filter((x) => x !== id);
  ids.splice(next !== undefined ? ids.indexOf(next) : ids.indexOf(others[others.length - 1]) + 1, 0, id);
  return ids;
}

export function storedText(d: Draft): string {
  const p = d.parsed?.text === d.text ? d.parsed.value : null;
  switch (p?.kind) {
    case "target": {
      const g = p.target;
      const harmless = g.kind === "server" || g.kind === "universe" || g.kind === "followUser" || (g.kind === "place" && !g.launchData);
      return harmless ? targetText(g) : "";
    }
    case "username":
      return `@${p.name}`;
    default:
      return "";
  }
}

function storedTabs(): HomeTab[] {
  let v: unknown = unsavedTabs;
  if (!v) {
    try {
      v = JSON.parse(localStorage.getItem(TABS) ?? "null");
    } catch {
    }
  }
  if (!Array.isArray(v)) return [];
  return (v as Partial<StoredTab>[])
    .filter((s) => typeof s?.accountId === "number" && typeof s.text === "string")
    .map((s) => ({ accountId: s.accountId!, draft: { ...EMPTY_DRAFT, text: s.text! } }));
}

const unparsed = (d: Draft) => !!d.text.trim() && d.parsed?.text !== d.text;

// -- Main func
export function useHomeTabs(accounts: AccountSummary[] | undefined) {
  const [tabs, setTabs] = useState(storedTabs);
  const [active, setActive] = useState<number | null>(null);
  const [order, setOrder] = useHomeOrder();
  const setDraft =(id: number, f: SetStateAction<Draft>) =>
    setTabs((list) =>
      list.map((tab) => {
        if (tab.accountId !== id) return tab;
        const draft = typeof f === "function" ? f(tab.draft) : f;
        return { ...tab, draft, status: tab.status === "failed" && draft.text !== tab.draft.text ? undefined : tab.status };
      }),
    );

  useEffect(() => {
    if (accounts) setTabs((list) => (list.every((tab) => accounts.some((a) => a.id === tab.accountId)) ? list : list.filter((tab) => accounts.some((a) => a.id === tab.accountId))));
  }, [accounts]);

  useEffect(() => {
    const stale = tabs.filter((tab) => unparsed(tab.draft));
    if (!stale.length) {
      const saved = tabs.map((tab): StoredTab => ({ accountId: tab.accountId, text: storedText(tab.draft) }));
      unsavedTabs = saved;
      try {
        localStorage.setItem(TABS, JSON.stringify(saved));
      } catch {
      }
      return;
    }
    const timer = setTimeout(() => {
      for (const { accountId, draft: { text } } of stale)
        api.targetParse(text).then(
          (value) => setDraft(accountId, (d) => (d.text === text ? { ...d, parsed: { text, value } } : d)),
          () => {},
        );
    }, 150);
    return () => clearTimeout(timer);
  }, [tabs]); // eslint-disable-line react-hooks/exhaustive-deps

  return {
    tabs,
    order,
    setOrder,
    active: tabs.some((tab) => tab.accountId === active) ? active : null,
    setActive,
    setDraft,
    add: (id: number) => {
      setTabs((list) => (list.some((tab) => tab.accountId === id) ? list : [...list, { accountId: id, draft: EMPTY_DRAFT }]));
      setActive(id);
    },
    close: (id: number) => {
      setTabs((list) => list.filter((tab) => tab.accountId !== id));
      setActive((a) => (a === id ? null : a));
    },
    setStatus: (ids: number[], status: HomeTab["status"]) =>
      setTabs((list) => list.map((tab) => (ids.includes(tab.accountId) ? { ...tab, status } : tab))),
  };
}
export type HomeTabs = ReturnType<typeof useHomeTabs>;
