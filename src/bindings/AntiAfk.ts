import type { AfkMode } from "./AfkMode";
import type { AfkPress } from "./AfkPress";

export type AntiAfk = { enabled: boolean, intervalMin: number, mode: AfkMode, press: AfkPress, };
