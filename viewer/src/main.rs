//! Native viewer for Pynmapper maps: the same JSON the web page renders, drawn
//! in a Bevy window. Each module is a port of one `pynmapper/graphics/web/*.js`
//! file (scene.rs ↔ scene.js, graph.rs ↔ graph.js, controls.rs ↔ controls.js,
//! picking.rs ↔ picking.js, ui.rs ↔ panel.js + the HTML overlays); this file
//! is app.js: arguments, wiring, and the few keys that need everything.
//!
//!     cargo run --features dev -- --json tests/fixtures/demo.json
//!     cargo run --features dev -- --json scan.json --exit-after-frames 120 --screenshot /tmp/map.png
//!
//! Live mode (following a running `python -m pynmapper` server) arrives in
//! step 7.4; until then `--json` is required.

mod controls;
mod graph;
mod map;
mod picking;
mod scene;
mod theme;
mod ui;

use std::path::PathBuf;
use std::process::ExitCode;

use bevy::prelude::*;
use bevy::render::RenderPlugin;
use bevy::render::settings::{Backends, WgpuSettings};
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use bevy::tasks::block_on;
use bevy::window::PrimaryWindow;
use clap::Parser;

use crate::controls::{ControlsPlugin, FlyControls};
use crate::graph::{Graph, GraphPlugin, MapDocument, Node};
use crate::map::{CONTRACT_VERSION, MapDoc};
use crate::picking::{Picker, PickingPlugin};
use crate::scene::{MainCamera, ResetView, ScenePlugin};
use crate::ui::{UiInput, UiPlugin};

#[derive(Parser, Debug)]
#[command(
    name = "pynmapper-viewer",
    version,
    about = "Native viewer for Pynmapper network maps"
)]
struct Args {
    /// Map server to follow (polls /api/health, downloads /api/map when the
    /// revision changes). Not implemented yet: step 7.4.
    #[arg(default_value = "http://127.0.0.1:8765")]
    url: String,

    /// Draw a map saved with `python -m pynmapper --save FILE` instead of a server.
    #[arg(long, value_name = "FILE")]
    json: Option<PathBuf>,

    /// Render N frames, then quit with status 0 (smoke test).
    #[arg(long, value_name = "N", help_heading = "Debugging")]
    exit_after_frames: Option<u32>,

    /// Save a PNG of the window, 30 frames before exiting (or at frame 30).
    #[arg(long, value_name = "FILE", help_heading = "Debugging")]
    screenshot: Option<PathBuf>,

    /// Select this host (by id) as soon as the map is drawn.
    #[arg(long, value_name = "HOST_ID", help_heading = "Debugging")]
    select: Option<String>,

    /// With --select: also fly to it, as a double click would.
    #[arg(long, requires = "select", help_heading = "Debugging")]
    fly: bool,
}

/// The per-frame order of everything in `Update`. Build the graph, frame it,
/// move the camera, pick under the new camera, then draw the overlays, so a
/// frame never mixes last frame's camera with this frame's picks.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ViewerSet {
    Build,
    Frame,
    Controls,
    Picking,
    Draw,
}

/// Frame counting for `--exit-after-frames` and `--screenshot`.
#[derive(Resource, Debug)]
struct SmokeTest {
    frames: u32,
    exit_after: Option<u32>,
    screenshot: Option<PathBuf>,
    screenshot_frame: u32,
}

/// `--select` / `--fly`, consumed once the first map is drawn.
#[derive(Resource, Debug)]
struct DebugSelect {
    id: Option<String>,
    fly: bool,
}

