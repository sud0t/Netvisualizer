//! What the window is started with: the log filter, the GPU probe behind
//! `WGPU_BACKEND`, and the smoke test, `--exit-after-frames` with
//! `--screenshot` and `--select`, which renders a few frames and quits so a
//! build can be checked without clicking through it; and `--script`, which
//! moves, clicks and types as a user would, so the window can be pictured
//! in use.

use std::collections::VecDeque;
use std::path::PathBuf;

use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput, NativeKeyCode};
use bevy::input::mouse::{MouseButtonInput, MouseScrollUnit, MouseWheel};
use bevy::input::touch::TouchPhase;
use bevy::log::{DEFAULT_FILTER, Level, LogPlugin};
use bevy::prelude::*;
use bevy::render::settings::{Backends, WgpuSettings};
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use bevy::tasks::block_on;
use bevy::window::{CursorMoved, PrimaryWindow, WindowEvent};

use crate::graph::Graph;
use crate::picking::SelectHost;
use crate::python;
use crate::scan::ScanJob;

/// The screenshot is taken this many frames before the end, and never
/// before this frame: the first frames are still black, and the GPU
/// readback has to finish before the program quits.
const SCREENSHOT_LEAD_FRAMES: u32 = 30;

/// Bevy logs its GPU set-up at "info", which is noise unless asked for with
/// `-v`; the program's own messages (what the scan finds) stay at "info".
/// A `RUST_LOG` variable still overrides both.
pub fn log_settings(verbose: bool) -> LogPlugin {
    let (level, ours) = if verbose {
        (Level::INFO, "debug")
    } else {
        (Level::WARN, "info")
    };
    LogPlugin {
        level,
        filter: format!("{DEFAULT_FILTER}netvisualizer={ours}"),
        ..default()
    }
}

/// `-v`: which Python the program embeds, and what it can import.
pub fn log_python() {
    info!("{}", python::describe());
}

/// Bevy picks the GPU backend from `WGPU_BACKEND` when that variable is set,
/// and panics with "Unable to find a GPU" if the backend it names has no
/// usable adapter. The dev machine's Hyprland config exports `WGPU_BACKEND=gl`
/// and wgpu's OpenGL path finds nothing there, so check first: when the
/// requested backend yields no adapter, say so and let wgpu choose from all
/// of them (Vulkan on that machine).
pub fn render_settings() -> WgpuSettings {
    let mut settings = WgpuSettings::default(); // this is where WGPU_BACKEND is read
    let Ok(requested) = std::env::var("WGPU_BACKEND") else {
        return settings;
    };
    let backends = settings.backends.unwrap_or(Backends::all());
    // Built the way Bevy builds its own (no display handle), so the probe
    // sees the same adapters Bevy will.
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends,
        flags: settings.instance_flags,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    if block_on(instance.enumerate_adapters(backends)).is_empty() {
        eprintln!(
            "netvisualizer: WGPU_BACKEND={requested} is set but no GPU here supports that backend; \
             letting wgpu choose one instead"
        );
        settings.backends = Some(Backends::all());
    }
    settings
}

/// Frame counting for `--exit-after-frames` and `--screenshot`.
#[derive(Resource, Debug)]
pub struct SmokeTest {
    frames: u32,
    exit_after: Option<u32>,
    screenshot: Option<PathBuf>,
    screenshot_frame: u32,
}

impl SmokeTest {
    /// Quit after `exit_after_frames` frames, if given; save `screenshot`
    /// [`SCREENSHOT_LEAD_FRAMES`] before the end, if given, and stretch a
    /// run too short for that rather than leave it without its picture.
    pub fn new(exit_after_frames: Option<u32>, screenshot: Option<PathBuf>) -> SmokeTest {
        let screenshot_frame = exit_after_frames
            .map_or(SCREENSHOT_LEAD_FRAMES, |n| {
                n.saturating_sub(SCREENSHOT_LEAD_FRAMES)
            })
            .max(SCREENSHOT_LEAD_FRAMES);
        let exit_after = exit_after_frames.map(|n| {
            if screenshot.is_some() {
                n.max(screenshot_frame + SCREENSHOT_LEAD_FRAMES)
            } else {
                n
            }
        });
        SmokeTest {
            frames: 0,
            exit_after,
            screenshot,
            screenshot_frame,
        }
    }
}

/// Count the frames; take the screenshot and quit when their frames come.
pub fn smoke_test(
    mut commands: Commands,
    mut smoke: ResMut<SmokeTest>,
    mut exit: MessageWriter<AppExit>,
) {
    smoke.frames += 1;
    if smoke.frames == smoke.screenshot_frame
        && let Some(path) = smoke.screenshot.take()
    {
        info!("saving screenshot to {}", path.display());
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path));
    }
    if smoke.exit_after.is_some_and(|n| smoke.frames >= n) {
        exit.write(AppExit::Success);
    }
}

/// `--select` / `--fly`, consumed once the first map is drawn.
#[derive(Resource, Debug)]
pub struct DebugSelect {
    pub id: Option<String>,
    pub fly: bool,
}

