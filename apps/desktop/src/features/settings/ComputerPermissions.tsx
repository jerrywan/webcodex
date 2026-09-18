import { useCallback, useEffect, useRef, useState } from "react";
import { desktopApi } from "../../lib/desktop-api";
import type { ComputerPermissions as Permissions } from "../../models/topology";
import { useLocale } from "../../i18n/locale";

const EXPLAINED_KEY = "desktop-permissions-explained";
function wasExplained() {
  try { return localStorage.getItem(EXPLAINED_KEY) === "1"; } catch { return false; }
}

export function ComputerPermissions({ welcome = false }: { welcome?: boolean }) {
  const { t } = useLocale();
  const [permissions, setPermissions] = useState<Permissions | null>(null);
  const [dismissed, setDismissed] = useState(() => welcome && wasExplained());
  const [foreground, setForeground] = useState(false);
  const [busy, setBusy] = useState(false);
  const [failed, setFailed] = useState(false);
  const dialogRef = useRef<HTMLDialogElement>(null);
  const generation = useRef(0);
  const requestInFlight = useRef(false);
  const observe = useCallback(async () => {
    if (requestInFlight.current) return;
    const current = ++generation.current;
    try {
      const next = await desktopApi.computerPermissions();
      if (generation.current !== current) return;
      setPermissions(next); setFailed(false);
      if (next.foreground) setForeground(true);
    } catch { if (generation.current === current) setFailed(true); }
  }, []);
  useEffect(() => {
    if (dismissed) return;
    void observe();
    const onFocus = () => { void observe(); };
    window.addEventListener("focus", onFocus);
    return () => { generation.current++; window.removeEventListener("focus", onFocus); };
  }, [dismissed, observe]);
  const showWelcome = welcome && !dismissed && foreground && permissions?.supported
    && !(permissions.desktop_accessibility && permissions.desktop_screen_recording);
  useEffect(() => {
    const dialog = dialogRef.current;
    if (!showWelcome || !dialog) return;
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    if (!dialog.open) dialog.showModal?.();
    return () => { dialog.close?.(); if (previous?.isConnected) previous.focus(); };
  }, [showWelcome]);
  const dismiss = () => {
    try { localStorage.setItem(EXPLAINED_KEY, "1"); } catch { /* Session-local dismissal still works. */ }
    setDismissed(true);
  };
  const request = async (action?: "accessibility" | "screen_recording" | "open_settings") => {
    if (requestInFlight.current) return;
    requestInFlight.current = true;
    setBusy(true); setFailed(false);
    const current = ++generation.current;
    try {
      const next = await (action ? desktopApi.requestComputerPermission(action) : desktopApi.computerPermissions());
      if (generation.current === current) setPermissions(next);
    } catch { if (generation.current === current) setFailed(true); }
    finally { requestInFlight.current = false; if (generation.current === current) setBusy(false); }
  };
  if (welcome && !showWelcome) return null;
  if (!welcome && permissions && !permissions.supported) return null;
  const content = <>
    <h2 id={welcome ? "permission-welcome-title" : "permission-settings-title"}>{t("permissions.title")}</h2>
    <p>{t("permissions.owner")}</p>
    {permissions && <dl className="detail-list">
      <div><dt>{t("permissions.accessibility")}</dt><dd>{t(permissions.desktop_accessibility ? "permissions.granted" : "permissions.notGranted")}</dd></div>
      <div><dt>{t("permissions.screen")}</dt><dd>{t(permissions.desktop_screen_recording ? "permissions.granted" : "permissions.notGranted")}</dd></div>
      <div><dt>Runner</dt><dd>{t("permissions.runnerUnknown")}</dd></div>
    </dl>}
    <div className="permission-actions">
      {permissions?.supported && <>
        <button type="button" className="primary-button" data-webcodex-action="request-accessibility" disabled={busy || permissions.desktop_accessibility} onClick={() => void request("accessibility")}>{t("permissions.requestAccessibility")}</button>
        <button type="button" className="secondary-button" data-webcodex-action="request-screen-recording" disabled={busy || permissions.desktop_screen_recording} onClick={() => void request("screen_recording")}>{t("permissions.requestScreen")}</button>
        <button type="button" className="secondary-button" disabled={busy} onClick={() => void request("open_settings")}>{t("permissions.openSettings")}</button>
      </>}
      <button type="button" className="secondary-button" data-webcodex-action="recheck-permissions" disabled={busy} onClick={() => void request()}>{t("permissions.recheck")}</button>
    </div>
    <p className="field-help">{t("permissions.restartHelp")}</p>
    {failed && <p role="alert">{t("permissions.error")}</p>}
    {!permissions && !failed && <p role="status">{t("common.checking")}</p>}
    {welcome && <button type="button" className="secondary-button permission-continue" data-webcodex-action="dismiss-permission-welcome" onClick={dismiss}>{t("permissions.later")}</button>}
  </>;
  return welcome
    ? <dialog ref={dialogRef} className="permission-dialog" aria-labelledby="permission-welcome-title" onCancel={event => { event.preventDefault(); dismiss(); }}>{content}</dialog>
    : <section className="detail-card permission-panel" aria-labelledby="permission-settings-title">{content}</section>;
}
