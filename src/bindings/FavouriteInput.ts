import type { LaunchTarget } from "./LaunchTarget";

export type FavouriteInput = { id?: number, name: string, target: LaunchTarget, accountIds: Array<number>, inTray: boolean, };
