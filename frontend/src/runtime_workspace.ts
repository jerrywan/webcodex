import { runtimeIcon } from "./runtime_icons.js";
import { translate, localizedWorkflowText, type RuntimeLanguage } from "./runtime_i18n.js";
import { workflowSessionOverviewPresentation } from "./workflow_session_state.js";
import { activityDescription, formatLivenessPresentation, formatUpdatedTime } from "./runtime_activity.js";
import { pendingAttentionCount } from "./runtime_overview.js";
import { renderProjectWindowCards, formatWindowEmptyState } from "./runtime_window.js";

// These projections use only authorized Workflow Session evidence. Window
// observations never contribute to Session status, completion or authority.
export function workspaceSessionGroups(sessions: any[]): { attention: any[]; working: any[]; completed: any[]; recent: any[] } {
  const rows = [...sessions].sort((a, b) => Number(b.updated_at || 0) - Number(a.updated_at || 0));
  const attention = rows.filter(row => pendingAttentionCount(row.overview?.attention) > 0 || row.overview?.validation?.state === "failed");
  const working = rows.filter(row => row.running_call === true || Number(row.running_jobs) > 0);
  const completed = rows.filter(row => row.lifecycle === "closed");
  return { attention, working, completed, recent: rows };
}

export function workspaceSessionEvidence(detail: any): { files: string[]; jobs: any[]; completed: any[] } {
  const activity = Array.isArray(detail?.activity) ? detail.activity : [];
  // Paths from exploration are not changed files. This is retained edit evidence,
  // not a claim about the current Git working tree.
  const files = [...new Set<string>(activity.filter((row: any) => row.kind === "Edited")
    .flatMap((row: any) => Array.isArray(row.paths) ? row.paths.map(String) : []))];
  const jobs = activity.filter((row: any) => row.job_id);
  const completed = activity.filter((row: any) => typeof row.finished_at === "number" && row.state === "succeeded" && !row.job_handoff);
  return { files, jobs, completed: completed.slice(-5).reverse() };
}

function workspaceNode<K extends keyof HTMLElementTagNameMap>(tag: K, text = "", className = ""): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  node.textContent = text;
  node.className = className;
  return node;
}

function workspaceButton(text: string, action: string, callback: () => void): HTMLButtonElement {
  const button = workspaceNode("button", text, "workspace-action");
  button.type = "button";
  button.dataset.action = action;
  workspaceControlKeys.set(button, action);
  button.addEventListener("click", callback);
  return button;
}

export interface WorkspaceHomeOptions {
  language: RuntimeLanguage;
  projects: any[];
  project: any | null;
  sessions: any[];
  sessionsStatus: string;
  sessionsAvailable: boolean;
  windows: any[];
  windowAvailability: "idle" | "loading" | "available" | "stale" | "unavailable";
  windowScope: "global" | "principal";
  windowStatus: string;
  onProject: (runner: string, project: string) => void;
  onSession: (session: any) => void;
  onWindow: (key: string) => void;
  onWindows: () => void;
  onSearch: () => void;
}

