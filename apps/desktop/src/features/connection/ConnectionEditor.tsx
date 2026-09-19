import { useRef, useState, type FormEvent } from "react";
import { desktopApi } from "../../lib/desktop-api";
import type { DesktopState } from "../../models/topology";
import type { TunnelConnection } from "../../models/connections-tools";
import { useProduct } from "../../i18n/product";
import { useConnectionsTools } from "../../i18n/connections-tools";
import { WorkspaceDialog } from "../workspace/WorkspaceDialog";

export function ConnectionEditor({ profile, onState, onClose }: { profile: TunnelConnection | null; onState: (state: DesktopState) => void; onClose: () => void }) {
  const p = useProduct(); const c = useConnectionsTools();
  const [name, setName] = useState(profile?.name || "");
  const [tunnelId, setTunnelId] = useState(profile?.tunnel_id || "");
  const [apiKey, setApiKey] = useState("");
  const [autostart, setAutostart] = useState(profile?.autostart ?? true);
  const [busy, setBusy] = useState(false); const [failed, setFailed] = useState(false);
  const submitted = useRef(false);
  const submit = async (event: FormEvent) => {
    event.preventDefault(); if (submitted.current) return;
    submitted.current = true; setBusy(true); setFailed(false);
    const request = { id: profile?.id ?? null, name: name.trim(), tunnel_id: tunnelId.trim(), api_key: apiKey || null, autostart, expected_revision: profile?.revision ?? null };
    setApiKey("");
    try { onState(await desktopApi.saveTunnelProfile(request)); onClose(); }
    catch { setFailed(true); }
    finally { request.api_key = null; submitted.current = false; setBusy(false); }
  };
  return <WorkspaceDialog title={profile ? `${p("edit")} ${profile.name}` : c("addConnection")} onClose={onClose} busy={busy}>
    <form className="profile-editor" onSubmit={event => void submit(event)} aria-busy={busy}>
      <div className="field-group"><label htmlFor="connection-profile-name">{c("name")}</label><input id="connection-profile-name" name="name" value={name} onChange={event => setName(event.target.value)} required maxLength={160} disabled={busy} autoFocus /></div>
      <div className="field-group"><label htmlFor="connection-profile-tunnel-id">Tunnel ID</label><input id="connection-profile-tunnel-id" name="tunnel_id" value={tunnelId} onChange={event => setTunnelId(event.target.value)} required maxLength={256} pattern="[A-Za-z0-9_-]+" spellCheck={false} autoCapitalize="none" disabled={busy} /></div>
      <div className="field-group"><label htmlFor="connection-profile-api-key">API Key</label><input id="connection-profile-api-key" name="api_key" type="password" value={apiKey} onChange={event => setApiKey(event.target.value)} required={!profile?.credential_present} maxLength={8192} autoComplete="new-password" spellCheck={false} disabled={busy} aria-describedby={profile?.credential_present ? "connection-key-retained" : undefined} />{profile?.credential_present && <small id="connection-key-retained">{p("savedKey")}</small>}</div>
      <label className="profile-checkbox" htmlFor="connection-profile-autostart"><input id="connection-profile-autostart" type="checkbox" checked={autostart} onChange={event => setAutostart(event.target.checked)} disabled={busy} />{c("autostart")}</label>
      {failed && <p className="workspace-notice" role="alert">{c("operationFailed")}</p>}
      <div className="connection-actions"><button type="submit" className="primary-button" disabled={busy || !name.trim() || !/^[A-Za-z0-9_-]+$/.test(tunnelId.trim()) || (!profile?.credential_present && !apiKey.trim())}>{c("saveApply")}</button><button type="button" className="secondary-button" disabled={busy} onClick={onClose}>{p("cancel")}</button></div>
    </form>
  </WorkspaceDialog>;
}
