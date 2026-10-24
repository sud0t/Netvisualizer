//! Turns the map document into the scene: one entity per host, drawn as its
//! model (models.rs: the terminal for the scanner, a rack for the rest),
//! sized by role and standing on the floor at the host's x and z, and one
//! line per link, in the colour of the host it leads to (its role, or grey
//! when it is down).
//!
//! `rebuild` runs whenever the [`MapDocument`] resource changes (or the
//! models come in) and replaces every node, which is cheap at LAN scale (a
//! /24 is a few hundred nodes of a few parts each). Links are drawn with
//! gizmos each frame from the resolved endpoints, so there is nothing to
//! tear down when the map changes.

use std::collections::HashMap;

use bevy::color::Alpha;
use bevy::prelude::*;

use crate::ViewerSet;
use crate::map::{Host, MapDoc, Role, Status};
use crate::models::{ModelKind, Models};
use crate::theme;

/// The floor every node stands on, where the grid is drawn: the layout's
/// plane (layout.rs).
pub const FLOOR_Y: f32 = 0.0;
/// World-unit gap between the top of a node's sphere and its label.
pub const LABEL_GAP: f32 = 0.45;
/// Hosts without a position go on a ring this far outside the positioned ones.
const FALLBACK_RING_GAP: f32 = 6.0;
/// Selected nodes grow by this much. Hover no longer resizes the node; it
/// grows and lights the label instead (ui/labels.rs).
pub const HIGHLIGHT_SCALE: f32 = 1.12;

/// A node's radius per role: the scanner is the biggest, then the gateway
/// and the routers. Its model fits inside a sphere of this radius that rests
/// on the floor (models.rs), which is what the picker tests and the label
/// sits on.
pub fn node_radius(role: Role) -> f32 {
    match role {
        Role::Scanner => 1.45,
        Role::Gateway => 1.15,
        Role::Router => 1.0,
        Role::Host => 0.8,
    }
}

/// Where a node's entity goes: the centre of its sphere, which rests on the
/// floor under the host's x and z (a map saved when the outer rings were
/// raised stands on the floor too), its radius times `scale` up.
pub fn node_centre(position: Vec3, radius: f32, scale: f32) -> Vec3 {
    Vec3::new(position.x, FLOOR_Y + radius * scale, position.z)
}

/// The colour of the link into a node and of its label: its role, unless it
/// is down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ColorKey {
    Scanner,
    Gateway,
    Router,
    Host,
    Down,
}

impl ColorKey {
    pub fn of(host: &Host) -> ColorKey {
        match (host.status(), host.role()) {
            (Status::Down, _) => ColorKey::Down,
            (_, Role::Scanner) => ColorKey::Scanner,
            (_, Role::Gateway) => ColorKey::Gateway,
            (_, Role::Router) => ColorKey::Router,
            (_, Role::Host) => ColorKey::Host,
        }
    }

    pub fn color(self) -> Color {
        match self {
            ColorKey::Scanner => theme::ROLE_SCANNER,
            ColorKey::Gateway => theme::ROLE_GATEWAY,
            ColorKey::Router => theme::ROLE_ROUTER,
            ColorKey::Host => theme::ROLE_HOST,
            ColorKey::Down => theme::ROLE_DOWN,
        }
    }
}

/// How a node is currently drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum NodeState {
    #[default]
    None,
    Hover,
    Selected,
}

impl NodeState {
    /// How much brighter than as made a node's model is drawn: its colours
    /// and its glow (screens, lights) times this.
    pub fn lift(self) -> f32 {
        match self {
            NodeState::None => 1.0,
            NodeState::Hover => 1.6,
            NodeState::Selected => 2.0,
        }
    }
}

/// The latest map: loaded once from `--json`, or replaced by scan.rs after
/// each stage of a scan. `None` until the first map arrives.
#[derive(Resource, Debug, Default)]
pub struct MapDocument(pub Option<MapDoc>);

/// One per host entity: what the rest of the viewer needs to know about it.
/// The entity holds the node's place (the centre of its sphere) and size;
/// its children are the parts of its model.
#[derive(Component, Debug)]
pub struct Node {
    pub id: String,
    /// The host's label, or its id when it has none: what the label above
    /// the node says.
    pub label: String,
    pub key: ColorKey,
    pub radius: f32,
    pub state: NodeState,
}

/// What the rest of the viewer needs to know about the drawn map.
#[derive(Resource, Debug, Default)]
pub struct Graph {
    pub nodes: HashMap<String, Entity>,
    pub positions: HashMap<String, Vec3>,
    /// Resolved link endpoints and the colour of the host each leads to (its
    /// target); links to unknown hosts are dropped, not fatal.
    pub links: Vec<(Vec3, Vec3, ColorKey)>,
    pub bounds: Bounds,
    pub host_count: usize,
    pub link_count: usize,
    /// False until a map document has been drawn, so the empty graph that
    /// exists at start-up is never mistaken for the first map.
    pub drawn: bool,
}

