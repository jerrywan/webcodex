import test from "node:test";
import assert from "node:assert/strict";
import {
  pendingAttentionCount,
  runnerAttentionCount,
  attentionLabel,
  formatProjectIdentity,
  extractProjectSelectorDevices,
  formatRuntimeOverviewMetrics,
} from "../dist/runtime_overview.js";

test("pendingAttentionCount and runnerAttentionCount calculate totals correctly", () => {
  assert.equal(pendingAttentionCount(null), 0);
  assert.equal(
    pendingAttentionCount({
      open_risks: 2,
      open_todos: 3,
      open_questions: 1,
      open_guidance: 0,
    }),
    6
  );
  assert.equal(
    pendingAttentionCount({
      open_risks: -5,
      open_todos: "invalid",
      open_questions: 4.8,
    }),
    4
  );

  const runner = {
    sessions: {
      attention: {
        open_todos: 5,
      },
    },
  };
  assert.equal(runnerAttentionCount(runner), 5);
  assert.equal(runnerAttentionCount(null), 0);
});

test("attentionLabel formats individual attention categories or returns fallback", () => {
  assert.equal(attentionLabel(null, "en"), "No retained pending attention");
  assert.equal(attentionLabel({}, "zh-CN"), "没有保留的待处理项");

  const attention = {
    open_risks: 1,
    open_todos: 2,
  };
  assert.equal(attentionLabel(attention, "en"), "1 risk · 2 todos");
  assert.equal(attentionLabel(attention, "zh-CN"), "1 个风险 · 2 个待办");
});

test("formatProjectIdentity formats English and localized identity strings", () => {
  const project = {
    id: "webcodex",
    client_id: "macbook-air",
    path: "/Users/dev/webcodex",
  };
  assert.equal(
    formatProjectIdentity(project, "en"),
    "Runner: macbook-air · Project: webcodex · Workspace: /Users/dev/webcodex"
  );
  assert.equal(
    formatProjectIdentity(project, "zh-CN"),
    "运行器：macbook-air · 项目：webcodex · 工作空间：/Users/dev/webcodex"
  );

  assert.equal(formatProjectIdentity(null, "zh-CN"), "尚未选择项目");
  assert.equal(
    formatProjectIdentity({ id: "p1" }, "zh-CN"),
    "运行器：未知 · 项目：p1 · 工作空间：不可用"
  );
});

test("extractProjectSelectorDevices consolidates and deduplicates devices", () => {
  const projects = [
    { client_id: "runner-a" },
    { client_id: "runner-b" },
  ];
  const known = ["runner-c"];
  const runners = [{ client_id: "runner-b" }, { client_id: "runner-d" }];
  const selected = "runner-e";

  const result = extractProjectSelectorDevices(projects, known, runners, selected);
  assert.deepEqual(result, ["runner-a", "runner-b", "runner-c", "runner-d", "runner-e"]);
});

test("formatRuntimeOverviewMetrics formats overview metric views", () => {
  assert.equal(formatRuntimeOverviewMetrics(null), null);

  const data = {
    service: "webcodex",
    version: "0.4.1",
    build_git_commit: "abc1234",
    build_git_dirty: true,
    runner_count: 2,
    runners_online: 1,
    runners_stale: 1,
    runners_unavailable: 0,
    projects_available: true,
    visible_projects: 4,
    projects_truncated: true,
    active_jobs: 1,
    mixed_builds_present: true,
    workflow_sessions: {
      active: 2,
      running: 1,
      truncated: false,
      open_todos: 1,
    },
    recent_sessions: {
      returned: 5,
      truncated: true,
      scan_truncated: false,
    },
  };

  const metricsEn = formatRuntimeOverviewMetrics(data, "en");
  assert.ok(metricsEn);
  assert.equal(metricsEn.identity, "webcodex · 0.4.1");
  assert.equal(metricsEn.build, "build abc1234 · dirty");
  assert.equal(metricsEn.runners, "2 Runners");
  assert.equal(metricsEn.alignment, "1 online · 1 stale · 0 unavailable");
  assert.equal(metricsEn.projects, "4 visible Projects · partial");
  assert.equal(metricsEn.jobs, "1 active Job · mixed builds");
  assert.equal(metricsEn.sessions, "2 active Sessions · 1 running Session");
  assert.equal(metricsEn.recentStatus, "5 Sessions · top 5");

  const metricsZh = formatRuntimeOverviewMetrics(data, "zh-CN");
  assert.ok(metricsZh);
  assert.equal(metricsZh.build, "构建 abc1234 · 有未提交更改");
  assert.equal(metricsZh.projects, "4 个可见项目 · 不完整");
  assert.equal(metricsZh.jobs, "1 个活跃任务 · 存在混合构建");
  assert.equal(metricsZh.recentStatus, "5 个会话 · 前 5");
});
