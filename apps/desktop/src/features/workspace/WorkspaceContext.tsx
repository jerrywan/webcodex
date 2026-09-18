import { createContext, useCallback, useContext, useEffect, useMemo, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { DesktopState } from "../../models/topology";
import type { RunnerOverview, WindowSummary, WorkspaceProject, WorkspaceRequest, WorkflowSession } from "../../models/workspace";

export function workspaceQuery<T>(request: WorkspaceRequest): Promise<T> {
  return invoke<T>("workspace_query", { request });
}
export function sameProjectPath(a?: string, b?: string): boolean {
  if (!a || !b) return false;
  const normalize = (value: string) => {
    const windows = /^[A-Za-z]:[\\/]/.test(value) || value.startsWith("\\\\");
    const path = value.replace(/[\\/]+$/, "") || "/";
    return windows ? path.replace(/\\/g, "/").toLowerCase() : path;
  };
  return normalize(a) === normalize(b);
}
export function sessionTitle(value: string): string {
  const title = value.trim().split(/\r?\n/).find(Boolean) || "Workflow Session";
  return title.length > 110 ? title.slice(0, 109) + "…" : title;
}
export function projectName(project: { name?: string; path?: string; id?: string }): string {
  return project.name || project.path?.split(/[\\/]/).filter(Boolean).pop() || project.id || "Project";
}
interface WorkspaceValue {
  state: DesktopState; runner: RunnerOverview | null; projects: WorkspaceProject[]; windows: WindowSummary[];
  sessions: WorkflowSession[]; loading: boolean; error: boolean; windowsError: boolean; refresh: () => void; revision: number;
  selection: { kind: "session"; project: string; id: string } | { kind: "window"; id: string } | null;
  setSelection: (value: WorkspaceValue["selection"]) => void;
}
const WorkspaceContext = createContext<WorkspaceValue | null>(null);
export function WorkspaceProvider({ state, children }: { state: DesktopState; children: ReactNode }) {
  const projectKey = state.project?.runtime_project_id || state.project?.path || "";
  const key = JSON.stringify([state.topology?.server, projectKey]);
  const [snapshot, setSnapshot] = useState<{ key: string; runner: RunnerOverview | null; windows: WindowSummary[]; error: boolean; windowsError: boolean } | null>(null);
  const [loading, setLoading] = useState(false);
  const [revision, setRevision] = useState(0);
  const [selection, setSelection] = useState<WorkspaceValue["selection"]>(null);
  const refresh = useCallback(() => setRevision(value => value + 1), []);
  const busy = Boolean(state.current_operation);
  const ready = state.readiness.runtime_ready;
  useEffect(() => {
    if (!ready || busy || !projectKey) return;
    let disposed = false;
    let timer: number | undefined;
    const poll = async () => {
      if (disposed) return;
      setLoading(true);
      const [runner, windows] = await Promise.allSettled([
        workspaceQuery<RunnerOverview>({ kind: "overview" }),
        workspaceQuery<{ windows: WindowSummary[] }>({ kind: "windows" }),
      ]);
      if (disposed) return;
      setSnapshot(old => ({
        key,
        runner: runner.status === "fulfilled" ? runner.value : old?.key === key ? old.runner : null,
        windows: windows.status === "fulfilled" ? windows.value.windows : old?.key === key ? old.windows : [],
        error: runner.status === "rejected", windowsError: windows.status === "rejected",
      }));
      setLoading(false);
      timer = window.setTimeout(() => { if (document.visibilityState === "visible") void poll(); }, 15_000);
    };
    const visible = () => { if (document.visibilityState === "visible") refresh(); };
    document.addEventListener("visibilitychange", visible);
    void poll();
    return () => { disposed = true; if (timer) window.clearTimeout(timer); document.removeEventListener("visibilitychange", visible); };
  }, [key, projectKey, ready, busy, revision, refresh]);
  useEffect(() => { setSelection(null); }, [key]);
  const current = snapshot?.key === key ? snapshot : null;
  const projects = useMemo(() => {
    const rows = [...(current?.runner?.projects || [])];
    for (const saved of state.saved_projects || (state.project ? [state.project] : [])) {
      if (!rows.some(row => sameProjectPath(row.path, saved.path))) rows.push({
        id: saved.runtime_project_id || "", path: saved.path, connected: false,
      });
    }
    return rows.sort((a, b) => (b.sessions?.latest_updated_at || 0) - (a.sessions?.latest_updated_at || 0));
  }, [current?.runner, state.saved_projects, state.project]);
  const ids = new Set(projects.map(project => project.id));
  return <WorkspaceContext.Provider value={{ state, runner: current?.runner || null, projects,
    windows: (current?.windows || []).filter(row => !row.last_project || ids.has(row.last_project)),
    sessions: current?.runner?.recent_sessions?.sessions || [], loading: ready && !busy && (loading || !current),
    error: Boolean(current?.error), windowsError: Boolean(current?.windowsError), refresh, revision, selection, setSelection,
  }}>{children}</WorkspaceContext.Provider>;
}
export function useWorkspace(): WorkspaceValue {
  const context = useContext(WorkspaceContext);
  if (!context) throw new Error("WorkspaceProvider is required");
  return context;
}
