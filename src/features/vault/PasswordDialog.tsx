import { useEffect, useState, type FormEvent, type ReactNode } from "react";
import { asError } from "../../api";
import { Dialog } from "../../ui/Dialog";
import { friendly } from "../../ui/Toast";
import { getLang, t } from "../../i18n";

export const MIN_PASSWORD = 8;

// -- Main func
export function PasswordDialog({
  open, onClose, title, text, submitLabel, onSubmit,
  current = false, next = false, currentLabel = t("settings.masterPassword"), nextLabel = t("vault.newPassword"), cancelLabel = t("common.cancel"),
}: {
  open: boolean;
  onClose: () => void;
  title: string;
  text: ReactNode;
  submitLabel: string;
  onSubmit: (v: { current: string; next: string }) => Promise<unknown>;
  current?: boolean;
  next?: boolean;
  currentLabel?: string;
  nextLabel?: string;
  cancelLabel?: string;
}) {
  const [cur, setCur] = useState("");
  const [nw, setNw] = useState("");
  const [confirm, setConfirm] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [shakes, setShakes] = useState(0);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (open) return;
    setCur("");
    setNw("");
    setConfirm("");
    setError(null);
    setShakes(0);
  }, [open]);

  const close = () => {
    if (!busy) onClose();
  };

  const fail = (msg: string) => {
    setError(msg);
    setShakes((n) => n + 1);
  };

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (busy) return;
    if (current && !cur) return fail(t("vault.enterField", { label: getLang() === "en" ? currentLabel.toLowerCase() : currentLabel }));
    if (next && nw.length < MIN_PASSWORD) return fail(t("vault.tooShort", { count: MIN_PASSWORD }));
    if (next && nw !== confirm) return fail(t("vault.mismatch"));
    setBusy(true);
    setError(null);
    try {
      await onSubmit({ current: cur, next: nw });
      onClose();
    } catch (err) {
      const e2 = asError(err);
      if (e2.kind === "wrongPassword") setCur("");
      if (e2.kind !== "cancelled") fail(e2.kind === "wrongPassword" ? t("ui.error.wrongPassword") : friendly(e2));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Dialog open={open} onClose={close} title={title}>
      <form onSubmit={submit} className={`flex flex-col gap-4 p-5 ${shakes ? (shakes % 2 ? "shake" : "shake2") : ""}`}>
        <div className="text-dim">{text}</div>
        {current && (
          <Field label={currentLabel} value={cur} onChange={setCur} autoComplete="current-password" invalid={!!error} />
        )}
        {next && (
          <>
            <Field label={nextLabel} value={nw} onChange={setNw} autoComplete="new-password" hint={t("vault.minLength", { count: MIN_PASSWORD })} />
            <Field label={t("vault.confirm")} value={confirm} onChange={setConfirm} autoComplete="new-password" />
          </>
        )}
        {error && <div role="alert" className="alert">{error}</div>}
        <div className="flex justify-end gap-2 pt-1">
          <button type="button" className="btn" onClick={close} disabled={busy}>{cancelLabel}</button>
          <button type="submit" className="btn btn-primary" disabled={busy}>{busy ? t("vault.working") : submitLabel}</button>
        </div>
      </form>
    </Dialog>
  );
}

function Field({ label, value, onChange, autoComplete, hint, invalid }: {
  label: string;
  value: string;
  onChange: (v: string) => void;
  autoComplete: string;
  hint?: string;
  invalid?: boolean;
}) {
  return (
    <label className="flex flex-col gap-1.5">
      <span className="label">{label}</span>
      <input
        type="password"
        className="field"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        autoComplete={autoComplete}
        aria-invalid={invalid || undefined}
        spellCheck={false}
      />
      {hint && <span className="text-xs text-dim">{hint}</span>}
    </label>
  );
}
