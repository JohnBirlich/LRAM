import type { Presence } from "./Presence";
import type { Subscription } from "./Subscription";

export type AccountInfo = { accountId: number, userId: number, username: string, displayName: string, description: string,

created: string | null, isBanned: boolean, verifiedBadge: boolean, headshotUrl: string | null, robux: number | null, subscription: Subscription | null, friends: number | null, followers: number | null, following: number | null, emailVerified: boolean | null,

twoStepMethods: Array<string> | null,

presence: Presence | null,

fetchedAt: number, unavailable: Array<string>, };
