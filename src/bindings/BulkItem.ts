import type { BulkStage } from "./BulkStage";

export type BulkItem = { accountId: number,

userId: number, stage: BulkStage,

instanceId: string | null, };
