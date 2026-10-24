//! The UI over the 3D view, built with `bevy_lunex`: a second camera draws a
//! layer of 2D nodes (boxes, toggles, text) on top of the map, and lunex
//! lays the nodes out and tells `bevy_picking` what the pointer is over. The
//! look is a terminal's (theme.rs): one monospace face, phosphor green on
//! near-black boxes, scanlines over the window.
//!
//! The pieces: hud.rs (the status box and the legend), dock.rs (the Scan
//! block and its toggles), rail.rs (the host table and the details dump),
//! labels.rs (the names over the nodes), help.rs (the controls overlay),
//! tooltip.rs; and under them shape.rs (the one shader every box is painted
//! with), widgets.rs (text, buttons, toggles), `text_input.rs` (the fields)
//! and pick.rs (what the pointer is over). This file wires them up and keeps the [`UiInput`] flags that
//! tell the controls and the 3D picking when the UI owns the pointer or the
//! keyboard.

pub mod dock;
pub mod help;
pub mod hud;
pub mod labels;
pub mod pick;
pub mod rail;
pub mod shape;
pub mod text_input;
pub mod tooltip;
pub mod widgets;

use bevy::app::{HierarchyPropagatePlugin, Propagate};
use bevy::camera::Hdr;
use bevy::camera::visibility::RenderLayers;
use bevy::picking::hover::HoverMap;
use bevy::picking::pointer::PointerId;
use bevy::prelude::*;
use bevy::render::view::{DebandDither, Tonemapping};
use bevy::window::{PrimaryWindow, SystemCursorIcon};
use bevy_lunex::CursorIconQueue;
use bevy_lunex::prelude::*;

use crate::ViewerSet;
use crate::controls::FlyControls;
use crate::map::Role;
use crate::picking::Picker;
use crate::theme;
use crate::ui::text_input::Focus;
use crate::ui::widgets::Fonts;

/// The render layer of the UI: its camera sees only this layer, so the 3D
/// camera's gizmos (the grid, the links) stay out of it, and the 3D camera
/// never draws the UI's sprites.
pub const UI_LAYER: usize = 1;
/// The panels' depth: well above the host labels (1), under the help (100),
/// the tooltips (200) and the scanlines over everything (300).
pub const PANEL_Z: f32 = 10.0;
pub const LABELS_Z: f32 = 1.0;
pub const OVERLAY_Z: f32 = 100.0;
pub const TOOLTIP_Z: f32 = 200.0;
pub const SCANLINES_Z: f32 = 300.0;

/// What the UI wants this frame, so the 3D controls and picking stay out of
/// its way.
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct UiInput {
    /// The pointer is over a panel, or a press that began on one still holds.
    pub pointer: bool,
    /// A text field has focus: keys type instead of flying.
    pub keyboard: bool,
}

/// The HUD's status line.
#[derive(Resource, Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub text: String,
    /// Drawn in the error colour.
    pub error: bool,
}

impl Default for Status {
    fn default() -> Self {
        Status::message("no scan yet · press Scan, or start with --scan")
    }
}

impl Status {
    /// A plain message, shown until the first map arrives.
    pub fn message(text: &str) -> Status {
        Status {
            text: text.to_string(),
            error: false,
        }
    }

    pub fn show_message(&mut self, text: &str) {
        *self = Status::message(text);
    }

    /// A new map arrived: show its summary.
    pub fn show_map(&mut self, summary: String) {
        self.text = summary;
        self.error = false;
    }

    pub fn show_error(&mut self, message: String) {
        self.text = message;
        self.error = true;
    }
}

/// The help overlay with the controls (`?`, `h`, the HUD button; Escape or a
/// click on the backdrop closes it).
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Help {
    pub open: bool,
}

/// The lunex root every panel hangs from.
#[derive(Resource, Debug, Clone, Copy)]
pub struct UiRoot {
    pub entity: Entity,
}

/// `--no-ui`: the whole interface stays hidden (and, hidden, takes no clicks).
#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct UiHidden(pub bool);

/// The camera that draws the UI layer.
#[derive(Component)]
pub struct UiCamera;

/// Whether the left button went down on the UI, so a drag that starts on a
/// panel and leaves it never reaches the 3D view.
#[derive(Resource, Debug, Default)]
struct PointerState {
    pressed_on_ui: bool,
}

