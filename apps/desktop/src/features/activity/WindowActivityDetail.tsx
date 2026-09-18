import { useEffect, useState } from "react";
import type { WindowDetail } from "../../models/workspace";
import { useLocale } from "../../i18n/locale";
import { useProduct, type ProductKey } from "../../i18n/product";
import { projectName, useWorkspace, workspaceQuery } from "../workspace/WorkspaceContext";
import { WorkspaceDialog } from "../workspace/WorkspaceDialog";
import { observationTime } from "../workspace/WorkspaceStatus";

function windowAction(tool: string | undefined, p: (key: ProductKey) => string): string {
  if (!tool) return p("activity");
  if (/read|search|inspect|skill_load/.test(tool)) return p("exploration");
  if (/edit|write|patch/.test(tool)) return p("editing");
  if (/test|check|build/.test(tool)) return p("validation");
  if (/diff|changes|review|hygiene/.test(tool)) return p("review");
  if (/run|job/.test(tool)) return p("jobs");
  return p("activity");
}
export function WindowActivityDetail({ id, onClose }: { id: string; onClose: () => void }) {
  const p = useProduct(); const { locale } = useLocale(); const workspace = useWorkspace();
  const [detail, setDetail] = useState<WindowDetail | null>(null);
  const [failed, setFailed] = useState(false);
  const [revision, setRevision] = useState(0);
  useEffect(() => {
    let cancelled = false;
    setDetail(null); setFailed(false);
    void workspaceQuery<WindowDetail>({ kind: "window", client_window_key: id }).then(next => { if (!cancelled) setDetail(next); }).catch(() => { if (!cancelled) setFailed(true); });
    return () => { cancelled = true; };
  }, [id, revision]);
  const name = (project?: string) => projectName(workspace.projects.find(row => row.id === project) || { id: project || "—" });
  return <WorkspaceDialog title={`${p("windows")} · ${id.slice(-12)}`} onClose={onClose}>
    {failed && <p role="alert">{p("loadError")} <button className="text-button" onClick={() => setRevision(value => value + 1)}>{p("refresh")}</button></p>}
    {!detail && !failed && <p role="status">{p("loading")}</p>}
    {detail && <>
      <div className="session-detail-meta"><span className="workspace-badge">{p(detail.active_count ? "inProgress" : "observed")}</span><span>{observationTime(detail.last_meaningful_activity_at_ms || detail.last_seen_at_ms, locale)}</span></div>
      <section className="workspace-section"><h3>{p("associatedSessions")}</h3>
        {detail.linked_sessions.map(session => <button type="button" className="workspace-activity-link" key={session.workflow_session_id} disabled={!session.project || !session.workflow_session_id} onClick={() => {
          if (session.project && session.workflow_session_id) workspace.setSelection({ kind: "session", project: session.project, id: session.workflow_session_id });
        }}><strong>{session.title || session.workflow_session_id?.slice(-12)}</strong><span>{name(session.project)}</span></button>)}
        {!detail.linked_sessions.length && <p>{p("noSessions")}</p>}
        {detail.sessions_truncated && <p className="workspace-notice">{p("partial")}</p>}
      </section>
      <section className="workspace-section"><h3>{p("recentActivity")}</h3><div className="workspace-timeline">
        {detail.activity.filter(row => row.meaningful !== false).map((row, index) => <article key={`${row.started_at_ms}-${index}`}>
          <div><strong>{windowAction(row.tool_name, p)}</strong><span>{name(row.project)}</span></div>
          <span className="activity-outcome">{p(row.status === "failed" || row.status === "error" ? "failed" : "observed")}</span>
          <time>{observationTime(row.ended_at_ms || row.started_at_ms, locale)}</time>
        </article>)}
      </div>{detail.activity_truncated && <p className="workspace-notice">{p("partial")}</p>}</section>
      <details className="workspace-technical"><summary>{p("details")}</summary><code>{id}</code></details>
    </>}
  </WorkspaceDialog>;
}
