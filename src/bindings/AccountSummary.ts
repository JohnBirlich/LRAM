import type { AccountStatus } from "./AccountStatus";

export type AccountSummary = { id: number, userId: number, username: string, displayName: string, alias: string, notes: string, color: string | null, favourite: boolean, sortOrder: number, groupIds: Array<number>, status: AccountStatus,

statusCheckedAt: number | null,

createdAt: number,

allowDisconnect: boolean,

launchArgs: string | null,

discordMuted: boolean,

fpsCap: number | null,

fullQuality: boolean, };
