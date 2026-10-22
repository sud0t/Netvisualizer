//! Netvisualizer: finds the hosts on a network and shows them as a 3D map in
//! its own window (Bevy).
//!
//! scan.rs runs a scan (the scanner crate) when asked and hands the viewer a
//! map after each stage; map.rs is the map document (the JSON contract) and
//! layout.rs places the hosts in it. scene.rs sets up the camera, lights,
//! fog, floor and framing; graph.rs draws the hosts and links; controls.rs
//! flies the camera; picking.rs handles hover, clicks and the selection;
//! panel.rs is the sidebar; ui.rs draws the labels, HUD, legend and help;
//! theme.rs holds the colours; python.rs is the embedded Python and
//! vendors.rs the scan stage that runs in it. This file holds the
//! arguments, the wiring, and the keys that need everything.
//!
//!     cargo run --features dev                                      # the window, no scan
//!     cargo run --features dev -- --scan --ports                    # scan the local subnet, then common ports
//!     cargo run --features dev -- --scan --no-window                # scan and print, no window
//!     cargo run --features dev -- --json tests/fixtures/demo.json   # a saved map
//!     cargo run --features dev -- --json tests/fixtures/demo.json --exit-after-frames 120 --screenshot /tmp/map.png

mod controls;
mod graph;
mod layout;
mod map;
mod panel;
mod picking;
mod python;
mod scan;
mod scene;
mod theme;
mod ui;
mod vendors;

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::log::{DEFAULT_FILTER, Level, LogPlugin};
use bevy::prelude::*;
use bevy::render::RenderPlugin;
use bevy::render::settings::{Backends, WgpuSettings};
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use bevy::tasks::block_on;
use bevy::window::PrimaryWindow;
use clap::Parser;
use scanner::Ipv4Net;
use scanner::local;
use scanner::ports::parse_port_list;

use crate::controls::{ControlsPlugin, command_modifier_held};
use crate::graph::{Graph, GraphPlugin, MapDocument};
use crate::map::{CONTRACT_VERSION, MapDoc};
use crate::panel::{Panel, PanelPlugin};
use crate::picking::{Picker, PickingPlugin, SelectHost};
use crate::scan::{ScanPlan, ScanPlugin, ScanSettings, StartScan};
use crate::scene::{ResetView, ScenePlugin};
use crate::ui::{Help, Status, UiInput, UiPlugin};

/// The biggest sweep allowed: a /16. It takes a few minutes; anything bigger
/// is a typo.
const MAX_SWEEP_ADDRESSES: u64 = 65_536;

#[derive(Parser, Debug)]
#[command(
    name = "netvisualizer",
    version,
    about = "Find the hosts on a network and show them as a 3D map in its own window.",
    after_help = "Examples:\n  \
        netvisualizer --scan                          scan the subnet of the default interface\n  \
        netvisualizer --scan 10.0.0.0/24 --ports      another network, then common TCP ports\n  \
        netvisualizer --scan --no-window --save s.json   scan without a window, keep the map\n  \
        netvisualizer --json s.json                   show a saved map"
)]
struct Args {
    /// Scan a network: CIDR like 192.168.1.0/24, or without one the subnet of
    /// the interface with the default route. Nothing touches the network
    /// without this.
    #[arg(long, value_name = "CIDR", num_args = 0..=1, value_parser = parse_network, help_heading = "Scanning")]
    scan: Option<Option<Ipv4Net>>,

