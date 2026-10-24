//! Fly camera. W/A/S/D move, Q/E go down/up, Shift is fast, hold the right
//! mouse button to look around, the mouse wheel changes the speed.
//!
//! This module also owns the camera tweens behind "fly to host" (F, double
//! click, host list) and "reset view" (R), so exactly one thing ever moves the
//! camera and the two can never fight. Only the movement keys are read here;
//! main.rs maps R/F/Escape to actions because those need the graph and picker.

use std::f32::consts::FRAC_PI_2;

use bevy::input::mouse::{AccumulatedMouseScroll, MouseMotion, MouseScrollUnit};
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};

use crate::ViewerSet;
use crate::scene::{DEFAULT_VIEW_DIR, MainCamera};
use crate::ui::UiInput;

const MOVE_KEYS: [KeyCode; 6] = [
    KeyCode::KeyW,
    KeyCode::KeyA,
    KeyCode::KeyS,
    KeyCode::KeyD,
    KeyCode::KeyQ,
    KeyCode::KeyE,
];
const FAST_KEYS: [KeyCode; 2] = [KeyCode::ShiftLeft, KeyCode::ShiftRight];
/// Ctrl, Alt and Super: with one of these held a key is a shortcut for
/// someone else (Ctrl+S, Alt+Tab), never a fly key.
const COMMAND_MODIFIERS: [KeyCode; 6] = [
    KeyCode::ControlLeft,
    KeyCode::ControlRight,
    KeyCode::AltLeft,
    KeyCode::AltRight,
    KeyCode::SuperLeft,
    KeyCode::SuperRight,
];
/// Never quite straight up or down.
const PITCH_LIMIT: f32 = FRAC_PI_2 - 0.02;
/// A single motion event bigger than this is a glitch, not a hand movement.
/// Checked per event, not per frame: a frame's total can legitimately
/// exceed it during a quick turn on a slow frame.
const MAX_LOOK_DELTA_PX: f32 = 300.0;
/// y of the approach direction: never fly in from below.
const MIN_FLY_ELEVATION: f32 = 0.2;
/// Pixels of wheel travel per notch. Bevy reports a mouse wheel in lines and
/// a touchpad in pixels; the speed curve works in pixels, so a line counts
/// as this many.
const WHEEL_LINE_PX: f32 = 100.0;
/// Speed change per pixel of wheel travel: one notch multiplies the speed by
/// e^0.15, about 1.16.
const WHEEL_RATE: f32 = 0.0015;
/// A frame's dt is clamped to this, so the camera does not jump after a stall.
const MAX_DT: f32 = 0.1;

/// The fly camera's settings and live state.
#[derive(Resource, Debug, Clone)]
pub struct FlyControls {
    /// False while something else owns the input (nothing does yet).
    pub enabled: bool,
    /// World units per second.
    pub speed: f32,
    pub min_speed: f32,
    pub max_speed: f32,
    /// While Shift is held.
    pub fast_multiplier: f32,
    /// Radians per pixel of mouse movement.
    pub look_speed: f32,
    /// Higher = snappier start and stop.
    pub damping: f32,
    /// Fly-to / reset duration.
    pub tween_seconds: f32,

    pub velocity: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    /// Right mouse button held.
    pub looking: bool,
    /// Set once the user flies, looks or flies to a host.
    pub user_moved: bool,
    pub tween: Option<Tween>,
}

impl Default for FlyControls {
    fn default() -> Self {
        FlyControls {
            enabled: true,
            speed: 18.0,
            min_speed: 1.0,
            max_speed: 500.0,
            fast_multiplier: 3.5,
            look_speed: 0.0022,
            damping: 9.0,
            tween_seconds: 0.9,
            velocity: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            looking: false,
            user_moved: false,
            tween: None,
        }
    }
}

