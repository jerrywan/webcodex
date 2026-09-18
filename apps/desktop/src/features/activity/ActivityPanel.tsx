import { useEffect, useState } from "react";
import type { ActivityEntry } from "../../models/topology";
import type { WorkflowSession } from "../../models/workspace";
import { useLocale } from "../../i18n/locale";
import { useProduct } from "../../i18n/product";
import { projectName, sessionTitle, useWorkspace, workspaceQuery } from "../workspace/WorkspaceContext";
import { observationTime } from "../workspace/WorkspaceStatus";
import { SystemActivity } from "./SystemActivity";
import { WorkflowSessionDetail, activityTitle, sessionLifecycle, SessionAttention } from "./WorkflowSessionDetail";
import { WindowActivityDetail } from "./WindowActivityDetail";

type View = "windows" | "sessions" | "system";
const VIEWS: View[] = ["windows", "sessions", "system"];
export function ActivityPanel({ activity }: { activity: ActivityEntry[] }) {
  const p = useProduct(); const { locale } = useLocale(); const workspace = useWorkspace();
  const [view, setView] = useState<View>(workspace.selection?.kind === "session" ? "sessions" : "windows");
  const [filter, setFilter] = useState("");
  const [projectSessions, setProjectSessions] = useState<{ project: string; rows: WorkflowSession[]; truncated: boolean } | null>(null);
  const [failed, setFailed] = useState(false);
  const [loading, setLoading] = useState(false);
  const [revision, setRevision] = useState(0);
  useEffect(() => {
    if (!filter || view !== "sessions") return;
    let cancelled = false;
    setLoading(true); setFailed(false); setProjectSessions(null);
    void workspaceQuery<{ sessions: WorkflowSession[]; truncated: boolean }>({ kind: "sessions", project: filter }).then(result => {
      if (!cancelled) setProjectSessions({ project: filter, rows: result.sessions.map(session => ({ ...session, project_id: filter })), truncated: result.truncated });
    }).catch(() => { if (!cancelled) setFailed(true); }).finally(() => { if (!cancelled) setLoading(false); });
    return () => { cancelled = true; };
  }, [filter, view, revision]);
  const sessions = filter ? projectSessions?.project === filter ? projectSessions.rows : [] : workspace.sessions;
  const windows = workspace.windows.filter(row => !filter || row.last_project === filter);
  const projectLabel = (id?: string) => projectName(workspace.projects.find(project => project.id === id) || { id: id || "—" });
  const refresh = () => { workspace.refresh(); setRevision(value => value + 1); };
  return <section className="page-section workspace-page" aria-labelledby="activity-title" data-webcodex-page="activity">
    <header className="page-heading-row"><h1 id="activity-title">{p("activity")}</h1><button className="secondary-button" onClick={refresh} disabled={workspace.loading || loading}>{p("refresh")}</button></header>
    <div className="workspace-tabs" role="tablist" aria-label={p("activity")}>
      {VIEWS.map(item => <button type="button" key={item} role="tab" id={`activity-tab-${item}`} aria-selected={view === item} aria-controls={`activity-view-${item}`} tabIndex={view === item ? 0 : -1} onClick={() => setView(item)} onKeyDown={event => {
        if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
        event.preventDefault(); const index = (VIEWS.indexOf(item) + (event.key === "ArrowRight" ? 1 : 2)) % 3;
        setView(VIEWS[index]); document.getElementById(`activity-tab-${VIEWS[index]}`)?.focus();
      }}>{p(item)}</button>)}
    </div>
    {view !== "system" && <div className="activity-project-filter"><label htmlFor="activity-project">{p("projects")}</label><select id="activity-project" value={filter} onChange={event => setFilter(event.target.value)}><option value="">{p("allProjects")}</option>{workspace.projects.filter(project => project.id).map(project => <option key={project.id} value={project.id}>{projectName(project)}</option>)}</select></div>}
    <section role="tabpanel" id={`activity-view-${view}`} aria-labelledby={`activity-tab-${view}`}>
      {view === "windows" && <>
        {workspace.windowsError && <p role="alert" className="workspace-notice">{p("loadError")}</p>}
        {windows.map(row => <button type="button" className="workspace-window-row" key={row.client_window_key} onClick={() => workspace.setSelection({ kind: "window", id: row.client_window_key })}>
          <span className="project-avatar" aria-hidden="true">W</span><span className="project-row-main"><strong>{p("windows")} · {row.client_window_key.slice(-12)}</strong><span>{projectLabel(row.last_project)}</span><small>{row.linked_session_count} {p("associatedSessions")}</small></span>
          <span className="window-row-state"><span className={`workspace-badge ${row.active_count ? "working" : ""}`}>{p(row.active_count ? "inProgress" : "observed")}</span><time>{observationTime(row.last_meaningful_activity_at_ms || row.last_seen_at_ms, locale)}</time></span>
        </button>)}
        {!windows.length && <p className="workspace-empty">{p(workspace.loading ? "loading" : "noWindows")}</p>}
      </>}
      {view === "sessions" && <>
        {(failed || workspace.error) && <p role="alert" className="workspace-notice">{p("loadError")}</p>}
        {sessions.map(session => <button type="button" className="workspace-session-row" key={`${session.project_id}:${session.session_id}`} onClick={() => session.project_id && workspace.setSelection({ kind: "session", project: session.project_id, id: session.session_id })}>
          <span className="session-row-heading"><strong>{sessionTitle(session.title)}</strong><span className="workspace-badge">{sessionLifecycle(session.lifecycle, p)}</span></span>
          <span className="session-row-project">{session.project_name || projectLabel(session.project_id)} · {observationTime(session.updated_at * 1000, locale)}</span>
          <span className="session-row-task">{activityTitle(session.current_activity || session.last_activity, p)}</span>
          <SessionAttention session={session} />
        </button>)}
        {!sessions.length && <p className="workspace-empty">{p(loading || workspace.loading ? "loading" : "noSessions")}</p>}
        {(filter ? projectSessions?.truncated : workspace.runner?.recent_sessions?.scan_truncated || workspace.runner?.recent_sessions?.truncated) && <p className="workspace-notice">{p("partial")}</p>}
      </>}
      {view === "system" && <SystemActivity activity={activity} />}
    </section>
    {workspace.selection?.kind === "session" && <WorkflowSessionDetail key={workspace.selection.id} project={workspace.selection.project} id={workspace.selection.id} onClose={() => workspace.setSelection(null)} />}
    {workspace.selection?.kind === "window" && <WindowActivityDetail key={workspace.selection.id} id={workspace.selection.id} onClose={() => workspace.setSelection(null)} />}
  </section>;
}
