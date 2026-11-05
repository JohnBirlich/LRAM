import { usePrivacy } from "../../privacy";
import { useEffect, useLayoutEffect, useRef, useState, type CSSProperties, type ReactNode } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { openUrl } from "@tauri-apps/plugin-opener";
import { api, asError, type AccountInfo, type AccountPatch, type AccountSummary, type Group, type Presence, type ServerVisit, type VaultMode } from "../../api";
import { groupColor, qk, useAccountInfo, useBulk, useInstances, useMultiStatus, useSettings, useUpdateAccount } from "../../data";
import { launchBlocked } from "../launch/LaunchView";
import { NumberField, Switch, TextField } from "../../ui/controls";
import { Icon } from "../../ui/Icon";
import { friendly, toast, toastError } from "../../ui/Toast";
import { PasswordDialog } from "../vault/PasswordDialog";
import { fmtDate, fmtList, fmtNum, t, type Key } from "../../i18n";
import { accountAge, ago, Avatar, formatDate, PresenceStatus, Status, SWATCHES } from "./shared";

const REVEAL_SECONDS = 20;
const HEX = /^#[0-9a-f]{6}$/i;
const NO_ANSWER: Key = "accounts.detail.noAnswer";
const STATS: Key[] = [
  "accounts.detail.robux",
  "accounts.detail.friends",
  "accounts.detail.followers",
  "accounts.detail.following",
  "accounts.detail.joined",
  "accounts.detail.age",
];

const clip = (text: string, done: string) => writeText(text).then(() => toast(done), toastError);
const openLink = (url: string) => openUrl(url).catch(toastError);