export function renderWorkspaceHome(node: HTMLElement | null, options: WorkspaceHomeOptions): void {
  if (!node) return;
  // Preserve focused controls on polling when the evidence has not changed.
  const signature = JSON.stringify([options.language, options.projects, options.project, options.sessions,
    options.sessionsAvailable, options.sessionsStatus, options.windows, options.windowAvailability, options.windowStatus]);
  // Keep the fingerprint in memory, never copy authorized evidence to data-*.
  if (workspaceHomeSignatures.get(node) === signature) return;
  workspaceHomeSignatures.set(node, signature);
  const focusedKey = node.contains(document.activeElement) ? workspaceControlKeys.get(document.activeElement as HTMLElement) : null;
  node.replaceChildren();
  const tr = (text: string): string => translate(text, options.language);
  const project = options.project;
  const heading = workspaceNode("header", "", "workspace-home-heading");
  heading.appendChild(workspaceNode("p", project ? String(project.client_id || "") : tr("Your workspace"), "eyebrow"));
  heading.appendChild(workspaceNode("h2", project ? String(project.name || project.id) : tr("Choose where to work")));
  heading.appendChild(workspaceNode("p", project
    ? tr(project.connected === false ? "Runner disconnected. Open diagnostics to check the connection." : "Review observed work, then choose a Session to continue.")
    : tr("Find a project, review recent work, or inspect client activity."), "muted"));
  heading.appendChild(workspaceButton(tr("Find a project"), "workspace-find-project", options.onSearch));
  node.appendChild(heading);
  if (!project) {
    const projects = workspaceNode("section", "", "workspace-projects");
    projects.appendChild(workspaceNode("h3", tr("Projects")));
    for (const row of options.projects) {
      const button = workspaceButton("", "workspace-open-project", () => options.onProject(String(row.client_id || ""), String(row.id || "")));
      workspaceControlKeys.set(button, "project:" + String(row.id));
      button.appendChild(workspaceNode("strong", String(row.name || row.id)));
      button.appendChild(workspaceNode("span", String(row.client_id || "") + " · " + tr(row.connected === false ? "offline" : "Project"), "muted small"));
      projects.appendChild(button);
    }
    if (!options.projects.length) projects.appendChild(workspaceNode("p", tr("No visible Projects"), "muted"));
    node.appendChild(projects);
  }
  const groups = workspaceSessionGroups(options.sessions);
  const work = workspaceNode("div", "", "workspace-work-sections");
  for (const [label, rows, empty] of (options.sessionsAvailable || options.sessions.length ? [
    ["Needs attention", groups.attention, "No attention requests in loaded Sessions."],
    ["Working now", groups.working, "No active work observed in loaded Sessions."],
    ["Recently closed", groups.completed, "No closed Sessions in this retained view."],
    ["Recent work Sessions", groups.recent, "Choose a project to load its work Sessions."],
  ] as const : [])) {
    const section = workspaceNode("section", "", "workspace-work-section");
    section.appendChild(workspaceNode("h3", tr(label) + " · " + rows.length));
    for (const session of rows.slice(0, 6)) {
      const button = workspaceButton("", "workspace-open-session", () => options.onSession(session));
      workspaceControlKeys.set(button, label + ":" + String(session.session_id));
      button.appendChild(workspaceNode("strong", String(session.title || tr("Untitled Session"))));
      const liveness = formatLivenessPresentation(session, options.language);
      button.appendChild(workspaceNode("span", (session.lifecycle === "closed" ? tr("closed") : liveness.label) + " · " + formatUpdatedTime(session.updated_at, options.language), "muted small"));
      const overview = workflowSessionOverviewPresentation(session.overview);
      const preview = session.current_activity || session.last_activity;
      button.appendChild(workspaceNode("span", label === "Needs attention"
        ? localizedWorkflowText(overview.attentionText + " · " + overview.validationText, options.language)
        : preview ? activityDescription(preview, options.language) : localizedWorkflowText(overview.workText, options.language), "workspace-preview small"));
      section.appendChild(button);
    }
    if (!rows.length) section.appendChild(workspaceNode("p", tr(empty), "muted small"));
    if (rows.length > 6) section.appendChild(workspaceNode("p", tr("More Sessions are available in the sidebar."), "muted small"));
    work.appendChild(section);
  }
  node.appendChild(workspaceNode("p", options.sessionsStatus || tr("Loaded evidence only; counts may be bounded."), "muted small workspace-retention"));
  node.appendChild(work);
  const windows = workspaceNode("section", "", "workspace-window-section");
  windows.appendChild(workspaceNode("h3", tr("Window activity")));
  windows.appendChild(workspaceNode("p", tr("Client calls, separate from work Sessions. Observation does not mean the host is online."), "muted small"));
  windows.appendChild(workspaceButton(tr("Browse Window activity"), "workspace-open-windows", options.onWindows));
  if (project) {
    windows.appendChild(workspaceNode("p", options.windowStatus || (options.windows.length && options.windowAvailability === "available" ? String(options.windows.length) + " · " + tr("Window activity") : formatWindowEmptyState(options.windowAvailability, options.windowScope, true, options.language)), "muted small"));
    const list = workspaceNode("div", "", "workspace-window-list");
    renderProjectWindowCards(list, options.windows, options.onWindow, Date.now(), options.language);
    windows.appendChild(list);
  }
  node.appendChild(windows);
  if (focusedKey) Array.from(node.querySelectorAll<HTMLElement>("button")).find(button => workspaceControlKeys.get(button) === focusedKey)?.focus();
}
const workspaceControlKeys = new WeakMap<HTMLElement, string>();
const workspaceHomeSignatures = new WeakMap<HTMLElement, string>();

