import { useEffect, useRef, useState, type FormEvent } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { open as openFile } from "@tauri-apps/plugin-dialog";
import { api, asError, type AccountSummary, type ImportReport } from "../../api";
import { qk } from "../../data";
import { Dialog } from "../../ui/Dialog";
import { Icon } from "../../ui/Icon";
import { friendly, toast } from "../../ui/Toast";
import { fmtList, t, tx } from "../../i18n";

export type Tab = "login" | "cookie" | "many";

const RAM_FILTER = [{ name: "Roblox Account Manager", extensions: ["json"] }];

const CREDENTIALS = /^\s*[^\s:@]{1,64}\s*:\s*[^\s:]+/m;
const looksLikeCredentials = (text: string) => !text.includes("_|WARNING:-DO-NOT-SHARE-THIS.") && CREDENTIALS.test(text);

// -- Main func
export function AddAccountDialog({ open, startOn = "login", onClose, onAdded }: {
  open: boolean;
  startOn?: Tab;
  onClose: () => void;
  onAdded: (a: AccountSummary) => void;
}) {
  const qc = useQueryClient();
  const [tab, setTab] = useState<Tab>(startOn);
  const [error, setError] = useState<string | null>(null);
  const [wasOpen, setWasOpen] = useState(open);
  if (open !== wasOpen) {
    setWasOpen(open);
    if (open) {
      setTab(startOn);
      setError(null);
    }
  }
  const stillOpen = useRef(open);
  stillOpen.current = open;
  const [waiting, setWaiting] = useState(false);
  const [busy, setBusy] = useState(false);
  const [cookie, setCookie] = useState("");
  const [many, setMany] = useState("");

  useEffect(() => {
    if (open) return;
    setCookie("");
    setMany("");
    setError(null);
  }, [open]);

  const added = (a: AccountSummary) => {
    qc.invalidateQueries({ queryKey: qk.accounts });
    qc.invalidateQueries({ queryKey: qk.vault });
    toast(t("accounts.add.added", { name: a.username }));
    if (stillOpen.current) {
      onAdded(a);
      onClose();
    }
  };

  async function signIn() {
    setError(null);
    setWaiting(true);
    try {
      added(await api.accountAddLogin());
    } catch (e) {
      const err = asError(e);
      if (err.kind !== "cancelled") setError(friendly(err));
    } finally {
      setWaiting(false);
    }
  }

  async function paste(e: FormEvent) {
    e.preventDefault();
    const value = cookie;
    setCookie("");
    if (!value.trim() || busy) return;
    setError(null);
    if (looksLikeCredentials(value)) {
      setError(t("accounts.add.credentialsNotCookie"));
      return;
    }
    setBusy(true);
    try {
      added(await api.accountAddCookie(value));
    } catch (e) {
      setError(friendly(asError(e)));
    } finally {
      setBusy(false);
    }
  }

  const imported = (r: ImportReport) => {
    qc.invalidateQueries({ queryKey: qk.accounts });
    qc.invalidateQueries({ queryKey: qk.vault });
    const parts = [
      t("accounts.add.importAdded", { count: r.added }),
      r.updated && t("accounts.add.importRefreshed", { count: r.updated }),
      r.skipped && t("accounts.add.importRepeated", { count: r.skipped }),
      r.failed && t("accounts.add.importFailed", { count: r.failed }),
    ].filter((p): p is string => !!p);
    toast(t("accounts.add.importResult", { list: fmtList(parts) }));
    if (r.added + r.updated > 0) onClose();
  };

  async function runImport(run: () => Promise<ImportReport>) {
    setError(null);
    setBusy(true);
    try {
      imported(await run());
    } catch (e) {
      setError(friendly(asError(e)));
    } finally {
      setBusy(false);
    }
  }

  function pasteMany(e: FormEvent) {
    e.preventDefault();
    const value = many;
    setMany("");
    if (!value.trim() || busy) return;
    if (looksLikeCredentials(value)) {
      setError(t("accounts.add.credentialsNotCookie"));
      return;
    }
    runImport(() => api.accountsImportCookies(value));
  }

  async function fromRam() {
    if (busy) return;
    const path = await openFile({ multiple: false, directory: false, filters: RAM_FILTER, title: t("accounts.add.ramPickerTitle") }).catch((e) => {
      setError(friendly(asError(e)));
      return null;
    });
    if (path) runImport(() => api.accountsImportRam(path));
  }

  const choice = (id: Tab, label: string, desc: string, easiest?: boolean) => (
    <button
      type="button"
      className="btn pick"
      aria-pressed={tab === id}
      onClick={() => {
        setTab(id);
        setError(null);
      }}
    >
      <span aria-hidden="true" className="tick tick-round"><Icon name="check" size={14} /></span>
      <span className="flex-1 min-w-0 flex flex-col">
        <span className="flex flex-wrap items-center gap-2">
          {label}
          {easiest && <span className="tag text-accent-text font-medium">{t("accounts.add.easiest")}</span>}
        </span>
        <span className="text-[13px] font-normal text-dim">{desc}</span>
      </span>
    </button>
  );

  return (
    <Dialog open={open} onClose={onClose} title={t("accounts.add.dialogTitle")} width={540}>
      <p className="px-5 pb-3 text-dim">{t("accounts.add.staysHere")}</p>
      <div role="group" aria-label={t("accounts.add.how")} className="flex flex-col gap-3 px-5">
        {choice("login", t("accounts.add.tabLogin"), t("accounts.add.descLogin"), true)}
        {choice("cookie", t("accounts.add.tabCookie"), t("accounts.add.descCookie"))}
        {choice("many", t("accounts.add.tabMany"), t("accounts.add.descMany"))}
      </div>

      {tab === "login" ? (
        <div key="login" className="enter flex flex-col gap-3 p-5">
          {waiting ? (
            <div className="enter flex flex-col gap-2.5 py-1" role="status">
              <div className="live-bar" />
              <span>{t("accounts.add.waiting")}</span>
            </div>
          ) : (
            <button type="button" className="btn btn-primary min-h-[52px]" onClick={signIn} data-autofocus>
              <Icon name="external" size={14} />
              {t("accounts.add.openSignIn")}
            </button>
          )}
          <p className="text-dim text-xs">
            {t("accounts.add.loginPrivate")}
          </p>
          {error && <div role="alert" className="alert">{error}</div>}
        </div>
      ) : tab === "many" ? (
        <form key="many" onSubmit={pasteMany} className="enter flex flex-col gap-3 p-5">
          <label className="flex flex-col gap-1.5">
            <span className="label">{t("accounts.add.manyLabel")}</span>
            <textarea
              className="field secret mono text-xs"
              rows={6}
              value={many}
              onChange={(e) => setMany(e.target.value)}
              autoComplete="off"
              autoCorrect="off"
              autoCapitalize="off"
              spellCheck={false}
              placeholder={t("accounts.add.manyPlaceholder")}
            />
          </label>
          <p className="text-xs text-dim">
            {tx("accounts.add.manyHelp", {}, { mono: (s) => <span className="mono">{s}</span> })}
          </p>
          {busy && <div className="live-bar" role="status" aria-label={t("accounts.add.importing")} />}
          {error && <div role="alert" className="alert">{error}</div>}
          <div className="flex flex-wrap justify-between gap-2 pt-1">
            <button type="button" className="btn" onClick={fromRam} disabled={busy} title={t("accounts.add.ramHint")}>
              <Icon name="upload" size={14} />
              {t("accounts.add.ramImport")}
            </button>
            <button type="submit" className="btn btn-primary" disabled={busy || !many.trim()}>
              {busy ? t("accounts.add.importingEllipsis") : t("accounts.add.import")}
            </button>
          </div>
        </form>
      ) : (
        <form key="cookie" onSubmit={paste} className="enter flex flex-col gap-3 p-5">
          <label className="flex flex-col gap-1.5">
            <span className="label">{t("accounts.add.cookieLabel")}</span>
            <textarea
              className="field secret mono text-xs"
              rows={4}
              value={cookie}
              onChange={(e) => setCookie(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && !e.shiftKey && (e.preventDefault(), e.currentTarget.form?.requestSubmit())}
              autoComplete="off"
              autoCorrect="off"
              autoCapitalize="off"
              spellCheck={false}
              placeholder={t("accounts.add.cookiePlaceholder")}
            />
          </label>
          <p className="text-xs text-dim">
            {tx("accounts.add.cookieHelp", {}, { mono: (s) => <span className="mono">{s}</span> })}
          </p>
          {error && <div role="alert" className="alert">{error}</div>}
          <div className="flex justify-end gap-2 pt-1">
            <button type="button" className="btn" onClick={onClose}>{t("common.cancel")}</button>
            <button type="submit" className="btn btn-primary" disabled={busy || !cookie.trim()}>
              {busy ? t("accounts.status.checking") : t("accounts.add.title")}
            </button>
          </div>
        </form>
      )}
    </Dialog>
  );
}