// -- Main func
export function DetailPanel({ account: a, groups, vaultMode, clipboardSeconds, checking, playing, onValidate, onRemove, onLaunch }: {
  account: AccountSummary;
  groups: Group[];
  vaultMode: VaultMode;
  clipboardSeconds: number;
  checking: boolean;
  playing: boolean;
  onValidate: () => void;
  onRemove: () => void;
  onLaunch: () => void;
}) {
  const privacy = usePrivacy();
  const qc = useQueryClient();
  const update = useUpdateAccount();
  const settings = useSettings().data;
  const save = (patch: AccountPatch) => update.mutate({ id: a.id, patch });
  const info = useAccountInfo(a.id);
  const refresh = useMutation({
    mutationFn: () => api.accountInfo(a.id, true),
    onSuccess: (fresh) => {
      qc.setQueryData(qk.info(a.id), fresh);
      qc.invalidateQueries({ queryKey: qk.headshots });
    },
  });
  const session = useMutation({
    mutationFn: () => api.accountOpenSession(a.id),
    onSuccess: () =>
      toast(
        a.status === "valid"
          ? t("accounts.detail.sessionOpenedValid")
          : t("accounts.detail.sessionOpenedOther"),
      ),
  });
  const [alias, setAlias] = useState(a.alias);
  const [notes, setNotes] = useState(a.notes);
  const [hex, setHex] = useState(a.color ?? "");
  const [cookie, setCookie] = useState<string | null>(null);
  const [ask, setAsk] = useState<"reveal" | "copy" | null>(null);
  const [lastAsk, setLastAsk] = useState<"reveal" | "copy">("reveal");
  const [more, setMore] = useState(false);
  const d = info.data?.userId === a.userId ? info.data : undefined;
  const username = d?.username || a.username;
  const busy = refresh.isPending || info.isFetching;

  useEffect(() => {
    if (!cookie) return;
    const timer = setTimeout(() => setCookie(null), REVEAL_SECONDS * 1000);
    return () => clearTimeout(timer);
  }, [cookie]);

  const reveal = async (password?: string) => setCookie(await api.accountRevealCookie(a.id, password));
  const copy = async (password?: string) => {
    await api.accountCopyCookie(a.id, password);
    toast(t("accounts.detail.cookieCopied", { count: clipboardSeconds }));
  };
  const start = (kind: "reveal" | "copy") => {
    if (vaultMode === "master") {
      setLastAsk(kind);
      setAsk(kind);
    } else {
      (kind === "reveal" ? reveal() : copy()).catch(toastError);
    }
  };

  const commitHex = () => {
    const v = hex.trim();
    if (v === (a.color ?? "")) return;
    if (v === "" || HEX.test(v)) save({ color: v.toLowerCase() });
    else setHex(a.color ?? "");
  };

  const toggleGroup = (gid: number) =>
    save({ groupIds: a.groupIds.includes(gid) ? a.groupIds.filter((x) => x !== gid) : [...a.groupIds, gid] });

  const copyId = () => clip(String(a.userId), t("accounts.menu.userIdCopied"));
  const copyName = () => clip(username, t("accounts.menu.usernameCopied"));

  if (privacy) return <aside className="panel p-5"><p>Hidden</p><p className="text-dim">Disable Privacy mode to view or edit account details.</p></aside>;
  return (
    <div className="enter flex flex-col">
      <div className="flex gap-4 items-start p-5">
        <Avatar account={a} size={72} url={d?.headshotUrl} />
        <div className="min-w-0 flex flex-col gap-1">
          <div className="text-xl font-semibold tracking-[-0.01em] truncate">{d?.displayName || a.displayName}</div>
          <div className="flex items-center gap-1.5 min-w-0">
            <span className="mono text-dim truncate selectable">@{username}</span>
            <CopyButton label={t("accounts.menu.copyUsername")} onClick={copyName} />
          </div>
          <div className="flex items-center gap-1.5">
            <span className="mono text-xs text-dim selectable">{t("accounts.detail.userId", { id: a.userId })}</span>
            <CopyButton label={t("accounts.menu.copyUserId")} onClick={copyId} />
          </div>
        </div>
      </div>

      <Section>
        <div className="flex items-center justify-between gap-2">
          <Status status={a.status} checking={checking} />
          <span className="mono text-[11px] text-dim">{t("accounts.detail.checked", { when: ago(a.statusCheckedAt) })}</span>
        </div>
        {a.status === "valid" ? (
          <button className="btn btn-primary" title={t("accounts.detail.launchHint")} disabled={playing} onClick={onLaunch}>
            <Icon name="play" size={12} fill />
            {playing ? t("play.already") : t("common.launch")}
          </button>
        ) : (
          <button className="btn btn-primary" title={t("accounts.detail.openSessionHint")} disabled={session.isPending} onClick={() => session.mutate()}>
            <Icon name="external" size={14} />
            {t("accounts.detail.signInAgain")}
          </button>
        )}
        <button className="btn" onClick={onValidate} disabled={checking}>
          <Icon name="refresh" size={14} className={checking ? "spin" : ""} />
          {t("accounts.menu.validate")}
        </button>
      </Section>

      <Section>
        <div className="flex items-center justify-between gap-3">
          <span className="flex flex-col gap-0.5 min-w-0 break-words">
            <span className="font-medium">{t("accounts.detail.fullQuality")}</span>
            <span className="text-[13px] text-dim">{t("accounts.detail.fullQualityHint")}</span>
          </span>
          <Switch label={t("accounts.detail.fullQuality")} checked={a.fullQuality} onChange={(fullQuality) => save({ fullQuality })} />
        </div>
        <div className="flex items-center justify-between gap-3">
          <span className="flex flex-col gap-0.5 min-w-0 break-words">
            <span className="font-medium">{t("accounts.detail.bringBack")}</span>
            <span className="text-[13px] text-dim">{settings && !settings.autoRejoin && !settings.autoRestartOnCrash ? t("accounts.detail.bringBackOff") : t("accounts.detail.bringBackHint")}</span>
          </span>
          <Switch label={t("accounts.detail.bringBack")} checked={!a.allowDisconnect} onChange={(v) => save({ allowDisconnect: !v })} />
        </div>
      </Section>

      <ServerHistory accountId={a.id} />

      <div className="px-5 pt-3">
        <button className="btn btn-ghost w-full" aria-expanded={more} onClick={() => setMore(!more)}>
          {more ? t("accounts.detail.less") : t("accounts.detail.more")}
          <Icon name="chevron" size={12} className={more ? "rotate-180" : ""} />
        </button>
      </div>

      {more && (
      <div className="enter flex flex-col">
      <div className="flex flex-col gap-1 px-5 py-2.5 border-b border-line">
        <div className="flex items-center justify-between gap-2">
          <span className="mono text-[11px] text-dim">
            {d ? t("accounts.detail.updated", { when: ago(d.fetchedAt) }) : info.isError ? t("accounts.detail.notLoaded") : t("common.loading")}
          </span>
          <button className="btn btn-sm" onClick={() => refresh.mutate()} disabled={busy} title={t("accounts.detail.refreshHint")}>
            <Icon name="refresh" size={12} className={busy ? "spin" : ""} />
            {t("accounts.detail.refresh")}
          </button>
        </div>
        {d && d.unavailable.length > 0 && (
          <div className="text-xs text-dim">{t("accounts.detail.unavailable", { parts: fmtList(d.unavailable.map(partName)) })}</div>
        )}
        <div className="mono text-[11px] text-dim">{t("accounts.detail.addedOn", { date: formatDate(a.createdAt) })}</div>
      </div>

      {d ? (
        <Info d={d} />
      ) : info.isError ? (
        <div className="flex flex-col items-start gap-3 px-5 py-4 border-b border-line">
          <div role="alert" className="alert">{friendly(asError(info.error))}</div>
          <button className="btn btn-sm" onClick={() => info.refetch()} disabled={info.isFetching}>
            <Icon name="refresh" size={12} className={info.isFetching ? "spin" : ""} />
            {t("common.tryAgain")}
          </button>
        </div>
      ) : (
        <InfoSkeleton />
      )}

      <Section>
        <div className="grid grid-cols-2 gap-2">
          <ActionButton icon="external" label={t("accounts.menu.openProfile")} onClick={() => openLink(`https://www.roblox.com/users/${a.userId}/profile`)} />
          <ActionButton
            icon="external"
            label={t("accounts.menu.openSession")}
            title={t("accounts.detail.openSessionHint")}
            disabled={session.isPending}
            onClick={() => session.mutate()}
          />
          <ActionButton icon="copy" label={t("accounts.menu.copyUserId")} onClick={copyId} />
          <ActionButton icon="copy" label={t("accounts.menu.copyUsername")} onClick={copyName} />
        </div>
      </Section>

      <Section>
        <label className="flex flex-col gap-1.5">
          <span className="label">{t("accounts.detail.alias")}</span>
          <input
            className="field"
            value={alias}
            placeholder={a.displayName}
            maxLength={64}
            onChange={(e) => setAlias(e.target.value)}
            onBlur={() => alias.trim() !== a.alias && save({ alias: alias.trim() })}
            onKeyDown={(e) => {
              if (e.key === "Enter") e.currentTarget.blur();
              if (e.key === "Escape" && alias !== a.alias) {
                e.stopPropagation();
                setAlias(a.alias);
              }
            }}
          />
        </label>
        <label className="flex flex-col gap-1.5">
          <span className="label">{t("accounts.detail.notes")}</span>
          <textarea
            className="field"
            rows={3}
            value={notes}
            maxLength={2000}
            onChange={(e) => setNotes(e.target.value)}
            onBlur={() => notes !== a.notes && save({ notes })}
          />
        </label>
        <div className="flex flex-col gap-1.5">
          <span className="label" id="colour-label">{t("accounts.detail.colour")}</span>
          <div className="flex flex-wrap items-center gap-1.5" role="group" aria-labelledby="colour-label">
            {SWATCHES.map((c) => (
              <button
                key={c}
                className="swatch"
                style={{ background: c }}
                aria-label={t("accounts.groups.colourValue", { color: c })}
                aria-pressed={a.color?.toLowerCase() === c.toLowerCase()}
                onClick={() => {
                  setHex(c.toLowerCase());
                  save({ color: c.toLowerCase() });
                }}
              />
            ))}
            <input
              className="field mono min-h-8 w-[92px] px-2 text-xs"
              aria-label={t("accounts.detail.customColour")}
              placeholder="#rrggbb"
              value={hex}
              maxLength={7}
              spellCheck={false}
              aria-invalid={hex !== "" && !HEX.test(hex)}
              onChange={(e) => setHex(e.target.value)}
              onBlur={commitHex}
              onKeyDown={(e) => e.key === "Enter" && e.currentTarget.blur()}
            />
          </div>
        </div>
        <div className="flex flex-col gap-1.5">
          <span className="label" id="groups-label">{t("accounts.groups.title")}</span>
          {groups.length === 0 ? (
            <span className="text-dim text-xs">{t("accounts.detail.noGroups")}</span>
          ) : (
            <div className="flex flex-wrap gap-1.5" role="group" aria-labelledby="groups-label">
              {groups.map((g) => (
                <button key={g.id} className="chip chip-sm" aria-pressed={a.groupIds.includes(g.id)} onClick={() => toggleGroup(g.id)}>
                  <span aria-hidden="true" className="w-2 h-2 rounded-full" style={{ background: groupColor(g) }} />
                  {g.name}
                </button>
              ))}
            </div>
          )}
        </div>
        <div className="flex items-center justify-between gap-2">
          <span>{t("accounts.detail.favourite")}</span>
          <Switch label={t("accounts.detail.favourite")} checked={a.favourite} onChange={(v) => save({ favourite: v })} />
        </div>
        <div className="flex items-center justify-between gap-2">
          <span className="flex flex-col gap-0.5 min-w-0 break-words">
            <span>{t("accounts.detail.muteDiscord")}</span>
            <span className="text-xs text-dim">{t("accounts.detail.muteDiscordHint")}</span>
          </span>
          <Switch label={t("accounts.detail.muteDiscord")} checked={a.discordMuted} onChange={(v) => save({ discordMuted: v })} />
        </div>
        <label className="flex flex-col gap-1.5">
          <span className="label">{t("accounts.detail.launchArgs")}</span>
          <TextField label={t("accounts.detail.launchArgs")} placeholder={t("accounts.detail.launchArgsDefault")} value={a.launchArgs ?? ""} onCommit={(launchArgs) => update.mutateAsync({ id: a.id, patch: { launchArgs } })} />
        </label>
        <div className="flex items-center justify-between gap-2">
          <span className="flex flex-col gap-0.5 min-w-0 break-words">
            <span>{t("accounts.detail.fpsCap")}</span>
            <span className="text-xs text-dim">{t("accounts.detail.fpsCapHint")}</span>
          </span>
          <NumberField unit={t("accounts.detail.fpsUnit")} short="FPS" min={0} max={360} value={a.fpsCap ?? 0} onCommit={(fpsCap) => save({ fpsCap })} />
        </div>
      </Section>

      <Section>
        <div className="flex items-center justify-between">
          <span className="label">{t("accounts.detail.cookie")}</span>
          <span className="mono text-[11px] text-dim">
            {a.status === "invalid" ? t("accounts.detail.cookieExpired") : t("accounts.detail.cookieEncrypted")}
          </span>
        </div>
        {cookie ? (
          <div className="enter well flex flex-col overflow-hidden">
            <div className="mono text-[11px] leading-snug p-2.5 max-h-28 overflow-auto break-all selectable scroll">{cookie}</div>
            <div className="h-0.5 bg-line">
              <div className="h-full bg-accent-text drain" />
            </div>
          </div>
        ) : (
          <span className="field mono flex items-center text-dim tracking-[0.1em] overflow-hidden">
            ••••••••••••••••••••
          </span>
        )}
        <div className="grid grid-cols-2 gap-2">
          <button className="btn" onClick={() => (cookie ? setCookie(null) : start("reveal"))}>
            <Icon name={cookie ? "eyeOff" : "lock"} size={14} />
            {cookie ? t("accounts.detail.hide") : t("accounts.detail.reveal")}
          </button>
          <button className="btn" onClick={() => start("copy")}>
            <Icon name="copy" size={14} />
            {t("common.copy")}
          </button>
        </div>
        <div className="text-xs text-dim">
          {cookie
            ? t("accounts.detail.cookieHides", { count: REVEAL_SECONDS })
            : vaultMode === "master"
              ? t("accounts.detail.cookieAskMaster")
              : t("accounts.detail.cookieAskHello")}
        </div>
      </Section>

      </div>
      )}

      <div className="p-5">
        <button className="btn btn-danger w-full" onClick={onRemove}>
          <Icon name="trash" size={14} />
          {t("accounts.detail.remove")}
        </button>
      </div>

      <PasswordDialog
        open={ask !== null}
        onClose={() => setAsk(null)}
        title={lastAsk === "reveal" ? t("accounts.detail.revealTitle") : t("accounts.detail.copyTitle")}
        text={lastAsk === "reveal" ? t("accounts.detail.revealText", { name: a.username }) : t("accounts.detail.copyText", { name: a.username })}
        submitLabel={lastAsk === "reveal" ? t("accounts.detail.reveal") : t("common.copy")}
        current
        onSubmit={({ current }) => (lastAsk === "reveal" ? reveal(current) : copy(current))}
      />
    </div>
  );
}

