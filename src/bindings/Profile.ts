import type { UltraCpu } from "./UltraCpu";
import type { Adaptive } from "./Adaptive";
import type { MemThrottle } from "./MemThrottle";
import type { RenderFlags } from "./RenderFlags";

export type Profile = { fpsCap?: number, lowGraphics?: boolean, lighterRendering?: boolean, renderFlags?: RenderFlags, minimizeOnLaunch?: boolean, liveHiddenPreviews?: boolean, microWidth?: number, microHeight?: number, tinyOnLaunch?: boolean, memRestartMb?: number, memThrottle?: MemThrottle, trimEveryMin?: number, autoSpread?: boolean, fairCap?: boolean, adaptive?: Adaptive, ultraCpu?: UltraCpu,

previewFps?: number, };