/// A glide from one camera pose to another.
#[derive(Debug, Clone, PartialEq)]
pub struct Tween {
    pub from: (Vec3, Quat),
    pub to: (Vec3, Quat),
    /// Progress, 0 to 1.
    pub t: f32,
    pub seconds: f32,
}

impl Tween {
    pub fn new(from: &Transform, position: Vec3, rotation: Quat, seconds: f32) -> Tween {
        Tween {
            from: (from.translation, from.rotation),
            to: (position, rotation),
            t: 0.0,
            seconds: seconds.max(1e-3),
        }
    }

    /// Advance by `dt` seconds and return the pose for this frame.
    pub fn step(&mut self, dt: f32) -> (Vec3, Quat) {
        self.t = (self.t + dt / self.seconds).min(1.0);
        let k = ease_in_out_cubic(self.t);
        (
            self.from.0.lerp(self.to.0, k),
            self.from.1.slerp(self.to.1, k),
        )
    }

    pub fn done(&self) -> bool {
        self.t >= 1.0
    }
}

pub fn ease_in_out_cubic(t: f32) -> f32 {
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}

/// Shift is held: fly faster.
pub fn fast_held(keys: &ButtonInput<KeyCode>) -> bool {
    keys.any_pressed(FAST_KEYS)
}

impl FlyControls {
    pub fn set_speed(&mut self, value: f32) {
        self.speed = value.clamp(self.min_speed, self.max_speed);
    }

    /// Yaw around world Y first, then pitch: the camera can never roll.
    pub fn apply_rotation(&self, camera: &mut Transform) {
        camera.rotation = Quat::from_euler(EulerRot::YXZ, self.yaw, self.pitch, 0.0);
    }

    /// Read yaw/pitch back after something else (framing, a finished tween)
    /// positioned the camera, and drop any roll it may have picked up.
    pub fn sync_from_camera(&mut self, camera: &mut Transform) {
        let (yaw, pitch, _roll) = camera.rotation.to_euler(EulerRot::YXZ);
        self.yaw = yaw;
        self.pitch = pitch.clamp(-PITCH_LIMIT, PITCH_LIMIT);
        self.apply_rotation(camera);
    }

    /// Turn by a mouse movement of `dx`, `dy` pixels.
    pub fn look(&mut self, dx: f32, dy: f32, camera: &mut Transform) {
        self.user_moved = true;
        self.yaw -= dx * self.look_speed;
        self.pitch = (self.pitch - dy * self.look_speed).clamp(-PITCH_LIMIT, PITCH_LIMIT);
        self.apply_rotation(camera);
    }

    pub fn tween_to(&mut self, camera: &Transform, position: Vec3, rotation: Quat) {
        self.velocity = Vec3::ZERO;
        self.tween = Some(Tween::new(camera, position, rotation, self.tween_seconds));
    }

    /// Glide to `distance` from `target`, approaching from the side the camera
    /// is already on, and end up looking straight at it.
    pub fn fly_to(&mut self, camera: &Transform, target: Vec3, distance: f32) {
        self.user_moved = true;
        let (position, rotation) = fly_to_pose(camera.translation, target, distance);
        self.tween_to(camera, position, rotation);
    }

    pub fn cancel_tween(&mut self, camera: &mut Transform) {
        if self.tween.take().is_some() {
            self.sync_from_camera(camera);
        }
    }
}

/// Where a fly-to ends: `distance` from `target` along the direction the
/// camera already sits in (lifted to at least `MIN_FLY_ELEVATION`), looking at it.
pub fn fly_to_pose(camera_position: Vec3, target: Vec3, distance: f32) -> (Vec3, Quat) {
    let mut dir = camera_position - target;
    if dir.length_squared() < 1e-6 {
        dir = DEFAULT_VIEW_DIR;
    }
    dir = dir.normalize();
    if dir.y < MIN_FLY_ELEVATION {
        dir.y = MIN_FLY_ELEVATION;
        dir = dir.normalize();
    }
    let position = target + dir * distance;
    let rotation = Transform::from_translation(position)
        .looking_at(target, Vec3::Y)
        .rotation;
    (position, rotation)
}

