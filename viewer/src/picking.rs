//! Mouse picking: hover highlight, click to select, double click to fly to.
//!
//! Every node's model fits inside a sphere of the node's radius (models.rs),
//! so every node is tested as that sphere: a little generous round a slim
//! rack, and cheap enough to run each frame while the pointer is over the
//! window. The selection is remembered by host id so it survives a graph
//! rebuild when a new map revision arrives.

use bevy::color::Alpha;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use crate::ViewerSet;
use crate::controls::FlyControls;
use crate::graph::{Graph, Node, NodeState, set_state};
use crate::models::PartLooks;
use crate::scene::MainCamera;
use crate::theme;
use crate::ui::UiInput;

/// A press that moved further than this is a drag, not a click.
const CLICK_SLOP_PX: f32 = 5.0;
/// Pick radius / node radius: small nodes are easier to hit.
const PICK_SLACK: f32 = 1.2;
/// Selection ring radius / node radius.
const RING_SCALE: f32 = 1.45;
/// Two clicks closer than this in time (and within the slop) are a double click.
const DOUBLE_CLICK_SECS: f32 = 0.4;
const RING_SEGMENTS: u32 = 48;
const RING_WIDTH_PX: f32 = 2.5;

/// How far from a host a fly-to stops: bigger nodes, more room.
pub fn fly_distance(radius: f32) -> f32 {
    4.0 + radius * 6.0
}

/// A selection from anywhere but the 3D view: the host list, the details
/// card's buttons, F and Escape, `--select`. Applied in the Picking set, so a
/// request written while the UI is drawn lands next frame.
#[derive(Message, Debug, Clone, PartialEq, Eq)]
pub struct SelectHost {
    /// The host to select; `None` deselects. An id not in the map deselects too.
    pub id: Option<String>,
    /// Also fly to the host, as a double click would.
    pub fly: bool,
}

#[derive(Resource, Debug, Default)]
pub struct Picker {
    /// Node under the pointer.
    pub hovered: Option<Entity>,
    /// Selected host id (survives rebuilds; the entity does not).
    pub selected_id: Option<String>,
    /// Where the left button went down, to tell a click from a drag.
    down: Option<Vec2>,
    /// When and where the last click happened, for double clicks.
    last_click: Option<(f32, Vec2)>,
}

/// Outcome of a left-button release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Click {
    Single,
    Double,
}

impl Picker {
    pub fn selected_entity(&self, graph: &Graph) -> Option<Entity> {
        graph.nodes.get(self.selected_id.as_deref()?).copied()
    }

    /// Remember the host; the states and ring follow on the next frame.
    pub fn select(&mut self, id: Option<String>) {
        self.selected_id = id;
    }

    pub fn press(&mut self, at: Option<Vec2>) {
        self.down = at;
    }

    /// Tell a click from a drag, and two quick clicks from a double click.
    pub fn release(&mut self, at: Option<Vec2>, now: f32) -> Option<Click> {
        let (down, up) = (self.down.take()?, at?);
        if down.distance(up) > CLICK_SLOP_PX {
            return None;
        }
        let double = self.last_click.is_some_and(|(when, at)| {
            now - when <= DOUBLE_CLICK_SECS && at.distance(up) <= CLICK_SLOP_PX
        });
        self.last_click = if double { None } else { Some((now, up)) };
        Some(if double { Click::Double } else { Click::Single })
    }
}

/// The selection ring has its own gizmo group: drawn over everything and
/// thicker than the grid and links.
#[derive(Default, Reflect, GizmoConfigGroup)]
pub struct SelectionGizmos;

pub struct PickingPlugin;

impl Plugin for PickingPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Picker>()
            .add_message::<SelectHost>()
            .init_gizmo_group::<SelectionGizmos>()
            .add_systems(Startup, configure_ring_gizmos)
            .add_systems(
                Update,
                (
                    refresh_after_rebuild,
                    apply_requests,
                    hover,
                    clicks,
                    apply_states,
                    draw_ring,
                )
                    .chain()
                    .in_set(ViewerSet::Picking),
            );
    }
}

fn configure_ring_gizmos(mut store: ResMut<GizmoConfigStore>) {
    let (config, _) = store.config_mut::<SelectionGizmos>();
    config.line.width = RING_WIDTH_PX;
    config.depth_bias = -1.0; // visible through other nodes, like depthTest: false
}

/// The hit nearest to the ray origin, among nodes tested as spheres a little
/// bigger than the one their model fits in.
pub fn pick_at<'a>(
    cursor: Vec2,
    camera: &Camera,
    camera_transform: &Transform,
    nodes: impl IntoIterator<Item = (Entity, &'a Node, &'a Transform)>,
) -> Option<Entity> {
    let ray = camera
        .viewport_to_world(&GlobalTransform::from(*camera_transform), cursor)
        .ok()?;
    let mut best: Option<(f32, Entity)> = None;
    for (entity, node, transform) in nodes {
        let Some(distance) = ray_sphere_distance(
            ray.origin,
            *ray.direction,
            transform.translation,
            node.radius * PICK_SLACK,
        ) else {
            continue;
        };
        if best.is_none_or(|(d, _)| distance < d) {
            best = Some((distance, entity));
        }
    }
    best.map(|(_, entity)| entity)
}

