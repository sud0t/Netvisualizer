//! Native viewer for Netvisualizer maps: the JSON that `python -m netvisualizer`
//! serves or saves, drawn in a Bevy window. scene.rs sets up the camera,
//! lights, fog, floor and framing; graph.rs draws the hosts and links;
//! controls.rs flies the camera; picking.rs handles hover, clicks and the
//! selection; panel.rs is the sidebar; ui.rs draws the labels, HUD, legend
//! and help; fetch.rs follows a live map server; map.rs is the JSON contract
//! and theme.rs the colours. This file holds the arguments, the wiring, and
//! the keys that need everything.
//!
//!     cargo run --features dev                                   # follow http://127.0.0.1:8765
//!     cargo run --features dev -- http://10.0.0.5:8765           # another map server
//!     cargo run --features dev -- --json tests/fixtures/demo.json
//!     cargo run --features dev -- --json scan.json --exit-after-frames 120 --screenshot /tmp/map.png

mod controls;
mod fetch;
mod graph;
mod map;
mod panel;
mod picking;
mod scene;
mod theme;
mod ui;

use std::path::PathBuf;
use std::process::ExitCode;

use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;
use bevy::render::RenderPlugin;
use bevy::render::settings::{Backends, WgpuSettings};
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use bevy::tasks::block_on;
use bevy::window::PrimaryWindow;
use clap::Parser;

use crate::controls::{ControlsPlugin, command_modifier_held};
use crate::fetch::{FetchPlugin, Fetcher};
use crate::graph::{Graph, GraphPlugin, MapDocument};
use crate::map::{CONTRACT_VERSION, MapDoc};
use crate::panel::{Panel, PanelPlugin};
use crate::picking::{Picker, PickingPlugin, SelectHost};
use crate::scene::{ResetView, ScenePlugin};
use crate::ui::{Help, Status, UiInput, UiPlugin};

#[derive(Parser, Debug)]
#[command(
    name = "netvisualizer-viewer",
    version,
    about = "Native viewer for Netvisualizer network maps"
)]
struct Args {
    /// Map server to follow: polls /api/health every 2 s and downloads
    /// /api/map when the revision changes, so the map redraws as a scan runs.
    #[arg(default_value = "http://127.0.0.1:8765")]
    url: String,

    /// Draw a map saved with `python -m netvisualizer --save FILE` instead of a server.
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

    /// Open the help overlay, as `?` would.
    #[arg(long, help_heading = "Debugging")]
    help_overlay: bool,
}

/// The per-frame order of everything in `Update`. Take in what the server
/// sent, build the graph, frame it, move the camera, pick under the new
/// camera, then draw the overlays, so a frame never mixes last frame's camera
/// with this frame's picks.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ViewerSet {
    Fetch,
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

    let mut status = Status::default();
    let (doc, fetcher) = match args.json.as_deref() {
        Some(path) => match MapDoc::load(path) {
            Ok(doc) => {
                if doc.version > CONTRACT_VERSION {
                    eprintln!(
                        "netvisualizer-viewer: map contract version {} is newer than this viewer ({CONTRACT_VERSION})",
                        doc.version
                    );
                }
                println!("{}", doc.summary());
                status.show_map(doc.summary());
                (Some(doc), None)
            }
            Err(err) => {
                eprintln!("netvisualizer-viewer: could not load {}: {err}", path.display());
                return ExitCode::from(1);
            }
        },
        None => {
            println!("following {}", fetch::base_url(&args.url));
            (None, Some(Fetcher::spawn(&args.url)))
        }
    };

    // Take the screenshot early enough that the GPU readback finishes before we quit.
    let screenshot_frame = args
        .exit_after_frames
        .map_or(30, |n| n.saturating_sub(30))
        .max(2);

    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: "Netvisualizer".into(),
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
            ViewerSet::Fetch,
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
        PanelPlugin,
        UiPlugin,
    ))
    .insert_resource(MapDocument(doc))
    .insert_resource(status)
    .insert_resource(Help {
        open: args.help_overlay,
    })
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
    .add_systems(Update, (update_title, smoke_test).in_set(ViewerSet::Draw));
    if let Some(fetcher) = fetcher {
        app.insert_resource(fetcher).add_plugins(FetchPlugin);
    }
    app.run();
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
            "netvisualizer-viewer: WGPU_BACKEND={requested} is set but no GPU here supports that backend; \
             letting wgpu choose one instead"
        );
        settings.backends = Some(Backends::all());
    }
    settings
}

