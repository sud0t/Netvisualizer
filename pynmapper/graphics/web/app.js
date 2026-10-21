// Entry point: build the scene, wire controls / picking / panel together,
// fetch /api/map and keep polling for new revisions.
//
// The page polls the tiny /api/health endpoint every POLL_MS and only
// re-downloads /api/map when its `revision` changes, so the browser can be
// opened before a scan has finished and will redraw when results arrive.

import { createScene } from "./scene.js";
import { Graph } from "./graph.js";
import { FlyControls, isTypingTarget } from "./controls.js";
import { Picker } from "./picking.js";
import { Panel } from "./panel.js";

const POLL_MS = 2000;
const CONTRACT_VERSION = 1;

const viewport = document.getElementById("viewport");
const statusEl = document.getElementById("status");
const speedEl = document.getElementById("speed");
const fatalEl = document.getElementById("fatal");
const helpEl = document.getElementById("help");
const sidebarEl = document.getElementById("sidebar");

let view = null;
let graph = null;
let controls = null;
let picker = null;
let panel = null;
let revision = null;     // revision currently drawn, null until the first map arrives
let framed = false;      // the first map is framed instantly; later maps glide to a new framing
                         // only while the user has not taken the camera themselves
let lastSummary = "";

// ----- HUD -------------------------------------------------------------------
function setStatus(text, state = "ok") {
  statusEl.textContent = text;
  statusEl.dataset.state = state;
}

function showSpeed(speed) {
  speedEl.textContent = `${speed < 10 ? speed.toFixed(1) : Math.round(speed)} u/s`;
}

function showFatal(message) {
  fatalEl.textContent = message;
  fatalEl.hidden = false;
}

function toggleHelp(force) {
  helpEl.hidden = force === undefined ? !helpEl.hidden : !force;
}

function summarise(doc) {
  const scan = doc.scan ?? {};
  const where = [scan.type, scan.network].filter(Boolean).join(" ");
  const stage = scan.stage ? ` · ${scan.stage}` : "";
  return `${where || "map"}${stage} · ${doc.hosts.length} hosts · ${doc.links.length} links · rev ${doc.revision}`;
}

// ----- camera ---------------------------------------------------------------------
// scene.frame() positions the camera instantly (and retunes fog, grid and far
// plane), fitting the map into the part of the canvas the sidebar does not
// cover; for R we take that pose and glide to it instead.
function frameInset() {
  if (sidebarEl.classList.contains("collapsed")) return {};
  return { right: Math.ceil(window.innerWidth - sidebarEl.getBoundingClientRect().left) };
}

function resetView(animate) {
  const camera = view.camera;
  const fromP = camera.position.clone();
  const fromQ = camera.quaternion.clone();
  view.frame(graph.bounds, { inset: frameInset() });
  if (!animate) {
    controls.syncFromCamera();
    return;
  }
  const toP = camera.position.clone();
  const toQ = camera.quaternion.clone();
  camera.position.copy(fromP);
  camera.quaternion.copy(fromQ);
  controls.tweenTo(toP, toQ);
}

// ----- data ---------------------------------------------------------------------------
async function fetchJSON(url) {
  const res = await fetch(url, { cache: "no-store" });
  if (!res.ok) throw new Error(`${url}: HTTP ${res.status}`);
  return res.json();
}

async function loadMap() {
  const doc = await fetchJSON("/api/map");
  if (doc.version > CONTRACT_VERSION) {
    console.warn(`map contract version ${doc.version} is newer than this page (${CONTRACT_VERSION})`);
  }
  graph.update(doc);          // picker re-applies hover/selection via graph.onUpdate
  panel.setHosts(doc.hosts);
  revision = doc.revision;
  if (!framed) {
    resetView(false);
    framed = true;
  } else if (!controls.userMoved) {
    resetView(true); // hosts arrived while the view was still the default: keep them all in shot
  }
  lastSummary = summarise(doc);
  setStatus(lastSummary);
  document.title = `Pynmapper · ${doc.hosts.length} hosts`;
}

async function poll() {
  try {
    const health = await fetchJSON("/api/health");
    if (health.revision !== revision) {
      await loadMap();
    } else if (statusEl.dataset.state !== "ok") {
      setStatus(lastSummary); // server is back after an outage
    }
  } catch (err) {
    console.warn("poll failed:", err);
    setStatus(`server unreachable, retrying… (${err.message})`, "error");
  } finally {
    setTimeout(poll, POLL_MS);
  }
}

// ----- keyboard ---------------------------------------------------------------------------
// Movement keys live in controls.js; everything else is here.
function onKeyDown(e) {
  if (e.ctrlKey || e.metaKey || e.altKey) return;
  const typing = isTypingTarget(e.target);
  if (e.key === "Escape") {
    if (!helpEl.hidden) toggleHelp(false);
    else if (typing) e.target.blur();
    else picker.select(null);
    return;
  }
  if (typing) return;
  switch (e.key) {
    case "?":
    case "h":
    case "H":
      toggleHelp();
      break;
    case "r":
    case "R":
      resetView(true);
      break;
    case "f":
    case "F":
      picker.flyToSelected();
      break;
    case "/":
      e.preventDefault();
      panel.focusFilter();
      break;
    default:
      return;
  }
}

// ----- start ------------------------------------------------------------------------------
function main() {
  try {
    view = createScene(viewport);
  } catch (err) {
    showFatal(`Could not start WebGL.\n${err.message}`);
    throw err;
  }
  graph = new Graph(view.scene);

  controls = new FlyControls(view.camera, view.renderer.domElement);
  controls.onSpeedChange = showSpeed;
  showSpeed(controls.speed);
  view.onFrame((dt) => controls.update(dt)); // registered before the picker so it sees this frame's camera

  picker = new Picker({ view, graph, controls });
  panel = new Panel(sidebarEl, {
    onSelect: (id, opts) => picker.select(id, opts),
    onFly: () => picker.flyToSelected(),
  });
  picker.onSelect((host) => panel.setSelected(host ? host.id : null));

  view.onFrame(() => {
    const fog = view.scene.fog; // null when someone switches fog off in the console
    graph.fadeLabels(view.camera, fog ? fog.near : Infinity, fog ? fog.far : Infinity);
  });

  window.addEventListener("keydown", onKeyDown);
  document.getElementById("help-button").addEventListener("click", () => toggleHelp());
  helpEl.addEventListener("click", (e) => {
    if (e.target === helpEl) toggleHelp(false); // click on the backdrop closes it
  });

  view.start();
  poll();

  // Handy in the devtools console: pynmapper.graph.nodes, pynmapper.reload()
  window.pynmapper = { view, graph, controls, picker, panel, reload: loadMap };
}

main();
