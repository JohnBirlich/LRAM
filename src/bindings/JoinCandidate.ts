import type { Presence } from "./Presence";

export type JoinCandidate = { userId: number, username: string, displayName: string,

accountId: number | null, presence: Presence, };
