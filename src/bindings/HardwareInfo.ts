import type { Display } from "./Display";
import type { GraphicsAdapter } from "./GraphicsAdapter";

export type HardwareInfo = { board: string | null, cpu: string | null, logicalProcessors: number | null, memoryRates: number[], gpus: GraphicsAdapter[], displays: Display[] };
