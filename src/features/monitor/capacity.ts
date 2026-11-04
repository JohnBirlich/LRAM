// Deliberately has no network, identity, location or hardware-model inputs.
export type Load = { cpu: number; resident: number; commit: number; gpu: number | null; vram: number | null };
export type Frame = { clients: Load[]; systemCpu: number; availableMb: number; totalMb: number };
export type Range = { low: number; high: number };
export type Capacity = { low: number; high: number; count: number; cpu: Range; ram: Range; commit: Range; gpu: Range | null; vram: Range | null; cpuLimited: boolean };

const quantile = (values: number[], fraction: number) => {
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.floor((sorted.length - 1) * fraction)];
};
const range = (values: number[]): Range => ({ low: quantile(values, 0.1), high: quantile(values, 0.9) });

export function estimateCapacity(frames: Frame[]): Capacity | null {
  if (frames.length < 30) return null;
  const count = frames[0].clients.length;
  if (!count || frames.some((f) => f.clients.length !== count || !Number.isFinite(f.systemCpu) || f.systemCpu < 0 || f.systemCpu > 100
    || !Number.isFinite(f.availableMb) || f.availableMb < 0 || !Number.isFinite(f.totalMb) || f.totalMb <= 0 || f.availableMb > f.totalMb
    || f.clients.some((c) => ![c.cpu, c.resident, c.commit].every(Number.isFinite) || c.cpu < 0 || c.cpu > 100 || c.resident <= 0 || c.commit < 0))) return null;
  const clients = frames.flatMap((f) => f.clients);
  const cpu = range(clients.map((c) => c.cpu));
  const ram = range(clients.map((c) => c.resident));
  const commit = range(clients.map((c) => c.commit));
  // Use the heaviest sampled client, not just the average across accounts.
  // A minimum CPU cost prevents an idle/rounded-to-zero sample predicting infinite capacity.
  const perCpu = Math.max(0.1, ...Array.from({ length: count }, (_, i) => quantile(frames.map((f) => f.clients[i].cpu), 0.9)));
  const perRam = Math.max(...Array.from({ length: count }, (_, i) => quantile(frames.map((f) => Math.max(f.clients[i].resident, f.clients[i].commit)), 0.9)));
  if (perRam <= 0) return null;
  // Keep 20% CPU free; reserve at least 2 GiB or 10% physical memory for the OS.
  const cpuRoom = Math.max(0, 80 - quantile(frames.map((f) => f.systemCpu), 0.9));
  const reserve = Math.max(2048, frames[0].totalMb * 0.1);
  const memoryRoom = Math.max(0, quantile(frames.map((f) => f.availableMb), 0.1) - reserve);
  const extra = (margin: number) => Math.max(0, Math.floor(Math.min(cpuRoom / (perCpu * margin), memoryRoom / (perRam * margin))));
  const optional = (pick: (c: Load) => number | null) => {
    const values = clients.map(pick);
    return values.every((v): v is number => v !== null && Number.isFinite(v) && v >= 0) ? range(values) : null;
  };
  return { low: count + extra(1.5), high: count + extra(1.25), count, cpu, ram, commit, gpu: optional((c) => c.gpu), vram: optional((c) => c.vram), cpuLimited: cpuRoom / perCpu < memoryRoom / perRam };
}
