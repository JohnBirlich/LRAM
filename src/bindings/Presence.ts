import type { PresenceKind } from "./PresenceKind";

export type Presence = { kind: PresenceKind,

lastLocation: string | null, placeId: number | null, rootPlaceId: number | null, universeId: number | null,

gameId: string | null, };
