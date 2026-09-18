import { translate } from "./runtime_i18n.js";
import type { ProductServices } from "./runtime_product.js";
import { productNode, productButton, productDialog, productName } from "./runtime_product_view.js";

type ExtensionTab = "instructions" | "skills" | "plugins";
const PRODUCT_EXTENSION_TABS: ExtensionTab[] = ["instructions", "skills", "plugins"];
export class ProductExtensions {
  private project = "";
  private tab: ExtensionTab = "instructions";
  private generation = 0;
  private request: AbortController | null = null;
  private catalog: any = null;
  private failed = false;
  private loading = false;
  private language = "";
  private body: HTMLElement | null = null;
  private dialogs = new Set<{ close: () => void }>();
  constructor(private readonly services: ProductServices) {}
  reset(): void {
    this.generation++; this.request?.abort(); this.request = null;
    for (const dialog of this.dialogs) dialog.close(); this.dialogs.clear();
    this.project = ""; this.catalog = null; this.failed = false; this.loading = false; this.body = null; this.language = "";
    document.getElementById("runtime-extensions-content")?.replaceChildren();
  }
  open(): void {
    const context = this.services.context();
    const project = context.projects.some(row => row.id === this.project) ? this.project : context.selectedProject || context.projects[0]?.id || "";
    if (this.project !== project || this.language !== context.language || !this.body?.isConnected) {
      this.request?.abort(); this.generation++; this.project = project; this.catalog = null; this.language = context.language;
      this.renderShell(); void this.refresh();
    }
  }
  async refresh(): Promise<void> {
    this.request?.abort(); const request = new AbortController(); this.request = request;
    const generation = ++this.generation; const project = this.project;
    this.loading = Boolean(project); this.failed = false; this.catalog = null; this.render();
    if (!project) return;
    const result = await this.services.post("extensions", { project }, request.signal);
    if (generation !== this.generation || request.signal.aborted) return;
    this.loading = false;
    if (result?.status === 401) { this.services.unauthorized(); return; }
    this.failed = !result?.ok; this.catalog = result?.ok ? result.data : null; this.render();
  }
  private renderShell(): void {
    const root = document.getElementById("runtime-extensions-content"); if (!root) return;
    const context = this.services.context(); const tr = (text: string) => translate(text, context.language);
    root.replaceChildren(); const heading = productNode("header", "", "product-page-heading"); heading.appendChild(productNode("h2", tr("Extensions"))); heading.appendChild(productButton(tr("Refresh"), () => { void this.refresh(); })); root.appendChild(heading);
    const filter = productNode("div", "", "product-filters"); const label = productNode("label", tr("Project")); label.htmlFor = "product-extensions-project";
    const select = productNode("select"); select.id = "product-extensions-project";
    for (const project of context.projects) select.appendChild(new Option(productName(project), project.id)); select.value = this.project;
    select.addEventListener("change", () => { this.project = select.value; void this.refresh(); }); filter.append(label, select); root.appendChild(filter);
    const tabs = productNode("div", "", "product-tabs"); tabs.setAttribute("role", "tablist"); tabs.setAttribute("aria-label", tr("Extensions"));
    const selectTab = (tab: ExtensionTab) => {
      this.tab = tab;
      for (const control of Array.from(tabs.querySelectorAll<HTMLButtonElement>("button"))) { const selected = control.dataset.tab === tab; control.setAttribute("aria-selected", String(selected)); control.tabIndex = selected ? 0 : -1; }
      this.render();
    };
    for (const tab of PRODUCT_EXTENSION_TABS) {
      const control = productButton(tr(tab === "instructions" ? "Instructions" : tab === "skills" ? "Skills" : "Plugins"), () => selectTab(tab), "product-tab");
      control.id = "product-tab-" + tab; control.dataset.tab = tab; control.setAttribute("role", "tab"); control.setAttribute("aria-controls", "product-extensions-panel"); control.setAttribute("aria-selected", String(this.tab === tab)); control.tabIndex = this.tab === tab ? 0 : -1;
      control.addEventListener("keydown", event => { if (!['ArrowLeft', 'ArrowRight'].includes(event.key)) return; event.preventDefault(); const next = PRODUCT_EXTENSION_TABS[(PRODUCT_EXTENSION_TABS.indexOf(tab) + (event.key === 'ArrowRight' ? 1 : 2)) % 3]; selectTab(next); document.getElementById("product-tab-" + next)?.focus(); }); tabs.appendChild(control);
    }
    root.appendChild(tabs); this.body = productNode("section"); this.body.id = "product-extensions-panel"; this.body.setAttribute("role", "tabpanel"); root.appendChild(this.body);
  }
  private render(): void {
    if (!this.body) return;
    const context = this.services.context(); const tr = (text: string) => translate(text, context.language);
    this.body.replaceChildren(); this.body.setAttribute("aria-labelledby", "product-tab-" + this.tab);
    if (this.loading) { const status = productNode("p", tr("Refreshing…"), "muted"); status.setAttribute("role", "status"); this.body.appendChild(status); return; }
    if (this.failed) { const error = productNode("p", tr("Could not refresh. Check the connection and try again."), "error"); error.setAttribute("role", "alert"); this.body.appendChild(error); return; }
    if (!this.project) { this.body.appendChild(productNode("p", tr("Add a project to manage its extensions."), "product-empty")); return; }
    if (!this.catalog) return;
    if (this.tab === "instructions") {
      const files = Array.isArray(this.catalog.instructions?.files) ? this.catalog.instructions.files : [];
      for (const file of files) {
        const title = file.source_scope === "runner" ? tr("Global instructions") : "Project " + String(file.path).split(/[\\/]/).pop();
        const row = this.row(title, (file.source_scope === "runner" ? "Runner" : productName(context.projects.find(project => project.id === this.project) || {})) + " · " + tr("Available"));
        const details = productNode("details"); details.append(productNode("summary", tr("Details")), productNode("code", String(file.path))); row.firstElementChild?.appendChild(details);
        const open = productButton(tr("Open"), () => { void this.openInstruction(file); }); open.setAttribute("aria-label", tr("Open") + " " + title); row.appendChild(open); this.body.appendChild(row);
      }
      if (!files.length) this.body.appendChild(productNode("p", tr(this.catalog.instructions?.scan_complete ? "No instructions configured" : "Unavailable"), "product-empty"));
      if (this.catalog.instructions?.truncated) this.body.appendChild(productNode("p", tr("Showing recent results"), "muted small"));
    } else if (this.tab === "skills") {
      const result = this.catalog.skills;
      const skills = Array.isArray(result?.catalog?.skills) ? result.catalog.skills : [];
      for (const skill of skills) {
        const row = this.row(String(skill.name), tr("Available") + " · " + (skill.source_scope === "project" ? tr("Project") : "Runner"));
        if (skill.description) row.firstElementChild?.appendChild(productNode("p", String(skill.description), "muted")); this.body.appendChild(row);
      }
      if (!skills.length) this.body.appendChild(productNode("p", tr(result?.available ? "Nothing installed yet" : "Unavailable"), "product-empty"));
      if (result?.catalog?.truncated) this.body.appendChild(productNode("p", tr("Showing recent results"), "muted small"));
    } else {
      const result = this.catalog.plugins;
      const plugins = result?.catalog?.plugins || result?.catalog?.providers || [];
      for (const plugin of Array.isArray(plugins) ? plugins : []) {
        const id = String(plugin.id || plugin.plugin || "");
        const status = plugin.status === "error" ? "Unavailable" : plugin.status === "ready" || plugin.status === "available" ? "Available" : "Registered";
        const row = this.row(String(plugin.name || id), tr(status) + " · " + (plugin.tool_count ?? plugin.tools?.length ?? "—") + " " + tr("tools"));
        if (id && this.catalog.can_reload_plugins) row.appendChild(productButton(tr("Reload"), () => { void this.reloadPlugin(id, row); })); this.body.appendChild(row);
      }
      if (!plugins.length) this.body.appendChild(productNode("p", tr(result?.available ? "Nothing installed yet" : "Unavailable"), "product-empty"));
    }
  }
  private row(title: string, subtitle: string): HTMLElement {
    const row = productNode("article", "", "product-extension-row"); const body = productNode("div"); body.append(productNode("h3", title), productNode("span", subtitle, "muted small")); row.appendChild(body); return row;
  }
  private async openInstruction(file: any): Promise<void> {
    const project = this.project; const generation = this.generation; const language = this.services.context().language;
    const popup = productDialog(file.source_scope === "runner" ? translate("Global instructions", language) : String(file.path), language); this.dialogs.add(popup);
    popup.body.appendChild(productNode("p", translate("Refreshing…", language)));
    const result = await this.services.post("instruction", { project, source_scope: file.source_scope, path: file.path, fingerprint: file.fingerprint }, this.request?.signal);
    if (generation !== this.generation || !popup.dialog.isConnected) { popup.close(); this.dialogs.delete(popup); return; }
    if (result?.status === 401) { this.services.unauthorized(); return; }
    popup.body.replaceChildren();
    if (!result?.ok) popup.body.appendChild(productNode("p", translate("Could not refresh. Check the connection and try again.", language), "error"));
    else { popup.body.appendChild(productNode("pre", String(result.data?.content || ""), "product-instructions")); if (result.data?.truncated) popup.body.appendChild(productNode("p", translate("Showing recent results", language))); }
  }
  private async reloadPlugin(plugin: string, row: HTMLElement): Promise<void> {
    const project = this.project; const generation = this.generation; const language = this.services.context().language;
    const control = row.querySelector("button"); if (!control || control.disabled) return; control.disabled = true;
    const result = await this.services.post("plugin-reload", { project, plugin }, this.request?.signal);
    if (generation !== this.generation || !row.isConnected) return;
    if (result?.status === 401) { this.services.unauthorized(); return; }
    if (result?.ok) { await this.refresh(); return; }
    const error = productNode("p", translate("Reload could not be confirmed. Refresh before trying again.", language), "error"); error.setAttribute("role", "alert"); row.appendChild(error); control.disabled = false;
  }
}
