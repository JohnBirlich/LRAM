import type { Msaa } from "./Msaa";
import type { RenderMode } from "./RenderMode";
import type { TextureQuality } from "./TextureQuality";

export type RenderFlags = { msaa: Msaa, texture: TextureQuality, mode: RenderMode, graySky: boolean, noGrass: boolean, lowestFrm: boolean, pauseVoxelizer: boolean, };