/// Ctrl, Alt or Super is held (Shift is not a command modifier: it means fast).
pub fn command_modifier_held(keys: &ButtonInput<KeyCode>) -> bool {
    keys.any_pressed(COMMAND_MODIFIERS)
}

/// (right, up, back), each -1, 0 or 1, from the held movement keys.
pub fn movement_input(keys: &ButtonInput<KeyCode>) -> Vec3 {
    let axis = |positive: KeyCode, negative: KeyCode| {
        f32::from(i8::from(keys.pressed(positive)) - i8::from(keys.pressed(negative)))
    };
    Vec3::new(
        axis(KeyCode::KeyD, KeyCode::KeyA),
        axis(KeyCode::KeyE, KeyCode::KeyQ),
        axis(KeyCode::KeyS, KeyCode::KeyW),
    )
}

/// Forward follows the look direction (real flying); strafing stays level;
/// Q/E are world down/up no matter where the camera points.
pub fn velocity_target(input: Vec3, rotation: Quat, speed: f32) -> Vec3 {
    if input.length_squared() == 0.0 {
        return Vec3::ZERO;
    }
    let forward = rotation * Vec3::NEG_Z;
    let right = (rotation * Vec3::X).with_y(0.0).normalize_or_zero();
    let mut target = forward * -input.z + right * input.x;
    target.y += input.y;
    target.normalize_or_zero() * speed
}

pub struct ControlsPlugin;

impl Plugin for ControlsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FlyControls>().add_systems(
            Update,
            (read_keyboard, mouse_look, mouse_wheel, move_camera)
                .chain()
                .in_set(ViewerSet::Controls),
        );
    }
}

/// A movement key takes the camera away from any tween.
fn read_keyboard(
    keys: Res<ButtonInput<KeyCode>>,
    ui: Res<UiInput>,
    mut controls: ResMut<FlyControls>,
    mut camera: Single<&mut Transform, With<MainCamera>>,
) {
    if !controls.enabled || ui.keyboard || command_modifier_held(&keys) {
        return;
    }
    if keys.any_just_pressed(MOVE_KEYS) {
        controls.user_moved = true;
        controls.cancel_tween(&mut camera);
    }
}

/// Right button held = look. The cursor is locked and hidden meanwhile, so
/// there is no screen edge to hit.
/// (Bevy falls back to confining the cursor where locking is unsupported, X11.)
fn mouse_look(
    buttons: Res<ButtonInput<MouseButton>>,
    mut motion: MessageReader<MouseMotion>,
    ui: Res<UiInput>,
    window: Single<(&Window, &mut CursorOptions), With<PrimaryWindow>>,
    mut controls: ResMut<FlyControls>,
    mut camera: Single<&mut Transform, With<MainCamera>>,
) {
    let (window, mut cursor) = window.into_inner();
    if controls.enabled && !ui.pointer && buttons.just_pressed(MouseButton::Right) {
        controls.looking = true;
        controls.cancel_tween(&mut camera);
        cursor.grab_mode = CursorGrabMode::Locked;
        cursor.visible = false;
    }
    // Losing focus can swallow the button release, so it ends the look too.
    if controls.looking && (!buttons.pressed(MouseButton::Right) || !window.focused) {
        controls.looking = false;
        cursor.grab_mode = CursorGrabMode::None;
        cursor.visible = true;
    }
    let delta: Vec2 = motion
        .read()
        .map(|event| event.delta)
        .filter(|d| d.x.abs() <= MAX_LOOK_DELTA_PX && d.y.abs() <= MAX_LOOK_DELTA_PX)
        .sum();
    if controls.looking && delta != Vec2::ZERO {
        controls.look(delta.x, delta.y, &mut camera);
    }
}

