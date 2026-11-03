import type { CrashKind } from "./CrashKind";

export type Crash = { id: number,

instanceId: string, accountId: number, kind: CrashKind,

at: number,

uptimeMs: number,

reason: string | null, placeId: number | null,

hasLog: boolean, };