/// Distance along the ray to a sphere:
/// the near hit, or the far one when the origin is inside; `None` for a miss
/// or a sphere entirely behind the ray.
pub fn ray_sphere_distance(
    origin: Vec3,
    direction: Vec3,
    center: Vec3,
    radius: f32,
) -> Option<f32> {
    let to_center = center - origin;
    let along = to_center.dot(direction);
    let off_axis_sq = to_center.length_squared() - along * along;
    let radius_sq = radius * radius;
    if off_axis_sq > radius_sq {
        return None;
    }
    let half_chord = (radius_sq - off_axis_sq).sqrt();
    let near = along - half_chord;
    let far = along + half_chord;
    if far < 0.0 {
        return None;
    }
    Some(if near < 0.0 { far } else { near })
}

/// After a graph rebuild the entities are new: forget the hover and drop a
/// selection whose host left the map.
fn refresh_after_rebuild(graph: Res<Graph>, mut picker: ResMut<Picker>) {
    if !graph.is_changed() {
        return;
    }
    picker.hovered = None;
    if picker
        .selected_id
        .as_deref()
        .is_some_and(|id| !graph.nodes.contains_key(id))
    {
        picker.select(None);
    }
}

/// Apply [`SelectHost`] requests in the order they were written.
fn apply_requests(
    mut requests: MessageReader<SelectHost>,
    graph: Res<Graph>,
    nodes: Query<(&Node, &Transform)>,
    camera: Single<&Transform, (With<MainCamera>, Without<Node>)>,
    mut picker: ResMut<Picker>,
    mut controls: ResMut<FlyControls>,
) {
    for request in requests.read() {
        let id = request.id.clone().filter(|id| graph.nodes.contains_key(id));
        picker.select(id);
        if request.fly {
            fly_to_selected(&picker, &graph, &nodes, &mut controls, &camera);
        }
    }
}

fn hover(
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &Transform), With<MainCamera>>,
    controls: Res<FlyControls>,
    ui: Res<UiInput>,
    nodes: Query<(Entity, &Node, &Transform)>,
    mut picker: ResMut<Picker>,
) {
    let (camera, camera_transform) = camera.into_inner();
    let hovered = match window.cursor_position() {
        Some(cursor) if !controls.looking && !ui.pointer => {
            pick_at(cursor, camera, camera_transform, nodes.iter())
        }
        _ => None,
    };
    if hovered != picker.hovered {
        picker.hovered = hovered;
    }
}

/// Left click selects what is under the pointer (or clears the selection);
/// a double click also flies there.
#[allow(clippy::too_many_arguments)] // a Bevy system lists what it reads
fn clicks(
    buttons: Res<ButtonInput<MouseButton>>,
    time: Res<Time>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &Transform), With<MainCamera>>,
    ui: Res<UiInput>,
    nodes: Query<(Entity, &Node, &Transform)>,
    mut picker: ResMut<Picker>,
    mut controls: ResMut<FlyControls>,
) {
    // While the right button holds the look, the cursor is hidden and
    // locked: a left click then is no click, as hover already treats it.
    let cursor = window.cursor_position().filter(|_| !controls.looking);
    if buttons.just_pressed(MouseButton::Left) {
        picker.press(if ui.pointer { None } else { cursor });
    }
    if !buttons.just_released(MouseButton::Left) {
        return;
    }
    let Some(click) = picker.release(cursor, time.elapsed_secs()) else {
        return;
    };
    let (camera, camera_transform) = camera.into_inner();
    let cursor = cursor.expect("release() returned a click, so the cursor is inside");
    let hit = pick_at(cursor, camera, camera_transform, nodes.iter());
    picker.hovered = hit;
    let id = hit
        .and_then(|entity| nodes.get(entity).ok())
        .map(|(_, node, _)| node.id.clone());
    picker.select(id);
    if click == Click::Double
        && let Some(entity) = hit
        && let Ok((_, node, transform)) = nodes.get(entity)
    {
        controls.fly_to(
            camera_transform,
            transform.translation,
            fly_distance(node.radius),
        );
    }
}