/// Scrolling up speeds up, scrolling down slows down, exponentially.
fn mouse_wheel(
    scroll: Res<AccumulatedMouseScroll>,
    ui: Res<UiInput>,
    mut controls: ResMut<FlyControls>,
) {
    if !controls.enabled || ui.pointer || scroll.delta.y == 0.0 {
        return;
    }
    let pixels = match scroll.unit {
        MouseScrollUnit::Line => scroll.delta.y * WHEEL_LINE_PX,
        MouseScrollUnit::Pixel => scroll.delta.y,
    };
    // Bevy's y is positive scrolling up, so scrolling up multiplies by more than 1.
    let speed = controls.speed * (pixels * WHEEL_RATE).exp();
    controls.set_speed(speed);
}

/// Step the tween if there is one, else fly with damping.
/// Keys typed into the filter box (or pressed with Ctrl/Alt/Super) count as
/// released, so the camera coasts to a stop instead of flying off.
fn move_camera(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    ui: Res<UiInput>,
    mut controls: ResMut<FlyControls>,
    mut camera: Single<&mut Transform, With<MainCamera>>,
) {
    let dt = time.delta_secs().min(MAX_DT);
    if let Some(tween) = controls.tween.as_mut() {
        let (position, rotation) = tween.step(dt);
        let done = tween.done();
        camera.translation = position;
        camera.rotation = rotation;
        if done {
            controls.tween = None;
            controls.sync_from_camera(&mut camera);
        }
        return;
    }
    if !controls.enabled {
        return;
    }
    let input = if ui.keyboard || command_modifier_held(&keys) {
        Vec3::ZERO
    } else {
        movement_input(&keys)
    };
    let speed = controls.speed
        * if fast_held(&keys) {
            controls.fast_multiplier
        } else {
            1.0
        };
    let target = velocity_target(input, camera.rotation, speed);
    let k = (controls.damping * dt).min(1.0);
    controls.velocity = controls.velocity.lerp(target, k);
    if controls.velocity.length_squared() < 1e-6 {
        controls.velocity = Vec3::ZERO;
        return;
    }
    camera.translation += controls.velocity * dt;
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bevy::time::TimeUpdateStrategy;

    use super::*;

    const FRAME: Duration = Duration::from_millis(16);

    /// A headless app: fixed 16 ms frames, input resources driven by hand
    /// (no `InputPlugin`, so `just_pressed` is ours to clear), one camera.
    /// With `window`, a primary window entity too, for the mouse-look system.
    fn test_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(TimeUpdateStrategy::ManualDuration(FRAME))
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .add_message::<MouseMotion>()
            .init_resource::<AccumulatedMouseScroll>()
            .init_resource::<UiInput>()
            .add_plugins(ControlsPlugin);
        app.world_mut().spawn((
            PrimaryWindow,
            Window {
                focused: true,
                ..default()
            },
            CursorOptions::default(),
        ));
        app.world_mut()
            .spawn((MainCamera, Transform::from_xyz(0.0, 0.0, 10.0)));
        app.update(); // first frame has no delta
        app
    }

    fn frames(app: &mut App, n: usize) {
        for _ in 0..n {
            app.update();
            app.world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .clear();
            app.world_mut()
                .resource_mut::<ButtonInput<MouseButton>>()
                .clear();
        }
    }

    fn camera(app: &mut App) -> Transform {
        *app.world_mut()
            .query_filtered::<&Transform, With<MainCamera>>()
            .single(app.world())
            .unwrap()
    }

    fn cursor_options(app: &mut App) -> CursorOptions {
        app.world_mut()
            .query::<&CursorOptions>()
            .single(app.world())
            .unwrap()
            .clone()
    }

    fn press(app: &mut App, key: KeyCode) {
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(key);
    }

    #[test]
    fn holding_w_flies_forward_and_counts_as_user_movement() {
        let mut app = test_app();
        assert!(!app.world().resource::<FlyControls>().user_moved);
        press(&mut app, KeyCode::KeyW);
        frames(&mut app, 60); // ~1 s at 18 u/s, minus the damped start
        let t = camera(&mut app);
        assert!(t.translation.z < 10.0 - 10.0, "{:?}", t.translation);
        assert!(t.translation.x.abs() < 1e-4 && t.translation.y.abs() < 1e-4);
        assert!(app.world().resource::<FlyControls>().user_moved);
    }

    #[test]
    fn shift_is_faster_and_releasing_keys_coasts_to_a_stop() {
        let mut app = test_app();
        press(&mut app, KeyCode::KeyW);
        frames(&mut app, 30);
        let slow = 10.0 - camera(&mut app).translation.z;

        let mut app = test_app();
        press(&mut app, KeyCode::KeyW);
        press(&mut app, KeyCode::ShiftLeft);
        frames(&mut app, 30);
        let fast = 10.0 - camera(&mut app).translation.z;
        assert!(fast > slow * 2.5, "slow {slow} fast {fast}");

        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release_all();
        frames(&mut app, 150); // the damped velocity needs ~70 frames to drop under the cutoff
        let stopped = camera(&mut app).translation.z;
        frames(&mut app, 10);
        assert_eq!(camera(&mut app).translation.z, stopped);
        assert_eq!(app.world().resource::<FlyControls>().velocity, Vec3::ZERO);
    }

    #[test]
    fn strafing_stays_level_and_q_e_are_world_up_down() {
        // Looking 45° down: W should descend, D should not, E goes straight up.
        let pitched = Quat::from_euler(EulerRot::YXZ, 0.0, -std::f32::consts::FRAC_PI_4, 0.0);
        let w = velocity_target(Vec3::new(0.0, 0.0, -1.0), pitched, 10.0);
        assert!(w.y < -5.0 && w.z < -5.0, "{w:?}");
        let d = velocity_target(Vec3::new(1.0, 0.0, 0.0), pitched, 10.0);
        assert!((d - Vec3::new(10.0, 0.0, 0.0)).length() < 1e-4, "{d:?}");
        let e = velocity_target(Vec3::new(0.0, 1.0, 0.0), pitched, 10.0);
        assert!((e - Vec3::new(0.0, 10.0, 0.0)).length() < 1e-4, "{e:?}");
        assert_eq!(velocity_target(Vec3::ZERO, pitched, 10.0), Vec3::ZERO);
    }

    #[test]
    fn looking_turns_without_roll_and_clamps_the_pitch() {
        let mut controls = FlyControls::default();
        let mut camera = Transform::from_xyz(0.0, 0.0, 10.0);
        controls.look(100.0, 0.0, &mut camera);
        assert!((controls.yaw - (-0.22)).abs() < 1e-5);
        assert!(controls.user_moved);
        controls.look(0.0, 100_000.0, &mut camera);
        assert!(
            (controls.pitch + PITCH_LIMIT).abs() < 1e-6,
            "pitch {}",
            controls.pitch
        );
        let (_, _, roll) = camera.rotation.to_euler(EulerRot::YXZ);
        assert!(roll.abs() < 1e-5);
        // Forward now points almost straight down.
        let forward = camera.rotation * Vec3::NEG_Z;
        assert!(forward.y < -0.99, "{forward:?}");
    }

    #[test]
    fn sync_from_camera_recovers_yaw_and_pitch_from_a_look_at() {
        let mut controls = FlyControls::default();
        let mut camera = Transform::from_xyz(10.0, 10.0, 10.0).looking_at(Vec3::ZERO, Vec3::Y);
        let before = camera.rotation;
        controls.sync_from_camera(&mut camera);
        assert!(camera.rotation.angle_between(before) < 1e-4);
        assert!(controls.pitch < 0.0, "looking down");
    }

    #[test]
    fn fly_to_approaches_from_the_near_side_but_never_from_below() {
        let target = Vec3::new(5.0, 0.0, 0.0);
        // Already above the host: keep that direction, stop `distance` away, look at it.
        let (p, q) = fly_to_pose(Vec3::new(5.0, 10.0, 20.0), target, 4.0);
        let dir = Vec3::new(0.0, 10.0, 20.0).normalize();
        assert!((p - (target + dir * 4.0)).length() < 1e-4, "{p:?}");
        let forward = q * Vec3::NEG_Z;
        assert!((forward + dir).length() < 1e-4, "{forward:?}");

        // From slightly below: the approach is lifted to the minimum elevation.
        let (p, _) = fly_to_pose(Vec3::new(5.0, -3.0, 20.0), target, 10.0);
        let dir = (p - target).normalize();
        assert!((dir.y - MIN_FLY_ELEVATION).abs() < 0.01, "{dir:?}");
        // From straight below there is nothing horizontal left: it comes from above.
        let (p, _) = fly_to_pose(Vec3::new(5.0, -30.0, 0.5), target, 10.0);
        assert!((p - target).normalize().y >= MIN_FLY_ELEVATION, "{p:?}");

        // Sitting exactly on the target: fall back to the default view direction.
        let (p, _) = fly_to_pose(target, target, 10.0);
        assert!(((p - target).normalize() - DEFAULT_VIEW_DIR.normalize()).length() < 1e-4);
    }

    #[test]
    fn a_tween_glides_to_its_pose_then_hands_back_to_the_fly_controls() {
        let mut app = test_app();
        {
            let world = app.world_mut();
            let camera = *world
                .query_filtered::<&Transform, With<MainCamera>>()
                .single(world)
                .unwrap();
            let mut controls = world.resource_mut::<FlyControls>();
            controls.fly_to(&camera, Vec3::new(3.0, 0.0, 0.0), 4.0);
        }
        frames(&mut app, 30); // halfway: moving, not there yet
        let mid = camera(&mut app);
        assert!(mid.translation.distance(Vec3::new(0.0, 0.0, 10.0)) > 1.0);
        assert!(app.world().resource::<FlyControls>().tween.is_some());
        frames(&mut app, 40); // 70 frames * 16 ms > 0.9 s
        let end = camera(&mut app);
        let (expected, _) = fly_to_pose(Vec3::new(0.0, 0.0, 10.0), Vec3::new(3.0, 0.0, 0.0), 4.0);
        assert!(
            end.translation.distance(expected) < 1e-3,
            "{:?}",
            end.translation
        );
        let controls = app.world().resource::<FlyControls>();
        assert!(controls.tween.is_none());
        assert!(controls.user_moved);
        let synced = Quat::from_euler(EulerRot::YXZ, controls.yaw, controls.pitch, 0.0);
        assert!(synced.angle_between(end.rotation) < 1e-3);
    }

    #[test]
    fn a_movement_key_cancels_a_tween() {
        let mut app = test_app();
        {
            let world = app.world_mut();
            let camera = *world
                .query_filtered::<&Transform, With<MainCamera>>()
                .single(world)
                .unwrap();
            world
                .resource_mut::<FlyControls>()
                .fly_to(&camera, Vec3::ZERO, 4.0);
        }
        frames(&mut app, 5);
        press(&mut app, KeyCode::KeyA);
        frames(&mut app, 1);
        assert!(app.world().resource::<FlyControls>().tween.is_none());
    }

    #[test]
    fn the_wheel_scales_speed_exponentially_within_limits() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AccumulatedMouseScroll>()
            .delta = Vec2::new(0.0, 1.0);
        app.world_mut()
            .resource_mut::<AccumulatedMouseScroll>()
            .unit = MouseScrollUnit::Line;
        frames(&mut app, 1);
        let speed = app.world().resource::<FlyControls>().speed;
        assert!(
            (speed - 18.0 * (100.0 * WHEEL_RATE).exp()).abs() < 1e-3,
            "{speed}"
        );

        app.world_mut()
            .resource_mut::<AccumulatedMouseScroll>()
            .delta = Vec2::new(0.0, -100_000.0);
        app.world_mut()
            .resource_mut::<AccumulatedMouseScroll>()
            .unit = MouseScrollUnit::Pixel;
        frames(&mut app, 1);
        assert_eq!(app.world().resource::<FlyControls>().speed, 1.0);
    }

    #[test]
    fn keys_typed_into_a_text_box_or_with_ctrl_do_not_fly() {
        let mut app = test_app();
        app.world_mut().resource_mut::<UiInput>().keyboard = true;
        press(&mut app, KeyCode::KeyW);
        frames(&mut app, 30);
        assert_eq!(camera(&mut app).translation, Vec3::new(0.0, 0.0, 10.0));
        assert!(!app.world().resource::<FlyControls>().user_moved);

        let mut app = test_app();
        press(&mut app, KeyCode::ControlLeft);
        press(&mut app, KeyCode::KeyS); // Ctrl+S
        frames(&mut app, 30);
        assert_eq!(camera(&mut app).translation, Vec3::new(0.0, 0.0, 10.0));
        assert!(!app.world().resource::<FlyControls>().user_moved);
    }

    #[test]
    fn right_drag_looks_and_only_single_glitch_events_are_dropped() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Right);
        frames(&mut app, 1);
        assert!(app.world().resource::<FlyControls>().looking);
        let cursor = cursor_options(&mut app);
        assert_eq!(cursor.grab_mode, CursorGrabMode::Locked);
        assert!(!cursor.visible);

        // Two 200 px events in one slow frame: a quick turn, both count.
        let yaw = app.world().resource::<FlyControls>().yaw;
        for _ in 0..2 {
            app.world_mut().write_message(MouseMotion {
                delta: Vec2::new(200.0, 0.0),
            });
        }
        frames(&mut app, 1);
        let turned = app.world().resource::<FlyControls>().yaw - yaw;
        assert!((turned + 400.0 * 0.0022).abs() < 1e-5, "turned {turned}");

        // One 1000 px event is a glitch and is ignored on its own.
        let yaw = app.world().resource::<FlyControls>().yaw;
        app.world_mut().write_message(MouseMotion {
            delta: Vec2::new(1000.0, 0.0),
        });
        app.world_mut().write_message(MouseMotion {
            delta: Vec2::new(10.0, 0.0),
        });
        frames(&mut app, 1);
        let turned = app.world().resource::<FlyControls>().yaw - yaw;
        assert!((turned + 10.0 * 0.0022).abs() < 1e-5, "turned {turned}");

        // Releasing the button gives the cursor back.
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .release(MouseButton::Right);
        frames(&mut app, 1);
        assert!(!app.world().resource::<FlyControls>().looking);
        let cursor = cursor_options(&mut app);
        assert_eq!(cursor.grab_mode, CursorGrabMode::None);
        assert!(cursor.visible);
    }

    #[test]
    fn right_click_over_the_sidebar_does_not_start_looking() {
        let mut app = test_app();
        app.world_mut().resource_mut::<UiInput>().pointer = true;
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Right);
        frames(&mut app, 1);
        assert!(!app.world().resource::<FlyControls>().looking);
    }

    #[test]
    fn easing_is_symmetric() {
        assert_eq!(ease_in_out_cubic(0.0), 0.0);
        assert_eq!(ease_in_out_cubic(1.0), 1.0);
        assert!((ease_in_out_cubic(0.5) - 0.5).abs() < 1e-6);
        assert!((ease_in_out_cubic(0.25) + ease_in_out_cubic(0.75) - 1.0).abs() < 1e-6);
    }
}
