# Isolated WebUI and Desktop renderer smoke

This harness renders the real production bundles against deterministic local fixtures. It never starts a WebCodex Server, Runner, Tunnel, Plugin, or native permission request. Its injected credentials and Tauri responses exist only in this loopback test server, never in product code.

From the repository root:

```sh
npm --prefix frontend ci
npm --prefix frontend run build
npm --prefix apps/desktop ci
npm --prefix apps/desktop run build
npm --prefix scripts/ui-smoke ci
# Needed where no installed macOS Google Chrome is available:
cd scripts/ui-smoke && npx playwright install chromium && cd ../..
npm --prefix scripts/ui-smoke test
```

The script checks 1440, 1024, and 390-pixel viewports, all Desktop navigation destinations, active/stopped Tunnel editing, write-only key clearing, exact project selection/addition, exact-target instruction/Skill saves, native Plugin registration, keyboard navigation, foreground permission explanation and dismissal, explicit Workflow Session vs Window activity, preserved drafts, command-dialog focus return, dark WebUI, and horizontal overflow. Project titles have a separate visibility assertion so status badges cannot consume their width.

Screenshots and `report.json` are regenerated in `artifacts/issue470-ui/` (ignored by Git). The report explicitly says `fixture: true` and `nativeBackend: false`. Browser errors fail the run; the browser and server close in `finally`.

For interactive Browser Use, run `npm --prefix scripts/ui-smoke run serve` and use the printed loopback URL. Routes are `/runtime/`, `/desktop/`, `/desktop/?state=disconnected`, and `/desktop/?permissions`. The server expires after 30 minutes. Stop only its exact owned process when finished.

This is not a substitute for native permission, installer, real Tunnel, or Windows platform testing. Native Rust tests independently cover configuration preservation, stale-target rejection, process ownership, project inventory, and Server/Runner PID preservation on failed Tunnel replacement.
