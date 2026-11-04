import { invoke as rawInvoke, type InvokeArgs } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { AccountInfo } from "./bindings/AccountInfo";
import type { AccountPatch } from "./bindings/AccountPatch";
import type { AccountPresence } from "./bindings/AccountPresence";
import type { AccountSummary } from "./bindings/AccountSummary";
import type { AppUpdate } from "./bindings/AppUpdate";
import type { BandwidthStatus } from "./bindings/BandwidthStatus";
import type { BulkProgress } from "./bindings/BulkProgress";
import type { BulkRequest } from "./bindings/BulkRequest";
import type { ClientMetrics } from "./bindings/ClientMetrics";
import type { SystemUsage } from "./bindings/SystemUsage";
import type { Profile } from "./bindings/Profile";
import type { ServerVisit } from "./bindings/ServerVisit";
import type { ConflictPolicy } from "./bindings/ConflictPolicy";
import type { Crash } from "./bindings/Crash";
import type { DataDir } from "./bindings/DataDir";
import type { DiscordStatus } from "./bindings/DiscordStatus";
import type { ErrorPayload } from "./bindings/ErrorPayload";
import type { Favourite } from "./bindings/Favourite";
import type { FavouriteInput } from "./bindings/FavouriteInput";
import type { GameInfo } from "./bindings/GameInfo";
import type { Group } from "./bindings/Group";
import type { GroupInput } from "./bindings/GroupInput";
import type { Headshot } from "./bindings/Headshot";
import type { ImportReport } from "./bindings/ImportReport";
import type { InstallProgress } from "./bindings/InstallProgress";
import type { Instance } from "./bindings/Instance";
import type { JoinCandidate } from "./bindings/JoinCandidate";
import type { LaunchInfo } from "./bindings/LaunchInfo";
import type { LaunchTarget } from "./bindings/LaunchTarget";
import type { Limits } from "./bindings/Limits";
import type { LogsCleaned } from "./bindings/LogsCleaned";
import type { MultiInstanceStatus } from "./bindings/MultiInstanceStatus";
import type { Notice } from "./bindings/Notice";
import type { ParsedInput } from "./bindings/ParsedInput";
import type { PrivateServerPage } from "./bindings/PrivateServerPage";
import type { NetworkStatus } from "./bindings/NetworkStatus";
import type { RobloxClient } from "./bindings/RobloxClient";
import type { ServerOrder } from "./bindings/ServerOrder";
import type { ServerPage } from "./bindings/ServerPage";
import type { Settings } from "./bindings/Settings";
import type { SettingsPatch } from "./bindings/SettingsPatch";
import type { UserRef } from "./bindings/UserRef";
import type { VaultMode } from "./bindings/VaultMode";
import type { VaultStatus } from "./bindings/VaultStatus";

export type {
  AccountInfo, AccountPresence, Headshot,
  Favourite, FavouriteInput, JoinCandidate, LaunchInfo, LaunchTarget, ParsedInput, UserRef,
  BandwidthStatus, ClientMetrics, Crash, DiscordStatus, Instance, MultiInstanceStatus, BulkProgress, BulkRequest, ServerOrder, ServerPage, GameInfo, PrivateServerPage, DataDir, LogsCleaned, Notice, RobloxClient,
  AccountPatch, AccountSummary, AppUpdate, ConflictPolicy, ErrorPayload, Group, GroupInput,
  ImportReport, InstallProgress, Settings, SettingsPatch, VaultMode, VaultStatus,
};
export type { AccountStatus } from "./bindings/AccountStatus";
export type { Adaptive } from "./bindings/Adaptive";
export type { Profile } from "./bindings/Profile";
export type { SystemUsage } from "./bindings/SystemUsage";
export type { ServerVisit } from "./bindings/ServerVisit";
export type { AfkMode } from "./bindings/AfkMode";
export type { AfkPress } from "./bindings/AfkPress";
export type { AntiAfk } from "./bindings/AntiAfk";
export type { BulkItem } from "./bindings/BulkItem";
export type { BulkStage } from "./bindings/BulkStage";
export type { CrashKind } from "./bindings/CrashKind";
export type { Density } from "./bindings/Density";
export type { DiscordEvents } from "./bindings/DiscordEvents";
export type { GameServer } from "./bindings/GameServer";
export type { PrivateServer } from "./bindings/PrivateServer";
export type { SameServer } from "./bindings/SameServer";
export type { InstanceState } from "./bindings/InstanceState";
export type { Limits } from "./bindings/Limits";
export type { Presence } from "./bindings/Presence";
export type { PresenceKind } from "./bindings/PresenceKind";
export type { Subscription } from "./bindings/Subscription";
export type { Theme } from "./bindings/Theme";

