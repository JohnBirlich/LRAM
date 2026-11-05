import { isPrivate, usePrivacy } from "../../privacy";
import { useState, type CSSProperties } from "react";
import type { AccountStatus, AccountSummary, Presence, PresenceKind } from "../../api";
import { useHeadshot } from "../../data";
import { fmtDate, t, type Key } from "../../i18n";

export const SWATCHES = ["#C6F432", "#7AB0FF", "#5FE0C8", "#FFA05C", "#B49CFF", "#FF8FC7", "#ECEEF1", "#5E6672"];

export const accountName = (a: AccountSummary) => isPrivate() ? "Hidden" : a.alias || a.displayName || a.username;

export function initials(a: AccountSummary) {
  return accountName(a).replace(/[^\p{L}\p{N}]/gu, "").slice(0, 2).toUpperCase() || "?";
}

export const avatarColor = (a: AccountSummary) => a.color ?? SWATCHES[Math.abs(a.userId) % 6];

function inkOn(hex: string) {
  const n = parseInt(hex.slice(1), 16);
  const lum = 0.2126 * ((n >> 16) & 255) + 0.7152 * ((n >> 8) & 255) + 0.0722 * (n & 255);
  return lum > 110 ? "#0A0B0D" : "#FFFFFF";
}

// -- Main func
export function Avatar({ account, size, url }: { account: AccountSummary; size: number; url?: string | null }) {
  const privacy = usePrivacy();
  const fetched = useHeadshot(account.id);
  const src = privacy ? null : fetched ?? url ?? null;
  const [loaded, setLoaded] = useState<string | null>(null);
  const [broken, setBroken] = useState<string | null>(null);
  const bg = privacy ? "#808080" : avatarColor(account);
  const show = src !== null && src !== broken;
  const done = show && loaded === src;
  return (
    <span
      aria-hidden="true"
      className="relative flex-none grid place-content-center font-bold overflow-hidden rounded-full"
      style={{ width: size, height: size, background: done ? "transparent" : bg, color: inkOn(bg), fontSize: Math.round(size * 0.36) }}
    >
      <span className={done ? "opacity-0" : undefined}>{privacy ? "" : initials(account)}</span>
      {show && (
        <img
          key={src}
          src={src}
          alt=""
          loading="lazy"
          decoding="async"
          className={`absolute inset-0 w-full h-full object-cover ${done ? "opacity-100" : "opacity-0"}`}
          onLoad={() => setLoaded(src)}
          onError={() => setBroken(src)}
        />
      )}
    </span>
  );
}

type Look = { label: Key; color: string; mark: CSSProperties };
const hollow: CSSProperties = { border: "1px solid var(--grey)", borderRadius: "50%" };

const LOOK: Record<AccountStatus, Look> = {
  valid: { label: "accounts.status.valid", color: "var(--text)", mark: { background: "var(--accent-text)", borderRadius: "50%" } },
  invalid: { label: "accounts.status.invalid", color: "var(--orange)", mark: { background: "var(--orange)", transform: "rotate(45deg) scale(.85)" } },
  challenged: {
    label: "accounts.status.challenged",
    color: "var(--blue)",
    mark: { background: "var(--blue)", clipPath: "polygon(50% 0, 100% 100%, 0 100%)" },
  },
  unknown: { label: "accounts.status.unknown", color: "var(--dim)", mark: hollow },
};

const PRESENCE: Record<PresenceKind, Look> = {
  inGame: { label: "accounts.presence.inGame", color: "var(--text)", mark: { background: "var(--accent-text)", borderRadius: "50%" } },
  online: { label: "accounts.presence.online", color: "var(--text)", mark: { background: "var(--blue)", borderRadius: "50%" } },
  studio: { label: "accounts.presence.studio", color: "var(--text)", mark: { border: "2px solid var(--blue)", borderRadius: "50%" } },
  offline: { label: "accounts.presence.offline", color: "var(--dim)", mark: hollow },
  unknown: { label: "accounts.presence.unknown", color: "var(--dim)", mark: hollow },
};

function Mark({ look, label, detail }: { look: Look; label: string; detail?: string }) {
  return (
    <span className="inline-flex items-center gap-2 min-w-0 max-w-full" style={{ color: look.color }}>
      <span aria-hidden="true" className="w-2 h-2 flex-none" style={look.mark} />
      <span className="truncate flex-none max-w-[calc(100%-16px)]" title={label}>{label}</span>
      {detail && <span className="mono text-xs text-dim truncate">{detail}</span>}
    </span>
  );
}

export function PresenceStatus({ presence: p }: { presence: Presence }) {
  const privacy = usePrivacy();
  if (privacy) return <span>Hidden</span>;
  const label = p.kind === "inGame" && p.lastLocation ? t("accounts.presence.inGameAt", { place: p.lastLocation }) : t(PRESENCE[p.kind].label);
  return <Mark look={PRESENCE[p.kind]} label={label} />;
}

export function Status({ status, presence, checking, detail }: {
  status: AccountStatus;
  presence?: Presence;
  checking?: boolean;
  detail?: string;
}) {
  const privacy = usePrivacy();
  if (privacy) return <span>Hidden</span>;
  if (checking) {
    return (
      <span className="inline-flex items-center gap-2 min-w-0 max-w-full text-dim">
        <span aria-hidden="true" className="w-2 h-2 flex-none rounded-full bg-dim pulse" />
        <span className="truncate">{t("accounts.status.checking")}</span>
      </span>
    );
  }
  if (status === "valid" && presence && presence.kind !== "unknown") return <PresenceStatus presence={presence} />;
  return <Mark look={LOOK[status]} label={t(LOOK[status].label)} detail={detail} />;
}

export function ago(ms: number | null, now = Date.now()) {
  if (ms === null) return t("common.never");
  const s = Math.max(0, (now - ms) / 1000);
  if (s < 60) return t("accounts.ago.justNow");
  if (s < 3600) return t("accounts.ago.minutes", { count: Math.floor(s / 60) });
  if (s < 86400) return t("accounts.ago.hours", { count: Math.floor(s / 3600) });
  return t("accounts.ago.days", { count: Math.floor(s / 86400) });
}

export const formatDate = (ms: number) => fmtDate(ms, { day: "numeric", month: "short", year: "numeric" });

export function accountAge(created: string, now = new Date()) {
  const c = new Date(created);
  if (Number.isNaN(c.getTime())) return null;
  const m = Math.max(
    0,
    (now.getFullYear() - c.getFullYear()) * 12 + now.getMonth() - c.getMonth() - (now.getDate() < c.getDate() ? 1 : 0),
  );
  return t("accounts.age", { y: Math.floor(m / 12), m: m % 12 });
}
