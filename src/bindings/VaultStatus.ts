import type { VaultMode } from "./VaultMode";

export type VaultStatus = { mode: VaultMode, locked: boolean, accounts: number, autoLockMinutes: number,

hasRecovery: boolean,

retryIn?: number | null, };