export type WindowShow = "focus" | "minimize" | "restore";

// -- Main func
export function asError(e: unknown): ErrorPayload {
  if (e && typeof e === "object" && "kind" in e && "message" in e) return e as ErrorPayload;
  return { kind: "internal", message: String(e) };
}

export function debugLog(level: "error" | "warn" | "info" | "debug", message: string) {
  rawInvoke("debug_log", { level, message }).catch(() => {});
}

function invoke<T>(cmd: string, args?: InvokeArgs): Promise<T> {
  return rawInvoke<T>(cmd, args).catch((e: unknown) => {
    if (!(e && typeof e === "object" && "kind" in e && "message" in e)) debugLog("error", `invoke ${cmd} failed: ${String(e)}`);
    throw e;
  });
}

export type LogRecord = { level: number; message: string };

export const api = {
  vaultStatus: () => invoke<VaultStatus>("vault_status"),
  vaultUnlock: (password: string) => invoke<void>("vault_unlock", { password }),
  vaultLock: () => invoke<void>("vault_lock"),
  vaultSetMode: (mode: VaultMode, currentPassword?: string, newPassword?: string) =>
    invoke<void>("vault_set_mode", { mode, currentPassword, newPassword }),

  vaultRecover: (code: string, newPassword: string) => invoke<string>("vault_recover", { code, newPassword }),

  vaultRecoveryCreate: (currentPassword: string) => invoke<string>("vault_recovery_create", { currentPassword }),
  vaultRecoverySave: (path: string, code: string) => invoke<void>("vault_recovery_save", { path, code }),
  vaultRecoveryCopy: (code: string) => invoke<void>("vault_recovery_copy", { code }),
  appActivity: () => invoke<void>("app_activity"),

  splashDone: () => invoke<void>("splash_done"),

  accountsList: () => invoke<AccountSummary[]>("accounts_list"),
  accountAddCookie: (cookie: string) => invoke<AccountSummary>("account_add_cookie", { cookie }),

  accountsImportCookies: (text: string) => invoke<ImportReport>("accounts_import_cookies", { text }),

  accountsImportRam: (path: string) => invoke<ImportReport>("accounts_import_ram", { path }),

  accountAddLogin: () => invoke<AccountSummary>("account_add_login"),
  accountUpdate: (id: number, patch: AccountPatch) => invoke<AccountSummary>("account_update", { id, patch }),
  accountsReorder: (ids: number[]) => invoke<void>("accounts_reorder", { ids }),
  accountsRemove: (ids: number[]) => invoke<void>("accounts_remove", { ids }),
  accountsValidate: (ids?: number[]) => invoke<AccountSummary[]>("accounts_validate", { ids }),

  accountRevealCookie: (id: number, password?: string) => invoke<string>("account_reveal_cookie", { id, password }),
  accountCopyCookie: (id: number, password?: string) => invoke<void>("account_copy_cookie", { id, password }),

  accountInfo: (id: number, refresh = false) => invoke<AccountInfo>("account_info", { id, refresh }),

  accountsHeadshots: (refresh = false) => invoke<Headshot[]>("accounts_headshots", { refresh }),

  accountsPresence: (ids: number[]) => invoke<AccountPresence[]>("accounts_presence", { ids }),

  accountOpenSession: (id: number) => invoke<void>("account_open_session", { id }),

  targetParse: (input: string) => invoke<ParsedInput>("target_parse", { input }),

  userLookup: (name: string) => invoke<UserRef>("user_lookup", { name }),

  launch: (accountId: number, target: LaunchTarget, favouriteId?: number, fpsCap?: number | null) =>
    invoke<LaunchInfo>("launch", { accountId, target, favouriteId, fpsCap: fpsCap || null }),
  favouritesList: () => invoke<Favourite[]>("favourites_list"),
  favouriteSave: (input: FavouriteInput) => invoke<Favourite>("favourite_save", { input }),
  favouriteRemove: (id: number) => invoke<void>("favourite_remove", { id }),
  favouritesReorder: (ids: number[]) => invoke<void>("favourites_reorder", { ids }),

  joinCandidates: (accountId: number) => invoke<JoinCandidate[]>("join_candidates", { accountId }),

  friendsRequests: (accountId: number) => invoke<UserRef[]>("friends_requests", { accountId }),

  friendRequestSend: (accountId: number, name: string) => invoke<UserRef>("friend_request_send", { accountId, name }),

  friendRequestAnswer: (accountId: number, requesterId: number, accept: boolean) =>
    invoke<void>("friend_request_answer", { accountId, requesterId, accept }),

  instancesList: () => invoke<Instance[]>("instances_list"),

  instanceClose: (id: string, force: boolean) => invoke<void>("instance_close", { id, force }),
  instancesCloseAll: (force: boolean) => invoke<void>("instances_close_all", { force }),
  instancesClearEnded: () => invoke<void>("instances_clear_ended"),

  instanceSetLimits: (id: string, limits: Limits) => invoke<void>("instance_set_limits", { id, limits }),

  instanceTrim: (id: string) => invoke<[number, number]>("instance_trim", { id }),

  instanceSetAntiAfk: (id: string, enabled: boolean) => invoke<void>("instance_set_anti_afk", { id, enabled }),

  instanceSetMuted: (id: string, muted: boolean) => invoke<void>("instance_set_muted", { id, muted }),

  instanceThumbnail: (id: string, maxW: number, known: number) => invoke<ArrayBuffer>("instance_thumbnail", { id, maxW, known }),

  instanceShow: (id: string, what: WindowShow) => invoke<void>("instance_show", { id, what }),

  instancesShowAll: (what: Exclude<WindowShow, "focus">) => invoke<void>("instances_show_all", { what }),

  instanceMicro: (id: string, on: boolean, size?: [number, number]) => invoke<[number, number]>("instance_micro", { id, on, size }),

  instancesTile: (micro: boolean) => invoke<number>("instances_tile", { micro }),

  instancesMetrics:() => invoke<ClientMetrics[]>("instances_metrics"),

  systemUsage: () => invoke<SystemUsage | null>("system_usage"),

  instanceStopRelaunch: (id: string) => invoke<void>("instance_stop_relaunch", { id }),

  instancesRetryRelaunch: () => invoke<void>("instances_retry_relaunch"),

  bandwidthStatus: () => invoke<BandwidthStatus>("bandwidth_status"),

  bandwidthApply: (kbps: number) => invoke<BandwidthStatus>("bandwidth_apply", { kbps }),
  multiInstanceStatus: () => invoke<MultiInstanceStatus>("multi_instance_status"),
  multiInstanceSet: (enabled: boolean) => invoke<MultiInstanceStatus>("multi_instance_set", { enabled }),

  robloxCloseAll: (force: boolean) => invoke<MultiInstanceStatus>("roblox_close_all", { force }),

  panicKill: () => invoke<MultiInstanceStatus>("panic_kill"),

  crashesList: () => invoke<Crash[]>("crashes_list"),

  diagnosticsText: () => invoke<string>("diagnostics_text"),
  diagnosticsSave: (path: string) => invoke<void>("diagnostics_save", { path }),
  serverHistory: (accountId: number) => invoke<ServerVisit[]>("server_history", { accountId }),

  crashOpenLog: (id: number) => invoke<void>("crash_open_log", { id }),

  discordGet: () => invoke<DiscordStatus>("discord_get"),

  discordSet: (url: string | null) => invoke<DiscordStatus>("discord_set", { url }),

  discordTest: () => invoke<void>("discord_test"),

  bulkLaunch: (request: BulkRequest) => invoke<string>("bulk_launch", { request }),
  bulkCancel: (batchId: string) => invoke<void>("bulk_cancel", { batchId }),
  bulkCurrent: () => invoke<BulkProgress | null>("bulk_current"),
  serversList: (placeId: number, order: ServerOrder, excludeFull: boolean, limit: 10 | 25 | 50 | 100, cursor?: string, accountId?: number) =>
    invoke<ServerPage>("servers_list", { placeId, order, excludeFull, limit, cursor, accountId }),
  gameInfo: (placeId: number) => invoke<GameInfo>("game_info", { placeId }),

  targetPlace: (target: LaunchTarget, accountId?: number) => invoke<number>("target_place", { target, accountId }),
  privateServersList: (placeId: number, accountId: number, cursor?: string) =>
    invoke<PrivateServerPage>("private_servers_list", { placeId, accountId, cursor }),

  groupsList: () => invoke<Group[]>("groups_list"),
  groupSave: (input: GroupInput) => invoke<Group>("group_save", { input }),
  groupRemove: (id: number) => invoke<void>("group_remove", { id }),

  performanceProfile: (previewFps: number) => invoke<string>("performance_profile", { previewFps }),
  performanceExport: (path: string, previewFps: number) => invoke<void>("performance_export", { path, previewFps }),

  performanceRead: (path: string) => invoke<Profile>("performance_read", { path }),
  performanceParse: (text: string) => invoke<Profile>("performance_parse", { text }),
  backupExport: (path: string, password: string, currentPassword?: string, accountIds?: number[]) =>
    invoke<number>("backup_export", { path, password, currentPassword, accountIds }),
  backupImport: (path: string, password: string, onConflict: ConflictPolicy) =>
    invoke<ImportReport>("backup_import", { path, password, onConflict }),

  robloxClient: () => invoke<RobloxClient>("roblox_client"),

  robloxInstall: () => invoke<RobloxClient>("roblox_install"),

  selfUpdateStatus: () => invoke<AppUpdate>("self_update_status"),

  selfUpdateCheck: () => invoke<AppUpdate>("self_update_check"),

  selfUpdateInstall: () => invoke<void>("self_update_install"),
  dataDirInfo: () => invoke<DataDir>("data_dir_info"),
  dataDirOpen: () => invoke<void>("data_dir_open"),

  dataDirMove: (path: string) => invoke<void>("data_dir_move", { path }),

  logsOpen: () => invoke<void>("logs_open"),

  debugTail: (lines: number) => invoke<string[]>("debug_tail", { lines }),
  backgroundSet: (path: string) => invoke<void>("background_set", { path }),
  backgroundClear: () => invoke<void>("background_clear"),
  backgroundGet: () => invoke<ArrayBuffer>("background_get"),

  logsClean: () => invoke<LogsCleaned>("logs_clean"),

  shutdownSchedule: (when: { atMs?: number; inSecs?: number }, closeClients: boolean) =>
    invoke<number | null>("shutdown_schedule", { atMs: when.atMs ?? null, inSecs: when.inSecs ?? null, closeClients }),
  shutdownCancel: () => invoke<number | null>("shutdown_cancel"),

  shutdownStatus: () => invoke<number | null>("shutdown_status"),

  networkStatus: () => invoke<NetworkStatus>("network_status"),

  settingsGet: () => invoke<Settings>("settings_get"),
  settingsSet: (patch: SettingsPatch) => invoke<Settings>("settings_set", { patch }),
};

