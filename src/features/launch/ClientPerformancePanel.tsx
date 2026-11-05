import { useState } from "react";
import { api, type ClientPerformance, type Instance, type Settings } from "../../api";
import { PRESETS } from "../performance/PerformanceView";
import { t } from "../../i18n";
import { ConfirmDialog } from "../../ui/Dialog";
import { toastError } from "../../ui/Toast";

export function ClientPerformancePanel({ inst, settings }: { inst: Instance; settings: Settings }) {
  const [draft, setDraft] = useState<ClientPerformance>(() => inst.performance ?? {
    fpsCap: inst.fpsCap ?? settings.fpsCap,
    lowGraphics: settings.lowGraphics, lighterRendering: settings.lighterRendering, renderFlags: settings.renderFlags,
    strictCpu: settings.ultraCpu.enabled,
    limits: settings.ultraCpu.enabled ? { ...inst.limits, cpuCap: settings.ultraCpu.cap, eco: true } : inst.limits,
  });
  const [preset, setPreset] = useState("");
  const [all, setAll] = useState(false);
  const [busy, setBusy] = useState(false);
  const [restart, setRestart] = useState(false);
  const [applied, setApplied] = useState(false);
  const choose = (key: string) => {
    setPreset(key); setApplied(false);
    if (!key) return;
    const p = PRESETS[Number(key)].profile;
    setDraft({ fpsCap: p.fpsCap ?? 0, lowGraphics: p.lowGraphics ?? false, lighterRendering: p.lighterRendering ?? false,
      renderFlags: p.renderFlags ?? settings.renderFlags, strictCpu: p.ultraCpu?.enabled ?? false,
      limits: p.adaptive?.enabled ? { ...p.adaptive.background } : { cpuCap: null, eco: false, trimEveryMin: 0, affinity: null } });
  };
  async function apply() {
    setBusy(true);
    try { await api.instancePerformance(inst.id, draft, all); setApplied(true); }
    catch(e) { toastError(e); } finally { setBusy(false); }
  }
  return <section className="well flex flex-col gap-3 p-3" aria-label="Client performance">
    <h3 className="section-title">Profile, FPS and graphics</h3>
    <label>Profile<select className="field w-full" value={preset} onChange={e => choose(e.target.value)}>
      <option value="">Custom / current settings</option>{PRESETS.map((p, i) => <option key={i} value={String(i)}>{t(p.label)}</option>)}
    </select></label>
    <label>FPS cap (0 = off)<input className="field w-full" type="number" min={0} max={360} value={draft.fpsCap} onChange={e => { setApplied(false); setDraft({ ...draft, fpsCap: Number(e.target.value) }); }} /></label>
    <label>Graphics<select className="field w-full" value={!draft.lowGraphics && !draft.lighterRendering ? "defaults" : draft.renderFlags.graySky ? "maximum" : "balanced"} onChange={e => {
      setApplied(false);
      const p = PRESETS[e.target.value === "maximum" ? 0 : e.target.value === "balanced" ? 2 : 3].profile;
      setDraft({ ...draft, lowGraphics: p.lowGraphics ?? false, lighterRendering: p.lighterRendering ?? false, renderFlags: p.renderFlags ?? settings.renderFlags });
    }}><option value="maximum">Maximum performance</option><option value="balanced">Balanced</option><option value="defaults">Roblox defaults</option></select></label>
    {draft.strictCpu && <label>CPU ceiling after warm-up<select className="field w-full" value={draft.limits.cpuCap ?? 1} onChange={e => { setApplied(false); setDraft({ ...draft, limits: { ...draft.limits, cpuCap: Number(e.target.value) } }); }}><option value={1}>1%</option><option value={2}>2%</option></select></label>}
    <label className="flex gap-2"><input type="checkbox" checked={all} onChange={e => setAll(e.target.checked)} />Apply to all running accounts</label>
    <p className="text-xs text-dim">All profiles load without a CPU ceiling or Eco, then wait 90 seconds after confirmed game entry before applying CPU/Eco limits. Apply updates live limits only after that warm-up and saves FPS/graphics for the next launch. Disconnects, teleports and rejoins start a new cycle. Window size and resolution are unchanged.</p>
    {(inst.performancePending || applied) && <p role="status">Requires restart — FPS and graphics have not been applied to the current client.</p>}
    <div className="flex flex-wrap gap-2">
      <button className="btn btn-primary" disabled={busy || inst.checkingProcess} onClick={() => void apply()}>Apply</button>
      <button className="btn" disabled={busy || inst.checkingProcess || !inst.performancePending} onClick={() => setRestart(true)}>Restart to apply</button>
    </div>
    <p className="text-xs text-dim">Restart applies to this client only. For other running accounts, confirm each restart separately.</p>
    <ConfirmDialog open={restart} title="Restart to apply FPS and graphics?" text="This closes the selected Roblox client and starts it again. Your game session will end; progress and return to the same server are not guaranteed. Other clients are not restarted." confirmLabel="Restart to apply" busy={busy} onClose={() => setRestart(false)} onConfirm={async () => { setBusy(true); try { await api.instanceRestartPerformance(inst.id); setRestart(false); } catch(e) { toastError(e); } finally { setBusy(false); } }} />
  </section>;
}
