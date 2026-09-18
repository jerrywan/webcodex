import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { startFixtureServer } from './server.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const output = path.join(root, 'artifacts/issue470-ui');
fs.mkdirSync(output, { recursive: true });
const fixture = await startFixtureServer();
const chrome = '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome';
const browser = await chromium.launch({ headless: true, ...(fs.existsSync(chrome) ? { executablePath: chrome } : {}) });
const report = { fixture: true, nativeBackend: false, checks: [], screenshots: [], errors: [] };
let page;
async function screenshot(name) {
  const filename = name + '.png';
  await page.screenshot({ path: path.join(output, filename), fullPage: false });
  report.screenshots.push(filename);
}
async function noOverflow(label) {
  const sizes = await page.evaluate(() => ({ width: innerWidth, body: document.body.scrollWidth, root: document.documentElement.scrollWidth }));
  assert(sizes.body <= sizes.width + 1 && sizes.root <= sizes.width + 1, `${label}: horizontal overflow ${JSON.stringify(sizes)}`);
  report.checks.push(label);
}
async function desktopNav(name) {
  await page.locator(`[data-webcodex-action="navigate-${name}"]`).click();
  await page.locator(`[data-webcodex-page="${name}"]`).waitFor();
}
async function prepare(route, width = 1440, height = 900) {
  const context = await browser.newContext({ viewport: { width, height }, colorScheme: 'light' });
  page = await context.newPage(); page.setDefaultTimeout(10000);
  page.on('pageerror', error => report.errors.push(error.message));
  page.on('console', message => { if (message.type() === 'error') report.errors.push(message.text()); });
  await page.goto(fixture.url + route);
  return context;
}
try {
  for (const width of [1440, 1024, 390]) {
    const context = await prepare('/desktop/', width, width === 390 ? 844 : 900);
    await page.locator('[data-webcodex-page="home"]').waitFor();
    await noOverflow(`Desktop home ${width}`);
    await screenshot(`desktop-home-${width}`);
    for (const name of ['projects', 'connection', 'extensions', 'activity', 'settings']) {
      await desktopNav(name); await noOverflow(`Desktop ${name} ${width}`);
    }
    await desktopNav('connection');
    const id = page.locator('[data-webcodex-control="tunnel-id"]');
    const key = page.locator('[data-webcodex-control="tunnel-api-key"]');
    assert(await id.isEditable()); assert(await key.isEditable());
    await id.fill('tunnel_ui_fixture'); await key.fill('fixture-only-api-key');
    await page.locator('[data-webcodex-action="save-tunnel-config"]').click();
    await page.waitForFunction(() => window.__fixtureCalls.some(call => call.cmd === 'update_tunnel_config'));
    assert.equal(await key.inputValue(), '');
    assert.equal(await id.inputValue(), 'tunnel_ui_fixture');
    report.checks.push(`Running Tunnel edit/save ${width}`);
    await screenshot(`desktop-connection-${width}`);
    if (width === 1440) {
      await desktopNav('projects');
      await page.locator('.project-list li').filter({ hasText: '/fixture/beta' }).getByRole('button').click();
      await page.waitForFunction(() => window.__fixtureCalls.some(call => call.cmd === 'activate_local_project' && call.args.request.projectPath === '/fixture/beta'));
      await page.locator('[data-webcodex-action="change-project"]').click();
      await page.locator('.project-list li').filter({ hasText: '/fixture/gamma' }).waitFor();
      assert.equal(await page.locator('.project-list li').count(), 3);
      report.checks.push('One Runner: select and add exact project roots');
      await desktopNav('extensions');
      await page.getByLabel('Global instruction files').fill('/fixture/AGENTS.md');
      await page.getByLabel('Configured Skill roots').fill('/fixture/skills-next');
      await page.locator('[data-webcodex-action="save-runner-settings"]').click();
      await page.waitForFunction(() => window.__fixtureCalls.some(call => call.cmd === 'update_runner_settings'));
      const request = await page.evaluate(() => window.__fixtureCalls.find(call => call.cmd === 'update_runner_settings').args.request);
      assert.equal(request.target.client_id, 'fixture-runner');
      assert.deepEqual(request.paths, { instruction_files: ['/fixture/AGENTS.md'], skill_roots: ['/fixture/skills-next'] });
      await page.getByText('Add a native Tool Plugin', { exact: true }).click();
      await page.getByLabel('Plugin ID', { exact: true }).fill('ui-fixture-plugin');
      await page.getByLabel('Display name', { exact: true }).fill('UI fixture plugin');
      await page.getByLabel('Executable', { exact: true }).fill('node');
      await page.getByLabel('Arguments (JSON array)', { exact: true }).fill('["/fixture/plugin.js"]');
      await page.locator('[data-webcodex-action="add-plugin-registration"]').click();
      await page.locator('li code').filter({ hasText: 'ui-fixture-plugin' }).waitFor();
      report.checks.push('Exact Runner instruction/Skill save and native Plugin registration');
      await screenshot('desktop-extensions');
      await page.keyboard.press('Control+5');
      await page.locator('[data-webcodex-page="activity"]').waitFor();
      report.checks.push('Desktop keyboard navigation');
    }
    await context.close();
  }
  {
    const context = await prepare('/desktop/?state=disconnected');
    await desktopNav('connection');
    assert(await page.getByLabel('Tunnel ID', { exact: true }).isEditable());
    assert(await page.getByLabel('Tunnel API key', { exact: true }).isEditable());
    report.checks.push('Disconnected Tunnel credentials stay editable');
    await context.close();
  }
  {
    const context = await prepare('/desktop/?permissions');
    await page.getByRole('dialog').waitFor();
    await screenshot('desktop-permissions');
    const before = await page.evaluate(() => window.__fixtureCalls.filter(call => call.cmd === 'request_computer_permission').length);
    assert.equal(before, 0);
    await page.keyboard.press('Escape');
    await page.getByRole('dialog').waitFor({ state: 'hidden' });
    report.checks.push('Foreground permission explanation: native dialog and Escape, no automatic permission request');
    await context.close();
  }
  for (const width of [1440, 1024, 390]) {
    const context = await prepare('/runtime/', width, width === 390 ? 844 : 900);
    await page.locator('#runtime-home-stage').waitFor();
    await page.locator('[data-action="workspace-open-project"]').first().waitFor();
    await noOverflow(`WebUI home ${width}`);
    await screenshot(`web-home-${width}`);
    await page.locator('[data-action="workspace-open-project"]').first().click();
    await page.locator('[data-action="workspace-open-session"]').first().waitFor();
    await noOverflow(`WebUI project ${width}`);
    if (width > 900) {
      const titleWidth = await page.locator('.project-row.selected .project-row-title').evaluate(el => el.getBoundingClientRect().width);
      assert(titleWidth > 90, 'Project name must not be squeezed out by Window/work badges');
      report.checks.push(`Readable Project name beside activity indicators ${width}`);
    }
    await screenshot(`web-project-${width}`);
    await page.locator('[data-action="workspace-open-session"]').first().click();
    await page.locator('#runtime-session-detail').waitFor();
    const composer = page.locator('#runtime-message-body');
    await composer.fill('Fixture unsent draft');
    await noOverflow(`WebUI Session ${width}`);
    await screenshot(`web-session-${width}`);
    if (width === 1440) {
      const selected = await page.locator('#runtime-session-id').textContent();
      await page.locator('#runtime-view-windows').click();
      await page.locator('#runtime-window-list button').first().click();
      await page.getByText('read_files', { exact: false }).first().waitFor();
      const windowCalls = fixture.requests.filter(request => request.route === 'window');
      assert(windowCalls.length > 0);
      assert.equal(windowCalls.at(-1).payload.client_window_key, '4700'.repeat(16));
      await screenshot('web-window-activity');
      await page.locator('#runtime-view-sessions').click();
      assert.equal(await page.locator('#runtime-session-id').textContent(), selected);
      assert.equal(await composer.inputValue(), 'Fixture unsent draft');
      await page.locator('#runtime-open-commands').click();
      await page.getByRole('dialog').waitFor();
      await page.keyboard.press('Escape');
      assert.equal(await page.locator('#runtime-open-commands').evaluate(el => document.activeElement === el), true);
      report.checks.push('Window observation remains separate from explicit Workflow Session; draft and command focus preserved');
      await page.evaluate(() => localStorage.setItem('webcodex.runtime.appearance.v1', 'dark'));
      await page.reload(); await page.locator('#runtime-console').waitFor();
      await screenshot('web-dark');
    } else if (width === 390) {
      await page.locator('#runtime-mobile-nav-toggle').click();
      await page.locator('#runtime-view-windows').click();
      await page.locator('#runtime-window-list button').first().waitFor();
      await noOverflow('WebUI mobile Window navigation');
    }
    await context.close();
  }
  assert.deepEqual(report.errors, [], 'Browser errors');
  report.passed = true;
} catch (error) {
  report.passed = false; report.failure = error.stack;
  if (page && !page.isClosed()) {
    await screenshot('failure').catch(() => {});
    fs.writeFileSync(path.join(output, 'failure-text.txt'), (await page.locator('body').innerText()).slice(0, 24000));
  }
  process.exitCode = 1;
} finally {
  fs.writeFileSync(path.join(output, 'report.json'), JSON.stringify(report, null, 2));
  console.log(JSON.stringify(report, null, 2));
  await browser.close();
  await new Promise(resolve => fixture.server.close(resolve));
}