export const events = {
  onAccountsChanged: (f: () => void): Promise<UnlistenFn> => listen("accounts://changed", f),
  onVaultLocked: (f: () => void): Promise<UnlistenFn> => listen("vault://locked", f),
  onInstancesChanged: (f: () => void): Promise<UnlistenFn> => listen("instances://changed", f),
  onNotice: (f: (n: Notice) => void): Promise<UnlistenFn> => listen<Notice>("app://notice", (e) => f(e.payload)),
  onLog: (f: (r: LogRecord) => void): Promise<UnlistenFn> => listen<LogRecord>("log://log", (e) => f(e.payload)),
  onBulkProgress: (f: (p: BulkProgress) => void): Promise<UnlistenFn> => listen<BulkProgress>("bulk://progress", (e) => f(e.payload)),

  onRobloxClient: (f: (c: RobloxClient) => void): Promise<UnlistenFn> => listen<RobloxClient>("roblox://client", (e) => f(e.payload)),

  onAppUpdate: (f: (u: AppUpdate) => void): Promise<UnlistenFn> => listen<AppUpdate>("app://update", (e) => f(e.payload)),
  onRobloxInstall: (f: (p: InstallProgress) => void): Promise<UnlistenFn> => listen<InstallProgress>("roblox://install", (e) => f(e.payload)),

  onNetwork: (f: (n: NetworkStatus) => void): Promise<UnlistenFn> => listen<NetworkStatus>("app://network", (e) => f(e.payload)),
};
