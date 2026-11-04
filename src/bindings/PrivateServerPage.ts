import type { PrivateServer } from "./PrivateServer";

export type PrivateServerPage = { servers: Array<PrivateServer>,

nextCursor: string | null,

joinRestricted: boolean, };