/// The entity in whose name the pointing hand is requested while a node
/// is under the pointer (lunex's cursor queue wants a requester).
#[derive(Component)]
struct HoverCursor;

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            UiLunexPlugins,
            HierarchyPropagatePlugin::<RenderLayers>::new(PostUpdate),
            pick::UiPickPlugin,
            shape::ShapePlugin,
            widgets::WidgetsPlugin,
            text_input::TextInputPlugin,
            tooltip::TooltipPlugin,
            hud::HudPlugin,
            dock::DockPlugin,
            rail::RailPlugin,
            labels::LabelsPlugin,
            help::HelpPlugin,
        ))
        .init_resource::<UiInput>()
        .init_resource::<Status>()
        .init_resource::<Help>()
        .init_resource::<PointerState>()
        // Before every panel's own Startup system: they spawn under the root
        // with these fonts.
        .add_systems(PreStartup, (load_fonts, spawn_camera_and_root).chain())
        .add_systems(Startup, spawn_scanlines)
        .add_systems(Update, read_ui_input.in_set(ViewerSet::Receive))
        .add_systems(Update, pointer_cursor.in_set(ViewerSet::Draw));
    }
}

/// The font ships inside the binary, so the viewer runs from any directory.
fn load_fonts(mut commands: Commands, mut fonts: ResMut<Assets<Font>>) {
    let mono = fonts.add(Font::from_bytes(
        include_bytes!("../../assets/fonts/JetBrainsMono.ttf").to_vec(),
    ));
    commands.insert_resource(Fonts { mono });
}

/// The UI camera draws after the 3D camera, over its picture, and only
/// what is on the UI layer; the root follows the camera's viewport.
///
/// Cameras that render to the same window share a render texture only
/// when they ask for the same kind: this one is HDR like the 3D camera
/// (scene.rs), with the same tone mapping, so it paints straight onto the
/// 3D frame. A camera of another kind gets a texture of its own, which
/// "no clear colour" then never clears: every frame's UI would pile up on
/// the last frame's until the panels went opaque.
fn spawn_camera_and_root(mut commands: Commands, hidden: Option<Res<UiHidden>>) {
    commands.spawn((
        Name::new("ui camera"),
        UiCamera,
        Camera2d,
        UiSourceCamera::<0>,
        Camera {
            order: 1,
            clear_color: ClearColorConfig::None,
            ..default()
        },
        Hdr,
        Tonemapping::None,
        DebandDither::Disabled,
        RenderLayers::layer(UI_LAYER),
        // Nodes stack up the z axis from 0; the camera looks down at them.
        Transform::from_xyz(0.0, 0.0, 1000.0),
    ));
    let root = commands
        .spawn((
            Name::new("ui root"),
            UiLayoutRoot::new_2d(),
            UiFetchFromCamera::<0>,
            // Every node under the root is on the UI layer.
            Propagate(RenderLayers::layer(UI_LAYER)),
            // The root covers the window; it must not count as "the UI".
            Pickable::IGNORE,
            if hidden.is_some_and(|h| h.0) {
                Visibility::Hidden
            } else {
                Visibility::Inherited
            },
        ))
        .id();
    commands.spawn((Name::new("hover cursor"), HoverCursor));
    commands.insert_resource(UiRoot { entity: root });
}

/// The scanlines: one box over the whole window, above everything else,
/// that takes no clicks (see shape.wgsl).
fn spawn_scanlines(
    mut commands: Commands,
    root: Res<UiRoot>,
    fonts: Res<Fonts>,
    shapes: Res<shape::ShapeAssets>,
    mut materials: ResMut<Assets<shape::ShapeMaterial>>,
) {
    let mut kit = widgets::Kit {
        fonts: &fonts,
        shapes: &shapes,
        materials: &mut materials,
    };
    commands.entity(root.entity).with_children(|ui| {
        ui.spawn((
            Name::new("scanlines"),
            UiLayout::window().full().pack(),
            UiDepth::Set(SCANLINES_Z),
            Pickable::IGNORE,
        ))
        .with_children(|n| {
            n.spawn(kit.skin(shape::Look::scanlines(theme::SCANLINE_STRENGTH), 0.0));
        });
    });
}

/// A point in the window (logical pixels, y down from the top left) in the
/// root's space (origin at the centre, y up), for `size` the root's size.
pub fn root_point(viewport: Vec2, size: Vec2) -> Vec2 {
    Vec2::new(viewport.x - size.x / 2.0, size.y / 2.0 - viewport.y)
}

