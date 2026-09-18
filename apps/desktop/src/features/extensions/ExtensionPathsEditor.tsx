import { useEffect, useState } from "react";
import type { RunnerSettings } from "../../models/topology";
import { useLocale } from "../../i18n/locale";
import { useProduct } from "../../i18n/product";

export function ExtensionPathsEditor({ settings, disabled, onSave }: {
  settings: RunnerSettings; disabled: boolean; onSave: (paths: RunnerSettings["paths"]) => Promise<boolean>;
}) {
  const { t } = useLocale(); const p = useProduct();
  const [instructions, setInstructions] = useState(settings.paths.instruction_files.join("\n"));
  const [skills, setSkills] = useState(settings.paths.skill_roots.join("\n"));
  useEffect(() => { setInstructions(settings.paths.instruction_files.join("\n")); setSkills(settings.paths.skill_roots.join("\n")); }, [settings]);
  return <form className="extension-editor" onSubmit={event => {
    event.preventDefault(); if (disabled) return;
    const lines = (value: string) => value.split(/\r?\n/).map(line => line.trim()).filter(Boolean);
    void onSave({ instruction_files: lines(instructions), skill_roots: lines(skills) });
  }}>
    <label htmlFor="instruction-files">{t("extensions.instructionFiles")}</label>
    <textarea id="instruction-files" rows={3} value={instructions} onChange={event => setInstructions(event.target.value)} disabled={disabled} spellCheck={false} />
    <label htmlFor="skill-roots">{t("extensions.skillRoots")}</label>
    <textarea id="skill-roots" rows={3} value={skills} onChange={event => setSkills(event.target.value)} disabled={disabled} spellCheck={false} />
    <button className="primary-button" type="submit" data-webcodex-action="save-runner-settings" disabled={disabled}>{p("save")}</button>
  </form>;
}
