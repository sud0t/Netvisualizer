// Turns the /api/map document into meshes: one sphere per host (sized by role,
// coloured by role and status), one LineSegments object for every link, one
// HTML label per host. `update(doc)` rebuilds everything, which is cheap at
// LAN scale (a /24 is a few hundred draw calls); switch nodes to InstancedMesh
// if the backend ever produces thousands of hosts.

import * as THREE from "three";
import { CSS2DObject } from "./vendor/CSS2DRenderer.js";
import { cssColor } from "./theme.js";

export const NODE_RADIUS = { scanner: 1.1, gateway: 0.85, host: 0.6 };
export const STATES = ["none", "hover", "selected"];
const LABEL_GAP = 0.45;         // world units between the top of a sphere and its label
const FALLBACK_RING_GAP = 6;    // hosts without a position go on a ring outside the rest
const HIGHLIGHT_SCALE = 1.12;   // hovered / selected spheres grow by this much

const FALLBACK_COLOR = { scanner: "#22d3ee", gateway: "#f59e0b", host: "#60a5fa", down: "#6b7280" };
const EMISSIVE = { none: 0.28, hover: 0.75, selected: 0.95 };

export function roleOf(host) {
  return Object.hasOwn(NODE_RADIUS, host.role) ? host.role : "host";
}

// The Host model has no status field yet. Until it does, "down" is read from a
// top-level `status` or from `extra.status`, so a backend that starts reporting
// it in either place greys those nodes out with no frontend change.
export function statusOf(host) {
  const status = host.status ?? host.extra?.status;
  return typeof status === "string" && status.toLowerCase() === "down" ? "down" : "up";
}

export class Graph {
  constructor(scene) {
    this.scene = scene;
    this.group = new THREE.Group();
    this.group.name = "network";
    scene.add(this.group);

    this.doc = null;
    this.nodes = new Map();        // host id -> Mesh (mesh.userData.host is the JSON host)
    this.links = null;             // LineSegments or null
    this.labels = [];              // { element, mesh } for distance fading
    this.listeners = new Set();    // called after every update(doc)
    this.bounds = new THREE.Box3();
    this.boundingSphere = new THREE.Sphere();

    // Shared resources: one unit sphere scaled per node, one material per
    // (state, colour key) pair; highlighting is a material swap, not a clone.
    this.geometry = new THREE.SphereGeometry(1, 32, 20);
    this.materials = { none: {}, hover: {}, selected: {} };
    for (const key of [...Object.keys(NODE_RADIUS), "down"]) {
      const color = new THREE.Color(cssColor(`--role-${key}`, FALLBACK_COLOR[key]));
      for (const state of STATES) {
        this.materials[state][key] = new THREE.MeshStandardMaterial({
          color,
          emissive: color,
          emissiveIntensity: EMISSIVE[state],
          roughness: 0.4,
          metalness: 0.15,
        });
      }
    }
    this.linkMaterial = new THREE.LineBasicMaterial({
      color: new THREE.Color(cssColor("--link", "#3d5a85")),
      transparent: true,
      opacity: 0.6,
    });
  }

  onUpdate(cb) {
    this.listeners.add(cb);
    return () => this.listeners.delete(cb);
  }

  // ----- building ------------------------------------------------------------
  update(doc) {
    this.clear();
    this.doc = doc;
    const positions = resolvePositions(doc.hosts ?? []);
    for (const host of doc.hosts ?? []) this.addNode(host, positions.get(host.id));
    this.addLinks(doc.links ?? []);
    this.computeBounds();
    for (const cb of this.listeners) cb(this);
    return this;
  }

  addNode(host, position) {
    const role = roleOf(host);
    const key = statusOf(host) === "down" ? "down" : role;
    const radius = NODE_RADIUS[role];
    const mesh = new THREE.Mesh(this.geometry, this.materials.none[key]);
    mesh.name = host.id;
    mesh.scale.setScalar(radius);
    mesh.position.copy(position);
    mesh.userData = { host, role, key, radius, state: "none" };

    const label = makeLabel(host, role, key);
    // Child of the scaled mesh, so divide the world-unit gap by the scale.
    label.position.set(0, 1 + LABEL_GAP / radius, 0);
    mesh.add(label);
    this.labels.push({ element: label.element, mesh });

    this.group.add(mesh);
    this.nodes.set(host.id, mesh);
    return mesh;
  }

