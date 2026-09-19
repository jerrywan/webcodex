import { useEffect, useRef, type ReactNode } from "react";
import { useProduct } from "../../i18n/product";

export function WorkspaceDialog({ title, onClose, children, busy = false }: { title: string; onClose: () => void; children: ReactNode; busy?: boolean }) {
  const p = useProduct();
  const ref = useRef<HTMLDialogElement>(null);
  const closeRef = useRef(onClose);
  closeRef.current = onClose;
  useEffect(() => {
    const dialog = ref.current;
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    dialog?.showModal?.();
    return () => { dialog?.close?.(); if (previous?.isConnected) previous.focus(); };
  }, []);
  return <dialog ref={ref} className="workspace-dialog" aria-label={title}
    onCancel={event => { event.preventDefault(); if (!busy) closeRef.current(); }}>
    <header className="workspace-section-heading"><h2>{title}</h2><button type="button" className="secondary-button" onClick={onClose} disabled={busy}>{p("close")}</button></header>
    {children}
  </dialog>;
}
