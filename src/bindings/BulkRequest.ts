import type { LaunchTarget } from "./LaunchTarget";
import type { SameServer } from "./SameServer";

export type BulkRequest = {

accountIds: Array<number>, target: LaunchTarget,

delayMs: number, sameServer: SameServer,

favouriteId: number | null,

fpsCap: number | null, };
