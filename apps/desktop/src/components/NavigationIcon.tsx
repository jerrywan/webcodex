import type { Navigation } from "./Sidebar";
const paths: Record<Navigation, string> = {
  home: "M3 10 12 3l9 7v10a1 1 0 0 1-1 1h-5v-7H9v7H4a1 1 0 0 1-1-1Z",
  projects: "M3 7V5h6l2 3h10v12H3V7Z",
  connection: "m9 15 6-6M7 14l-2 2a3 3 0 0 0 4 4l3-3M12 7l3-3a3 3 0 0 1 4 4l-2 2",
  extensions: "M3 3h7v7H3ZM14 3h7v7h-7ZM3 14h7v7H3ZM14 14h7v7h-7Z",
  activity: "M3 12h4l3-8 4 16 3-8h4",
  settings: "M4 6h16M4 12h16M4 18h16M9 3v6M15 9v6M8 15v6",
};
export function NavigationIcon({ name }: { name: Navigation }) {
  return <svg className="navigation-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d={paths[name]} /></svg>;
}
