// Fly camera. W/A/S/D move, Q/E go down/up, Shift is fast, hold the right
// mouse button to look around, the mouse wheel changes the speed.
//
// This module also owns the camera tweens behind "fly to host" (F, double
// click, host list) and "reset view" (R), so exactly one thing ever moves the
// camera and the two can never fight. Only the movement keys are read here;
// app.js maps R/F/?/Escape to actions because those need the graph and panel.

import * as THREE from "three";

const MOVE_KEYS = new Set(["KeyW", "KeyA", "KeyS", "KeyD", "KeyQ", "KeyE"]);
const FAST_KEYS = new Set(["ShiftLeft", "ShiftRight"]);
const PITCH_LIMIT = Math.PI / 2 - 0.02;   // never quite straight up or down
const MAX_LOOK_DELTA_PX = 300;            // pointer lock occasionally reports one huge jump
const MIN_FLY_ELEVATION = 0.2;            // y of the approach direction: never fly in from below
const DEFAULT_VIEW_DIR = new THREE.Vector3(0, 0.55, 1).normalize();

export const DEFAULTS = {
  speed: 18,            // world units per second
  minSpeed: 1,
  maxSpeed: 500,
  fastMultiplier: 3.5,  // while Shift is held
  lookSpeed: 0.0022,    // radians per pixel of mouse movement
  damping: 9,           // higher = snappier start and stop
  tweenSeconds: 0.9,    // fly-to / reset duration
};

function easeInOutCubic(t) {
  return t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2;
}

