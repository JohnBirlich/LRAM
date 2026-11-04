import type { CpuMode } from "./CpuMode";
import type { InstanceState } from "./InstanceState";
import type { LaunchTarget } from "./LaunchTarget";
import type { Limits } from "./Limits";

export type Instance = { id: string, accountId: number, pid: number, state: InstanceState, target: LaunchTarget, placeId: number | null, jobId: string | null,

startedAt: number, endedAt: number | null, viaBootstrapper: boolean,

note: string | null,

rejoins: number, limits: Limits,

antiAfk: boolean,

muted: boolean,

afkAt: number | null,

throttled: boolean,

micro: boolean,

fpsCap: number | null,

relaunching: boolean,

relaunchProblem: string | null,

signInProblem: boolean,

appliedLimits: Limits, checkingProcess: boolean, cpuMode: CpuMode, cpuWarmupRemainingS: number, };
