// QuotaGlass front end. One page serves two windows: the pill (default) and the flyout (?view=flyout).

import { onBackdrop, onTheme, setZoom } from "./ipc";
import { startFlyout } from "./flyout-app";
import { startPill } from "./pill-app";

/** Windows text size zooms the web view; the pill is sized from this zoom (Rust ignores the flyout's), and follows changes. */
function watchZoom(): void {
  void setZoom(window.devicePixelRatio);
  matchMedia(`(resolution: ${window.devicePixelRatio}dppx)`).addEventListener("change", watchZoom, { once: true });
}

// The pill is not a web page: no Back / Refresh / Save as / Inspect menu on right-click.
document.addEventListener("contextmenu", (event) => event.preventDefault());

void onBackdrop((payload) => document.documentElement.classList.toggle("no-acrylic", !payload.acrylic));
void onTheme((theme) => (document.documentElement.dataset.theme = theme));
watchZoom();

const view = new URLSearchParams(location.search).get("view");
if (view === "flyout") startFlyout();
else startPill();