/// An axis-aligned box, empty until a point is added.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub min: Vec3,
    pub max: Vec3,
}

impl Default for Bounds {
    fn default() -> Self {
        Bounds::EMPTY
    }
}

impl Bounds {
    pub const EMPTY: Bounds = Bounds {
        min: Vec3::INFINITY,
        max: Vec3::NEG_INFINITY,
    };

    pub fn is_empty(&self) -> bool {
        self.min.x > self.max.x || self.min.y > self.max.y || self.min.z > self.max.z
    }

    pub fn expand_by_point(&mut self, point: Vec3) {
        self.min = self.min.min(point);
        self.max = self.max.max(point);
    }

    pub fn expand_by_scalar(&mut self, amount: f32) {
        self.min -= Vec3::splat(amount);
        self.max += Vec3::splat(amount);
    }

    pub fn center(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }

    pub fn size(&self) -> Vec3 {
        self.max - self.min
    }
}

pub struct GraphPlugin;

impl Plugin for GraphPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Graph>().add_systems(
            Update,
            (
                rebuild.in_set(ViewerSet::Build),
                draw_links.in_set(ViewerSet::Draw),
            ),
        );
    }
}

/// Despawn every node, spawn the new ones, resolve links and recompute the
/// bounds. Runs only when the document resource changed, or when the models
/// came in (a map that arrived before them waits for them).
fn rebuild(
    mut commands: Commands,
    doc: Res<MapDocument>,
    models: Res<Models>,
    mut graph: ResMut<Graph>,
    existing: Query<Entity, With<Node>>,
) {
    if !(doc.is_changed() || models.is_changed()) || !models.ready() {
        return;
    }
    let Some(doc) = &doc.0 else {
        return;
    };
    for entity in &existing {
        commands.entity(entity).despawn();
    }
    let positions = resolve_positions(&doc.hosts);

    let mut next = Graph::default();
    let mut keys: HashMap<&str, ColorKey> = HashMap::new();
    let mut max_radius: f32 = 0.0;
    for (host, position) in doc.hosts.iter().zip(positions) {
        let role = host.role();
        let key = ColorKey::of(host);
        let radius = node_radius(role);
        let centre = node_centre(position, radius, 1.0);
        let model = models.get(ModelKind::of(role));
        let entity = commands
            .spawn((
                Node {
                    id: host.id.clone(),
                    label: host.display_label().to_string(),
                    key,
                    radius,
                    state: NodeState::None,
                },
                Transform::from_translation(centre).with_scale(Vec3::splat(radius)),
                Visibility::default(),
            ))
            .with_children(|node| {
                for part in &model.parts {
                    node.spawn(part.bundle(key == ColorKey::Down));
                }
            })
            .id();
        next.nodes.insert(host.id.clone(), entity);
        next.positions.insert(host.id.clone(), centre);
        next.bounds.expand_by_point(centre);
        keys.insert(&host.id, key);
        max_radius = max_radius.max(radius);
    }
    for link in &doc.links {
        if let (Some(a), Some(b), Some(key)) = (
            next.positions.get(&link.source),
            next.positions.get(&link.target),
            keys.get(link.target.as_str()),
        ) {
            next.links.push((*a, *b, *key));
        }
    }
    // The bounds: a point box for an empty map, then room for the nodes
    // themselves and for the labels above the top row.
    if next.bounds.is_empty() {
        next.bounds = Bounds {
            min: Vec3::ZERO,
            max: Vec3::ZERO,
        };
    }
    next.bounds.expand_by_scalar(max_radius);
    next.bounds.max.y += LABEL_GAP + 1.0;
    next.host_count = doc.hosts.len();
    next.link_count = doc.links.len();
    next.drawn = true;
    *graph = next;
}

/// Selection makes the node a little bigger, growing up from the floor
/// rather than into it; hover leaves its size alone (the label reacts
/// instead, ui/labels.rs). Returns whether the state changed, in which case
/// the caller gives the node's parts their materials for it
/// ([`crate::models::PartLooks`]).
pub fn set_state(node: &mut Node, transform: &mut Transform, state: NodeState) -> bool {
    if node.state == state {
        return false;
    }
    node.state = state;
    let scale = if state == NodeState::Selected {
        HIGHLIGHT_SCALE
    } else {
        1.0
    };
    transform.scale = Vec3::splat(node.radius * scale);
    transform.translation = node_centre(transform.translation, node.radius, scale);
    true
}

fn draw_links(graph: Res<Graph>, mut gizmos: Gizmos) {
    for &(a, b, key) in &graph.links {
        gizmos.line(a, b, key.color().with_alpha(theme::LINK_OPACITY));
    }
}

