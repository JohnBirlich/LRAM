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
  defaultOptions: { queries: { staleTime: Infinity, retry: false, refetchOnWindowFocus: false } },
  mutationCache: new MutationCache({
    onError: (e, _vars, _res, m) => {
      if (!m.meta?.quiet) toastError(e);
    },
  }),
});

const bySortOrder = <T extends { sortOrder: number }>(list: T[]) => [...list].sort((a, b) => a.sortOrder - b.sortOrder);

export const useVault = () => useQuery({ queryKey: qk.vault, queryFn: api.vaultStatus });
export const useSettings = () => useQuery({ queryKey: qk.settings, queryFn: api.settingsGet });
export const useAccounts = () => {
  const privacy = useSettings().data?.privacyMode ?? true;
  return useQuery({ queryKey: qk.accounts, queryFn: api.accountsList, select: (rows) => bySortOrder(rows).map(a => privacy ? { ...a, username: "hidden", displayName: "Hidden", alias: "Hidden", notes: "Hidden", color: null } : a) });
};

const groupNumber = (name: string) => {
  const digits = name.match(/^\d+/)?.[0];
  return digits === undefined ? null : { n: Math.min(Number(digits), 4294967295), rest: name.slice(digits.length).replace(/^[\s\-_.:]+/, "").toLowerCase() };
};

export function byGroupOrder(a: Group, b: Group): number {
  const [na, nb] = [groupNumber(a.name), groupNumber(b.name)];
  if (na && nb && na.n !== nb.n) return na.n - nb.n;
  if (na && nb && na.rest !== nb.rest) return na.rest < nb.rest ? -1 : 1;
  if (!na !== !nb) return na ? -1 : 1;
  return a.sortOrder - b.sortOrder || a.id - b.id;
}

export const useGroups = () => {
  const privacy = useSettings().data?.privacyMode ?? true;
  return useQuery({ queryKey: qk.groups, queryFn: api.groupsList, select: (list: Group[]) => [...list].sort(byGroupOrder).map(g => privacy ? { ...g, name: "Hidden", color: "#808080" } : g) });
};
export const useFavourites = () => useQuery({ queryKey: qk.favourites, queryFn: api.favouritesList, select: bySortOrder });

export const useInstances = () => useQuery({ queryKey: qk.instances, queryFn: api.instancesList, refetchInterval: 5_000 });

export const useCrashes = () => useQuery({ queryKey: qk.crashes, queryFn: api.crashesList });

export const useMetrics = () =>
  useQuery({ queryKey: qk.metrics, queryFn: api.instancesMetrics, staleTime: 0, refetchInterval: 2000, refetchIntervalInBackground: false });

export const useSystemUsage = (enabled: boolean) =>
  useQuery({ queryKey: qk.system, queryFn: api.systemUsage, enabled, staleTime: 0, refetchInterval: 2000, refetchIntervalInBackground: false });

export const useMultiStatus = () =>
  useQuery({
    queryKey: qk.multi,
    queryFn: api.multiInstanceStatus,
    refetchInterval: (q) => (q.state.data?.enabled && !q.state.data.active ? 5000 : false),
    refetchIntervalInBackground: false,
  });

export const useNetwork = () => useQuery({ queryKey: qk.network, queryFn: api.networkStatus });

export function publicBlock(network: { public: boolean } | undefined, settings: { allowPublicNetworks: boolean } | undefined) {
  if (!network?.public || settings?.allowPublicNetworks !== false) return null;
  return t("app.publicBlocked");
}

export const useRobloxClient = () => useQuery({ queryKey: qk.client, queryFn: api.robloxClient });

let dismissedBulk: string | null = null;
export function dismissBulk(qc: QueryClient, batchId: string) {
  dismissedBulk = batchId;
  qc.setQueryData(qk.bulk, null);
}
export const useBulk = () =>
  useQuery({ queryKey: qk.bulk, queryFn: () => api.bulkCurrent().then((p) => (p && p.batchId === dismissedBulk ? null : p)) });

const headshots = queryOptions({ queryKey: qk.headshots, queryFn: () => api.accountsHeadshots(), staleTime: 6 * 3_600_000 });

export const useHeadshot = (accountId: number) =>
  useQuery({ ...headshots, select: (list) => list.find((h) => h.accountId === accountId)?.url }).data;

const byAccount = (list: AccountPresence[]) => new Map(list.map((p) => [p.accountId, p.presence]));

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

// -- Main func
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

export function useSetSettings() {
  const qc = useQueryClient();
  return useMutation({
    mutationKey: qk.settings,
    mutationFn: (patch: SettingsPatch) => api.settingsSet(patch),
    onMutate: (patch) => qc.setQueryData<Settings>(qk.settings, (s) => (s ? { ...s, ...patch } : s)),
    onSuccess: (s) => {
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
