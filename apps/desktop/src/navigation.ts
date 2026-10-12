// Navigation shell for the D1 desktop: a left rail with the three primary
// views (Status, Projects, Events) and a connection banner. The projection
// owns the pane content; this only owns the rail and which view is focused.
import type { Projection } from "./projection";

export type ViewName = "status" | "projects" | "events";

const VIEWS: Array<[ViewName, string]> = [
  ["status", "Core status"],
  ["projects", "Projects"],
  ["events", "Events"],
];

/** Rebuilds the left rail + banner from the current projection. */
export function renderNavigation(root: HTMLElement, view: ViewName, proj: Projection): void {
  const stateClass = proj.core === "live" ? "ok" : proj.core === "error" ? "error" : "wait";
  const stateLabel = proj.core === "live" ? "core live" : proj.core === "error" ? "core error" : "connecting…";

  const rail = document.createElement("nav");
  for (const [name, label] of VIEWS) {
    const link = document.createElement("a");
    link.textContent = name === "projects" ? `Projects (${proj.projects.length})` : name === "events" ? `Events (${proj.events.length})` : label;
    link.href = `#${name}`;
    link.className = name === view ? "active" : "";
    link.addEventListener("click", (e) => {
      e.preventDefault();
      onNavigate(name);
    });
    rail.append(link);
  }

  const banner = document.createElement("div");
  banner.className = `banner ${stateClass}`;
  banner.textContent = stateLabel;
  const aside = document.createElement("aside");
  aside.append(rail);

  root.replaceChildren(aside, banner);
}

let onNavigate: (view: ViewName) => void = () => {};

/** main.ts wires this so the rail can drive view switches. */
export function setNavigateHandler(handler: (view: ViewName) => void): void {
  onNavigate = handler;
}
