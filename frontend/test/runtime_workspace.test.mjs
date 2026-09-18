import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import vm from "node:vm";
import { workspaceSessionGroups, workspaceSessionEvidence, renderWorkspaceHome, renderWorkspaceEvidence, installWorkspaceCommands } from "../dist/runtime_workspace.js";
import { workspaceViewPreference } from "../dist/runtime_storage.js";
import { resolveRuntimeContextState } from "../dist/runtime_context_state.js";

class Element {
  constructor(tag = "div") { this.tagName = tag; this.children = []; this.dataset = {}; this.attributes = {}; this.listeners = {}; this.value = ""; this.open = false; }
  set textContent(value) { this.text = String(value); this.children = []; }
  get textContent() { return (this.text || "") + this.children.map(child => child.textContent).join(" "); }
  appendChild(child) { this.children.push(child); return child; }
  replaceChildren() { this.children = []; this.text = ""; }
  get childElementCount() { return this.children.length; }
  setAttribute(key, value) { this.attributes[key] = value; }
  addEventListener(name, handler) { this.listeners[name] = handler; }
  click() { this.listeners.click?.({}); }
  focus() { document.activeElement = this; }
  contains(node) { return this === node || this.children.some(child => child.contains(node)); }
  querySelectorAll(selector) { return this.children.flatMap(child => [...(child.tagName === selector ? [child] : []), ...child.querySelectorAll(selector)]); }
  querySelector(selector) { return this.querySelectorAll(selector)[0] || null; }
  showModal() { this.open = true; }
  close() { this.open = false; this.listeners.close?.(); }
}
function withDom(run) {
  const prior = globalThis.document;
  const ids = new Map();
  const listeners = {};
  globalThis.document = { activeElement: null, createElement: tag => new Element(tag), getElementById: id => ids.get(id), addEventListener: (name, callback) => { listeners[name] = callback; } };
  try { run(ids, listeners); } finally { globalThis.document = prior; }
}
const session = { session_id: "s1", title: "Fix editor", lifecycle: "active", updated_at: 100, overview: { attention: {}, validation: { state: "not_run" }, work: {} } };
function options(overrides = {}) {
  return { language: "en", projects: [], project: { id: "p1", client_id: "runner", name: "Editor" }, sessions: [session], sessionsAvailable: true, sessionsStatus: "Loaded Sessions", windows: [], windowAvailability: "available", windowScope: "principal", windowStatus: "", onProject() {}, onSession() {}, onWindow() {}, onWindows() {}, onSearch() {}, ...overrides };
}

test("Session work groups never infer work or completion from Window activity or recent timestamps", () => {
  const idle = { ...session, active_count: 9, client_window_key: "hash", updated_at: Date.now() / 1000 };
  const failed = { ...session, session_id: "failed", overview: { validation: { state: "failed" } } };
  const closed = { ...session, session_id: "closed", lifecycle: "closed" };
  const working = { ...session, session_id: "job", running_jobs: 1 };
  const groups = workspaceSessionGroups([idle, failed, closed, working]);
  assert.deepEqual(groups.working, [working]);
  assert.deepEqual(groups.attention, [failed]);
  assert.deepEqual(groups.completed, [closed]);
  assert.equal(workspaceSessionGroups([idle]).completed.length, 0);
});

test("retained edit and completion evidence excludes exploration, failures, and job handoffs", () => {
  const detail = { activity: [
    { kind: "Explored", paths: ["read-only.ts"], state: "succeeded", finished_at: 1 },
    { kind: "Edited", paths: ["app.ts", "app.ts"], state: "succeeded", finished_at: 2 },
    { kind: "Ran", state: "succeeded", finished_at: 3, job_handoff: true, job_id: "job1" },
    { kind: "Tested", state: "failed", finished_at: 4 },
  ] };
  const evidence = workspaceSessionEvidence(detail);
  assert.deepEqual(evidence.files, ["app.ts"]);
  assert.equal(evidence.jobs.length, 1);
  assert.equal(evidence.completed.length, 2);
  assert.equal(evidence.completed[0].kind, "Edited");
  assert.deepEqual(workspaceSessionEvidence({}), { files: [], jobs: [], completed: [] });
});

test("home navigates explicit Session and Window destinations with safe text and stable buttons", () => withDom(() => {
  const node = new Element();
  const calls = [];
  const untrusted = { ...session, title: '<img src=x onerror="alert(1)">' };
  const config = options({ sessions: [untrusted], windows: [{ client_window_key: "hash", linked_session_count: 0, active_count: 1 }], onSession: row => calls.push(["session", row.session_id]), onWindow: key => calls.push(["window", key]), onWindows: () => calls.push(["windows"]) });
  renderWorkspaceHome(node, config);
  const buttons = node.querySelectorAll("button");
  const work = buttons.find(button => button.dataset.action === "workspace-open-session");
  assert.equal(work.type, "button");
  assert.ok(work.textContent.includes(untrusted.title));
  assert.equal(node.querySelectorAll("img").length, 0);
  work.click();
  buttons.find(button => button.dataset.action === "workspace-open-window").click();
  buttons.find(button => button.dataset.action === "workspace-open-windows").click();
  assert.deepEqual(calls, [["session", "s1"], ["window", "hash"], ["windows"]]);
  assert.equal(Object.keys(node.dataset).length, 0, "no evidence fingerprints in data attributes");
  assert.ok(!node.textContent.includes("host is online"));
  assert.ok(!node.textContent.includes("ChatGPT connected"));
  work.focus();
  renderWorkspaceHome(node, config);
  assert.equal(document.activeElement, work, "unchanged polling preserves the exact control");
  renderWorkspaceHome(node, { ...config, sessionsStatus: "Updated" });
  assert.equal(document.activeElement.textContent, work.textContent, "changed evidence preserves the same destination focus");
}));