/// Apply `--select` / `--fly` as soon as a drawn map has the host. With
/// `--scan` the first map holds only the scanner, so the host may take a
/// few stages to appear; it is only missing for good once the scan is over.
pub fn debug_select(
    graph: Res<Graph>,
    job: Option<Res<ScanJob>>,
    mut debug: ResMut<DebugSelect>,
    mut select: MessageWriter<SelectHost>,
) {
    if !graph.is_changed() || !graph.drawn {
        return;
    }
    let Some(id) = debug.id.as_deref() else {
        return;
    };
    if !graph.nodes.contains_key(id) {
        if job.is_none() {
            warn!("--select {id}: no such host in the map");
            debug.id = None;
        }
        return;
    }
    select.write(SelectHost {
        id: debug.id.take(),
        fly: debug.fly,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_parse_step_by_step_and_refuse_nonsense() {
        let script = parse_script(
            "wait 0.5; click 480 748 ;text 10.0.0.0/24;key Enter; scroll -3; shot /tmp/a.png;",
        )
        .unwrap();
        assert_eq!(
            script.steps,
            [
                Step::Wait(0.5),
                Step::Click(Vec2::new(480.0, 748.0)),
                Step::Text("10.0.0.0/24".into()),
                Step::Key("Enter".into()),
                Step::Scroll(-3.0),
                Step::Shot("/tmp/a.png".into()),
            ]
        );
        assert!(parse_script("click 10").unwrap_err().contains("X and Y"));
        assert!(
            parse_script("jump 10 10")
                .unwrap_err()
                .contains("not a script step")
        );
        assert!(
            parse_script("wait soon")
                .unwrap_err()
                .contains("not a number")
        );
        assert!(parse_script("key").is_err());
    }

    #[test]
    fn the_screenshot_comes_thirty_frames_before_the_end_but_never_too_early() {
        let plain = SmokeTest::new(Some(240), None);
        assert_eq!((plain.exit_after, plain.screenshot_frame), (Some(240), 210));
        assert!(plain.screenshot.is_none());

        let short = SmokeTest::new(Some(10), Some("x.png".into()));
        assert_eq!(short.screenshot_frame, 30, "the first frames are black");
        assert_eq!(short.exit_after, Some(60), "stretched to fit the readback");

        let endless = SmokeTest::new(None, Some("x.png".into()));
        assert_eq!((endless.exit_after, endless.screenshot_frame), (None, 30));
    }
}

// ---------------------------------------------------------------------------
// --script
// ---------------------------------------------------------------------------

/// One step of a `--script`.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// Do nothing for this many seconds.
    Wait(f32),
    /// Put the pointer here (logical pixels from the top left).
    Move(Vec2),
    /// Move the pointer here and press and release the left button.
    Click(Vec2),
    /// Turn the wheel this many lines (up is positive).
    Scroll(f32),
    /// Press a key: Escape, Enter, Backspace, or one character.
    Key(String),
    /// Type these characters.
    Text(String),
    /// Save a PNG of the window.
    Shot(PathBuf),
}

/// A `--script`: the steps left to run.
#[derive(Resource, Debug, Clone, Default, PartialEq)]
pub struct Script {
    steps: VecDeque<Step>,
    /// Seconds to sit out before the next step (the frame rate varies too
    /// much to count frames).
    wait: f32,
    /// A frame between steps, so each is seen before the next.
    gap: bool,
    /// What was pressed last frame and is released this frame: the button,
    /// and the keys.
    release_button: bool,
    release_keys: Vec<String>,
}

/// `--script`'s value: steps separated by ";".
pub fn parse_script(text: &str) -> Result<Script, String> {
    let mut steps = VecDeque::new();
    for raw in text.split(';') {
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        let (word, rest) = raw.split_once(' ').unwrap_or((raw, ""));
        let rest = rest.trim();
        let point = || -> Result<Vec2, String> {
            let (x, y) = rest
                .split_once(' ')
                .ok_or_else(|| format!("{word} needs X and Y: {raw:?}"))?;
            Ok(Vec2::new(number(x)?, number(y)?))
        };
        let step = match word {
            "wait" => Step::Wait(number(rest)?),
            "move" => Step::Move(point()?),
            "click" => Step::Click(point()?),
            "scroll" => Step::Scroll(number(rest)?),
            "key" if !rest.is_empty() => Step::Key(rest.to_string()),
            "text" => Step::Text(rest.to_string()),
            "shot" if !rest.is_empty() => Step::Shot(PathBuf::from(rest)),
            _ => return Err(format!("not a script step: {raw:?}")),
        };
        steps.push_back(step);
    }
    Ok(Script {
        steps,
        ..Script::default()
    })
}

fn number(text: &str) -> Result<f32, String> {
    text.trim()
        .parse::<f32>()
        .map_err(|_| format!("not a number: {text:?}"))
}

/// A key as Bevy reports it: the named keys, else one typed character.
fn key_event(name: &str, state: ButtonState, window: Entity) -> KeyboardInput {
    let (key_code, logical_key, text) = match name {
        "Escape" => (KeyCode::Escape, Key::Escape, None),
        "Enter" => (KeyCode::Enter, Key::Enter, None),
        "Backspace" => (KeyCode::Backspace, Key::Backspace, None),
        c => (
            KeyCode::Unidentified(NativeKeyCode::Unidentified),
            Key::Character(c.into()),
            Some(c.into()),
        ),
    };
    KeyboardInput {
        key_code,
        logical_key,
        state,
        text,
        repeat: false,
        window,
    }
}

/// The input messages the window's own events arrive as: each one on its
/// own (for the input resources and the text fields) and wrapped in a
/// [`WindowEvent`] (which is what `bevy_picking` reads). The queues
/// themselves, not writers: while a script runs it owns the input, and
/// the real mouse and keyboard are dropped from them first (a mouse
/// resting on the window would otherwise keep moving the pointer back).
struct Input<'w> {
    window: Entity,
    events: ResMut<'w, Messages<WindowEvent>>,
    cursor: ResMut<'w, Messages<CursorMoved>>,
    buttons: ResMut<'w, Messages<MouseButtonInput>>,
    wheel: ResMut<'w, Messages<MouseWheel>>,
    keys: ResMut<'w, Messages<KeyboardInput>>,
}

