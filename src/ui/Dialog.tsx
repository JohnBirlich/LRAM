import { useEffect, useId, useReducer, useRef, type ReactNode } from "react";
import { Icon } from "./Icon";
import { t } from "../i18n";

const FADE_MS = 250;

// -- Main func
export function Dialog({ open, onClose, title, width = 420, describedBy, children }: {
  open: boolean;
  onClose: () => void;
  title: string;
  width?: number;
  describedBy?: string;
  children: ReactNode;
}) {
  const ref = useRef<HTMLDialogElement>(null);
  const downOnBackdrop = useRef(false);
  const openedAt = useRef(0);
  const titleId = useId();
  const kept = useRef<{ title: string; children: ReactNode } | null>(null);
  const [, letGo] = useReducer((n: number) => n + 1, 0);
  if (open) kept.current = { title, children };
  useEffect(() => {
    if (open) return;
    const timer = setTimeout(() => {
      kept.current = null;
      letGo();
    }, FADE_MS);
    return () => clearTimeout(timer);
  }, [open]);
  const shown = kept.current ?? { title, children: null };

  useEffect(() => {
    const d = ref.current;
    if (!d) return;
    if (open && !d.open) {
      d.showModal();
      openedAt.current = Date.now();
      const first = d.querySelector<HTMLElement>("[data-autofocus]") ?? d.querySelector<HTMLElement>("input:not([disabled]):not([type=checkbox]):not([type=radio]), textarea");
      first?.focus();
    }
    if (!open && d.open) d.close();
  }, [open]);

  return (
    <dialog
      ref={ref}
      className="dlg"
      style={{ width }}
      aria-labelledby={titleId}
      aria-describedby={describedBy}
      onCancel={(e) => {
        e.preventDefault();
        onClose();
      }}
      onPointerDown={(e) => (downOnBackdrop.current = e.target === e.currentTarget)}
      onPointerUp={(e) => (downOnBackdrop.current &&= e.target === e.currentTarget)}
      onClick={(e) => {
        if (downOnBackdrop.current && e.target === e.currentTarget && Date.now() - openedAt.current > 400) onClose();
      }}
      onClose={(e) => {
        if (open && !e.currentTarget.open) {
          e.currentTarget.showModal();
          onClose();
        }
      }}
    >
      <div className="flex items-center gap-3 ps-5 pe-2.5 pt-2.5 pb-2 min-h-14">
        <h2 id={titleId} className="flex-1 text-lg font-semibold tracking-[-0.01em]">{shown.title}</h2>
        <button type="button" className="wc w-10 h-10" aria-label={t("common.close")} onClick={onClose}>
          <Icon name="x" size={14} />
        </button>
      </div>
      {shown.children}
    </dialog>
  );
}

export function ConfirmDialog({ open, onClose, onConfirm, title, text, confirmLabel, busy, safe }: {
  open: boolean;
  onClose: () => void;
  onConfirm: () => void;
  title: string;
  text: ReactNode;
  confirmLabel: string;
  busy?: boolean;

  safe?: boolean;
}) {
  const textId = useId();
  return (
    <Dialog open={open} onClose={onClose} title={title} width={400} describedBy={textId}>
      <div id={textId} className="px-5 pb-5 text-dim">{text}</div>
      <div className="flex justify-end gap-2 px-5 pb-5">
        <button type="button" className="btn" onClick={onClose} data-autofocus>{t("common.cancel")}</button>
        <button type="button" className={`btn ${safe ? "btn-primary" : "btn-danger"}`} onClick={onConfirm} disabled={busy}>
          {confirmLabel}
        </button>
      </div>
    </Dialog>
  );
}
