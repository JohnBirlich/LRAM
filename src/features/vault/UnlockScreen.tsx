import { useEffect, useRef, useState, type FormEvent } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { api, asError, type VaultStatus } from "../../api";
import { qk } from "../../data";
import { Icon } from "../../ui/Icon";
import { friendly } from "../../ui/Toast";
import { t } from "../../i18n";
import { MIN_PASSWORD } from "./PasswordDialog";

export type LockReason = "start" | "manual" | "idle";

export function UnlockScreen({ status, reason, onRecovered }: { status: VaultStatus; reason: LockReason; onRecovered: (code: string) => void }) {
  const qc = useQueryClient();
  const input = useRef<HTMLInputElement>(null);
  const [recovering, setRecovering] = useState(false);
  const [pw, setPw] = useState("");
  const [peek, setPeek] = useState(false);
  const [code, setCode] = useState("");
  const [confirm, setConfirm] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [shakes, setShakes] = useState(0);
  const [busy, setBusy] = useState(false);
  // Seconds a wrong password keeps the next one waiting; counted down here, so the button and the
  // field wake up by themselves.
  const [wait, setWait] = useState(status.retryIn ?? 0);

  useEffect(() => {
    const total = status.retryIn ?? 0;
    setWait(total);
    if (!total) return;
    const from = Date.now();
    const id = setInterval(() => setWait(Math.max(0, total - Math.floor((Date.now() - from) / 1000))), 250);
    return () => clearInterval(id);
  }, [status.retryIn]);

  useEffect(() => input.current?.focus(), [recovering]);

  const fail = (msg: string) => {
    setError(msg);
    setShakes((n) => n + 1);
  };

  const unlocked = () => {
    setPw("");
    setCode("");
    setConfirm("");
    qc.setQueryData<VaultStatus>(qk.vault, { ...status, locked: false });
    qc.invalidateQueries();
  };

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (busy) return;
    if (recovering) {
      if (!code.trim()) return fail(t("vault.recovery.enterCode"));
      if (pw.length < MIN_PASSWORD) return fail(t("vault.tooShort", { count: MIN_PASSWORD }));
      if (pw !== confirm) return fail(t("vault.mismatch"));
    } else if (!pw) return;
    setBusy(true);
    try {
      if (recovering) onRecovered(await api.vaultRecover(code, pw));
      else await api.vaultUnlock(pw);
      unlocked();
    } catch (err) {
      const e2 = asError(err);
      if (e2.kind !== "wrongPassword") fail(friendly(e2));
      else fail(t(recovering ? "vault.recovery.wrongCode" : "ui.error.wrongPassword"));
      // A refused attempt comes back as a wait: take it from the status rather than guess it.
      qc.invalidateQueries({ queryKey: qk.vault });
      input.current?.select();
    } finally {
      setBusy(false);
    }
  }

  const switchMode = () => {
    setRecovering(!recovering);
    setPw("");
    setCode("");
    setConfirm("");
    setError(null);
  };

  // From the tray with auto-lock off it comes as "idle" too.
  const lead = reason === "idle" && status.autoLockMinutes > 0 ? t("vault.lockedIdle", { count: status.autoLockMinutes }) : reason === "start" ? "" : t("vault.lockedManual");

  return (
    <main className="flex-1 grid place-items-center px-4 py-10 overflow-auto">
      <form
        onSubmit={submit}
        className={`enter ${shakes ? (shakes % 2 ? "shake" : "shake2") : ""} panel w-full max-w-[400px] flex flex-col gap-5 p-8`}
      >
        <div className="flex flex-col gap-3.5">
          <svg width="36" height="36" viewBox="0 0 36 36" aria-hidden="true">
            <rect x="6" y="15" width="24" height="17" rx="3" fill="none" stroke="currentColor" strokeWidth="2" />
            <path d="M11 15V9a7 7 0 0 1 14 0v6" fill="none" stroke="currentColor" strokeWidth="2" />
            <circle cx="18" cy="23" r="2.5" fill="var(--accent-text)" />
          </svg>
          <h1 className="page-title">{recovering ? t("vault.recovery.forgotTitle") : t("vault.title")}</h1>
          <p className="text-dim">
            {recovering ? t("vault.recovery.forgotText") : <>{lead && `${lead} `}{t("vault.prompt", { count: status.accounts })}</>}
          </p>
        </div>

        {recovering && (
          <div className="flex flex-col gap-2">
            <label className="label" htmlFor="unlock-code">{t("vault.recovery.code")}</label>
            <input
              ref={input}
              id="unlock-code"
              className="field mono uppercase tracking-[0.12em]"
              value={code}
              onChange={(e) => {
                setCode(e.target.value);
                setError(null);
              }}
              placeholder="XXX-XXX-XXX"
              autoComplete="off"
              spellCheck={false}
              aria-invalid={!!error}
            />
          </div>
        )}

        <div className="flex flex-col gap-2">
          <label className="label" htmlFor="unlock-pw">{recovering ? t("vault.newPassword") : t("settings.masterPassword")}</label>
          <div className="field flex p-0 overflow-hidden" style={{ borderColor: error && !recovering ? "var(--orange)" : undefined }}>
            <input
              ref={recovering ? undefined : input}
              id="unlock-pw"
              type={peek ? "text" : "password"}
              value={pw}
              onChange={(e) => {
                setPw(e.target.value);
                setError(null);
              }}
              autoComplete={recovering ? "new-password" : "current-password"}
              spellCheck={false}
              aria-invalid={!recovering && !!error}
              aria-describedby={error ? "unlock-err" : undefined}
              className="flex-1 min-w-0 h-11 px-3 bg-transparent outline-none tracking-[0.08em]"
            />
            <button
              type="button"
              className="wc h-11 w-11"
              aria-label={peek ? t("vault.hidePassword") : t("vault.showPassword")}
              aria-pressed={peek}
              onClick={() => setPeek(!peek)}
            >
              <Icon name={peek ? "eyeOff" : "eye"} />
            </button>
          </div>
          {recovering && <span className="text-xs text-dim">{t("vault.minLength", { count: MIN_PASSWORD })}</span>}
        </div>

        {recovering && (
          <div className="flex flex-col gap-2">
            <label className="label" htmlFor="unlock-confirm">{t("vault.confirm")}</label>
            <input
              id="unlock-confirm"
              type={peek ? "text" : "password"}
              className="field"
              value={confirm}
              onChange={(e) => {
                setConfirm(e.target.value);
                setError(null);
              }}
              autoComplete="new-password"
              spellCheck={false}
            />
          </div>
        )}

        {error && <div id="unlock-err" role="alert" className="alert">{error}</div>}
        {wait > 0 && !recovering && (
          <div id="unlock-wait" role="status" className="alert">
            {t("vault.retryIn", { count: wait })}
          </div>
        )}
        {error && !recovering && !status.hasRecovery && <p className="text-[13px] text-dim -mt-3">{t("vault.forgot")}</p>}

        <button type="submit" className="btn btn-primary" disabled={busy || (wait > 0 && !recovering)}>
          {recovering
            ? busy ? t("vault.working") : t("vault.recovery.forgotSubmit")
            : busy ? t("vault.unlocking") : t("vault.unlock")}
        </button>

        {status.hasRecovery && (
          <button type="button" className="btn self-center" onClick={switchMode} disabled={busy}>
            {recovering ? t("vault.recovery.backToPassword") : t("vault.recovery.forgotLink")}
          </button>
        )}

        <div className="mono flex flex-col gap-1 pt-4 border-t border-line text-[11px] text-dim">
          <span>{t("vault.local")}</span>
        </div>
      </form>
    </main>
  );
}
