// Mouse picking: hover highlight, click to select, double click to fly to.
//
// Nodes are spheres, so instead of a triangle-level Raycaster test (a /24 of
// 32x20-segment spheres is ~300k triangles) every node is tested as a bounding
// sphere, which is exact for a sphere and cheap enough to run each frame while
// the pointer is over the canvas. The selection is remembered by host id so it
// survives a graph rebuild when a new map revision arrives.

import * as THREE from "three";
import { cssColor } from "./theme.js";

const CLICK_SLOP_PX = 5;    // a press that moved further than this is a drag, not a click
const PICK_SLACK = 1.2;     // pick radius / sphere radius: small nodes are easier to hit
const RING_SCALE = 1.45;    // selection ring radius / sphere radius

export function flyDistance(radius) {
  return 4 + radius * 6;
}

export class Picker {
  constructor({ view, graph, controls }) {
    this.view = view;
    this.graph = graph;
    this.controls = controls;
    this.camera = view.camera;
    this.canvas = view.renderer.domElement;

    this.hovered = null;         // Mesh under the pointer
    this.selectedId = null;      // host id
    this.pointer = new THREE.Vector2();
    this.pointerInside = false;
    this.raycaster = new THREE.Raycaster();
    this.listeners = new Set();  // (host | null) => void

    this._sphere = new THREE.Sphere();
    this._hit = new THREE.Vector3();
    this._down = null;

    this.ring = makeRing();
    view.scene.add(this.ring);

    this._handlers = [
      ["pointermove", (e) => this.onPointerMove(e)],
      ["pointerleave", () => { this.pointerInside = false; this.setHovered(null); }],
      ["pointerdown", (e) => { this._down = { x: e.clientX, y: e.clientY, button: e.button }; }],
      ["click", (e) => this.onClick(e)],
      ["dblclick", (e) => this.onDoubleClick(e)],
    ];
    for (const [type, fn] of this._handlers) this.canvas.addEventListener(type, fn);
    this._offFrame = view.onFrame(() => this.update());
    this._offUpdate = graph.onUpdate(() => this.refresh());
  }

  onSelect(cb) {
    this.listeners.add(cb);
    return () => this.listeners.delete(cb);
  }

  get selected() {
    return this.selectedId === null ? null : this.graph.nodes.get(this.selectedId) ?? null;
  }

  // ----- events ------------------------------------------------------------------
  onPointerMove(e) {
    const rect = this.canvas.getBoundingClientRect();
    this.pointer.set(
      ((e.clientX - rect.left) / rect.width) * 2 - 1,
      -((e.clientY - rect.top) / rect.height) * 2 + 1,
    );
    this.pointerInside = true;
  }

  isClick(e) {
    const d = this._down;
    return Boolean(d) && d.button === 0 && Math.hypot(e.clientX - d.x, e.clientY - d.y) <= CLICK_SLOP_PX;
  }

  onClick(e) {
    if (!this.isClick(e)) return;
    this.onPointerMove(e);
    this.pick();
    this.select(this.hovered ? this.hovered.name : null);
  }

  onDoubleClick(e) {
    this.onPointerMove(e);
    this.pick();
    if (this.hovered) this.select(this.hovered.name, { fly: true });
  }

  // ----- selection ---------------------------------------------------------------
  select(id, { fly = false } = {}) {
    const previous = this.selected;
    this.selectedId = id ?? null;
    if (previous) this.applyState(previous);
    const mesh = this.selected;
    if (mesh) this.applyState(mesh);
    if (fly && mesh) this.flyTo(mesh);
    const host = mesh ? mesh.userData.host : null;
    for (const cb of this.listeners) cb(host);
    return host;
  }

  flyTo(mesh) {
    this.controls.flyTo(mesh.position, flyDistance(mesh.userData.radius));
  }

  flyToSelected() {
    const mesh = this.selected;
    if (mesh) this.flyTo(mesh);
    return Boolean(mesh);
  }

  // After graph.update() the meshes are new objects: re-apply the states.
  refresh() {
    this.hovered = null;
    this.canvas.style.cursor = "";
    const mesh = this.selected;
    if (mesh) this.applyState(mesh);
    else if (this.selectedId !== null) this.select(null); // the selected host left the map
  }

  // ----- per frame -----------------------------------------------------------------
  update() {
    if (this.pointerInside && !this.controls.looking) this.pick();
    else if (this.hovered) this.setHovered(null);

    const mesh = this.selected;
    this.ring.visible = Boolean(mesh);
    if (mesh) {
      this.ring.position.copy(mesh.position);
      this.ring.quaternion.copy(this.camera.quaternion); // always face the camera
      this.ring.scale.setScalar(mesh.userData.radius * RING_SCALE);
    }
  }

  pick() {
    this.raycaster.setFromCamera(this.pointer, this.camera);
    const ray = this.raycaster.ray;
    let best = null;
    let bestDistance = Infinity;
    for (const mesh of this.graph.nodes.values()) {
      this._sphere.set(mesh.position, mesh.userData.radius * PICK_SLACK);
      if (!ray.intersectSphere(this._sphere, this._hit)) continue;
      const distance = ray.origin.distanceToSquared(this._hit);
      if (distance < bestDistance) {
        bestDistance = distance;
        best = mesh;
      }
    }
    this.setHovered(best);
  }

  setHovered(mesh) {
    if (mesh === this.hovered) return;
    const previous = this.hovered;
    this.hovered = mesh;
    if (previous) this.applyState(previous);
    if (mesh) this.applyState(mesh);
    this.canvas.style.cursor = mesh ? "pointer" : "";
  }

  applyState(mesh) {
    const state = mesh.name === this.selectedId ? "selected" : mesh === this.hovered ? "hover" : "none";
    this.graph.setState(mesh, state);
  }

  dispose() {
    this._offFrame();
    this._offUpdate();
    for (const [type, fn] of this._handlers) this.canvas.removeEventListener(type, fn);
    this.view.scene.remove(this.ring);
    this.ring.geometry.dispose();
    this.ring.material.dispose();
  }
}

function makeRing() {
  const ring = new THREE.Mesh(
    new THREE.RingGeometry(1, 1.1, 48),
    new THREE.MeshBasicMaterial({
      color: new THREE.Color(cssColor("--selection", "#ffffff")),
      side: THREE.DoubleSide,
      transparent: true,
      opacity: 0.9,
      depthTest: false, // visible through other nodes
      fog: false,
    }),
  );
  ring.name = "selection-ring";
  ring.renderOrder = 10;
  ring.visible = false;
  return ring;
}
