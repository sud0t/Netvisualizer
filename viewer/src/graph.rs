//! Turns the map document into the scene: one sphere entity per host (sized by
//! role, coloured by role and status) and one line per link. Port of graph.js.
//!
//! `rebuild` runs whenever the [`MapDocument`] resource changes and replaces
//! every node, which is cheap at LAN scale (a /24 is a few hundred entities).
//! Links are drawn with gizmos each frame from the resolved endpoints, so there
//! is nothing to tear down when the map changes.

use std::collections::HashMap;

use bevy::color::Alpha;
use bevy::prelude::*;

use crate::ViewerSet;
use crate::map::{Host, MapDoc, Role, Status};
use crate::theme;

/// World-unit gap between the top of a sphere and its label.
pub const LABEL_GAP: f32 = 0.45;
/// Hosts without a position go on a ring this far outside the positioned ones.
const FALLBACK_RING_GAP: f32 = 6.0;
/// Hovered / selected spheres grow by this much.
pub const HIGHLIGHT_SCALE: f32 = 1.12;
const SPHERE_SECTORS: u32 = 32;
const SPHERE_STACKS: u32 = 20;

/// `NODE_RADIUS` in graph.js.
pub fn node_radius(role: Role) -> f32 {
    match role {
        Role::Scanner => 1.1,
        Role::Gateway => 0.85,
        Role::Host => 0.6,
    }
}

/// Which colour a node takes: its role, unless it is down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ColorKey {
    Scanner,
    Gateway,
    Host,
    Down,
}

impl ColorKey {
    pub fn of(host: &Host) -> ColorKey {
        match (host.status(), host.role()) {
            (Status::Down, _) => ColorKey::Down,
            (_, Role::Scanner) => ColorKey::Scanner,
            (_, Role::Gateway) => ColorKey::Gateway,
            (_, Role::Host) => ColorKey::Host,
        }
    }

    pub fn color(self) -> Color {
        match self {
            ColorKey::Scanner => theme::ROLE_SCANNER,
            ColorKey::Gateway => theme::ROLE_GATEWAY,
            ColorKey::Host => theme::ROLE_HOST,
            ColorKey::Down => theme::ROLE_DOWN,
        }
    }

    const ALL: [ColorKey; 4] = [
        ColorKey::Scanner,
        ColorKey::Gateway,
        ColorKey::Host,
        ColorKey::Down,
    ];
}

/// `STATES` in graph.js: how a node is currently drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum NodeState {
    #[default]
    None,
    Hover,
    Selected,
}

impl NodeState {
    /// `EMISSIVE` in graph.js: how much of its own colour a sphere glows with.
    fn emissive(self) -> f32 {
        match self {
            NodeState::None => 0.28,
            NodeState::Hover => 0.75,
            NodeState::Selected => 0.95,
        }
    }

    const ALL: [NodeState; 3] = [NodeState::None, NodeState::Hover, NodeState::Selected];
}

/// The latest map. Step 7.1 loads it from a file once; the fetch thread of
/// step 7.4 replaces it whenever the server's revision changes.
#[derive(Resource, Debug)]
pub struct MapDocument(pub MapDoc);

/// Shared resources: one unit sphere scaled per node and one material per
/// (state, colour) pair, so highlighting a node is a handle swap, not a clone.
#[derive(Resource)]
pub struct GraphAssets {
    pub sphere: Handle<Mesh>,
    pub materials: HashMap<(NodeState, ColorKey), Handle<StandardMaterial>>,
}

impl GraphAssets {
    pub fn material(&self, state: NodeState, key: ColorKey) -> Handle<StandardMaterial> {
        self.materials[&(state, key)].clone()
    }
}

/// One per host entity (`mesh.userData` in graph.js).
#[derive(Component, Debug)]
pub struct Node {
    pub id: String,
    /// `host.label || host.id`, what the label above the sphere says.
    pub label: String,
    #[allow(dead_code)] // the sidebar's role badge reads it (7.3)
    pub role: Role,
    pub key: ColorKey,
    pub radius: f32,
    pub state: NodeState,
}

