import { useEffect, useState } from "react";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { desktopApi } from "../../lib/desktop-api";
import type { DesktopError, DesktopState } from "../../models/topology";
import { useLocale } from "../../i18n/locale";
import { useProduct } from "../../i18n/product";
import { desktopErrorPresentation, normalizeDesktopError } from "../../i18n/presentation";
import { ChatgptObservation, WorkspaceStatus, statusKey } from "../workspace/WorkspaceStatus";
import { TunnelConfigDiagnostics } from "./TunnelConfigDiagnostics";

export function ConnectionPanel({ state, onState }: { state: DesktopState; onState: (state: DesktopState) => void }) {
  const { t } = useLocale(); const p = useProduct();
  const [editing, setEditing] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<DesktopError | null>(null);
  const [copied, setCopied] = useState(false);
  const [applied, setApplied] = useState(false);
  const tunnelId = state.openai_tunnel_config.effective_tunnel_id ?? state.openai_tunnel_config.saved_tunnel_id;
  const disabled = busy || Boolean(state.current_operation);
  const local = state.topology?.server.kind === "local" && state.topology.experience === "full";
  useEffect(() => { setCopied(false); }, [tunnelId]);
  useEffect(() => { if (copied) { const timer = window.setTimeout(() => setCopied(false), 2500); return () => window.clearTimeout(timer); } }, [copied]);
  const run = async (action: "start" | "stop" | "restart") => {
    if (disabled) return;
    setBusy(true); setError(null);
    try {
      if (action === "stop" || action === "restart") onState(await desktopApi.stopRegularTunnel());
      if (action === "start" || action === "restart") onState(await desktopApi.startRegularTunnel());
    } catch (value) { setError(normalizeDesktopError(value)); }
    finally { setBusy(false); }
  };
  return <section className="page-section workspace-page" aria-labelledby="connection-title" data-webcodex-page="connection">
    <header className="page-heading-row"><h1 id="connection-title">{p("connect")}</h1></header>
    <WorkspaceStatus state={state} />
    <section className="connection-summary" aria-labelledby="secure-tunnel-title">
      <header className="workspace-section-heading"><div><h2 id="secure-tunnel-title">Secure Tunnel</h2><span className={`connection-state ${state.regular_tunnel?.status === "ready" ? "ready" : ""}`}>{p(local ? state.regular_tunnel ? statusKey(state.regular_tunnel.status) : state.openai_tunnel_configured ? "stopped" : "notConfigured" : "unknown")}</span></div>
        {local && !editing && <div className="connection-actions"><button type="button" className="secondary-button" onClick={() => { setEditing(true); setApplied(false); }} disabled={disabled} aria-label={p("edit") + " Secure Tunnel"}>{p("edit")}</button>
          {state.regular_tunnel ? <><button className="secondary-button" onClick={() => void run("restart")} disabled={disabled}>{p("restart")}</button><button className="secondary-button" data-webcodex-action="stop-regular-tunnel" onClick={() => void run("stop")} disabled={disabled}>{p("stop")}</button></> : <button className="primary-button" data-webcodex-action="start-regular-tunnel" onClick={() => void run("start")} disabled={disabled || !state.openai_tunnel_configured || !state.readiness.runtime_ready}>{p("start")}</button>}
        </div>}
      </header>
      {local && tunnelId && <div className="connection-id"><span>Tunnel ID</span><code>{tunnelId}</code><button type="button" className="text-button" onClick={() => { void writeText(tunnelId).then(() => setCopied(true)).catch(value => setError(normalizeDesktopError(value))); }}>{copied ? p("copied") : t("connection.copyTunnelId")}</button></div>}
      {state.topology?.server.kind === "remote" && <p className="workspace-observation">{state.topology.server.url}</p>}
      {state.topology?.experience === "quick_share" && <button className="secondary-button" onClick={() => { void desktopApi.stopQuickShare().then(onState).catch(value => setError(normalizeDesktopError(value))); }} disabled={disabled}>{p("stop")} Quick Share</button>}
      {editing && <TunnelConfigDiagnostics state={state} onState={onState} onApplied={() => { setEditing(false); setApplied(true); }} onCancel={() => setEditing(false)} />}
      {applied && <p role="status" className="workspace-observation">{p("applied")}</p>}
      {error && <div role="alert" className="error-card"><strong>{desktopErrorPresentation(error, t).title}</strong><span>{desktopErrorPresentation(error, t).action}</span><details><summary>{p("details")}</summary><p>{error.message}</p></details></div>}
    </section>
    <section className="workspace-section connection-chatgpt"><h2>ChatGPT</h2><ChatgptObservation state={state} /></section>
  </section>;
}
