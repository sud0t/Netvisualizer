// Renderer, camera, lights, fog, the grid floor and the render loop.
// Nothing here knows about hosts; graph.js adds the network into `scene`.

import * as THREE from "three";
import { CSS2DRenderer } from "./vendor/CSS2DRenderer.js";
import { cssColor } from "./theme.js";

const MAX_PIXEL_RATIO = 2;
const DEFAULT_VIEW_DIR = new THREE.Vector3(0, 0.55, 1).normalize(); // elevated, looking in from +z
const MIN_FRAME_SIZE = 12;   // world units; keeps a one-host map from filling the screen
const FRAME_MARGIN = 1.08;   // a little air around the fitted box
const GRID_CELL = 5;         // world units per grid cell (major lines every 5 cells)
const FLOOR_Y = -3;          // the grid floats a little below the lowest ring
const FLOOR_SIZE = 20000;    // big enough to never fly past its edge

export function createScene(container) {
  const background = new THREE.Color(cssColor("--scene-bg", "#0b0f17"));

  // Throws if WebGL is unavailable; app.js turns that into a visible error.
  const renderer = new THREE.WebGLRenderer({ antialias: true, powerPreference: "high-performance" });
  renderer.setPixelRatio(Math.min(window.devicePixelRatio, MAX_PIXEL_RATIO));
  renderer.domElement.id = "gl";
  container.appendChild(renderer.domElement);

  // Labels are real DOM elements drawn in a transparent layer over the canvas.
  const labelRenderer = new CSS2DRenderer();
  labelRenderer.domElement.id = "labels";
  container.appendChild(labelRenderer.domElement);

  const scene = new THREE.Scene();
  scene.background = background;
  scene.fog = new THREE.Fog(background, 60, 160);

  const camera = new THREE.PerspectiveCamera(60, 1, 0.1, 2000);
  camera.position.set(0, 20, 38);
  camera.lookAt(0, 0, 0);

  scene.add(new THREE.HemisphereLight(0xbfd4ff, 0x141a26, 1.4));
  const sun = new THREE.DirectionalLight(0xffffff, 1.8);
  sun.position.set(14, 30, 18);
  scene.add(sun);

  // The floor is a plane that draws its own grid in the fragment shader rather
  // than a GridHelper made of GL lines: Chrome drops any line segment that
  // crosses the near plane, so with a free-flying camera half the grid vanished
  // whenever the camera was over it. The shader version is also anti-aliased at
  // every distance, fades out like the fog, and has no edge to fly past.
  const floor = makeFloor();
  scene.add(floor);

  function resize() {
    const width = container.clientWidth || window.innerWidth;
    const height = container.clientHeight || window.innerHeight;
    camera.aspect = width / height;
    camera.updateProjectionMatrix();
    renderer.setSize(width, height);
    labelRenderer.setSize(width, height);
  }
  window.addEventListener("resize", resize);
  resize();

  // Move the camera back along `direction` until every corner of `box` (a
  // THREE.Box3) is inside the view frustum. Fitting the box rather than its
  // bounding sphere matters because a LAN laid out on rings is a flat disc:
  // seen from above it needs far less distance than a sphere would.
  // `inset` (pixels per edge) excludes parts of the canvas hidden under HTML,
  // such as the sidebar, so the map is fitted and centred in what is visible.
  // Fog starts just past the far side of the map so the map itself stays
  // crisp and only the grid fades into the background.
  function frame(box, { direction = DEFAULT_VIEW_DIR, inset = {} } = {}) {
    const center = new THREE.Vector3();
    const size = new THREE.Vector3();
    if (!box.isEmpty()) {
      box.getCenter(center);
      box.getSize(size);
    }
    size.x = Math.max(size.x, MIN_FRAME_SIZE);
    size.y = Math.max(size.y, MIN_FRAME_SIZE / 2);
    size.z = Math.max(size.z, MIN_FRAME_SIZE);

    const width = renderer.domElement.clientWidth || 1;
    const height = renderer.domElement.clientHeight || 1;
    const pad = { left: 0, right: 0, top: 0, bottom: 0, ...inset };
    const usableW = Math.max(width - pad.left - pad.right, width * 0.3);
    const usableH = Math.max(height - pad.top - pad.bottom, height * 0.3);

    // Half-angles of the frustum slice that covers the usable region.
    const tanV = Math.tan(THREE.MathUtils.degToRad(camera.fov) / 2);
    const tanUsableV = (tanV * usableH) / height;
    const tanUsableH = (tanV * usableW) / height;

    // Camera basis for a camera sitting on `direction` and looking at `center`.
    const forward = direction.clone().negate();
    const right = new THREE.Vector3().crossVectors(forward, camera.up);
    if (right.lengthSq() < 1e-9) right.set(1, 0, 0); // looking straight down
    right.normalize();
    const up = new THREE.Vector3().crossVectors(right, forward).normalize();

    let distance = 0;
    const corner = new THREE.Vector3();
    for (let i = 0; i < 8; i++) {
      corner.set(
        (i & 1 ? 0.5 : -0.5) * size.x,
        (i & 2 ? 0.5 : -0.5) * size.y,
        (i & 4 ? 0.5 : -0.5) * size.z,
      );
      const towardCamera = corner.dot(direction);
      const needH = towardCamera + Math.abs(corner.dot(right)) / tanUsableH;
      const needV = towardCamera + Math.abs(corner.dot(up)) / tanUsableV;
      distance = Math.max(distance, needH, needV);
    }
    distance *= FRAME_MARGIN;

    // Slide sideways so the box centre lands in the middle of the usable
    // region rather than the middle of the canvas.
    const worldPerPixel = (2 * distance * tanV) / height;
    const dx = (pad.left - pad.right) / 2;   // px; + = usable centre is right of the canvas centre
    const dy = (pad.bottom - pad.top) / 2;   // px; + = usable centre is above the canvas centre
    const target = center.clone().addScaledVector(right, -dx * worldPerPixel).addScaledVector(up, -dy * worldPerPixel);

    camera.position.copy(target).addScaledVector(direction, distance);
    camera.lookAt(target);

    const radius = size.length() / 2;
    scene.fog.near = distance + radius;
    scene.fog.far = scene.fog.near + Math.max(3 * radius, 40);
    camera.far = Math.max(2000, scene.fog.far * 2);
    camera.updateProjectionMatrix();
    floor.material.uniforms.uFade.value.set(scene.fog.near, scene.fog.far);
  }

  // ----- render loop ------------------------------------------------------
  const frameCallbacks = new Set();
  let rafId = 0;
  let last = 0;

  function tick(now) {
    rafId = requestAnimationFrame(tick);
    const dt = Math.min((now - last) / 1000, 0.1); // clamp after a hidden tab
    last = now;
    for (const cb of frameCallbacks) cb(dt, now);
    renderer.render(scene, camera);
    labelRenderer.render(scene, camera);
  }

  function start() {
    if (!rafId) {
      last = performance.now();
      rafId = requestAnimationFrame(tick);
    }
  }

  function stop() {
    cancelAnimationFrame(rafId);
    rafId = 0;
  }

  function onFrame(cb) {
    frameCallbacks.add(cb);
    return () => frameCallbacks.delete(cb);
  }

  function dispose() {
    stop();
    window.removeEventListener("resize", resize);
    floor.geometry.dispose();
    floor.material.dispose();
    renderer.dispose();
    renderer.domElement.remove();
    labelRenderer.domElement.remove();
  }

  return { scene, camera, renderer, labelRenderer, start, stop, onFrame, resize, frame, dispose };
}