function Info({ d }: { d: AccountInfo }) {
  const joined = d.created ? new Date(d.created) : null;
  const when = joined && !Number.isNaN(joined.getTime()) ? joined : null;
  const stats: Stat[] = [
    num("accounts.detail.robux", d.robux),
    num("accounts.detail.friends", d.friends),
    num("accounts.detail.followers", d.followers),
    num("accounts.detail.following", d.following),
    ["accounts.detail.joined", when ? fmtDate(when, { month: "short", year: "numeric" }) : null, when ? fmtDate(when, { dateStyle: "long" }) : undefined],
    ["accounts.detail.age", d.created ? accountAge(d.created) : null],
  ];
  return (
    <>
      <div className="flex flex-wrap gap-1.5 px-5 py-3.5 border-b border-line">
        {badges(d).map((b) => (
          <span key={b.label} className="tag h-6 px-2" style={b.tone && TONE[b.tone]} title={b.title}>
            {b.label}
          </span>
        ))}
      </div>
      <div className="grid grid-cols-3 gap-px border-b border-line">
        {stats.map(([k, v, title]) => (
          <div key={k} className="min-w-0 px-4 py-3">
            <div className="text-xs text-dim truncate" title={t(k)}>{t(k)}</div>
            <div className="mono text-[15px] font-medium mt-0.5 truncate" title={v === null ? t(NO_ANSWER) : title}>
              {v ?? "—"}
            </div>
          </div>
        ))}
      </div>
      <PresenceBlock p={d.presence} />
      {d.description.trim() && <About text={d.description} />}
    </>
  );
}

