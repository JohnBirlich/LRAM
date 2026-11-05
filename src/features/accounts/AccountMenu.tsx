import { usePrivacy } from "../../privacy";
import { menuKeys } from "../../ui/menu";
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { useMutation } from "@tanstack/react-query";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { openUrl } from "@tauri-apps/plugin-opener";
import { api, type AccountSummary } from "../../api";
import { useUpdateAccount } from "../../data";
import { Icon, type IconName } from "../../ui/Icon";
import { toast, toastError } from "../../ui/Toast";
import { t } from "../../i18n";

export type MenuAt = { account: AccountSummary; x: number; y: number };

// -- Main func
export function AccountMenu({ at, onClose, selection, canLaunch, onLaunch, onValidate, onRemove, onFriends }: {
  at: MenuAt | null;
  onClose: () => void;

  selection: number[];
  canLaunch: (id: number) => boolean;
  onLaunch: (ids: number[]) => void;
  onValidate: (ids: number[]) => void;
  onRemove: (ids: number[]) => void;
  onFriends: (account: AccountSummary) => void;
}) {
  const privacy = usePrivacy();
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState({ left: 0, top: 0 });
  const update = useUpdateAccount();
  const session = useMutation({ mutationFn: (id: number) => api.accountOpenSession(id) });

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el || !at) return;
    el.showPopover();
    const r = el.getBoundingClientRect();
    setPos({ left: Math.max(4, Math.min(at.x, innerWidth - r.width - 4)), top: Math.max(4, Math.min(at.y, innerHeight - r.height - 4)) });
    el.querySelector<HTMLElement>(".menu-item:not(:disabled)")?.focus();
  }, [at]);

  useEffect(() => {
    const el = ref.current;
    const toggled = (e: Event) => (e as ToggleEvent).newState === "closed" && onClose();
    el?.addEventListener("toggle", toggled);
    return () => el?.removeEventListener("toggle", toggled);
  }, [onClose]);

  const a = at?.account;
  const ids = a && selection.length > 1 && selection.includes(a.id) ? selection : a ? [a.id] : [];
  const many = ids.length > 1;
  const anyValid = ids.some(canLaunch);
  const run = (f: () => unknown) => () => {
    ref.current?.hidePopover();
    void f();
  };
  const clip = (text: string, done: string) => writeText(text).then(() => toast(done), toastError);

  const items: ([IconName, string, () => unknown, boolean?] | null)[] = a
    ? [
        ["play", many ? t("launch.launchMany", { count: ids.length }) : t("common.launch"), () => onLaunch(ids), !anyValid],
        ["signin", t("accounts.menu.openSession"), () => session.mutateAsync(a.id).catch(toastError)],
        ["external", t("accounts.menu.openProfile"), () => openUrl(`https://www.roblox.com/users/${a.userId}/profile`).catch(toastError)],
        ["friends", t("accounts.menu.friends"), () => onFriends(a), privacy],
        null,
        ["copy", t("accounts.menu.copyUsername"), () => clip(a.username, t("accounts.menu.usernameCopied")), privacy],
        ["copy", t("accounts.menu.copyUserId"), () => clip(String(a.userId), t("accounts.menu.userIdCopied")), privacy],
        null,
        ["star", a.favourite ? t("accounts.menu.unfavourite") : t("accounts.menu.favourite"), () => update.mutate({ id: a.id, patch: { favourite: !a.favourite } })],
        [
          "check",
          a.allowDisconnect ? t("accounts.menu.disallowDisconnect") : t("accounts.menu.allowDisconnect"),
          () => update.mutate({ id: a.id, patch: { allowDisconnect: !a.allowDisconnect } }),
        ],
        ["refresh", many ? t("accounts.menu.validateMany", { count: ids.length }) : t("accounts.menu.validate"), () => onValidate(ids)],
        ["trash", many ? t("accounts.menu.removeMany", { count: ids.length }) : t("common.removeEllipsis"), () => onRemove(ids)],
      ]
    : [];

  return (
    <div
      ref={ref}
      popover="auto"
      role="menu"
      aria-label={a ? t("accounts.menu.actionsFor", { name: a.username }) : t("accounts.menu.actions")}
      className="menu"
      onKeyDown={menuKeys}
      style={{ position: "fixed", positionArea: "none", inset: "auto", left: pos.left, top: pos.top, margin: 0, maxHeight: "none" }}
    >
      {items.map((it, i) =>
        it === null ? (
          <div key={i} role="separator" className="h-px my-1 bg-line" />
        ) : (
          <button
            key={it[1]}
            role="menuitem"
            className={`menu-item ${it[0] === "trash" ? "text-orange" : ""}`}
            disabled={it[3]}
            onClick={run(it[2])}
          >
            <Icon name={it[0]} size={12} fill={it[0] === "star" && a?.favourite} />
            {it[1]}
          </button>
        ),
      )}
    </div>
  );
}
