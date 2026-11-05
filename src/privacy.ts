import { qk, queryClient, useSettings } from "./data";
import type { Settings } from "./api";

// Presentation only: never replace IDs used for commands or mutate cached/server data.
export const isPrivate = () => queryClient.getQueryData<Settings>(qk.settings)?.privacyMode ?? true;
export const privateText = (value: string | number | null | undefined) => isPrivate() ? "Hidden" : String(value ?? "");
export const usePrivacy = () => useSettings().data?.privacyMode ?? true;
