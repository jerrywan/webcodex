import { PluginRegistrationForm } from "./PluginRegistrationForm";
import { useEffect, useRef, useState } from "react";
import { desktopApi } from "../../lib/desktop-api";
import type { DesktopState, RunnerSettings } from "../../models/topology";
import { useLocale } from "../../i18n/locale";

export function ExtensionsPanel({ state, onState }: { state: DesktopState; onState: (state: DesktopState) => void }) {
  const { t } = useLocale();
  const alive = useRef(true);
  const [settings, setSettings] = useState<RunnerSettings | null>(null);
  const [instructions, setInstructions] = useState("");
  const [skills, setSkills] = useState("");
  const [busy, setBusy] = useState(false);
  const [failed, setFailed] = useState(false);
  const [saved, setSaved] = useState(false);
  const load = async () => {
    setFailed(false);
    try {
      const next = await desktopApi.runnerSettings();
      if (!alive.current) return;
      setSettings(next); setInstructions(next.paths.instruction_files.join("\n")); setSkills(next.paths.skill_roots.join("\n"));
    } catch { setFailed(true); }
  };
  useEffect(() => { alive.current = true; void load(); return () => { alive.current = false; }; }, []);
  const save = async () => {
    if (!settings || busy || state.current_operation) return;
    setBusy(true); setFailed(false); setSaved(false);
    const lines = (value: string) => value.split(/\r?\n/).map(line => line.trim()).filter(Boolean);
    try {
      onState(await desktopApi.updateRunnerSettings(settings.target, settings.paths, { instruction_files: lines(instructions), skill_roots: lines(skills) }));
      await load(); setSaved(true);
    } catch { setFailed(true); }
    finally { setBusy(false); }
  };
  const restart = async () => {
    setBusy(true); setFailed(false);
    try { onState(await desktopApi.restartOwnedRunner(settings!.target)); setSaved(false); await load(); }
    catch { setFailed(true); }
    finally { setBusy(false); }
  };
  return <section className="page-section" aria-labelledby="extensions-title" data-webcodex-page="extensions">
    <h1 id="extensions-title">{t("extensions.title")}</h1>
    <p className="lede">{t("extensions.description")}</p>
    {failed && <div role="alert" className="error-card"><p>{t("extensions.error")}</p><button className="secondary-button" disabled={busy} onClick={() => void load()}>{t("common.retry")}</button></div>}
    {!settings ? <p role="status">{failed ? t("extensions.error") : t("common.checking")}</p> : <>
      <article className="detail-card extension-editor">
        <h2>{t("extensions.instructions")}</h2>
        <p>{t("extensions.agentsHelp")}</p>
        {state.project && <code className="path-value">{state.project.path.replace(/[\\/]$/, "")}/AGENTS.md</code>}
        <label htmlFor="instruction-files">{t("extensions.instructionFiles")}</label>
        <textarea id="instruction-files" value={instructions} onChange={e => { setInstructions(e.target.value); setSaved(false); }} rows={4} spellCheck={false} disabled={busy} aria-describedby="runner-path-help" />
        <label htmlFor="skill-roots">{t("extensions.skillRoots")}</label>
        <textarea id="skill-roots" value={skills} onChange={e => { setSkills(e.target.value); setSaved(false); }} rows={4} spellCheck={false} disabled={busy} aria-describedby="runner-path-help" />
        <p id="runner-path-help" className="field-help">{t("extensions.pathHelp")}</p>
        <button className="primary-button" data-webcodex-action="save-runner-settings" disabled={busy || Boolean(state.current_operation)} onClick={() => void save()}>{t("extensions.save")}</button>
        {saved && <p role="status">{t("extensions.saved")}</p>}
        <p className="field-help">{t("extensions.restartHelp")}</p>
        <button className="secondary-button" data-webcodex-action="restart-owned-runner" disabled={busy || Boolean(state.current_operation) || !settings.can_restart} onClick={() => void restart()}>{t("extensions.restart")}</button>
      </article>
      <article className="detail-card">
        <h2>{t("extensions.plugins")}</h2>
        <p>{t("extensions.pluginHelp")}</p>
        {settings.plugin_ids.length ? <ul>{settings.plugin_ids.map(id => <li key={id}><code>{id}</code></li>)}</ul> : <p>{t("extensions.noPlugins")}</p>}
        <code className="path-value">{settings.target.config_path}</code>
        <PluginRegistrationForm disabled={busy || Boolean(state.current_operation)} onAdd={async provider => {
          if (busy || state.current_operation) return false;
          setBusy(true); setFailed(false); setSaved(false);
          try {
            onState(await desktopApi.addRunnerPlugin(settings.target, provider));
            await load(); setSaved(true); return true;
          } catch { setFailed(true); return false; }
          finally { setBusy(false); }
        }} />
      </article>
    </>}
  </section>;
}
