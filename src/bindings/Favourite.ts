import type { LaunchTarget } from "./LaunchTarget";

export type Favourite = { id: number, name: string, target: LaunchTarget,

accountIds: Array<number>, inTray: boolean, sortOrder: number,

lastUsedAt: number | null, };