    /// Scan from this interface (and its subnet, unless --scan names a network).
    #[arg(
        short,
        long,
        value_name = "IFACE",
        requires = "scan",
        help_heading = "Scanning"
    )]
    interface: Option<String>,

    /// After the sweep, try TCP connections to these ports on every host that
    /// answered: a list like 22,80,8000-8010, or "common" (the default).
    #[arg(long, value_name = "PORTS", num_args = 0..=1, value_parser = parse_ports, requires = "scan", help_heading = "Scanning")]
    ports: Option<Option<PortList>>,

    /// Skip the reverse DNS lookups.
    #[arg(long, requires = "scan", help_heading = "Scanning")]
    no_names: bool,

    /// Seconds to wait for each ping or connection.
    #[arg(long, value_name = "SECONDS", default_value_t = 1.0, value_parser = parse_seconds, help_heading = "Scanning")]
    timeout: f64,

    /// Pings or connections in flight at once.
    #[arg(
        long,
        value_name = "N",
        default_value_t = 254,
        help_heading = "Scanning"
    )]
    concurrency: usize,

    /// Show this machine's networks and exit.
    #[arg(long, exclusive = true, help_heading = "Scanning")]
    list_interfaces: bool,

    /// Write the finished map as JSON (show it again with --json).
    #[arg(long, value_name = "FILE", requires = "scan", help_heading = "Output")]
    save: Option<PathBuf>,

    /// Scan without opening a window: print what is found (pairs with --save).
    #[arg(long, requires = "scan", help_heading = "Output")]
    no_window: bool,

    /// Show every step, Bevy's included, and which Python is embedded.
    #[arg(short, long, help_heading = "Output")]
    verbose: bool,

    /// Draw a map saved with --save instead of scanning.
    #[arg(long, value_name = "FILE", conflicts_with = "scan")]
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

impl Args {
    /// The scan the command line asks for, if it asks for one.
    fn scan_settings(&self) -> Option<ScanSettings> {
        let network = *self.scan.as_ref()?;
        let ports = match &self.ports {
            None => Vec::new(),
            Some(None) => scanner::ports::COMMON_TCP_PORTS.to_vec(),
            Some(Some(PortList(ports))) => ports.clone(),
        };
        Some(ScanSettings {
            network,
            interface: self.interface.clone(),
            ports,
            names: !self.no_names,
            timeout: Duration::from_secs_f64(self.timeout),
            concurrency: self.concurrency,
            save: self.save.clone(),
        })
    }
}

/// `--scan`'s value: a network like 192.168.1.0/24 (host bits are dropped,
/// so 192.168.1.143/24 means the same) or a single address. A /16 at most.
fn parse_network(text: &str) -> Result<Ipv4Net, String> {
    let text = text.trim();
    let invalid = || format!("not an IPv4 network or address: {text:?}");
    let network = if text.contains('/') {
        text.parse::<Ipv4Net>().map_err(|_| invalid())?
    } else {
        Ipv4Net::from(text.parse::<std::net::Ipv4Addr>().map_err(|_| invalid())?)
    }
    .trunc();
    let addresses = 1u64 << (32 - network.prefix_len());
    if addresses > MAX_SWEEP_ADDRESSES {
        return Err(format!(
            "{network} has {addresses} addresses; a /16 is the largest sweep allowed"
        ));
    }
    Ok(network)
}

/// `--ports`' value, read by [`parse_port_list`]. (A plain `Vec` would make
/// clap expect several values.)
#[derive(Debug, Clone, PartialEq, Eq)]
struct PortList(Vec<u16>);

fn parse_ports(text: &str) -> Result<PortList, String> {
    parse_port_list(text).map(PortList)
}

/// `--timeout`'s value: seconds, more than 0, at most an hour.
fn parse_seconds(text: &str) -> Result<f64, String> {
    match text.trim().parse::<f64>() {
        Ok(seconds) if seconds > 0.0 && seconds <= 3600.0 => Ok(seconds),
        _ => Err(format!(
            "not a number of seconds between 0 and 3600: {text:?}"
        )),
    }
}

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
    if args.list_interfaces {
        return list_interfaces();
    }
    let scan = args.scan_settings();
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
            return scan::run_headless(plan);
        }
    }

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
            })
            .set(log_settings(args.verbose)),
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
    if args.verbose {
        app.add_systems(Startup, log_python);
    }
    if let Some(settings) = scan {
        // The same message the Scan button will send.
        app.insert_resource(settings);
        app.world_mut().write_message(StartScan);
    }
    app.run();
    ExitCode::SUCCESS
}

