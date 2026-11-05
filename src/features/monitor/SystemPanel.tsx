import { usePrivacy } from "../../privacy";
import { useEffect, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { api, type Instance } from "../../api";
import { useMetrics, useSettings, useSystemUsage } from "../../data";
import { t } from "../../i18n";
import { mb, pct } from "./MonitorView";
import { estimateCapacity, type Capacity, type Frame, type Range } from "./capacity";

export function SystemPanel({ running }: { running: Instance[] }) {
  const privacy = usePrivacy();
  const hardware = useQuery({ queryKey: ["hardware-info"], queryFn: api.hardwareInfo, staleTime: Infinity, retry: false });
  const network = useQuery({ queryKey: ["public-location"], queryFn: api.publicLocation, enabled: false, retry: false, gcTime: 0 });
  const [showNetwork, setShowNetwork] = useState(false);
  const metrics = useMetrics();
  const system = useSystemUsage(true);
  const settings = useSettings().data;
  const session = useRef<{ frames: Frame[]; start: number; stamp: number; signature: string } | null>(null);
  const [collecting, setCollecting] = useState(false);
  const [progress, setProgress] = useState(0);
  const [result, setResult] = useState<{ value: Capacity; signature: string; at: string } | null>(null);
  const [interrupted, setInterrupted] = useState(false);
  // Manual limits, launches, accounts and preset changes invalidate an earlier measurement.
  const signature = JSON.stringify({ settings, clients: running.map((i) => [i.id, i.pid, i.state, i.fpsCap, i.limits]).sort((a, b) => String(a[0]).localeCompare(String(b[0]))) });
  const canMeasure = running.length > 0 && running.every((i) => i.state === "inGame") && !!system.data;

  useEffect(() => {
    if (session.current && signature !== session.current.signature) {
      session.current = null;
      setCollecting(false);
      setInterrupted(true);
    }
  }, [signature]);

  useEffect(() => {
    const pause = () => {
      if (document.hidden && session.current) {
        session.current = null;
        setCollecting(false);
        setInterrupted(true);
      }
    };
    document.addEventListener("visibilitychange", pause);
    return () => document.removeEventListener("visibilitychange", pause);
  }, []);

  useEffect(() => {
    const s = session.current;
    const sys = system.data;
    if (!s || !sys || sys.cpuPct === null || document.hidden || signature !== s.signature || metrics.dataUpdatedAt === s.stamp) return;
    if (Date.now() - metrics.dataUpdatedAt > 4000 || Date.now() - system.dataUpdatedAt > 4000) return;
    const byId = new Map((metrics.data ?? []).map((m) => [m.instanceId, m]));
    const rows = running.map((i) => byId.get(i.id));
    if (rows.some((m) => !m || m.cpuPct === null)) return;
    s.stamp = metrics.dataUpdatedAt;
    s.frames.push({ clients: rows.map((m) => ({ cpu: m!.cpuPct!, resident: m!.memoryMb, commit: m!.privateMb, gpu: m!.gpuPct, vram: m!.gpuMemMb })), systemCpu: sys.cpuPct, totalMb: sys.memTotalMb, availableMb: Math.max(0, sys.memTotalMb - sys.memUsedMb) });
    const seconds = (Date.now() - s.start) / 1000;
    setProgress(Math.min(99, Math.floor(Math.min(seconds / 60, s.frames.length / 30) * 100)));
    if (seconds >= 60 && s.frames.length >= 30) {
      const value = estimateCapacity(s.frames);
      if (value) setResult({ value, signature, at: new Date().toLocaleTimeString() });
      else setInterrupted(true);
      session.current = null;
      setCollecting(false);
    } else if (seconds > 180) {
      session.current = null;
      setCollecting(false);
      setInterrupted(true);
    }
  }, [metrics.dataUpdatedAt, system.dataUpdatedAt, signature, metrics.data, system.data, running]);

  useEffect(() => {
    if (!collecting) return;
    const timer = setTimeout(() => {
      session.current = null;
      setCollecting(false);
      setInterrupted(true);
    }, 180_000);
    return () => clearTimeout(timer);
  }, [collecting]);

  const start = () => {
    session.current = { frames: [], start: Date.now(), stamp: metrics.dataUpdatedAt, signature };
    setResult(null); setInterrupted(false); setProgress(0); setCollecting(true);
  };
  const unknown = t("monitor.hardwareUnknown");
  const h = hardware.data;
  const showRange = (r: Range | null, format: (n: number) => string) => r ? `${format(r.low)} – ${format(r.high)}` : unknown;
  const validResult = result?.signature === signature ? result : null;

  return (
    <div className="monitor-system-grid mt-6">
      <section className="panel p-5 min-w-0">
        <h2 className="section-title mb-4">{t("monitor.hardwareTitle")}</h2>
        {privacy ? <p>Hidden</p> : <dl className="hardware-facts">
          <dt>{t("monitor.board")}</dt><dd>{h?.board ?? unknown}</dd>
          <dt>{t("monitor.processor")}</dt><dd>{h?.cpu ?? unknown}{h?.logicalProcessors ? ` · ${h.logicalProcessors} ${t("monitor.logicalCpus")}` : ""}</dd>
          <dt>{t("monitor.graphics")}</dt><dd>{h?.gpus.length ? h.gpus.map((g, i) => <div key={i}>{g.name} · {g.dedicatedMb === null ? unknown : mb(g.dedicatedMb)} {t("monitor.dedicatedVram")}</div>) : unknown}</dd>
          <dt>{t("monitor.physicalRam")}</dt><dd>{system.data ? mb(system.data.memTotalMb) : unknown}</dd>
          <dt>{t("monitor.memoryRate")}</dt><dd>{h?.memoryRates.length ? `${h.memoryRates.join(" / ")} MT/s` : unknown}</dd>
        </dl>}
        <h3 className="section-title mt-5">Virtual / software display adapters</h3>
        {privacy ? <p>Hidden</p> : h?.virtualAdapters?.length ? h.virtualAdapters.map((name, i) => <p key={i}>{name}</p>) : <p>{h ? "None reported by Windows" : unknown}</p>}
        {!!h?.otherAdapters?.length && <><h3 className="section-title mt-5">Other display adapters</h3>{privacy ? <p>Hidden</p> : h.otherAdapters.map((name, i) => <p key={i}>{name}</p>)}</>}
        <h3 className="section-title mt-5">Peripherals</h3>
        <h4 className="label mt-2">Displays</h4>
        {privacy ? <p>Hidden</p> : h?.displays?.length ? h.displays.map((d, i) => <p key={i}>{d.name ?? unknown} · {d.width && d.height ? `${d.width} × ${d.height}` : unknown} · {d.refreshHz ? `${d.refreshHz} Hz` : unknown}</p>) : <p>{unknown}</p>}
        <p className="text-xs text-dim mt-3">{t("monitor.hardwareHint")}</p>
        {hardware.isError && <p role="status" className="text-orange">{t("monitor.hardwareError")}</p>}
        <button className="btn btn-sm mt-3" disabled={hardware.isFetching} onClick={() => void hardware.refetch()}>{t("monitor.refreshHardware")}</button>
        <h3 className="section-title mt-6 mb-3">{t("monitor.networkTitle")}</h3>
        <p className="text-xs text-dim">{t("monitor.networkConsent")}</p>
        {!privacy && showNetwork && network.data && <dl className="hardware-facts mt-3"><dt>{t("monitor.publicIp")}</dt><dd>{network.data.ip}</dd><dt>{t("monitor.country")}</dt><dd>{network.data.country}</dd></dl>}
        {showNetwork && network.isError && <p role="status" className="text-orange">{t("monitor.networkError")}</p>}
        <div className="flex flex-wrap gap-2 mt-3">
          <button className="btn btn-sm" disabled={privacy || network.isFetching} onClick={() => { setShowNetwork(true); void network.refetch(); }}>{network.isFetching ? t("monitor.networkLoading") : t("monitor.loadNetwork")}</button>
          {showNetwork && <button className="btn btn-sm" onClick={() => setShowNetwork(false)}>{t("monitor.hideNetwork")}</button>}
        </div>
      </section>
      <section className="panel p-5 min-w-0">
        <h2 className="section-title mb-4">{t("monitor.capacityTitle")}</h2>
        <p className="text-dim">{t("monitor.capacityIntro")}</p>
        <div className="flex flex-wrap items-center gap-3 mt-4">
          <button className="btn btn-primary" disabled={!canMeasure || collecting} onClick={start}>{collecting ? t("monitor.measuring", { progress }) : t("monitor.measure")}</button>
          {collecting && <button className="btn" onClick={() => { session.current = null; setCollecting(false); }}>{t("common.cancel")}</button>}
        </div>
        {!canMeasure && <p className="text-xs text-dim mt-3">{t("monitor.measureRequires")}</p>}
        {interrupted && <p role="status" className="text-orange mt-3">{t("monitor.measureInterrupted")}</p>}
        {validResult && <div className="well p-4 mt-4" role="status">
          <p className="text-xl font-semibold">{t("monitor.capacityResult", { low: validResult.value.low, high: validResult.value.high })}</p>
          <p className="text-xs text-dim mt-2">{t("monitor.capacitySample", { count: validResult.value.count, at: validResult.at })}</p>
          <dl className="hardware-facts mt-4">
            <dt>{t("monitor.cpu")}</dt><dd>{showRange(validResult.value.cpu, pct)}</dd>
            <dt>{t("monitor.memory")}</dt><dd>{showRange(validResult.value.ram, mb)}</dd>
            <dt>{t("monitor.commitCol")}</dt><dd>{showRange(validResult.value.commit, mb)}</dd>
            <dt>{t("monitor.gpu")}</dt><dd>{showRange(validResult.value.gpu, pct)}</dd>
            <dt>{t("monitor.gpuMem")}</dt><dd>{showRange(validResult.value.vram, mb)}</dd>
          </dl>
          <p className="text-xs mt-3">{t(validResult.value.cpuLimited ? "monitor.capacityCpuBound" : "monitor.capacityRamBound")}</p>
        </div>}
        {result && !validResult && <p className="text-orange mt-3">{t("monitor.measureStale")}</p>}
        <p className="text-xs text-dim mt-4">{t("monitor.capacityLimits")}</p>
      </section>
    </div>
  );
}
