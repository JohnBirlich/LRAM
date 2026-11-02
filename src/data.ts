// TanStack Query wiring: one cache, keys, and the hooks the views share.
import { MutationCache, QueryClient, queryOptions, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api, type AccountPatch, type AccountPresence, type AccountSummary, type Group, type Settings, type SettingsPatch } from "./api";
import { toastError } from "./ui/Toast";
import { t } from "./i18n";

export const qk = {
  vault: ["vault"],
  settings: ["settings"],
  accounts: ["accounts"],
  groups: ["groups"],
  headshots: ["headshots"],
  presence: ["presence"],
  info: (id: number) => ["info", id],
  favourites: ["favourites"],
  join: ["join"],
  friendRequests: (id: number) => ["friendRequests", id] as const,
  instances: ["instances"],
  multi: ["multi"],
  crashes: ["crashes"],
  serverHistory: (id: number) => ["serverHistory", id] as const,
  metrics: ["metrics"],
  system: ["system"],
  bandwidth: ["bandwidth"],
  discord: ["discord"],
  bulk: ["bulk"],
  game: (placeId: number | null) => ["game", placeId],
  servers: ["servers"],
  privateServers: ["privateServers"],
  client: ["client"],
  appUpdate: ["appUpdate"],
  dataDir: ["dataDir"],
  shutdown: ["shutdown"],
  background: ["background"],
  network: ["network"],
} as const;

export const queryClient = new QueryClient({
  // Data only changes through our own mutations and backend events, which invalidate.
  defaultOptions: { queries: { staleTime: Infinity, retry: false, refetchOnWindowFocus: false } },
  // Every failed mutation toasts unless it handles its own errors (meta.quiet).
  mutationCache: new MutationCache({
    onError: (e, _vars, _res, m) => {
      if (!m.meta?.quiet) toastError(e);
    },
  }),
});

const bySortOrder = <T extends { sortOrder: number }>(list: T[]) => [...list].sort((a, b) => a.sortOrder - b.sortOrder);

export const useVault = () => useQuery({ queryKey: qk.vault, queryFn: api.vaultStatus });
export const useSettings = () => useQuery({ queryKey: qk.settings, queryFn: api.settingsGet });
export const useAccounts = () => useQuery({ queryKey: qk.accounts, queryFn: api.accountsList, select: bySortOrder });
export const useGroups = () => useQuery({ queryKey: qk.groups, queryFn: api.groupsList, select: bySortOrder });
export const useFavourites = () => useQuery({ queryKey: qk.favourites, queryFn: api.favouritesList, select: bySortOrder });

/** Refreshed by instances://changed, and every 5 s in case an event was missed. */
export const useInstances = () => useQuery({ queryKey: qk.instances, queryFn: api.instancesList, refetchInterval: 5_000 });
/** Refreshed by instances://changed (an ending client may add one). */
export const useCrashes = () => useQuery({ queryKey: qk.crashes, queryFn: api.crashesList });
/** Polled every 2 s while a caller is mounted and the window is visible. */
export const useMetrics = () =>
  useQuery({ queryKey: qk.metrics, queryFn: api.instancesMetrics, staleTime: 0, refetchInterval: 2000, refetchIntervalInBackground: false });
/** How busy the whole PC is; polled like the metrics. */
export const useSystemUsage = (enabled: boolean) =>
  useQuery({ queryKey: qk.system, queryFn: api.systemUsage, enabled, staleTime: 0, refetchInterval: 2000, refetchIntervalInBackground: false });
/** While the lock is wanted but not held, asks again every 5 s (Roblox may have been closed). */
export const useMultiStatus = () =>
  useQuery({
    queryKey: qk.multi,
    queryFn: api.multiInstanceStatus,
    refetchInterval: (q) => (q.state.data?.enabled && !q.state.data.active ? 5000 : false),
    refetchIntervalInBackground: false,
  });

/** Whether a Public network is connected; app://network replaces it. */
export const useNetwork = () => useQuery({ queryKey: qk.network, queryFn: api.networkStatus });

/** Why launching is refused on this network (`network::blocked`), or null. */
export function publicBlock(network: { public: boolean } | undefined, settings: { allowPublicNetworks: boolean } | undefined) {
  if (!network?.public || settings?.allowPublicNetworks !== false) return null;
  return t("app.publicBlocked");
}