fn main() -> ExitCode {
    let args = Args::parse();

    let Some(path) = args.json.as_deref() else {
        eprintln!(
            "pynmapper-viewer: live mode ({}) is not implemented yet (step 7.4); pass --json FILE",
            args.url
        );
        return ExitCode::from(2);
    };
    let doc = match MapDoc::load(path) {
        Ok(doc) => doc,
        Err(err) => {
            eprintln!("pynmapper-viewer: could not load {}: {err}", path.display());
            return ExitCode::from(1);
        }
    };
    if doc.version > CONTRACT_VERSION {
        eprintln!(
            "pynmapper-viewer: map contract version {} is newer than this viewer ({CONTRACT_VERSION})",
            doc.version
        );
    }
    println!("{}", doc.summary());

    // Take the screenshot early enough that the GPU readback finishes before we quit.
    let screenshot_frame = args
        .exit_after_frames
        .map_or(30, |n| n.saturating_sub(30))
        .max(2);

    App::new()
        .add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: "Pynmapper".into(),
                        ..default()
                    }),
                    ..default()
                })
                .set(RenderPlugin {
                    render_creation: render_settings().into(),
                    ..default()
                }),
        )
        .configure_sets(
            Update,
            (
                ViewerSet::Build,
                ViewerSet::Frame,
                ViewerSet::Controls,
                ViewerSet::Picking,
                ViewerSet::Draw,
            )
                .chain(),
        )
        .add_plugins((
            ScenePlugin,
            GraphPlugin,
            ControlsPlugin,
            PickingPlugin,
            UiPlugin,
        ))
        .insert_resource(MapDocument(doc))
        .insert_resource(SmokeTest {
            frames: 0,
            exit_after: args.exit_after_frames,
            screenshot: args.screenshot,
            screenshot_frame,
        })
        .insert_resource(DebugSelect {
            id: args.select,
            fly: args.fly,
        })
        .add_systems(Update, (shortcuts, debug_select).in_set(ViewerSet::Frame))
        .add_systems(Update, (update_title, smoke_test).in_set(ViewerSet::Draw))
        .run();
    ExitCode::SUCCESS
}

/// Bevy picks the GPU backend from `WGPU_BACKEND` when that variable is set,
/// and panics with "Unable to find a GPU" if the backend it names has no
/// usable adapter. The dev machine's Hyprland config exports `WGPU_BACKEND=gl`
/// and wgpu's OpenGL path finds nothing there, so check first: when the
/// requested backend yields no adapter, say so and let wgpu choose from all
/// of them (Vulkan on that machine).
fn render_settings() -> WgpuSettings {
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
            "pynmapper-viewer: WGPU_BACKEND={requested} is set but no GPU here supports that backend; \
             letting wgpu choose one instead"
        );
        settings.backends = Some(Backends::all());
    }
    settings
}

/// `onKeyDown` in app.js: the keys that need the graph, picker and camera at
/// once. Movement keys live in controls.rs. Escape closes the help overlay and
/// leaves the filter box first once those exist (step 7.3).
#[allow(clippy::too_many_arguments)] // a Bevy system lists what it reads
fn shortcuts(
    keys: Res<ButtonInput<KeyCode>>,
    ui: Res<UiInput>,
    graph: Res<Graph>,
    nodes: Query<(&Node, &Transform)>,
    camera: Single<&Transform, With<MainCamera>>,
    mut picker: ResMut<Picker>,
    mut controls: ResMut<FlyControls>,
    mut reset: MessageWriter<ResetView>,
) {
    const MODIFIERS: [KeyCode; 6] = [
        KeyCode::ControlLeft,
        KeyCode::ControlRight,
        KeyCode::AltLeft,
        KeyCode::AltRight,
        KeyCode::SuperLeft,
        KeyCode::SuperRight,
    ];
    if ui.keyboard || keys.any_pressed(MODIFIERS) {
        return;
    }
    if keys.just_pressed(KeyCode::Escape) {
        picker.select(None);
    }
    if keys.just_pressed(KeyCode::KeyR) {
        reset.write(ResetView { animate: true });
    }
    if keys.just_pressed(KeyCode::KeyF) {
        picking::fly_to_selected(&picker, &graph, &nodes, &mut controls, &camera);
    }
}

/// Apply `--select` / `--fly` the first time the map is drawn.
fn debug_select(
    graph: Res<Graph>,
    nodes: Query<(&Node, &Transform)>,
    camera: Single<&Transform, With<MainCamera>>,
    mut debug: ResMut<DebugSelect>,
    mut picker: ResMut<Picker>,
    mut controls: ResMut<FlyControls>,
) {
    if !graph.is_changed() || debug.id.is_none() {
        return;
    }
    let id = debug.id.take();
    if !graph.nodes.contains_key(id.as_deref().unwrap_or_default()) {
        warn!(
            "--select {}: no such host in the map",
            id.unwrap_or_default()
        );
        return;
    }
    picker.select(id);
    if debug.fly {
        picking::fly_to_selected(&picker, &graph, &nodes, &mut controls, &camera);
    }
}

/// `document.title = ...` in app.js.
fn update_title(graph: Res<Graph>, mut window: Single<&mut Window, With<PrimaryWindow>>) {
    if graph.is_changed() {
        window.title = format!("Pynmapper · {} hosts", graph.host_count);
    }
}

fn smoke_test(
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