/// The keys that need the picker, panel and help at once. Movement keys live
/// in controls.rs. These go by the character a key types (its logical key),
/// so `?` and `/` work on any layout, and they are read from the key events
/// themselves: a held key does not repeat.
#[allow(clippy::too_many_arguments)] // a Bevy system lists what it reads
fn shortcuts(
    mut key_events: MessageReader<KeyboardInput>,
    keys: Res<ButtonInput<KeyCode>>,
    ui: Res<UiInput>,
    picker: Res<Picker>,
    mut help: ResMut<Help>,
    mut panel: ResMut<Panel>,
    mut select: MessageWriter<SelectHost>,
    mut reset: MessageWriter<ResetView>,
) {
    for event in key_events.read() {
        if event.state != ButtonState::Pressed || event.repeat || command_modifier_held(&keys) {
            continue;
        }
        if event.logical_key == Key::Escape {
            if help.open {
                help.open = false;
            } else if !ui.keyboard {
                select.write(SelectHost {
                    id: None,
                    fly: false,
                });
            } // else egui has already taken the focus out of the filter box
            continue;
        }
        let Key::Character(typed) = &event.logical_key else {
            continue;
        };
        if ui.keyboard {
            continue; // typing into the filter box
        }
        match typed.as_str() {
            "?" | "h" | "H" => help.open = !help.open,
            "r" | "R" => {
                reset.write(ResetView { animate: true });
            }
            "f" | "F" => {
                if let Some(id) = &picker.selected_id {
                    select.write(SelectHost {
                        id: Some(id.clone()),
                        fly: true,
                    });
                }
            }
            "/" => panel.focus_filter(),
            _ => {}
        }
    }
}

/// Apply `--select` / `--fly` the first time a map is drawn.
fn debug_select(
    graph: Res<Graph>,
    mut debug: ResMut<DebugSelect>,
    mut select: MessageWriter<SelectHost>,
) {
    if !graph.is_changed() || !graph.drawn || debug.id.is_none() {
        return;
    }
    let id = debug.id.take().unwrap_or_default();
    if !graph.nodes.contains_key(&id) {
        warn!("--select {id}: no such host in the map");
        return;
    }
    select.write(SelectHost {
        id: Some(id),
        fly: debug.fly,
    });
}

