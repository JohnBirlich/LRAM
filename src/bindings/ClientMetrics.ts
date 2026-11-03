export type ClientMetrics = { instanceId: string, pid: number,

cpuPct: number | null,

memoryMb: number,

privateMb: number,

readBps: number | null, writeBps: number | null,

gpuPct: number | null,

gpuMemMb: number | null, };