export function renderWorkspaceEvidence(node: HTMLElement | null, detail: any, language: RuntimeLanguage): void {
  if (!node) return;
  node.replaceChildren();
  const tr = (text: string): string => translate(text, language);
  const evidence = workspaceSessionEvidence(detail);
  const overview = workflowSessionOverviewPresentation(detail?.overview);
  const facts: [string, string][] = [
    ["Observed work", localizedWorkflowText(overview.workText, language)],
    ["Attention", localizedWorkflowText(overview.attentionText, language)],
    ["Validation", localizedWorkflowText(overview.validationText, language)],
    ["Running Jobs", String(detail.running_jobs ?? 0) + (detail.running_jobs_complete === false ? " · " + tr("partial scan") : "")],
  ];
  for (const [label, text] of facts) {
    const fact = workspaceNode("div", "", "workspace-evidence-fact");
    fact.appendChild(workspaceNode("dt", tr(label)));
    fact.appendChild(workspaceNode("dd", text));
    node.appendChild(fact);
  }
  const edits = workspaceNode("div", "", "workspace-evidence-fact");
  edits.appendChild(workspaceNode("dt", tr("Files in retained edits")));
  edits.appendChild(workspaceNode("dd", evidence.files.length ? evidence.files.join(" · ") : tr("No file edits in loaded activity.")));
  node.appendChild(edits);
  const jobs = workspaceNode("div", "", "workspace-evidence-fact");
  jobs.appendChild(workspaceNode("dt", tr("Recent Job evidence")));
  jobs.appendChild(workspaceNode("dd", evidence.jobs.length ? evidence.jobs.slice(-3).reverse().map(row => activityDescription(row, language)).join(" · ") : tr("No Jobs in loaded activity.")));
  node.appendChild(jobs);
  const completed = workspaceNode("div", "", "workspace-evidence-fact");
  completed.appendChild(workspaceNode("dt", tr("Recently completed")));
  completed.appendChild(workspaceNode("dd", evidence.completed.length ? evidence.completed.slice(0, 3).map(row => activityDescription(row, language)).join(" · ") : tr("No successful completions in loaded activity.")));
  node.appendChild(completed);
}

function workspaceListChip(parent: HTMLElement, text: string, extraClass = ""): HTMLElement {
  const chip = workspaceNode("span", text, "chip " + extraClass);
  parent.appendChild(chip);
  return chip;
}
export function renderWorkspaceSessionList(node: HTMLElement, visible: any[], selected: string, language: RuntimeLanguage, onSelect: (id: string) => void): void {
  const tr = (text: string): string => translate(text, language);
  for (const session of visible) {
    const id = String(session && session.session_id || "");
    if (!id) continue;
    const wrapper = document.createElement("li");
    const item = document.createElement("button");
    item.type = "button";
    item.dataset.action = "select-work-session";
    item.className = "session-card" + (id === selected ? " selected" : "");
    if (id === selected) item.setAttribute("aria-current", "true");
    const icon = document.createElement("span"); icon.className = "session-card-icon"; icon.setAttribute("aria-hidden", "true"); icon.appendChild(runtimeIcon("message"));
    const main = document.createElement("div"); main.className = "session-card-main";
    const title = document.createElement("div"); title.className = "session-title"; title.textContent = session.title ? String(session.title) : id;
    const meta = document.createElement("div"); meta.className = "chips session-meta";
    const lifecycle = String(session.lifecycle || "unknown");
    if (lifecycle !== "active") workspaceListChip(meta, tr(lifecycle));
    const liveness = formatLivenessPresentation(session, language);
    const livenessChip = workspaceListChip(meta, liveness.label, liveness.state === "working" ? "tone-runtime" : liveness.state === "attention" ? "tone-warn" : "");
    livenessChip.title = liveness.tooltip;
    workspaceListChip(meta, formatUpdatedTime(session.updated_at, language));
    main.appendChild(title); main.appendChild(meta);
    item.appendChild(icon); item.appendChild(main);
    const select = (): void => onSelect(id);
    item.addEventListener("click", select);
    wrapper.appendChild(item);
    node.appendChild(wrapper);
  }
}