/// Hover and selection look: a brighter model, and for a selection a little
/// bigger (hover grows the label instead, ui/labels.rs). Each part swaps to
/// its material for the state, made up front (models.rs).
fn apply_states(
    picker: Res<Picker>,
    graph: Res<Graph>,
    mut nodes: Query<(Entity, &mut Node, &mut Transform, &Children)>,
    mut parts: Query<(&PartLooks, &mut MeshMaterial3d<StandardMaterial>)>,
) {
    if !picker.is_changed() && !graph.is_changed() {
        return;
    }
    let selected = picker.selected_entity(&graph);
    for (entity, mut node, mut transform, children) in &mut nodes {
        let state = if Some(entity) == selected {
            NodeState::Selected
        } else if Some(entity) == picker.hovered {
            NodeState::Hover
        } else {
            NodeState::None
        };
        if !set_state(&mut node, &mut transform, state) {
            continue;
        }
        for &part in children {
            if let Ok((looks, mut material)) = parts.get_mut(part) {
                material.0 = looks.get(state);
            }
        }
    }
}

/// A ring around the selected host that always faces the camera.
fn draw_ring(
    picker: Res<Picker>,
    graph: Res<Graph>,
    nodes: Query<(&Node, &Transform)>,
    camera: Single<&Transform, (With<MainCamera>, Without<Node>)>,
    mut gizmos: Gizmos<SelectionGizmos>,
) {
    let Some(entity) = picker.selected_entity(&graph) else {
        return;
    };
    let Ok((node, transform)) = nodes.get(entity) else {
        return;
    };
    gizmos
        .circle(
            Isometry3d::new(transform.translation, camera.rotation),
            node.radius * RING_SCALE,
            theme::SELECTION.with_alpha(0.9),
        )
        .resolution(RING_SEGMENTS);
}

/// Fly to the selected host: used by F and by the host list.
pub fn fly_to_selected(
    picker: &Picker,
    graph: &Graph,
    nodes: &Query<(&Node, &Transform)>,
    controls: &mut FlyControls,
    camera: &Transform,
) -> bool {
    let Some(entity) = picker.selected_entity(graph) else {
        return false;
    };
    let Ok((node, transform)) = nodes.get(entity) else {
        return false;
    };
    controls.fly_to(camera, transform.translation, fly_distance(node.radius));
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ray_sphere_returns_the_near_hit_or_the_far_one_from_inside() {
        let origin = Vec3::ZERO;
        let dir = Vec3::Z;
        assert_eq!(
            ray_sphere_distance(origin, dir, Vec3::new(0.0, 0.0, 10.0), 2.0),
            Some(8.0)
        );
        assert_eq!(
            ray_sphere_distance(origin, dir, Vec3::new(0.0, 0.0, 1.0), 2.0),
            Some(3.0)
        );
        assert_eq!(
            ray_sphere_distance(origin, dir, Vec3::new(0.0, 0.0, -10.0), 2.0),
            None
        );
        assert_eq!(
            ray_sphere_distance(origin, dir, Vec3::new(3.0, 0.0, 10.0), 2.0),
            None
        );
        let grazing = ray_sphere_distance(origin, dir, Vec3::new(2.0, 0.0, 10.0), 2.0).unwrap();
        assert!((grazing - 10.0).abs() < 1e-3);
    }

    #[test]
    fn a_click_must_stay_within_the_slop_and_two_quick_ones_are_a_double() {
        let mut picker = Picker::default();
        picker.press(Some(Vec2::new(100.0, 100.0)));
        assert_eq!(
            picker.release(Some(Vec2::new(120.0, 100.0)), 1.0),
            None,
            "a drag"
        );

        picker.press(Some(Vec2::new(100.0, 100.0)));
        assert_eq!(
            picker.release(Some(Vec2::new(102.0, 101.0)), 1.0),
            Some(Click::Single)
        );
        picker.press(Some(Vec2::new(101.0, 101.0)));
        assert_eq!(
            picker.release(Some(Vec2::new(101.0, 101.0)), 1.3),
            Some(Click::Double)
        );
        // The double click consumed the pair: a third click starts over.
        picker.press(Some(Vec2::new(101.0, 101.0)));
        assert_eq!(
            picker.release(Some(Vec2::new(101.0, 101.0)), 1.5),
            Some(Click::Single)
        );
        // Too slow, or too far away, is two singles.
        picker.press(Some(Vec2::new(101.0, 101.0)));
        assert_eq!(
            picker.release(Some(Vec2::new(101.0, 101.0)), 2.5),
            Some(Click::Single)
        );
        picker.press(Some(Vec2::new(300.0, 300.0)));
        assert_eq!(
            picker.release(Some(Vec2::new(300.0, 300.0)), 2.6),
            Some(Click::Single)
        );

        picker.press(None); // pressed with the pointer outside / over the UI
        assert_eq!(picker.release(Some(Vec2::ZERO), 3.0), None);
        picker.press(Some(Vec2::ZERO));
        assert_eq!(
            picker.release(None, 3.1),
            None,
            "released outside the window"
        );
    }

    #[test]
    fn fly_distance_grows_with_the_sphere() {
        assert!((fly_distance(0.6) - 7.6).abs() < 1e-5);
        assert!(fly_distance(1.1) > fly_distance(0.6));
    }
}