type Stat = [Key, string | null, string?];

function num(k: Key, n: number | null): Stat {
  if (n === null) return [k, null];
  if (n < 100_000) return [k, fmtNum(n)];
  return [k, fmtNum(n, { notation: "compact", maximumFractionDigits: 1 }), fmtNum(n)];
}

type Badge = { label: string; tone?: keyof typeof TONE; title?: string };
const TONE: Record<"on" | "warn", CSSProperties> = {
  on: { color: "var(--text)" },
  warn: { color: "var(--orange)" },
};

function badges(d: AccountInfo): Badge[] {
  const out: Badge[] = [];
  if (d.isBanned) out.push({ label: t("accounts.badge.banned"), tone: "warn" });
  if (d.subscription === "plus") out.push({ label: t("accounts.badge.plus"), tone: "on" });
  if (d.subscription === "premium") out.push({ label: t("accounts.badge.premium"), tone: "on" });
  const m = d.twoStepMethods;
  out.push(
    m === null
      ? { label: t("accounts.badge.twoStepUnknown"), title: t(NO_ANSWER) }
      : m.length
        ? { label: t("accounts.badge.twoStepOn"), tone: "on", title: fmtList(m) }
        : { label: t("accounts.badge.twoStepOff") },
  );
  out.push(
    d.emailVerified === null
      ? { label: t("accounts.badge.emailUnknown"), title: t(NO_ANSWER) }
      : d.emailVerified
        ? { label: t("accounts.badge.emailVerified"), tone: "on" }
        : { label: t("accounts.badge.emailNotVerified") },
  );
  if (d.verifiedBadge) out.push({ label: t("accounts.badge.verified"), tone: "on" });
  return out;
}