export function installWorkspaceCommands(options: {
  language: () => RuntimeLanguage;
  available: () => boolean;
  projects: () => any[];
  onProject: (runner: string, project: string) => void;
  onView: (view: "home" | "sessions" | "windows" | "operations") => void;
}): void {
  const dialog = document.getElementById("runtime-command-dialog") as HTMLDialogElement | null;
  const input = document.getElementById("runtime-command-query") as HTMLInputElement | null;
  const results = document.getElementById("runtime-command-results");
  if (!dialog || !input || !results) return;
  let returnFocus: HTMLElement | null = null;
  const render = (): void => {
    const language = options.language();
    const query = input.value.trim().toLocaleLowerCase();
    results.replaceChildren();
    const entries: { label: string; action: string; run: () => void }[] = [
      ...([ ["Project overview", "home"], ["Work Sessions", "sessions"], ["Window activity", "windows"], ["Diagnostics & Agents", "operations"] ] as const)
        .map(([label, view]) => ({ label: translate(label, language), action: "command-" + view, run: () => options.onView(view) })),
      ...options.projects().map(project => ({ label: String(project.name || project.id) + " · " + String(project.client_id || ""), action: "command-project", run: () => options.onProject(String(project.client_id || ""), String(project.id || "")) })),
    ];
    for (const entry of entries.filter(entry => entry.label.toLocaleLowerCase().includes(query))) {
      results.appendChild(workspaceButton(entry.label, entry.action, () => { dialog.close(); if (options.available()) entry.run(); }));
    }
    if (!results.childElementCount) results.appendChild(workspaceNode("p", translate("No matching destinations.", language), "muted"));
  };
  const open = (): void => {
    if (!options.available() || dialog.open) return;
    returnFocus = document.activeElement as HTMLElement | null;
    input.value = "";
    render();
    dialog.showModal();
    input.focus();
  };
  document.getElementById("runtime-open-commands")?.addEventListener("click", open);
  document.getElementById("runtime-command-close")?.addEventListener("click", () => dialog.close());
  dialog.addEventListener("close", () => returnFocus?.focus());
  input.addEventListener("input", render);
  input.addEventListener("keydown", event => {
    if (event.isComposing) return;
    if (event.key === "ArrowDown") { event.preventDefault(); results.querySelector<HTMLButtonElement>("button")?.focus(); }
    if (event.key === "Enter") { event.preventDefault(); results.querySelector<HTMLButtonElement>("button")?.click(); }
  });
  document.addEventListener("keydown", event => {
    if ((event.metaKey || event.ctrlKey) && event.shiftKey && !event.altKey && !event.isComposing && event.key.toLowerCase() === "k") {
      event.preventDefault(); open();
    }
  });
}

export function renderWorkspaceOverview(overview: any, language: RuntimeLanguage): void {
  const view = workflowSessionOverviewPresentation(overview);
  for (const [id, text] of [
    ["work", view.workText], ["attention", view.attentionText],
    ["validation", view.validationText + (typeof view.validationAt === "number" ? " · " + formatUpdatedTime(view.validationAt, language) : "")],
    ["progress", view.progressText + (typeof view.progressAt === "number" ? " · " + formatUpdatedTime(view.progressAt, language) : "")],
  ]) {
    const node = document.getElementById("runtime-overview-" + id);
    if (node) node.textContent = localizedWorkflowText(text, language);
  }
  for (const [id, tone] of [["validation", view.validationTone], ["attention", view.attentionTone]]) {
    const node = document.getElementById("runtime-overview-" + id + "-card");
    for (const name of ["pass", "warn", "fail", "muted"]) node?.classList.toggle("tone-card-" + name, tone === name);
  }
}