/// `-v`: which Python the program embeds, and what it can import.
fn log_python() {
    info!("{}", python::describe());
}

/// `--list-interfaces`: this machine's networks, one per line.
fn list_interfaces() -> ExitCode {
    match local::local_networks() {
        Ok(networks) if !networks.is_empty() => {
            for n in networks {
                let gateway = n.gateway.map_or("-".to_string(), |g| g.to_string());
                println!(
                    "{:<12} {:<16} {:<20} gateway {gateway}",
                    n.interface,
                    n.ip.to_string(),
                    n.network.to_string()
                );
            }
            ExitCode::SUCCESS
        }
        Ok(_) => {
            eprintln!("netvisualizer: no IPv4 networks found");
            ExitCode::FAILURE
        }
        Err(err) => {
            eprintln!("netvisualizer: could not list the network interfaces: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Bevy logs its GPU set-up at "info", which is noise unless asked for with
/// `-v`; the program's own messages (what the scan finds) stay at "info".
/// A `RUST_LOG` variable still overrides both.
fn log_settings(verbose: bool) -> LogPlugin {
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
            "netvisualizer: WGPU_BACKEND={requested} is set but no GPU here supports that backend; \
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

    // ----- the command line -----

    fn parse(args: &[&str]) -> Result<Args, clap::Error> {
        Args::try_parse_from(std::iter::once("netvisualizer").chain(args.iter().copied()))
    }

    fn settings(args: &[&str]) -> ScanSettings {
        parse(args).unwrap().scan_settings().unwrap()
    }

    #[test]
    fn without_scan_nothing_is_scanned() {
        assert!(parse(&[]).unwrap().scan_settings().is_none());
        assert!(
            parse(&["--json", "x.json"])
                .unwrap()
                .scan_settings()
                .is_none()
        );
    }

    #[test]
    fn scan_takes_an_optional_network_and_the_scan_options() {
        assert_eq!(settings(&["--scan"]), ScanSettings::default());

        let s = settings(&["--scan", "192.168.1.143/24", "--ports", "--no-names"]);
        assert_eq!(s.network, Some("192.168.1.0/24".parse().unwrap()));
        assert_eq!(s.ports, scanner::ports::COMMON_TCP_PORTS);
        assert!(!s.names);

        let s = settings(&[
            "--scan",
            "--ports",
            "22,80",
            "-i",
            "wlan0",
            "--timeout",
            "0.5",
            "--concurrency",
            "64",
            "--save",
            "s.json",
        ]);
        assert_eq!(s.network, None);
        assert_eq!(s.ports, [22, 80]);
        assert_eq!(s.interface.as_deref(), Some("wlan0"));
        assert_eq!(s.timeout, Duration::from_millis(500));
        assert_eq!(s.concurrency, 64);
        assert_eq!(s.save, Some(PathBuf::from("s.json")));
    }

    #[test]
    fn scan_options_need_scan_and_bad_values_are_refused() {
        for args in [
            &["--ports"][..],
            &["--no-names"],
            &["-i", "wlan0"],
            &["--save", "x.json"],
            &["--no-window"],
            &["--scan", "--json", "x.json"],
            &["--list-interfaces", "--scan"],
            &["--scan", "--timeout", "0"],
            &["--scan", "--ports", "http"],
            &["--scan", "10.0.0.0/8"],
        ] {
            assert!(parse(args).is_err(), "{args:?} should be refused");
        }
    }

    #[test]
    fn networks_are_ipv4_and_at_most_a_slash_16() {
        assert_eq!(
            parse_network("10.0.0.7"),
            Ok("10.0.0.7/32".parse().unwrap())
        );
        assert_eq!(
            parse_network(" 10.1.2.3/16 "),
            Ok("10.1.0.0/16".parse().unwrap())
        );
        assert_eq!(
            parse_network("10.0.0.0/15"),
            Err("10.0.0.0/15 has 131072 addresses; a /16 is the largest sweep allowed".to_string())
        );
        assert!(parse_network("fe80::/64").is_err());
        assert!(parse_network("lan").is_err());
    }
}