/** The Roblox client and its versions; roblox://client replaces it. */
export const useRobloxClient = () => useQuery({ queryKey: qk.client, queryFn: api.robloxClient });

/** The running or last batch; bulk://progress replaces it, null once dismissed. */
// A dismissed batch stays hidden when the query is fetched again (bulk_current keeps the last).
let dismissedBulk: string | null = null;
export function dismissBulk(qc: QueryClient, batchId: string) {
  dismissedBulk = batchId;
  qc.setQueryData(qk.bulk, null);
}
export const useBulk = () =>
  useQuery({ queryKey: qk.bulk, queryFn: () => api.bulkCurrent().then((p) => (p && p.batchId === dismissedBulk ? null : p)) });

const headshots = queryOptions({ queryKey: qk.headshots, queryFn: () => api.accountsHeadshots(), staleTime: 6 * 3_600_000 });
/** One shared query; each avatar picks its own URL out of it. */
export const useHeadshot = (accountId: number) =>
  useQuery({ ...headshots, select: (list) => list.find((h) => h.accountId === accountId)?.url }).data;

const byAccount = (list: AccountPresence[]) => new Map(list.map((p) => [p.accountId, p.presence]));
/** Presence of the given (on-screen, valid) accounts, polled once a minute while a caller is
 * mounted and the window is visible. Each account asks about itself, so keep the list short. */
export const usePresence = (ids: number[]) =>
  useQuery({
    queryKey: [...qk.presence, ids],
    queryFn: () => api.accountsPresence(ids),
    select: byAccount,
    enabled: ids.length > 0,
    staleTime: 60_000,
    refetchInterval: 60_000,
    refetchIntervalInBackground: false,
    placeholderData: (prev) => prev,
  });

/** The backend caches info for settings.cacheMinutes (presence inside it refreshes at most once
 * a minute), so asking again on open and once a minute while shown is cheap. */
export const useAccountInfo = (id: number) =>
  useQuery({
    queryKey: qk.info(id),
    queryFn: () => api.accountInfo(id),
    staleTime: 0,
    refetchInterval: 60_000,
    refetchIntervalInBackground: false,
  });

export function setAccounts(qc: ReturnType<typeof useQueryClient>, f: (list: AccountSummary[]) => AccountSummary[]) {
  qc.setQueryData<AccountSummary[]>(qk.accounts, (old) => (old ? f(old) : old));
}

/** Optimistic account edit; the server's copy replaces it on success. */
export function useUpdateAccount() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, patch }: { id: number; patch: AccountPatch }) => api.accountUpdate(id, patch),
    onMutate: ({ id, patch }) =>
      setAccounts(qc, (list) =>
        list.map((a) =>
          a.id === id ? { ...a, ...patch, color: patch.color === undefined ? a.color : patch.color || null } : a,
        ),
      ),
    onSuccess: (acc) => setAccounts(qc, (list) => list.map((a) => (a.id === acc.id ? acc : a))),
    onError: () => qc.invalidateQueries({ queryKey: qk.accounts }),
  });
}

/** Optimistic settings change (theme and density apply at once). */
export function useSetSettings() {
  const qc = useQueryClient();
  return useMutation({
    mutationKey: qk.settings,
    mutationFn: (patch: SettingsPatch) => api.settingsSet(patch),
    onMutate: (patch) => qc.setQueryData<Settings>(qk.settings, (s) => (s ? { ...s, ...patch } : s)),
    onSuccess: (s) => {
      // An earlier save's answer would undo the optimistic value of a later one still on its way.
      if (qc.isMutating({ mutationKey: qk.settings }) === 1) qc.setQueryData(qk.settings, s);
      qc.invalidateQueries({ queryKey: qk.vault });
    },
    onError: () => qc.invalidateQueries({ queryKey: qk.settings }),
  });
}

export function useSaveGroup() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: api.groupSave,
    onSuccess: (g: Group) =>
      qc.setQueryData<Group[]>(qk.groups, (list) => (list ? [...list.filter((x) => x.id !== g.id), g] : list)),
  });
}

export const groupColor = (g: Group) => g.color ?? "var(--grey)";
