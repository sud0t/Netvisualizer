//! Netvisualizer: finds the hosts on a network and shows them as a 3D map in
//! its own window (Bevy).
//!
//! cli.rs is the command line; startup.rs what the window is started with
//! (log, GPU probe, smoke test). scan.rs runs a scan (the scanner crate) when
//! asked and hands the viewer a map after each stage; scanpanel.rs is the
//! card that starts one; map.rs is the map document (the JSON contract) and
//! layout.rs places the hosts in it. scene.rs sets up the camera, lights,
//! fog, floor and framing; graph.rs draws the hosts and links; controls.rs
//! flies the camera; picking.rs handles hover, clicks and the selection;
//! panel.rs is the sidebar; ui.rs draws the labels, HUD, legend and help;
//! theme.rs holds the colours; python.rs is the embedded Python and
//! vendors.rs the scan stage that runs in it. This file holds the wiring,
//! and the keys that need everything.
//!
//!     cargo run --features dev                                      # the window, no scan
//!     cargo run --features dev -- --scan --ports                    # scan the local subnet, then common ports
//!     cargo run --features dev -- --scan --no-window                # scan and print, no window
//!     cargo run --features dev -- --json tests/fixtures/demo.json   # a saved map
//!     cargo run --features dev -- --json tests/fixtures/demo.json --exit-after-frames 120 --screenshot /tmp/map.png

mod cli;
mod controls;
mod graph;
mod layout;
mod map;
mod panel;
mod picking;
mod python;
mod scan;
mod scanpanel;
mod scene;
mod startup;
mod theme;
mod ui;
mod vendors;

use std::process::ExitCode;

use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;
use bevy::render::RenderPlugin;
use bevy::window::PrimaryWindow;
use clap::Parser;

use crate::cli::Args;
use crate::controls::{ControlsPlugin, command_modifier_held};
use crate::graph::{Graph, GraphPlugin, MapDocument};
use crate::map::{CONTRACT_VERSION, MapDoc};
use crate::panel::{Panel, PanelPlugin};
use crate::picking::{Picker, PickingPlugin, SelectHost};
use crate::scan::{ScanPlan, ScanPlugin, StartScan};
use crate::scanpanel::ScanPanelPlugin;
use crate::scene::{ResetView, ScenePlugin};
use crate::startup::{DebugSelect, SmokeTest};
use crate::ui::{Help, Status, UiInput, UiPlugin};

/// The per-frame order of everything in `Update`. Take in what the scan
/// sent, build the graph, frame it, move the camera, pick under the new
/// camera, then draw the overlays, so a frame never mixes last frame's camera
/// with this frame's picks.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ViewerSet {
    Receive,
    Build,
    Frame,
    Controls,
    Picking,
    Draw,
}

fn main() -> ExitCode {
    let args = Args::parse();
    if args.list_interfaces {
        return cli::list_interfaces();
    }
    let scan = args.scan_settings();
    let mut status = Status::default();
    let mut doc = None;
    if let Some(path) = args.json.as_deref() {
        match MapDoc::load(path) {
            Ok(loaded) => {
                if loaded.version > CONTRACT_VERSION {
                    eprintln!(
                        "netvisualizer: map contract version {} is newer than this viewer ({CONTRACT_VERSION})",
                        loaded.version
                    );
                }
                println!("{}", loaded.summary());
                status.show_map(loaded.summary());
                doc = Some(loaded);
            }
            Err(err) => {
                eprintln!("netvisualizer: could not load {}: {err}", path.display());
                return ExitCode::from(1);
            }
        }
    } else if scan.is_some() {
        status = Status::message("starting the scan…");
    }

    if let Some(settings) = &scan {
        // Find out where we are before any window opens: a mistyped
        // interface should be an error message, not an empty map.
        let plan = match ScanPlan::new(settings) {
            Ok(plan) => plan,
            Err(err) => {
                eprintln!("netvisualizer: {err}");
                return ExitCode::from(2);
            }
        };
        if args.no_window {
            if args.verbose {
                println!("{}", python::describe());
            }
            return scan::run_headless(plan, doc);
        }
    }

    let mut app = viewer(args.verbose);
    app.insert_resource(MapDocument(doc))
        .insert_resource(status)
        .insert_resource(Help {
            open: args.help_overlay,
        })
        .insert_resource(SmokeTest::new(args.exit_after_frames, args.screenshot))
        .insert_resource(DebugSelect {
            id: args.select,
            fly: args.fly,
        })
        .add_systems(
            Update,
            (shortcuts, startup::debug_select).in_set(ViewerSet::Frame),
        )
        .add_systems(
            Update,
            (update_title, startup::smoke_test).in_set(ViewerSet::Draw),
        );
    if args.verbose {
        app.add_systems(Startup, startup::log_python);
    }
    if let Some(settings) = scan {
        // The same message the Scan button sends.
        app.insert_resource(settings);
        app.world_mut().write_message(StartScan);
    }
    app.run();
    ExitCode::SUCCESS
}

/// The window with every plugin of the viewer, and the order its systems
/// run in ([`ViewerSet`]); the resources the command line decides come after.
fn viewer(verbose: bool) -> App {
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
                render_creation: startup::render_settings().into(),
                ..default()
            })
            .set(startup::log_settings(verbose)),
    )
    .configure_sets(
        Update,
        (
            ViewerSet::Receive,
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
        ScanPlugin,
        ScanPanelPlugin,
    ));
    app
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

/// The window title: "Netvisualizer · N hosts" once a map has been drawn.
fn update_title(graph: Res<Graph>, mut window: Single<&mut Window, With<PrimaryWindow>>) {
    if graph.is_changed() && graph.drawn {
        window.title = format!("Netvisualizer · {} hosts", graph.host_count);
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
        assert_eq!(selections(&app), []);
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