function PresenceBlock({ p }: { p: Presence | null }) {
  const job = p?.gameId ?? null;
  const place = p?.placeId ?? null;
  return (
    <div className="flex flex-col gap-2.5 px-5 py-4 border-b border-line">
      <span className="label">{t("accounts.detail.presence")}</span>
      {!p ? (
        <span className="text-dim self-start" title={t(NO_ANSWER)}>—</span>
      ) : p.kind !== "inGame" ? (
        <PresenceStatus presence={p} />
      ) : (
        <div className="flex flex-col gap-1.5">
          <div className="flex items-center gap-2 min-w-0">
            <span aria-hidden="true" className="w-2 h-2 flex-none rounded-full bg-accent-text" />
            {p.lastLocation && <span className="sr-only">{t("accounts.detail.inGameSr")}</span>}
            <span className="font-semibold truncate" title={p.lastLocation ?? undefined}>{p.lastLocation ?? t("accounts.presence.inGame")}</span>
          </div>
          {place !== null && (
            <div className="mono text-xs text-dim truncate selectable">
              {job
                ? t("accounts.detail.placeJob", { place, job: `${job.slice(0, 8)}…${job.slice(-4)}` })
                : t("accounts.detail.place", { place })}
            </div>
          )}
          {(place !== null || job) && (
            <div className="flex gap-2 mt-1">
              {place !== null && (
                <button className="btn flex-1 min-w-0" onClick={() => openLink(`https://www.roblox.com/games/${place}`)}>
                  <span className="truncate">{t("accounts.detail.openGame")}</span>
                  <Icon name="external" size={12} />
                </button>
              )}
              {job && (
                <button className="btn" onClick={() => clip(job, t("accounts.detail.jobIdCopied"))} aria-label={t("accounts.detail.copyJobId")} title={t("accounts.detail.copyJobId")}>
                  <Icon name="copy" size={12} />
                  {t("accounts.detail.jobId")}
                </button>
              )}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function About({ text }: { text: string }) {
  const p = useRef<HTMLParagraphElement>(null);
  const [cut, setCut] = useState(false);
  const [more, setMore] = useState(false);
  useLayoutEffect(() => {
    const el = p.current;
    if (el) setCut(el.scrollHeight > el.clientHeight + 1);
  }, [text]);
  return (
    <div className="flex flex-col gap-1.5 px-5 py-4 border-b border-line">
      <span className="label">{t("accounts.detail.about")}</span>
      <p ref={p} className={`whitespace-pre-line break-words selectable ${more ? "" : "line-clamp-3"}`}>{text}</p>
      {cut && (
        <button className="linkbtn self-start text-xs text-dim" aria-expanded={more} onClick={() => setMore(!more)}>
          {more ? t("accounts.detail.showLess") : t("accounts.detail.showMore")}
        </button>
      )}
    </div>
  );
}

function InfoSkeleton() {
  return (
    <div aria-busy="true" aria-label={t("accounts.detail.loadingInfo")}>
      <div className="flex gap-1.5 px-5 py-3.5 border-b border-line">
        {[56, 64, 96].map((w) => (
          <span key={w} className="skel h-6" style={{ width: w }} />
        ))}
      </div>
      <div className="grid grid-cols-3 gap-px border-b border-line">
        {STATS.map((k) => (
          <div key={k} className="px-4 py-3">
            <div className="text-xs text-dim truncate">{t(k)}</div>
            <div className="skel h-[18px] w-12 mt-1" />
          </div>
        ))}
      </div>
      <div className="flex flex-col gap-2.5 px-5 py-4 border-b border-line">
        <span className="label">{t("accounts.detail.presence")}</span>
        <span className="skel h-[18px] w-36" />
      </div>
    </div>
  );
}

function CopyButton({ label, onClick }: { label: string; onClick: () => void }) {
  return (
    <button className="wc w-7 h-7" aria-label={label} title={label} onClick={onClick}>
      <Icon name="copy" size={12} />
    </button>
  );
}

function ActionButton({ icon, label, title, disabled, onClick }: {
  icon: "external" | "copy";
  label: string;
  title?: string;
  disabled?: boolean;
  onClick: () => void;
}) {
  return (
    <button className="btn min-w-0 px-2.5 py-1 whitespace-normal leading-tight" title={title ?? label} disabled={disabled} onClick={onClick}>
      {icon === "copy" && <Icon name="copy" size={12} />}
      <span className="min-w-0">{label}</span>
      {icon === "external" && <Icon name="external" size={12} />}
    </button>
  );
}

function ServerHistory({ accountId }: { accountId: number }) {
  const servers = useQuery({
    queryKey: qk.serverHistory(accountId),
    queryFn: () => api.serverHistory(accountId),
    staleTime: 0,
    refetchInterval: 15_000,
  }).data;
  if (!servers?.length) return null;
  return (
    <Section>
      <span className="label">{t("accounts.history.title")}</span>
      <ul className="flex flex-col gap-2">
        {servers.slice(0, 10).map((v) => (
          <ServerRow key={`${v.jobId}-${v.joinedAt}`} accountId={accountId} visit={v} />
        ))}
      </ul>
    </Section>
  );
}

function ServerRow({ accountId, visit: v }: { accountId: number; visit: ServerVisit }) {
  const game = useQuery({ queryKey: qk.game(v.placeId), queryFn: () => api.gameInfo(v.placeId) }).data;
  const [busy, setBusy] = useState(false);
  const multi = useMultiStatus().data;
  const batch = useBulk().data;
  const others = (useInstances().data ?? []).some((i) => i.state !== "exited" && i.accountId !== accountId);
  const blocked = launchBlocked(others, multi, batch);
  const rejoin = () => {
    setBusy(true);
    api
      .launch(accountId, { kind: "server", placeId: v.placeId, jobId: v.jobId })
      .then(() => toast(t("accounts.history.joining")), toastError)
      .finally(() => setBusy(false));
  };
  return (
    <li className="flex items-center justify-between gap-2">
      <span className="flex flex-col gap-0.5 min-w-0">
        <span className="truncate">{game?.name ?? t("dashboard.place", { id: v.placeId })}</span>
        <span className="mono text-[11px] text-dim">{ago(v.joinedAt)}</span>
      </span>
      {v.private ? (
        <span className="text-xs text-dim">{t("accounts.history.private")}</span>
      ) : (
        <button className="btn btn-sm" onClick={rejoin} disabled={busy || blocked !== null} title={blocked ?? undefined}>
          {t("accounts.history.rejoin")}
        </button>
      )}
    </li>
  );
}

function Section({ children }: { children: ReactNode }) {
  return <div className="flex flex-col gap-3 px-5 py-4 border-b border-line">{children}</div>;
}

const PARTS: Record<string, Key> = {
  profile: "accounts.part.profile",
  avatar: "accounts.part.avatar",
  robux: "accounts.part.robux",
  friends: "accounts.part.friends",
  presence: "accounts.part.presence",
  email: "accounts.part.email",
  "2-step": "accounts.part.twoStep",
  subscription: "accounts.part.subscription",
};
const partName = (id: string) => (PARTS[id] ? t(PARTS[id]) : id);
