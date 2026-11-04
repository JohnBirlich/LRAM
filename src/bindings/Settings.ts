import type { UltraCpu } from "./UltraCpu";
import type { Adaptive } from "./Adaptive";
import type { AntiAfk } from "./AntiAfk";
import type { Density } from "./Density";
import type { DiscordEvents } from "./DiscordEvents";
import type { MemThrottle } from "./MemThrottle";
import type { RenderFlags } from "./RenderFlags";
import type { TabLayout } from "./TabLayout";
import type { Theme } from "./Theme";

export type Settings = { theme: Theme, tabLayout: TabLayout,

accent: string,

backgroundDim: number, density: Density,

autoLockMinutes: number, clipboardClearSeconds: number,

cacheMinutes: number,

multiInstance: boolean,

protectSharedLogin: boolean,

launchDelayMs: number,

autoRejoin: boolean,

autoRestartOnCrash: boolean,

breakerMax: number,

breakerWindowMin: number,

closeGraceS: number,

launchArgs: string,

closeToTray: boolean,

skipSplash: boolean,

memRestartMb: number,

memThrottle: MemThrottle,

trimEveryMin: number,

autoSpread: boolean,

fairCap: boolean, adaptive: Adaptive, ultraCpu: UltraCpu,

bandwidthKbps: number, antiAfk: AntiAfk,

muteClients: boolean,

discord: DiscordEvents,

autoUpdate: boolean,

selfUpdate: boolean,

minimizeOnLaunch: boolean,

liveHiddenPreviews: boolean,

panicHotkey: string,

cleanLogs: boolean,

fpsCap: number,

lowGraphics: boolean,

lighterRendering: boolean, renderFlags: RenderFlags,

microWidth: number, microHeight: number,

tinyOnLaunch: boolean,

autoTile: boolean,

allowPublicNetworks: boolean,
};