// A grid drawn in screen space: `gridLine` returns 1 on a line and 0 between
// lines, with the edge softened over one pixel using derivatives (fwidth).
// Colours arrive in linear space, so the chunk three.js uses in its own
// materials converts the result to the canvas colour space on the way out.
function makeFloor() {
  const material = new THREE.ShaderMaterial({
    uniforms: {
      uLine: { value: new THREE.Color(cssColor("--grid-line", "#233350")) },
      uAxis: { value: new THREE.Color(cssColor("--grid-axis", "#3a5280")) },
      uCell: { value: GRID_CELL },
      uOpacity: { value: 0.6 },
      uFade: { value: new THREE.Vector2(60, 160) }, // start / end of the distance fade
    },
    vertexShader: `
      varying vec3 vWorld;
      void main() {
        vec4 world = modelMatrix * vec4(position, 1.0);
        vWorld = world.xyz;
        gl_Position = projectionMatrix * viewMatrix * world;
      }`,
    fragmentShader: `
      uniform vec3 uLine;
      uniform vec3 uAxis;
      uniform float uCell;
      uniform float uOpacity;
      uniform vec2 uFade;
      varying vec3 vWorld;

      float gridLine(vec2 p, float cell) {
        vec2 q = p / cell;
        vec2 d = abs(fract(q - 0.5) - 0.5) / fwidth(q);
        return 1.0 - min(min(d.x, d.y), 1.0);
      }

      void main() {
        float minor = gridLine(vWorld.xz, uCell);
        float major = gridLine(vWorld.xz, uCell * 5.0);
        vec2 axisD = abs(vWorld.xz) / fwidth(vWorld.xz);
        float axis = 1.0 - min(min(axisD.x, axisD.y), 1.0);
        float fade = 1.0 - smoothstep(uFade.x, uFade.y, distance(cameraPosition, vWorld));
        float alpha = max(max(minor * 0.45, major), axis) * fade * uOpacity;
        if (alpha < 0.002) discard;
        gl_FragColor = vec4(mix(uLine, uAxis, axis), alpha);
        #include <colorspace_fragment>
      }`,
    transparent: true,
    depthWrite: false,
    side: THREE.DoubleSide,
  });
  const mesh = new THREE.Mesh(new THREE.PlaneGeometry(FLOOR_SIZE, FLOOR_SIZE), material);
  mesh.rotation.x = -Math.PI / 2;
  mesh.position.y = FLOOR_Y;
  mesh.name = "floor";
  mesh.renderOrder = -1; // under the (transparent) link lines
  return mesh;
}