/// The window title: "Netvisualizer · N hosts" once a map has been drawn.
fn update_title(graph: Res<Graph>, mut window: Single<&mut Window, With<PrimaryWindow>>) {
    if graph.is_changed() && graph.drawn {
        window.title = format!("Netvisualizer · {} hosts", graph.host_count);
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

#[cfg(test)]
mod tests {
    use bevy::ecs::message::Messages;
    use bevy::input::keyboard::NativeKey;

    use super::*;

    /// Just the shortcut system and what it touches, driven by key events.
    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<KeyboardInput>()
            .add_message::<SelectHost>()
            .add_message::<ResetView>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<UiInput>()
            .init_resource::<Picker>()
            .init_resource::<Help>()
            .init_resource::<Panel>()
            .add_systems(Update, shortcuts);
        app
    }

    fn key(app: &mut App, logical_key: Key) {
        app.world_mut().write_message(KeyboardInput {
            key_code: KeyCode::Unidentified(bevy::input::keyboard::NativeKeyCode::Unidentified),
            logical_key,
            state: ButtonState::Pressed,
            text: None,
            repeat: false,
            window: Entity::PLACEHOLDER,
        });
        app.update();
    }

    fn typed(app: &mut App, c: &str) {
        key(app, Key::Character(c.into()));
    }

    fn selections(app: &App) -> Vec<SelectHost> {
        let messages = app.world().resource::<Messages<SelectHost>>();
        messages.get_cursor().read(messages).cloned().collect()
    }

    fn resets(app: &App) -> usize {
        let messages = app.world().resource::<Messages<ResetView>>();
        messages.get_cursor().read(messages).count()
    }

    #[test]
    fn question_mark_and_h_toggle_help_and_escape_closes_it_without_deselecting() {
        let mut app = app();
        typed(&mut app, "?");
        assert!(app.world().resource::<Help>().open);
        typed(&mut app, "h");
        assert!(!app.world().resource::<Help>().open);
        typed(&mut app, "H");
        assert!(app.world().resource::<Help>().open);
        key(&mut app, Key::Escape);
        assert!(!app.world().resource::<Help>().open);
        assert!(
            selections(&app).is_empty(),
            "the first Escape only closed help"
        );
        key(&mut app, Key::Escape);
        assert_eq!(
            selections(&app),
            [SelectHost {
                id: None,
                fly: false
            }]
        );
    }

    #[test]
    fn r_resets_and_f_flies_to_the_selection_if_there_is_one() {
        let mut app = app();
        typed(&mut app, "f");
        assert!(
            selections(&app).is_empty(),
            "nothing selected, nothing to fly to"
        );
        app.world_mut().resource_mut::<Picker>().selected_id = Some("10.0.0.1".into());
        typed(&mut app, "F");
        assert_eq!(
            selections(&app),
            [SelectHost {
                id: Some("10.0.0.1".into()),
                fly: true
            }]
        );
        typed(&mut app, "r");
        assert_eq!(resets(&app), 1);
    }

    #[test]
    fn slash_opens_the_sidebar_and_focuses_the_filter() {
        let mut app = app();
        app.world_mut().resource_mut::<Panel>().collapsed = true;
        typed(&mut app, "/");
        let panel = app.world().resource::<Panel>();
        assert!(!panel.collapsed && panel.focus_filter);
    }

    #[test]
    fn typing_in_the_filter_box_triggers_nothing() {
        let mut app = app();
        app.world_mut().resource_mut::<UiInput>().keyboard = true;
        app.world_mut().resource_mut::<Picker>().selected_id = Some("10.0.0.1".into());
        for c in ["?", "r", "f", "/", "h"] {
            typed(&mut app, c);
        }
        key(&mut app, Key::Escape); // egui itself leaves the box
        assert!(!app.world().resource::<Help>().open);
        assert!(selections(&app).is_empty());
        assert_eq!(resets(&app), 0);
        assert!(!app.world().resource::<Panel>().focus_filter);
    }

    #[test]
    fn modifiers_repeats_and_releases_are_ignored() {
        let mut app = app();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::ControlLeft);
        typed(&mut app, "r"); // Ctrl+R
        assert_eq!(resets(&app), 0);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release_all();

        for (state, repeat) in [(ButtonState::Released, false), (ButtonState::Pressed, true)] {
            app.world_mut().write_message(KeyboardInput {
                key_code: KeyCode::Unidentified(bevy::input::keyboard::NativeKeyCode::Unidentified),
                logical_key: Key::Character("?".into()),
                state,
                text: None,
                repeat,
                window: Entity::PLACEHOLDER,
            });
            app.update();
        }
        assert!(!app.world().resource::<Help>().open);
        // Keys that type nothing (F5, media keys) are not shortcuts either.
        key(&mut app, Key::Unidentified(NativeKey::Unidentified));
        assert!(!app.world().resource::<Help>().open);
    }
}
