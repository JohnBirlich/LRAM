import type { BulkItem } from "./BulkItem";
import type { LaunchTarget } from "./LaunchTarget";
import type { SameServer } from "./SameServer";

export type BulkProgress = { batchId: string, target: LaunchTarget, sameServer: SameServer, delayMs: number,

jobId: string | null, items: Array<BulkItem>,

stopped: string | null,

done: boolean,

note: string | null, };