/// The role colours of the host list's dots and the legend's swatches
/// (unknown roles and down hosts use the host colour there; only the links
/// and labels in the 3D view turn grey).
pub fn role_color(role: Role) -> Color {
    match role {
        Role::Scanner => theme::ROLE_SCANNER,
        Role::Gateway => theme::ROLE_GATEWAY,
        Role::Router => theme::ROLE_ROUTER,
        Role::Host => theme::ROLE_HOST,
    }
}

/// The fly speed in the HUD: one decimal below 10 u/s, whole numbers above.
pub fn speed_text(speed: f32) -> String {
    if speed < 10.0 {
        format!("{speed:.1} u/s")
    } else {
        format!("{} u/s", speed.round())
    }
}

/// First thing each frame: is the pointer over a lunex node (`bevy_picking`
/// has already run), and does a field hold the keyboard?
#[allow(clippy::too_many_arguments)] // a Bevy system lists what it reads
fn read_ui_input(
    hover: Res<HoverMap>,
    nodes: Query<(), With<Dimension>>,
    names: Query<&Name>,
    buttons: Res<ButtonInput<MouseButton>>,
    focus: Res<Focus>,
    mut state: ResMut<PointerState>,
    mut ui: ResMut<UiInput>,
) {
    let over_ui = hover
        .get(&PointerId::Mouse)
        .is_some_and(|hits| hits.keys().any(|entity| nodes.get(*entity).is_ok()));
    if buttons.just_pressed(MouseButton::Left) {
        state.pressed_on_ui = over_ui;
    }
    if !buttons.pressed(MouseButton::Left) {
        state.pressed_on_ui = false;
    }
    let next = UiInput {
        pointer: over_ui || state.pressed_on_ui,
        keyboard: focus.field.is_some(),
    };
    if *ui != next {
        // What is under the pointer, by name, for `RUST_LOG=netvisualizer=debug`.
        let under: Vec<&str> = hover
            .get(&PointerId::Mouse)
            .into_iter()
            .flat_map(|hits| hits.keys())
            .map(|e| names.get(*e).map_or("?", Name::as_str))
            .collect();
        debug!("ui input: {next:?} (under the pointer: {under:?})");
        *ui = next;
    }
}

/// A pointing hand while a host is under the pointer, through lunex's
/// cursor queue so it and the widgets' own requests never fight.
fn pointer_cursor(
    picker: Res<Picker>,
    controls: Res<FlyControls>,
    ui: Res<UiInput>,
    window: Single<Entity, With<PrimaryWindow>>,
    requester: Single<Entity, With<HoverCursor>>,
    mut queue: ResMut<CursorIconQueue>,
    mut asked: Local<bool>,
) {
    let want = picker.hovered.is_some() && !controls.looking && !ui.pointer;
    if want == *asked {
        return;
    }
    *asked = want;
    if want {
        queue.request_cursor(
            PointerId::Mouse,
            Some(*window),
            *requester,
            SystemCursorIcon::Pointer,
            1,
        );
    } else {
        queue.cancel_cursor(PointerId::Mouse, &requester);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speed_text_has_one_decimal_below_ten() {
        assert_eq!(speed_text(18.0), "18 u/s");
        assert_eq!(speed_text(18.5), "19 u/s");
        assert_eq!(speed_text(9.96), "10.0 u/s");
        assert_eq!(speed_text(1.0), "1.0 u/s");
        assert_eq!(speed_text(500.0), "500 u/s");
    }

    #[test]
    fn the_status_line_shows_a_message_a_map_or_an_error() {
        let mut status = Status::default();
        assert_eq!(
            status.text,
            "no scan yet · press Scan, or start with --scan"
        );
        status.show_error("scan failed: boom".into());
        assert!(status.error);
        status.show_map("ping_sweep · done · 3 hosts · rev 4".into());
        assert_eq!(status.text, "ping_sweep · done · 3 hosts · rev 4");
        assert!(!status.error, "a new map clears the error");
    }

    #[test]
    fn root_points_have_the_origin_in_the_middle_and_y_up() {
        let size = Vec2::new(1280.0, 720.0);
        assert_eq!(root_point(Vec2::ZERO, size), Vec2::new(-640.0, 360.0));
        assert_eq!(root_point(size / 2.0, size), Vec2::ZERO);
        assert_eq!(
            root_point(Vec2::new(1280.0, 720.0), size),
            Vec2::new(640.0, -360.0)
        );
    }
}