test("loading and denied evidence never renders a zero-work assertion; Chinese home and summary are localized", () => withDom(() => {
  const node = new Element();
  renderWorkspaceHome(node, options({ sessions: [], sessionsAvailable: false, sessionsStatus: "Loading work Sessions…", windowAvailability: "unavailable" }));
  assert.ok(node.textContent.includes("Loading work Sessions"));
  assert.ok(!node.textContent.includes("runtime:read"));
  assert.ok(!node.textContent.includes("No activity observed yet"));
  assert.ok(!node.textContent.includes("No attention requests"));
  renderWorkspaceHome(node, options({ language: "zh-CN" }));
  assert.ok(node.textContent.includes("最近活动"));
  assert.ok(node.textContent.includes("窗口"));
  const summary = new Element();
  renderWorkspaceEvidence(summary, { running_jobs: 0, overview: session.overview, activity: [] }, "zh-CN");
  assert.ok(summary.textContent.includes("留存编辑涉及的文件"));
  assert.ok(!summary.textContent.includes("Agent:"));
}));

test("home is a real view, hides Session context and keeps conversation drafts mounted", async () => {
  assert.equal(workspaceViewPreference(null), "home");
  assert.equal(resolveRuntimeContextState({ userIntent: true, isWideViewport: true, isMobileViewport: false, hasSelectedSession: true, workspaceView: "home" }).visible, false);
  const source = await readFile(new URL("../dist/runtime.js", import.meta.url), "utf8");
  const start = source.indexOf("function applyWorkspaceView(");
  const end = source.indexOf("\n}", start) + 2;
  const panel = { hidden: false, draft: "unsent work", children: ["composer"] };
  const shown = new Map();
  const buttons = ["home", "sessions", "windows", "operations"].map(view => ({ dataset: { runtimeView: view }, classList: { toggle() {} }, setAttribute(key, value) { this[key] = value; }, removeAttribute(key) { delete this[key]; } }));
  const context = vm.createContext({ workspaceView: "sessions", token: "", document: { body: { classList: { toggle() {} } }, querySelectorAll: () => buttons }, parseWorkspaceViewPreference: workspaceViewPreference, el: () => ({ dataset: {} }), show(id, visible) { shown.set(id, visible); if (id === "runtime-conversation-stage") panel.hidden = !visible; }, renderHome() {}, stopWindowAuto() {}, renderWorkspaceHeading() {}, syncResponsiveNavigation() {}, setMobileNavigationOpen() {}, persistWorkspaceViewPreference() {} });
  vm.runInContext(source.slice(start, end), context);
  for (const view of ["home", "windows", "operations", "sessions"]) {
    context.applyWorkspaceView(view);
    assert.equal(shown.get("runtime-home-stage"), view === "home");
    assert.equal(shown.get("runtime-windows-stage"), view === "windows");
    assert.equal(panel.hidden, view !== "sessions");
    assert.equal(panel.draft, "unsent work");
    assert.deepEqual(buttons.filter(button => button["aria-current"] === "page").map(button => button.dataset.runtimeView), [view]);
  }
});

test("command search honors locking, IME, Enter navigation and close focus return", () => withDom((ids, listeners) => {
  for (const id of ["runtime-command-dialog", "runtime-command-query", "runtime-command-results", "runtime-open-commands", "runtime-command-close"]) ids.set(id, new Element(id === "runtime-command-query" ? "input" : "div"));
  const trigger = ids.get("runtime-open-commands");
  trigger.focus();
  let available = false;
  const calls = [];
  installWorkspaceCommands({ language: () => "en", available: () => available, projects: () => [{ id: "p1", name: "Editor", client_id: "runner" }], onProject: (...args) => calls.push(args), onView: view => calls.push(view) });
  const dialog = ids.get("runtime-command-dialog");
  trigger.click(); assert.equal(dialog.open, false);
  available = true;
  listeners.keydown({ metaKey: true, shiftKey: true, isComposing: true, key: "k", preventDefault() {} });
  assert.equal(dialog.open, false);
  trigger.click(); assert.equal(dialog.open, true);
  const input = ids.get("runtime-command-query");
  input.value = "editor"; input.listeners.input();
  input.listeners.keydown({ key: "Enter", preventDefault() {} });
  assert.deepEqual(calls, [["runner", "p1"]]);
  assert.equal(dialog.open, false);
  assert.equal(document.activeElement, trigger);
}));

test("daily navigation and settings expose semantic automation hooks and labelled inputs", async () => {
  const html = await readFile(new URL("../src/runtime.html", import.meta.url), "utf8");
  for (const id of ["runtime-view-home", "runtime-view-windows", "runtime-open-commands", "runtime-refresh", "runtime-lock", "runtime-mobile-nav-toggle", "runtime-project-search"]) {
    assert.match(html, new RegExp('(?:data-testid="' + id + '"|id="' + id + '"[^>]*data-action=)'));
  }
  assert.match(html, /<dialog[^>]+aria-label="Commands"/);
  assert.match(html, /<label for="runtime-command-query">/);
  assert.match(html, /data-theme-option="light" aria-pressed=/);
  assert.match(html, /data-context-target="runtime-context-activity" aria-controls=/);
});
