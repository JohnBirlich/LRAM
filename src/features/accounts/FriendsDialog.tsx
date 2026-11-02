import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api, asError, type AccountSummary, type UserRef } from "../../api";
import { qk } from "../../data";
import { Dialog } from "../../ui/Dialog";
import { Icon } from "../../ui/Icon";
import { friendly, toast, toastError } from "../../ui/Toast";
import { t } from "../../i18n";
import { accountName } from "./shared";

/** Incoming friend requests of one account, with accept / decline, plus sending one by username.
 * Both go through the account's own cookie, exactly as on roblox.com. */
export function FriendsDialog({ open, onClose, account }: {
  open: boolean;
  onClose: () => void;
  account: AccountSummary | null;
}) {
  const id = account?.id ?? 0;
  const qc = useQueryClient();
  const [name, setName] = useState("");
  useEffect(() => {
    if (open) setName("");
  }, [open]);

  const list = useQuery({
    queryKey: [...qk.friendRequests(id)],
    queryFn: () => api.friendsRequests(id),
    enabled: open && account !== null,
    staleTime: 0,
  });

  const send = useMutation({
    mutationFn: (who: string) => api.friendRequestSend(id, who),
    onSuccess: (u) => {
      setName("");
      toast(t("accounts.friends.sent", { name: u.username }));
    },
    onError: toastError,
  });

  const answer = useMutation({
    mutationFn: (v: { requesterId: number; accept: boolean }) => api.friendRequestAnswer(id, v.requesterId, v.accept),
    onSuccess: (_, v) => {
      qc.setQueryData<UserRef[]>([...qk.friendRequests(id)], (rows) => rows?.filter((r) => r.userId !== v.requesterId));
      toast(v.accept ? t("accounts.friends.accepted") : t("accounts.friends.declined"));
    },
    onError: toastError,
  });

  const rows = list.data ?? [];
  const busy = answer.isPending ? (answer.variables?.requesterId ?? null) : null;
  const typed = name.trim().replace(/^@/, "");

  return (
    <Dialog open={open} onClose={onClose} title={account ? t("accounts.friends.titleAs", { name: accountName(account) }) : t("accounts.friends.title")} width={460}>
      <form
        className="flex items-center gap-2 p-4 border-b border-line"
        onSubmit={(e) => {
          e.preventDefault();
          if (typed) send.mutate(typed);
        }}
      >
        <label className="flex items-center gap-2 flex-1 min-w-0 h-[34px] px-2.5 border border-line-strong bg-bg text-dim focus-within:border-[var(--focus)]">
          <Icon name="friends" size={14} />
          <span className="sr-only">{t("accounts.friends.username")}</span>
          <input
            className="flex-1 min-w-0 bg-transparent outline-none text-text"
            placeholder={t("accounts.friends.username")}
            spellCheck={false}
            autoComplete="off"
            value={name}
            onChange={(e) => setName(e.target.value)}
          />
        </label>
        <button className="btn btn-sm" type="submit" disabled={!typed || send.isPending}>
          <Icon name="plus" size={12} />
          {t("accounts.friends.send")}
        </button>
      </form>

      <div className="scroll max-h-[min(420px,60vh)]">
        {list.isError ? (
          <div className="flex flex-col items-start gap-3 p-5">
            <div role="alert" className="alert">{friendly(asError(list.error))}</div>
            <button className="btn btn-sm" onClick={() => list.refetch()} disabled={list.isFetching}>
              <Icon name="refresh" size={12} className={list.isFetching ? "spin" : ""} />
              {t("common.tryAgain")}
            </button>
          </div>
        ) : !list.data || list.isFetching ? (
          <div aria-busy="true" aria-label={t("common.loading")}>
            {[0, 1, 2].map((i) => (
              <div key={i} className="flex items-center gap-3 h-[52px] px-4 border-b border-line">
                <span className="skel w-7 h-7 rounded-full" />
                <span className="skel h-4 w-40" />
              </div>
            ))}
          </div>
        ) : rows.length === 0 ? (
          <p className="p-5 text-dim">{t("accounts.friends.none")}</p>
        ) : (
          rows.map((u) => (
            <div key={u.userId} className="flex items-center gap-3 h-[52px] px-4 border-b border-line">
              <span
                aria-hidden="true"
                className="flex-none grid place-content-center w-7 h-7 rounded-full bg-sel text-xs font-bold"
              >
                {(u.displayName || u.username).slice(0, 1).toUpperCase()}
              </span>
              <span className="flex-1 min-w-0">
                <span className="block truncate">{u.displayName || u.username}</span>
                <span className="block truncate text-xs text-dim">@{u.username}</span>
              </span>
              <button
                className="btn btn-sm btn-primary"
                disabled={busy !== null}
                onClick={() => answer.mutate({ requesterId: u.userId, accept: true })}
              >
                {t("accounts.friends.accept")}
              </button>
              <button className="btn btn-sm" disabled={busy !== null} onClick={() => answer.mutate({ requesterId: u.userId, accept: false })}>
                {t("accounts.friends.decline")}
              </button>
            </div>
          ))
        )}
      </div>
      <p className="px-5 py-3 border-t border-line text-xs text-dim">{t("accounts.friends.footer")}</p>
    </Dialog>
  );
}