impl Input<'_> {
    /// Drop what the real devices sent this frame.
    fn take_over(&mut self) {
        self.events.clear();
        self.cursor.clear();
        self.buttons.clear();
        self.wheel.clear();
        self.keys.clear();
    }

    fn move_to(&mut self, position: Vec2) {
        let event = CursorMoved {
            window: self.window,
            position,
            delta: None,
        };
        self.events.write(WindowEvent::CursorMoved(event.clone()));
        self.cursor.write(event);
    }

    fn button(&mut self, state: ButtonState) {
        let event = MouseButtonInput {
            button: MouseButton::Left,
            state,
            window: self.window,
        };
        self.events.write(WindowEvent::MouseButtonInput(event));
        self.buttons.write(event);
    }

    fn scroll(&mut self, lines: f32) {
        let event = MouseWheel {
            unit: MouseScrollUnit::Line,
            x: 0.0,
            y: lines,
            window: self.window,
            phase: TouchPhase::Moved, // what a mouse wheel always reports
        };
        self.events.write(WindowEvent::MouseWheel(event));
        self.wheel.write(event);
    }

    fn key(&mut self, name: &str, state: ButtonState) {
        let event = key_event(name, state, self.window);
        self.events.write(WindowEvent::KeyboardInput(event.clone()));
        self.keys.write(event);
    }
}

/// Run the next step of the script, one per frame with a frame between, so
/// a press lands, is seen, and is released like a real one.
#[allow(clippy::too_many_arguments)] // a Bevy system lists what it writes
pub fn run_script(
    mut script: ResMut<Script>,
    time: Res<Time>,
    mut commands: Commands,
    window: Single<(Entity, &mut Window), With<PrimaryWindow>>,
    events: ResMut<Messages<WindowEvent>>,
    cursor: ResMut<Messages<CursorMoved>>,
    buttons: ResMut<Messages<MouseButtonInput>>,
    wheel: ResMut<Messages<MouseWheel>>,
    keys: ResMut<Messages<KeyboardInput>>,
) {
    if script.steps.is_empty() && !script.release_button && script.release_keys.is_empty() {
        return;
    }
    let (window, mut window_data) = window.into_inner();
    let mut input = Input {
        window,
        events,
        cursor,
        buttons,
        wheel,
        keys,
    };
    // The script owns the input until its last step has run.
    input.take_over();
    let script = &mut *script;
    // Releases first: a press lasts one frame.
    if std::mem::take(&mut script.release_button) {
        input.button(ButtonState::Released);
    }
    for key in std::mem::take(&mut script.release_keys) {
        input.key(&key, ButtonState::Released);
    }
    if script.wait > 0.0 {
        script.wait -= time.delta_secs();
        return;
    }
    if std::mem::take(&mut script.gap) {
        return;
    }
    let Some(step) = script.steps.pop_front() else {
        return;
    };
    script.gap = true;
    match step {
        Step::Wait(seconds) => script.wait = seconds,
        Step::Move(position) => {
            // The window's own idea of the pointer too, for the 3D picking.
            window_data.set_cursor_position(Some(position));
            input.move_to(position);
        }
        Step::Click(position) => {
            window_data.set_cursor_position(Some(position));
            input.move_to(position);
            input.button(ButtonState::Pressed);
            script.release_button = true;
        }
        Step::Scroll(lines) => input.scroll(lines),
        Step::Key(name) => {
            input.key(&name, ButtonState::Pressed);
            script.release_keys.push(name);
        }
        Step::Text(text) => {
            for c in text.chars() {
                let name = c.to_string();
                input.key(&name, ButtonState::Pressed);
                script.release_keys.push(name);
            }
        }
        Step::Shot(path) => {
            info!("saving screenshot to {}", path.display());
            commands
                .spawn(Screenshot::primary_window())
                .observe(save_to_disk(path));
        }
    }
}