/// What the rest of the viewer needs to know about the drawn map.
#[derive(Resource, Debug, Default)]
pub struct Graph {
    pub nodes: HashMap<String, Entity>,
    pub positions: HashMap<String, Vec3>,
    /// Resolved link endpoints; links to unknown hosts are dropped, not fatal.
    pub links: Vec<(Vec3, Vec3)>,
    pub bounds: Bounds,
    pub host_count: usize,
    pub link_count: usize,
}

/// An axis-aligned box (`THREE.Box3`), empty until a point is added.
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
        app.init_resource::<Graph>()
            .add_systems(Startup, setup_assets)
            .add_systems(
                Update,
                (
                    rebuild.in_set(ViewerSet::Build),
                    draw_links.in_set(ViewerSet::Draw),
                ),
            );
    }
}

fn setup_assets(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let sphere = meshes.add(Sphere::new(1.0).mesh().uv(SPHERE_SECTORS, SPHERE_STACKS));
    let mut table = HashMap::new();
    for key in ColorKey::ALL {
        let color = key.color();
        for state in NodeState::ALL {
            let material = materials.add(StandardMaterial {
                base_color: color,
                emissive: color.to_linear() * state.emissive(),
                perceptual_roughness: 0.4,
                metallic: 0.15,
                ..default()
            });
            table.insert((state, key), material);
        }
    }
    commands.insert_resource(GraphAssets {
        sphere,
        materials: table,
    });
}

/// `Graph.update(doc)`: despawn every node, spawn the new ones, resolve links,
/// recompute the bounds. Runs only when the document resource changed.
fn rebuild(
    mut commands: Commands,
    doc: Res<MapDocument>,
    assets: Res<GraphAssets>,
    mut graph: ResMut<Graph>,
    existing: Query<Entity, With<Node>>,
) {
    if !doc.is_changed() {
        return;
    }
    for entity in &existing {
        commands.entity(entity).despawn();
    }
    let doc = &doc.0;
    let positions = resolve_positions(&doc.hosts);

    let mut next = Graph::default();
    let mut max_radius: f32 = 0.0;
    for (host, position) in doc.hosts.iter().zip(positions) {
        let role = host.role();
        let key = ColorKey::of(host);
        let radius = node_radius(role);
        let entity = commands
            .spawn((
                Node {
                    id: host.id.clone(),
                    label: host.display_label().to_string(),
                    role,
                    key,
                    radius,
                    state: NodeState::None,
                },
                Mesh3d(assets.sphere.clone()),
                MeshMaterial3d(assets.material(NodeState::None, key)),
                Transform::from_translation(position).with_scale(Vec3::splat(radius)),
            ))
            .id();
        next.nodes.insert(host.id.clone(), entity);
        next.positions.insert(host.id.clone(), position);
        next.bounds.expand_by_point(position);
        max_radius = max_radius.max(radius);
    }
    for link in &doc.links {
        if let (Some(a), Some(b)) = (
            next.positions.get(&link.source),
            next.positions.get(&link.target),
        ) {
            next.links.push((*a, *b));
        }
    }
    // `computeBounds`: a point box for an empty map, then room for the spheres
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
    *graph = next;
}

/// Hover and selection look (`setState` in graph.js): brighter material,
/// slightly bigger sphere. A handle swap, not a material clone.
pub fn set_state(
    assets: &GraphAssets,
    node: &mut Node,
    transform: &mut Transform,
    material: &mut MeshMaterial3d<StandardMaterial>,
    state: NodeState,
) {
    if node.state == state {
        return;
    }
    node.state = state;
    material.0 = assets.material(state, node.key);
    let scale = if state == NodeState::None {
        1.0
    } else {
        HIGHLIGHT_SCALE
    };
    transform.scale = Vec3::splat(node.radius * scale);
}

fn draw_links(graph: Res<Graph>, mut gizmos: Gizmos) {
    let color = theme::LINK.with_alpha(theme::LINK_OPACITY);
    for &(a, b) in &graph.links {
        gizmos.line(a, b, color);
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
        match host.position {
            Some([x, y, z]) => {
                out.push(Some(Vec3::new(x, y, z)));
                max_radius = max_radius.max(x.hypot(z));
            }
            None => {
                out.push(None);
                missing += 1;
            }
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
