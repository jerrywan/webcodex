import type { DesktopState } from "../../models/topology";
import { useProduct } from "../../i18n/product";
import { useConnectionsTools } from "../../i18n/connections-tools";
import { useLocale } from "../../i18n/locale";
import { sessionTitle, useWorkspace } from "../workspace/WorkspaceContext";
import { WorkspaceStatus, ChatgptObservation, observationTime } from "../workspace/WorkspaceStatus";
import { ProjectRows } from "../projects/ProjectRows";

interface DashboardProps {
  state: DesktopState; refreshing: boolean; onRefresh: () => void;
  onResumeRuntime: () => void;
  onChooseProject: () => void; onChangeSetup: () => void;
  onNavigate: (page: "projects" | "connection" | "activity" | "extensions") => void;
  onStopQuickShare: () => void; onStopRuntime: () => void;
  onOpenProject: (path: string) => void;
}
export function Dashboard(props: DashboardProps) {
  const { state, onNavigate } = props;
  const p = useProduct(); const c = useConnectionsTools(); const { locale } = useLocale();
  const workspace = useWorkspace();
  const recent = workspace.sessions.slice(0, 3);
  const busy = Boolean(state.current_operation);
  return <section className="page-section workspace-page" aria-labelledby="home-title" data-webcodex-page="home">
    <header className="page-heading-row">
      <div><span className="eyebrow">{p("workspace")}</span><h1 id="home-title">{state.readiness.runtime_ready ? p("ready") : "WebCodex"}</h1></div>
      <button type="button" className="secondary-button" disabled={busy || props.refreshing} onClick={() => { workspace.refresh(); props.onRefresh(); }}>{p("refresh")}</button>
    </header>
    <WorkspaceStatus state={state} />
    <div className="workspace-quick-actions">
      <span>{workspace.projects.length} {p("projects")}</span>
      <button className="primary-button" onClick={props.onChooseProject} disabled={busy}>{p("addProject")}</button>
      <button className="secondary-button" aria-label={`${p("manage")} ${c("connections")}`} onClick={() => onNavigate("connection")}>{c("connections")}</button>
      {!state.readiness.runtime_ready && <button className="secondary-button" onClick={state.readiness.next_action_kind === "restart_quick_share" ? props.onChangeSetup : props.onResumeRuntime} disabled={busy}>{state.readiness.next_action_kind === "restart_quick_share" ? p("restart") + " Quick Share" : p("start") + " WebCodex"}</button>}
      {(state.readiness.project === "error" || state.readiness.project === "reload_required") && <button className="secondary-button" onClick={props.onChooseProject} disabled={busy}>{p("setup")} {p("projects")}</button>}
    </div>
    <section className="workspace-section" aria-labelledby="recent-projects-title">
      <header className="workspace-section-heading"><h2 id="recent-projects-title">{p("recentProjects")}</h2><button className="text-button" onClick={() => onNavigate("projects")}>{p("allProjects")}</button></header>
      {workspace.error && <p role="alert" className="workspace-notice">{p("loadError")}</p>}
      <ProjectRows projects={workspace.projects.slice(0, 4)} onOpen={props.onOpenProject} />
      {!workspace.projects.length && <p className="workspace-empty">{p("noProjects")}</p>}
    </section>
    <section className="workspace-section" aria-labelledby="recent-activity-title">
      <header className="workspace-section-heading"><h2 id="recent-activity-title">{p("recentActivity")}</h2><button className="text-button" onClick={() => onNavigate("activity")}>{p("open")}</button></header>
      <ChatgptObservation state={state} />
      {recent.map(session => <button className="workspace-activity-link" key={session.session_id} onClick={() => {
        if (!session.project_id) return;
        workspace.setSelection({ kind: "session", project: session.project_id, id: session.session_id }); onNavigate("activity");
      }}><span className="workspace-badge">{p("sessions")}</span><strong>{sessionTitle(session.title)}</strong><span>{observationTime(session.updated_at * 1000, locale)}</span></button>)}
      {workspace.windows.slice(0, 2).map(window => <button className="workspace-activity-link" key={window.client_window_key} onClick={() => {
        workspace.setSelection({ kind: "window", id: window.client_window_key }); onNavigate("activity");
      }}><span className="workspace-badge">{p("windows")}</span><strong>{window.client_window_key.slice(-12)}</strong><span>{observationTime(window.last_meaningful_activity_at_ms || window.last_seen_at_ms, locale)}</span></button>)}
      {!recent.length && !workspace.windows.length && <p className="workspace-empty">{workspace.loading ? p("loading") : p("noActivity")}</p>}
    </section>
    {state.topology?.experience === "quick_share" && state.quick_share && <button className="secondary-button" onClick={props.onStopQuickShare} disabled={busy}>{p("stop")} Quick Share</button>}
  </section>;
}
