import { useEffect, useState, type FormEvent } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { open as openFile, save as saveFile } from "@tauri-apps/plugin-dialog";
import { api, asError, type ConflictPolicy, type ImportReport, type VaultMode } from "../../api";
import { qk } from "../../data";
import { Dialog } from "../../ui/Dialog";
import { Segmented } from "../../ui/controls";
import { t } from "../../i18n";
import { friendly, toast } from "../../ui/Toast";
import { PasswordDialog } from "../vault/PasswordDialog";

const cancelled = () => ({ kind: "cancelled", message: "" });

const profileFilter = () => [{ name: t("accounts.profiles.filter"), extensions: ["lram", "roalt"] }];

export function ExportProfilesDialog({ ids, open, onClose, vaultMode }: { ids: number[]; open: boolean; onClose: () => void; vaultMode: VaultMode }) {
  const master = vaultMode === "master";
  return (
    <PasswordDialog
      open={open}
      onClose={onClose}
      title={t("accounts.profiles.exportTitle", { count: ids.length })}
      text={t("accounts.profiles.exportText")}
      submitLabel={t("accounts.profiles.exportSubmit")}
      current={master}
      currentLabel={t("settings.masterPassword")}
      next
      nextLabel={t("settings.backupPassword")}
      onSubmit={async ({ current, next }) => {
        const date = new Date().toISOString().slice(0, 10);
        const name = ids.length === 1 ? "LRAM-profile" : "LRAM-profiles";
        const path = await saveFile({ defaultPath: `${name}-${date}.lram`, filters: profileFilter() });
        if (!path) throw cancelled();
        const n = await api.backupExport(path, next, master ? current : undefined, ids);
        toast(t("accounts.profiles.exported", { count: n }));
      }}
    />
  );
}

// -- Main func
export function ImportProfilesDialog({ open, onClose }: { open: boolean; onClose: () => void }) {
  const qc = useQueryClient();
  const [path, setPath] = useState("");
  const [pw, setPw] = useState("");
  const [policy, setPolicy] = useState<ConflictPolicy>("skip");
  const [report, setReport] = useState<ImportReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (open) return;
    setPath("");
    setPw("");
    setReport(null);
    setError(null);
  }, [open]);

  async function pick() {
    const picked = await openFile({ multiple: false, directory: false, filters: profileFilter() });
    if (typeof picked === "string") {
      setPath(picked);
      setReport(null);
      setError(null);
    }
  }

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!pw || !path || busy) return;
    setBusy(true);
    setError(null);
    try {
      const r = await api.backupImport(path, pw, policy);
      setReport(r);
      setPw("");
      await qc.invalidateQueries({ queryKey: qk.accounts });
      await qc.invalidateQueries({ queryKey: qk.groups });
    } catch (err) {
      const e2 = asError(err);
      setError(e2.kind === "wrongPassword" || e2.kind === "crypto" ? t("accounts.profiles.wrongPassword") : friendly(e2));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Dialog open={open} onClose={onClose} title={t("accounts.profiles.importTitle")}>
      {report ? (
        <div className="enter flex flex-col">
          <div className="grid grid-cols-4 border-b border-line">
            {(["added", "updated", "skipped", "failed"] as const).map((k) => (
              <div key={k} className="px-5 py-4 border-e border-line last:border-e-0">
                <div className="label">{t(({ added: "accounts.profiles.added", updated: "accounts.profiles.updated", skipped: "accounts.profiles.skipped", failed: "accounts.profiles.failed" } as const)[k])}</div>
                <div className="mono text-[22px] font-medium mt-0.5">{report[k]}</div>
              </div>
            ))}
          </div>
          <div className="flex justify-end p-5">
            <button className="btn btn-primary" onClick={onClose}>{t("common.done")}</button>
          </div>
        </div>
      ) : (
        <form onSubmit={submit} className="flex flex-col gap-4 p-5">
          <div className="flex flex-wrap items-center gap-2">
            <button type="button" className="btn" onClick={() => void pick()}>{t("accounts.profiles.pickFile")}</button>
            <span className="mono text-xs text-dim break-all selectable">{path ? path.split(/[\\/]/).pop() : t("accounts.profiles.noFile")}</span>
          </div>
          <label className="flex flex-col gap-1.5">
            <span className="label">{t("settings.backupPassword")}</span>
            <input
              type="password"
              className="field"
              value={pw}
              onChange={(e) => setPw(e.target.value)}
              autoComplete={t("settings.placeholderOff")}
              aria-invalid={!!error}
              disabled={!path}
            />
          </label>
          <div className="flex flex-col gap-1.5">
            <span className="label">{t("settings.importExisting")}</span>
            <Segmented
              label={t("settings.importExisting")}
              value={policy}
              options={[{ value: "skip", label: t("settings.importKeep") }, { value: "replace", label: t("settings.importReplace") }]}
              onChange={setPolicy}
            />
          </div>
          {error && <div role="alert" className="alert">{error}</div>}
          <div className="flex justify-end gap-2 pt-1">
            <button type="button" className="btn" onClick={onClose}>{t("common.cancel")}</button>
            <button type="submit" className="btn btn-primary" disabled={busy || !pw || !path}>{busy ? t("accounts.profiles.importing") : t("accounts.profiles.importSubmit")}</button>
          </div>
        </form>
      )}
    </Dialog>
  );
}
