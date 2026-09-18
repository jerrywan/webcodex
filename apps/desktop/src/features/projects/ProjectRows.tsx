import { useEffect, useState } from "react";
import { useLocale } from "../../i18n/locale";
import { useProduct } from "../../i18n/product";
import type { GitSummary, WorkspaceProject } from "../../models/workspace";
import { sameProjectPath, projectName, useWorkspace, workspaceQuery } from "../workspace/WorkspaceContext";
import { observationTime } from "../workspace/WorkspaceStatus";

export function ProjectRows({ projects, onOpen }: { projects: WorkspaceProject[]; onOpen: (path: string) => void }) {
  const { state } = useWorkspace();
  return <div className="workspace-project-list" role="list">{projects.map(project => <ProjectRow key={project.id || project.path} project={project}
    selected={sameProjectPath(project.path, state.project?.path)} busy={Boolean(state.current_operation)} onOpen={onOpen} />)}</div>;
}
function ProjectRow({ project, selected, busy, onOpen }: { project: WorkspaceProject; selected: boolean; busy: boolean; onOpen: (path: string) => void }) {
  const p = useProduct(); const { locale } = useLocale();
  const [git, setGit] = useState<GitSummary | null>(null);
  const { revision } = useWorkspace();
  useEffect(() => {
    let cancelled = false;
    setGit(null);
    if (project.id && project.connected) void workspaceQuery<GitSummary>({ kind: "project_git", project: project.id }).then(value => { if (!cancelled) setGit(value); }).catch(() => undefined);
    return () => { cancelled = true; };
  }, [project.id, project.connected, revision]);
  const name = projectName(project);
  return <article className={`workspace-project-row ${selected ? "selected" : ""}`} role="listitem" aria-label={name}>
    <div className="project-avatar" aria-hidden="true">{name.slice(0, 2).toUpperCase()}</div>
    <div className="project-row-main">
      <div className="project-row-title"><h3>{name}</h3>{selected && <span className="workspace-badge">{p("current")}</span>}</div>
      <span className="project-path" title={project.path}>{project.path}</span>
      <div className="project-row-meta">
        <span title={p("branch")}>{git?.branch || (git?.non_git_project ? p("notGit") : "—")}</span>
        <span>{project.sessions ? `${project.sessions.active_sessions}${project.sessions.sessions_truncated ? "+" : ""} ${p("activeSessions")}` : p(project.id ? "unknown" : "setup")}</span>
        <span>{observationTime(project.sessions?.latest_updated_at ? project.sessions.latest_updated_at * 1000 : null, locale)}</span>
      </div>
    </div>
    <button type="button" className="secondary-button" aria-label={`${p("openProject")} ${name}`} disabled={busy || !project.path}
      onClick={() => project.path && onOpen(project.path)} data-webcodex-action="open-project">{p("open")}</button>
  </article>;
}