// True for the filter box and the like: key presses there must not move the camera.
export function isTypingTarget(target) {
  if (!(target instanceof Element)) return false;
  if (target.isContentEditable) return true;
  return ["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName);
}

export class FlyControls {
  constructor(camera, domElement, options = {}) {
    this.camera = camera;
    this.domElement = domElement;
    Object.assign(this, DEFAULTS, options);

    this.enabled = true;
    this.keys = new Set();
    this.velocity = new THREE.Vector3();
    this.yaw = 0;
    this.pitch = 0;
    this.looking = false;        // right mouse button held
    this.userMoved = false;      // set once the user flies, looks or flies to a host
    this.tween = null;
    this.onSpeedChange = null;   // (speed) => void; app.js shows it in the HUD

    this._dir = new THREE.Vector3();
    this._target = new THREE.Vector3();
    this._forward = new THREE.Vector3();
    this._right = new THREE.Vector3();
    this._euler = new THREE.Euler(0, 0, 0, "YXZ");
    this._matrix = new THREE.Matrix4();
    this._lookPointer = null;

    // Yaw around world Y first, then pitch: the camera can never roll.
    camera.rotation.order = "YXZ";
    this.syncFromCamera();

    this._handlers = [
      [window, "keydown", (e) => this.onKeyDown(e)],
      [window, "keyup", (e) => this.keys.delete(e.code)],
      [window, "blur", () => this.keys.clear()],
      [domElement, "contextmenu", (e) => e.preventDefault()],
      [domElement, "pointerdown", (e) => this.onPointerDown(e)],
      [domElement, "pointermove", (e) => this.onPointerMove(e)],
      [domElement, "pointerup", (e) => this.onPointerUp(e)],
      [domElement, "pointercancel", (e) => this.onPointerUp(e)],
      [domElement, "wheel", (e) => this.onWheel(e), { passive: false }],
      [document, "pointerlockchange", () => this.onPointerLockChange()],
    ];
    for (const [el, type, fn, opts] of this._handlers) el.addEventListener(type, fn, opts);
  }

  // ----- keyboard -----------------------------------------------------------
  onKeyDown(e) {
    if (!this.enabled || e.ctrlKey || e.metaKey || e.altKey || isTypingTarget(e.target)) return;
    if (MOVE_KEYS.has(e.code)) {
      this.keys.add(e.code);
      this.userMoved = true;
      this.cancelTween();
      e.preventDefault();
    } else if (FAST_KEYS.has(e.code)) {
      this.keys.add(e.code);
    }
  }

  get fast() {
    return this.keys.has("ShiftLeft") || this.keys.has("ShiftRight");
  }

  // ----- mouse look -----------------------------------------------------------
  onPointerDown(e) {
    if (!this.enabled || e.button !== 2) return;
    e.preventDefault();
    this.looking = true;
    this._lookPointer = e.pointerId;
    this.cancelTween();
    // Capture keeps the drag alive when the pointer leaves the canvas. Pointer
    // lock, when the browser grants it, hides the cursor and removes the screen
    // edge; if it is refused, movementX/Y from the captured pointer still work.
    try { this.domElement.setPointerCapture(e.pointerId); } catch { /* not fatal */ }
    const request = this.domElement.requestPointerLock?.();
    if (request?.catch) request.catch(() => {});
  }

  onPointerMove(e) {
    if (!this.looking || e.pointerId !== this._lookPointer) return;
    const dx = e.movementX ?? 0;
    const dy = e.movementY ?? 0;
    if (Math.abs(dx) > MAX_LOOK_DELTA_PX || Math.abs(dy) > MAX_LOOK_DELTA_PX) return;
    this.look(dx, dy);
  }

  onPointerUp(e) {
    if (this.looking && e.pointerId === this._lookPointer) this.stopLooking();
  }

  onPointerLockChange() {
    const locked = document.pointerLockElement === this.domElement;
    // Escape releases the lock without a pointerup: stop looking then too.
    if (this.looking && !locked) this.stopLooking();
    // The lock was granted after the button was already released: give it back.
    if (!this.looking && locked) document.exitPointerLock();
  }

  stopLooking() {
    this.looking = false;
    try { this.domElement.releasePointerCapture(this._lookPointer); } catch { /* already released */ }
    this._lookPointer = null;
    if (document.pointerLockElement === this.domElement) document.exitPointerLock();
  }

  look(dx, dy) {
    this.userMoved = true;
    this.yaw -= dx * this.lookSpeed;
    this.pitch = THREE.MathUtils.clamp(this.pitch - dy * this.lookSpeed, -PITCH_LIMIT, PITCH_LIMIT);
    this.applyRotation();
  }

  // ----- speed --------------------------------------------------------------------
  onWheel(e) {
    if (!this.enabled) return;
    e.preventDefault();
    this.setSpeed(this.speed * Math.exp(-e.deltaY * 0.0015));
  }

  setSpeed(value) {
    this.speed = THREE.MathUtils.clamp(value, this.minSpeed, this.maxSpeed);
    this.onSpeedChange?.(this.speed);
  }

  // ----- orientation ----------------------------------------------------------------
  applyRotation() {
    this.camera.rotation.set(this.pitch, this.yaw, 0);
  }

  // Read yaw/pitch back after something else (scene.frame, a finished tween)
  // positioned the camera, and drop any roll it may have picked up.
  syncFromCamera() {
    this._euler.setFromQuaternion(this.camera.quaternion, "YXZ");
    this.yaw = this._euler.y;
    this.pitch = THREE.MathUtils.clamp(this._euler.x, -PITCH_LIMIT, PITCH_LIMIT);
    this.applyRotation();
  }

  // ----- tweens ------------------------------------------------------------------------
  tweenTo(position, quaternion, seconds = this.tweenSeconds) {
    this.velocity.set(0, 0, 0);
    this.tween = {
      fromP: this.camera.position.clone(),
      fromQ: this.camera.quaternion.clone(),
      toP: position.clone(),
      toQ: quaternion.clone(),
      t: 0,
      seconds: Math.max(seconds, 1e-3),
    };
  }

  // Glide to `distance` from `target`, approaching from the side the camera is
  // already on, and end up looking straight at it.
  flyTo(target, distance, seconds) {
    this.userMoved = true;
    const dir = this.camera.position.clone().sub(target);
    if (dir.lengthSq() < 1e-6) dir.copy(DEFAULT_VIEW_DIR);
    dir.normalize();
    if (dir.y < MIN_FLY_ELEVATION) {
      dir.y = MIN_FLY_ELEVATION;
      dir.normalize();
    }
    const position = target.clone().addScaledVector(dir, distance);
    this._matrix.lookAt(position, target, this.camera.up); // camera convention: -z toward target
    const quaternion = new THREE.Quaternion().setFromRotationMatrix(this._matrix);
    this.tweenTo(position, quaternion, seconds);
  }

  cancelTween() {
    if (!this.tween) return;
    this.tween = null;
    this.syncFromCamera();
  }

  stepTween(dt) {
    const tw = this.tween;
    tw.t = Math.min(1, tw.t + dt / tw.seconds);
    const k = easeInOutCubic(tw.t);
    this.camera.position.lerpVectors(tw.fromP, tw.toP, k);
    this.camera.quaternion.slerpQuaternions(tw.fromQ, tw.toQ, k);
    if (tw.t >= 1) {
      this.tween = null;
      this.syncFromCamera();
    }
  }

  // ----- per frame -----------------------------------------------------------------------
  // Returns true when the camera moved this frame.
  update(dt) {
    if (this.tween) {
      this.stepTween(dt);
      return true;
    }
    if (!this.enabled) return false;

    const k = this.keys;
    const dir = this._dir.set(
      (k.has("KeyD") ? 1 : 0) - (k.has("KeyA") ? 1 : 0),
      (k.has("KeyE") ? 1 : 0) - (k.has("KeyQ") ? 1 : 0),
      (k.has("KeyS") ? 1 : 0) - (k.has("KeyW") ? 1 : 0),
    );
    const target = this._target.set(0, 0, 0);
    if (dir.lengthSq() > 0) {
      // Forward follows the look direction (real flying); strafing stays level;
      // Q/E are world down/up no matter where the camera points.
      this._forward.set(0, 0, -1).applyQuaternion(this.camera.quaternion);
      this._right.set(1, 0, 0).applyQuaternion(this.camera.quaternion).setY(0).normalize();
      target.addScaledVector(this._forward, -dir.z).addScaledVector(this._right, dir.x);
      target.y += dir.y;
      target.normalize().multiplyScalar(this.speed * (this.fast ? this.fastMultiplier : 1));
    }

    this.velocity.lerp(target, Math.min(1, this.damping * dt));
    if (this.velocity.lengthSq() < 1e-6) {
      this.velocity.set(0, 0, 0);
      return false;
    }
    this.camera.position.addScaledVector(this.velocity, dt);
    return true;
  }

  dispose() {
    if (this.looking) this.stopLooking();
    for (const [el, type, fn, opts] of this._handlers) el.removeEventListener(type, fn, opts);
  }
}