/// Every host gets a position. Hosts the backend left without one are spread
/// evenly on a ring just outside the positioned ones, so a map built with
/// `layout=None` still renders instead of piling everything at the origin.
/// The result is parallel to `hosts`.
pub fn resolve_positions(hosts: &[Host]) -> Vec<Vec3> {
    let mut out: Vec<Option<Vec3>> = Vec::with_capacity(hosts.len());
    let mut missing = 0usize;
    let mut max_radius: f32 = 0.0;
    for host in hosts {
        if let Some([x, y, z]) = host.position {
            out.push(Some(Vec3::new(x, y, z)));
            max_radius = max_radius.max(x.hypot(z));
        } else {
            out.push(None);
            missing += 1;
        }
    }
    if missing > 0 {
        let radius = max_radius + FALLBACK_RING_GAP;
        let step = std::f32::consts::TAU / missing as f32;
        for (i, slot) in out.iter_mut().filter(|slot| slot.is_none()).enumerate() {
            let angle = i as f32 * step;
            *slot = Some(Vec3::new(radius * angle.cos(), 0.0, radius * angle.sin()));
        }
    }
    out.into_iter()
        .map(|slot| slot.expect("every host has a position now"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hosts(json: &str) -> Vec<Host> {
        serde_json::from_str(json).expect("host list JSON")
    }

    #[test]
    fn positioned_hosts_keep_their_positions() {
        let hosts =
            hosts(r#"[{"id": "a", "position": [1, 2, 3]}, {"id": "b", "position": [-4, 0, 0]}]"#);
        assert_eq!(
            resolve_positions(&hosts),
            [Vec3::new(1.0, 2.0, 3.0), Vec3::new(-4.0, 0.0, 0.0)]
        );
    }

    #[test]
    fn hosts_without_a_position_share_a_ring_outside_the_rest() {
        let hosts = hosts(
            r#"[{"id": "a", "position": [3, 9, 4]}, {"id": "b"}, {"id": "c", "position": null}]"#,
        );
        let positions = resolve_positions(&hosts);
        // max horizontal radius is hypot(3, 4) = 5, so the ring sits at 11.
        assert_eq!(positions[0], Vec3::new(3.0, 9.0, 4.0));
        assert!(
            (positions[1] - Vec3::new(11.0, 0.0, 0.0)).length() < 1e-4,
            "{:?}",
            positions[1]
        );
        assert!(
            (positions[2] - Vec3::new(-11.0, 0.0, 0.0)).length() < 1e-4,
            "{:?}",
            positions[2]
        );
    }

    #[test]
    fn an_all_missing_map_becomes_a_ring_around_the_origin() {
        let hosts = hosts(r#"[{"id": "a"}, {"id": "b"}, {"id": "c"}, {"id": "d"}]"#);
        let positions = resolve_positions(&hosts);
        assert_eq!(positions.len(), 4);
        for p in &positions {
            assert!((p.length() - FALLBACK_RING_GAP).abs() < 1e-4, "{p:?}");
            assert_eq!(p.y, 0.0);
        }
    }

    #[test]
    fn color_key_prefers_down_over_role() {
        let hosts = hosts(
            r#"[{"id": "a", "role": "scanner"}, {"id": "b", "role": "gateway", "extra": {"status": "down"}},
                {"id": "c", "role": "mystery"}, {"id": "d", "status": "Down"}]"#,
        );
        let keys: Vec<ColorKey> = hosts.iter().map(ColorKey::of).collect();
        assert_eq!(
            keys,
            [
                ColorKey::Scanner,
                ColorKey::Down,
                ColorKey::Host,
                ColorKey::Down
            ]
        );
    }

    #[test]
    fn a_node_rests_on_the_floor_whatever_its_height_in_the_map() {
        let raised = Vec3::new(4.0, 1.5, -2.0);
        assert_eq!(
            node_centre(raised, 0.8, 1.0),
            Vec3::new(4.0, FLOOR_Y + 0.8, -2.0)
        );
        // Grown by a selection, its sphere still rests on the floor.
        let grown = node_centre(raised, 0.8, HIGHLIGHT_SCALE);
        assert!((grown.y - (FLOOR_Y + 0.8 * HIGHLIGHT_SCALE)).abs() < 1e-6);
    }

    #[test]
    fn bounds_grow_from_empty() {
        let mut b = Bounds::EMPTY;
        assert!(b.is_empty());
        b.expand_by_point(Vec3::new(1.0, 2.0, 3.0));
        assert!(!b.is_empty());
        assert_eq!(b.size(), Vec3::ZERO);
        b.expand_by_point(Vec3::new(-1.0, 0.0, 5.0));
        assert_eq!(b.center(), Vec3::new(0.0, 1.0, 4.0));
        b.expand_by_scalar(0.5);
        assert_eq!(b.min, Vec3::new(-1.5, -0.5, 2.5));
        assert_eq!(b.max, Vec3::new(1.5, 2.5, 5.5));
    }
}