  addLinks(links) {
    const points = [];
    for (const link of links) {
      const a = this.nodes.get(link.source);
      const b = this.nodes.get(link.target);
      if (!a || !b) continue; // a link to an unknown host is ignored, not fatal
      points.push(a.position.x, a.position.y, a.position.z, b.position.x, b.position.y, b.position.z);
    }
    if (points.length === 0) return;
    const geometry = new THREE.BufferGeometry();
    geometry.setAttribute("position", new THREE.Float32BufferAttribute(points, 3));
    this.links = new THREE.LineSegments(geometry, this.linkMaterial);
    this.links.name = "links";
    this.group.add(this.links);
  }

  computeBounds() {
    this.bounds.makeEmpty();
    let maxRadius = 0;
    for (const mesh of this.nodes.values()) {
      this.bounds.expandByPoint(mesh.position);
      maxRadius = Math.max(maxRadius, mesh.userData.radius);
    }
    if (this.bounds.isEmpty()) this.bounds.setFromCenterAndSize(new THREE.Vector3(), new THREE.Vector3());
    this.bounds.expandByScalar(maxRadius);
    this.bounds.max.y += LABEL_GAP + 1; // room for the labels above the top row
    this.bounds.getBoundingSphere(this.boundingSphere);
  }

  // ----- per node / per frame ------------------------------------------------------
  // Hover and selection look: brighter material, slightly bigger sphere, label class.
  setState(mesh, state) {
    const data = mesh.userData;
    if (!data?.key || data.state === state) return;
    data.state = state;
    mesh.material = this.materials[state][data.key];
    mesh.scale.setScalar(data.radius * (state === "none" ? 1 : HIGHLIGHT_SCALE));
    const label = mesh.children.find((child) => child.isCSS2DObject);
    if (label) {
      label.element.classList.toggle("is-hover", state === "hover");
      label.element.classList.toggle("is-selected", state === "selected");
    }
  }

  // Labels fade out between `near` and `far` world units from the camera, so a
  // distant map reads as shapes and colours instead of a wall of text.
  fadeLabels(camera, near, far) {
    for (const { element, mesh } of this.labels) {
      const distance = camera.position.distanceTo(mesh.position);
      element.style.opacity = (1 - THREE.MathUtils.smoothstep(distance, near, far)).toFixed(3);
    }
  }

  // ----- teardown ------------------------------------------------------------
  clear() {
    for (const mesh of this.nodes.values()) {
      // Removing a parent does not fire "removed" on its children, so detach the
      // label DOM elements by hand instead of relying on CSS2DObject's own hook.
      mesh.traverse((obj) => {
        if (obj.isCSS2DObject) obj.element.remove();
      });
    }
    if (this.links) this.links.geometry.dispose();
    this.group.clear();
    this.nodes.clear();
    this.labels = [];
    this.links = null;
    this.doc = null;
  }

  dispose() {
    this.clear();
    this.scene.remove(this.group);
    this.geometry.dispose();
    for (const byKey of Object.values(this.materials)) {
      for (const material of Object.values(byKey)) material.dispose();
    }
    this.linkMaterial.dispose();
  }
}

// ----- helpers ------------------------------------------------------------------

function makeLabel(host, role, key) {
  const element = document.createElement("div");
  element.className = `label role-${role}`;
  if (key === "down") element.classList.add("is-down");
  element.textContent = host.label || host.id;
  element.dataset.id = host.id;
  const label = new CSS2DObject(element);
  label.center.set(0.5, 1); // anchor the bottom-centre of the label at the point
  label.name = `label:${host.id}`;
  return label;
}

// Every host gets a Vector3. Hosts the backend left without a position are
// spread evenly on a ring just outside the positioned ones, so a map built
// with `layout=None` still renders instead of piling everything at the origin.
function resolvePositions(hosts) {
  const out = new Map();
  const missing = [];
  let maxRadius = 0;
  for (const host of hosts) {
    const p = host.position;
    if (Array.isArray(p) && p.length === 3 && p.every(Number.isFinite)) {
      const v = new THREE.Vector3(p[0], p[1], p[2]);
      out.set(host.id, v);
      maxRadius = Math.max(maxRadius, Math.hypot(v.x, v.z));
    } else {
      missing.push(host);
    }
  }
  if (missing.length) {
    const radius = maxRadius + FALLBACK_RING_GAP;
    const step = (2 * Math.PI) / missing.length;
    missing.forEach((host, i) => {
      out.set(host.id, new THREE.Vector3(radius * Math.cos(i * step), 0, radius * Math.sin(i * step)));
    });
  }
  return out;
}
