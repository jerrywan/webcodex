import { NavigationIcon } from "./NavigationIcon";
import brandIcon from "../assets/brand.png";
import type { DesktopState } from "../models/topology";
import { LANGUAGES, useLocale } from "../i18n/locale";
import { runtimeLabel } from "../i18n/presentation";
export type Navigation = "home" | "projects" | "connection" | "extensions" | "activity" | "settings";
export const NAVIGATION: Navigation[] = ["home", "projects", "connection", "extensions", "activity", "settings"];
export function Sidebar({ state, navigation, setNavigation }: { state: DesktopState; navigation: Navigation; setNavigation: (page: Navigation) => void }) {
  const { locale, setLocale, t } = useLocale();
  return (
      <aside className="sidebar">
        <div className="brand"><img className="brand-mark" src={brandIcon} alt="" /><div><strong>WebCodex</strong><span>Desktop</span></div></div>
        <nav aria-label={t("nav.main")}>
          {NAVIGATION.map((item, index) => (
            <button
              key={item}
              className={navigation === item ? "active" : ""}
              onClick={() => setNavigation(item)}
              aria-current={navigation === item ? "page" : undefined}
              aria-keyshortcuts={`Control+${index + 1} Meta+${index + 1}`}
              title={`${t(`nav.${item}`)} (⌘ / Ctrl + ${index + 1})`}
              data-webcodex-action={`navigate-${item}`}
            >
              <NavigationIcon name={item} />
              {t(`nav.${item}`)}
              <kbd aria-hidden="true">{index + 1}</kbd>
            </button>
          ))}
        </nav>
        <div className="sidebar-locale">
          <label htmlFor="desktop-sidebar-locale">{t("locale.label")}</label>
          <select
            id="desktop-sidebar-locale"
            aria-label={t("locale.label")}
            value={locale}
            onChange={(event) => setLocale(event.target.value as typeof locale)}
            data-webcodex-control="locale"
          >
            {LANGUAGES.map((language) => <option key={language.value} value={language.value}>{language.label}</option>)}
          </select>
        </div>
        <div className="sidebar-status">
          <i className={`status-dot ${state.readiness.runtime_ready ? "ready" : "unknown"}`} aria-hidden="true" />
          <div><strong>{runtimeLabel(state, t)}</strong><span>{sidebarConnectionLabel(state, t)}</span></div>
        </div>
      </aside>
  );
}

function sidebarConnectionLabel(state: DesktopState, t: ReturnType<typeof useLocale>["t"]) {
  if (!state.readiness.runtime_ready) return t("workspace.afterStart");
  if (state.regular_tunnel?.status !== "error" && state.readiness.runtime_ready && state.chatgpt_activity?.observed) {
    return t("sidebar.chatgptObserved");
  }
  if (state.readiness.ready_for_chatgpt) return t("sidebar.chatgptReady");
  if (state.regular_tunnel?.status === "ready" && state.regular_tunnel.ready_for_chatgpt) return t("sidebar.tunnelWaiting");
  return t("sidebar.connectionIncomplete");
}
