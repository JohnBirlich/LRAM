import type { GameServer } from "./GameServer";

export type ServerPage = { servers: Array<GameServer>,

nextCursor: string | null, };
