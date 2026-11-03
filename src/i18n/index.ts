import { Fragment, createElement, type ReactNode } from "react";
import en from "./locales/en.json";

type Catalog = Record<string, string>;

type PluralSuffix = "_one" | "_other";
type Base<K> = K extends `${infer B}${PluralSuffix}` ? B : K;
export type Key = Base<keyof typeof en>;
type Params = Record<string, string | number>;

const catalog = en as Catalog;
const rules = new Intl.PluralRules("en");
const numbers = new Intl.NumberFormat("en");

document.documentElement.lang = "en";
document.documentElement.dir = "ltr";

export const getLang = () => "en";
export const isRtl = () => false;

function lookup(key: Key, p?: Params): string {
  const plural = typeof p?.count === "number" ? `${key}_${rules.select(p.count)}` : null;
  return (plural && catalog[plural]) ?? catalog[key] ?? catalog[`${key}_other`] ?? key;
}

const show = (name: string, v: string | number) => (name === "count" && typeof v === "number" ? numbers.format(v) : String(v));

export function t(key: Key, p?: Params): string {
  return lookup(key, p).replace(/\{(\w+)\}/g, (m, n: string) => (p && n in p ? show(n, p[n]) : m));
}

// -- Main func
export function tx(key: Key, p: Record<string, ReactNode | string | number> = {}, tags: Record<string, (s: string) => ReactNode> = {}): ReactNode {
  const text = lookup(key, p as Params);
  const out: ReactNode[] = [];
  let last = 0;
  for (const m of text.matchAll(/<(\w+)>(.*?)<\/\1>|\{(\w+)\}/g)) {
    out.push(text.slice(last, m.index));
    if (m[1]) out.push(tags[m[1]] ? tags[m[1]](m[2]) : m[2]);
    else {
      const v = p[m[3]];
      out.push(v === undefined ? m[0] : typeof v === "number" ? show(m[3], v) : v);
    }
    last = m.index + m[0].length;
  }
  out.push(text.slice(last));
  return createElement(Fragment, null, ...out);
}

export const fmtNum = (n: number, o?: Intl.NumberFormatOptions) => n.toLocaleString("en", o);
export const fmtDate = (d: Date | number, o?: Intl.DateTimeFormatOptions) => new Date(d).toLocaleString("en", o);
export const fmtList = (items: string[], type: Intl.ListFormatType = "conjunction") => new Intl.ListFormat("en", { type }).format(items);
