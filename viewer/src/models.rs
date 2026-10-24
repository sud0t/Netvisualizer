//! The node models: what each host is drawn as. The scanner is a street
//! terminal and every other host a server rack (Milestone 15 gives each
//! device type a model of its own). Both are glTF files that
//! `scripts/pack-model.py` packed into one `.glb` each, built into the binary
//! like the font, so the viewer runs from any directory;
//! `viewer/assets/models/CREDITS.txt` names their authors (CC-BY 4.0).
//!
//! A glTF file is a tree of nodes, each with a transform and maybe a mesh,
//! and a mesh is one or more primitives (vertices plus a material). Bevy can
//! spawn that tree as a scene, but a node on the map is one entity that the
//! picker, the labels and the camera treat as a whole, so the tree is
//! flattened once, when the file has loaded: every primitive becomes a
//! [`Part`] with its transform from the model's root, and a node entity gets
//! one child per part. The parts are moved and scaled together so the model
//! fits inside a sphere of radius 1 around the node entity's origin, centred
//! on the vertical axis with its base at the sphere's bottom: scaled by the
//! node's radius, it stays inside the sphere the picker tests, the selection
//! ring circles and the label sits on, as the old spheres did, and it
//! stands on the floor the sphere rests on (graph.rs).
//! The terminal's animation (the screen opening, the console rising) is not
//! played; its parts stay where the file puts them at rest.
//!
//! The models keep their own materials; the role colours are on the links
//! (graph.rs). Each material gets its six looks made up front, so hovering a
//! node only swaps handles: as it is, hovered and selected (brighter), and
//! the same three darker with the lights off for a host that is down.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use bevy::asset::RecursiveDependencyLoadState;
use bevy::asset::io::embedded::EmbeddedAssetRegistry;
use bevy::gltf::{Gltf, GltfMaterial, GltfMesh, GltfNode};
use bevy::mesh::VertexAttributeValues;
use bevy::pbr::gltf::standard_material_from_gltf_material;
use bevy::prelude::*;

use crate::ViewerSet;
use crate::graph::{Bounds, NodeState};
use crate::map::Role;
use crate::theme;

/// A down host's model keeps this much of its own colour; what glows (the
/// screen, the lights) goes dark.
const DOWN_SHADE: f32 = 0.3;
/// The stand-in for a model whose file would not load.
const SPHERE_SECTORS: u32 = 32;
const SPHERE_STACKS: u32 = 20;

/// Which model a host is drawn as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModelKind {
    Terminal,
    Rack,
}

impl ModelKind {
    const ALL: [ModelKind; 2] = [ModelKind::Terminal, ModelKind::Rack];

    pub fn of(role: Role) -> ModelKind {
        match role {
            Role::Scanner => ModelKind::Terminal,
            Role::Gateway | Role::Router | Role::Host => ModelKind::Rack,
        }
    }

    /// Where the file sits in Bevy's `embedded://` asset source, and its
    /// bytes.
    fn file(self) -> (&'static str, &'static [u8]) {
        match self {
            ModelKind::Terminal => (
                "netvisualizer/models/scanner_terminal.glb",
                include_bytes!("../assets/models/scanner_terminal.glb"),
            ),
            ModelKind::Rack => (
                "netvisualizer/models/server_rack.glb",
                include_bytes!("../assets/models/server_rack.glb"),
            ),
        }
    }

    /// The turn about the vertical that puts the model's front (the
    /// terminal's screen, the rack's blades) towards +z, where the camera
    /// starts out. The rack's file has its front facing -z.
    fn yaw(self) -> f32 {
        match self {
            ModelKind::Terminal => 0.0,
            ModelKind::Rack => std::f32::consts::PI,
        }
    }

    /// How much of its sphere the model fills. A rack is tall and narrow, so
    /// fitting it by its diagonal leaves it reading small; it is drawn a
    /// little larger than the unit sphere (it still stands on the floor and
    /// stays clear of the label above it).
    fn fill(self) -> f32 {
        match self {
            ModelKind::Terminal => 1.0,
            ModelKind::Rack => 1.3,
        }
    }
}

/// One drawable piece of a model: a mesh, where it sits in the model (the
/// unit sphere around the node's centre), and its material's looks.
#[derive(Debug, Clone)]
pub struct Part {
    pub mesh: Handle<Mesh>,
    pub transform: Transform,
    looks: Looks,
}

impl Part {
    /// The part's entity, a child of the node's.
    pub fn bundle(&self, down: bool) -> impl Bundle {
        let looks = self.looks.of(down);
        (
            Mesh3d(self.mesh.clone()),
            MeshMaterial3d(looks.get(NodeState::None)),
            self.transform,
            looks,
        )
    }
}

/// A part's material in each state, for a host that is up and one that is down.
#[derive(Debug, Clone)]
struct Looks {
    up: PartLooks,
    down: PartLooks,
}

impl Looks {
    fn of(&self, down: bool) -> PartLooks {
        if down {
            self.down.clone()
        } else {
            self.up.clone()
        }
    }
}

/// On each part entity: its material in each [`NodeState`].
#[derive(Component, Debug, Clone)]
pub struct PartLooks {
    none: Handle<StandardMaterial>,
    hover: Handle<StandardMaterial>,
    selected: Handle<StandardMaterial>,
}

impl PartLooks {
    pub fn get(&self, state: NodeState) -> Handle<StandardMaterial> {
        match state {
            NodeState::None => self.none.clone(),
            NodeState::Hover => self.hover.clone(),
            NodeState::Selected => self.selected.clone(),
        }
    }
}

/// A model, flattened into parts.
#[derive(Debug, Clone)]
pub struct Model {
    pub parts: Vec<Part>,
}

/// The models: the files still loading, and the models made from the ones
/// that are in.
#[derive(Resource, Debug, Default)]
pub struct Models {
    loading: Vec<(ModelKind, Handle<Gltf>)>,
    ready: HashMap<ModelKind, Model>,
}

impl Models {
    /// True once every model is in (a file that failed to load stands as
    /// a plain sphere).
    pub fn ready(&self) -> bool {
        self.loading.is_empty() && !self.ready.is_empty()
    }

    pub fn get(&self, kind: ModelKind) -> &Model {
        &self.ready[&kind]
    }
}

pub struct ModelsPlugin;

impl Plugin for ModelsPlugin {
    fn build(&self, app: &mut App) {
        // The files go into the `embedded://` source, so the asset server
        // loads them as if from disk, `.glb` loader and all.
        let registry = app.world().resource::<EmbeddedAssetRegistry>();
        for kind in ModelKind::ALL {
            let (path, bytes) = kind.file();
            let full_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("assets")
                .join(
                    Path::new(path)
                        .strip_prefix("netvisualizer")
                        .expect("embedded paths start with the crate name"),
                );
            registry.insert_asset(full_path, Path::new(path), bytes);
        }
        app.init_resource::<Models>()
            .add_systems(Startup, load_models)
            .add_systems(Update, finish_models.in_set(ViewerSet::Receive));
    }
}

fn load_models(asset_server: Res<AssetServer>, mut models: ResMut<Models>) {
    for kind in ModelKind::ALL {
        let (path, _) = kind.file();
        let handle = asset_server.load(format!("embedded://{path}"));
        models.loading.push((kind, handle));
    }
}

/// Turn each file that has loaded, textures and all, into a [`Model`].
#[allow(clippy::too_many_arguments)] // a Bevy system lists what it reads
fn finish_models(
    asset_server: Res<AssetServer>,
    gltfs: Res<Assets<Gltf>>,
    nodes: Res<Assets<GltfNode>>,
    gltf_meshes: Res<Assets<GltfMesh>>,
    gltf_materials: Res<Assets<GltfMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut models: ResMut<Models>,
) {
    let settled = |handle: &Handle<Gltf>| {
        !matches!(
            asset_server.recursive_dependency_load_state(handle),
            RecursiveDependencyLoadState::NotLoaded | RecursiveDependencyLoadState::Loading
        )
    };
    // Only touch the resource when a file has settled: a change to it is
    // what tells graph.rs to draw the map again.
    if !models.loading.iter().any(|(_, handle)| settled(handle)) {
        return;
    }
    for (kind, handle) in std::mem::take(&mut models.loading) {
        let gltf = match asset_server.recursive_dependency_load_state(&handle) {
            RecursiveDependencyLoadState::Loaded => gltfs.get(&handle),
            RecursiveDependencyLoadState::Failed(error) => {
                error!(
                    "netvisualizer: the {kind:?} model did not load ({error}); drawing a sphere"
                );
                None
            }
            _ => {
                models.loading.push((kind, handle));
                continue;
            }
        };
        let model = match gltf {
            Some(gltf) => {
                let pieces = flatten(gltf, &nodes, &gltf_meshes);
                build_model(kind, &pieces, &meshes, &gltf_materials, &mut materials)
            }
            None => sphere(&mut meshes, &mut materials),
        };
        models.ready.insert(kind, model);
    }
}

/// A primitive of the file, with its transform from the model's root.
struct Piece {
    mesh: Handle<Mesh>,
    material: Option<Handle<GltfMaterial>>,
    transform: Mat4,
}

/// Every primitive in the file's node tree, each node's transform applied
/// to its children's. The roots are the nodes no node lists as a child (the
/// files here have one scene, so that is its tree).
fn flatten(gltf: &Gltf, nodes: &Assets<GltfNode>, gltf_meshes: &Assets<GltfMesh>) -> Vec<Piece> {
    let children: HashSet<AssetId<GltfNode>> = gltf
        .nodes
        .iter()
        .filter_map(|handle| nodes.get(handle))
        .flat_map(|node| node.children.iter().map(Handle::id))
        .collect();
    let mut stack: Vec<(&Handle<GltfNode>, Mat4)> = gltf
        .nodes
        .iter()
        .filter(|handle| !children.contains(&handle.id()))
        .map(|handle| (handle, Mat4::IDENTITY))
        .collect();
    let mut pieces = Vec::new();
    while let Some((handle, parent)) = stack.pop() {
        let Some(node) = nodes.get(handle) else {
            continue;
        };
        let transform = parent * node.transform.to_matrix();
        if let Some(mesh) = node.mesh.as_ref().and_then(|mesh| gltf_meshes.get(mesh)) {
            for primitive in &mesh.primitives {
                pieces.push(Piece {
                    mesh: primitive.mesh.clone(),
                    material: primitive.material.clone(),
                    transform,
                });
            }
        }
        stack.extend(node.children.iter().map(|child| (child, transform)));
    }
    pieces
}

/// The pieces fitted into the unit sphere and turned to face +z, each
/// material with its looks (made once, however many pieces share it).
fn build_model(
    kind: ModelKind,
    pieces: &[Piece],
    meshes: &Assets<Mesh>,
    gltf_materials: &Assets<GltfMaterial>,
    materials: &mut Assets<StandardMaterial>,
) -> Model {
    let mut bounds = Bounds::EMPTY;
    for piece in pieces {
        let Some(VertexAttributeValues::Float32x3(positions)) = meshes
            .get(&piece.mesh)
            .and_then(|mesh| mesh.attribute(Mesh::ATTRIBUTE_POSITION))
        else {
            continue;
        };
        for &position in positions {
            bounds.expand_by_point(piece.transform.transform_point3(Vec3::from(position)));
        }
    }
    let place = Mat4::from_rotation_y(kind.yaw()) * fit(&bounds, kind.fill()).to_matrix();

    let fallback = GltfMaterial::default();
    let mut looks: HashMap<Option<AssetId<GltfMaterial>>, Looks> = HashMap::new();
    let parts = pieces
        .iter()
        .map(|piece| {
            let key = piece.material.as_ref().map(Handle::id);
            let looks = looks
                .entry(key)
                .or_insert_with(|| {
                    let source = piece
                        .material
                        .as_ref()
                        .and_then(|handle| gltf_materials.get(handle))
                        .unwrap_or(&fallback);
                    make_looks(&standard_material_from_gltf_material(source), materials)
                })
                .clone();
            Part {
                mesh: piece.mesh.clone(),
                // Rotations and non-uniform scales down the tree can in
                // general multiply into a shear, which a Transform cannot
                // hold; these files only scale a node's own mesh, so the
                // product comes apart exactly.
                transform: Transform::from_matrix(place * piece.transform),
                looks,
            }
        })
        .collect();
    Model { parts }
}

/// The transform that scales a box so its diagonal is `fill` times the unit
/// sphere's diameter (`fill` = 1 just fits it inside) and puts it on the
/// vertical axis with the middle of its base at the sphere's bottom,
/// (0, -1, 0); its base stays there whatever the fill. Its top is then at
/// most at the sphere's top for `fill` = 1, a little above for more.
pub fn fit(bounds: &Bounds, fill: f32) -> Transform {
    let half_diagonal = bounds.size().length() / 2.0;
    let scale = if half_diagonal > 0.0 {
        fill / half_diagonal
    } else {
        1.0
    };
    let centre = bounds.center();
    let base = Vec3::new(centre.x, bounds.min.y, centre.z);
    Transform::from_translation(Vec3::NEG_Y - base * scale).with_scale(Vec3::splat(scale))
}

fn make_looks(base: &StandardMaterial, materials: &mut Assets<StandardMaterial>) -> Looks {
    let mut set = |down| PartLooks {
        none: materials.add(look(base, NodeState::None, down)),
        hover: materials.add(look(base, NodeState::Hover, down)),
        selected: materials.add(look(base, NodeState::Selected, down)),
    };
    Looks {
        up: set(false),
        down: set(true),
    }
}

/// One look of a model's material: its colour and its glow times the state's
/// lift (more light off a hovered or selected node), and for a host that is
/// down its colour shaded and its glow off.
fn look(base: &StandardMaterial, state: NodeState, down: bool) -> StandardMaterial {
    let lift = state.lift();
    let shade = if down { DOWN_SHADE } else { 1.0 };
    let glow = if down { 0.0 } else { lift };
    StandardMaterial {
        base_color: scaled(base.base_color.to_linear(), lift * shade).into(),
        emissive: scaled(base.emissive, glow),
        ..base.clone()
    }
}

/// A colour's red, green and blue times `factor`, its alpha as it was.
fn scaled(color: LinearRgba, factor: f32) -> LinearRgba {
    LinearRgba::new(
        color.red * factor,
        color.green * factor,
        color.blue * factor,
        color.alpha,
    )
}

/// What a model whose file did not load is drawn as: a sphere in the text
/// colour, as the nodes were before there were models.
fn sphere(meshes: &mut Assets<Mesh>, materials: &mut Assets<StandardMaterial>) -> Model {
    let base = StandardMaterial {
        base_color: theme::TEXT,
        perceptual_roughness: 0.4,
        metallic: 0.15,
        ..default()
    };
    Model {
        parts: vec![Part {
            mesh: meshes.add(Sphere::new(1.0).mesh().uv(SPHERE_SECTORS, SPHERE_STACKS)),
            transform: Transform::IDENTITY,
            looks: make_looks(&base, materials),
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_stands_a_box_on_the_bottom_of_the_unit_sphere() {
        // The terminal's file: centimetres, its base 32 cm above the origin.
        let bounds = Bounds {
            min: Vec3::new(-1.0, 32.0, -2.0),
            max: Vec3::new(3.0, 140.0, 10.0),
        };
        let fit = fit(&bounds, 1.0);
        let base = Vec3::new(1.0, 32.0, 4.0);
        let placed = fit.transform_point(base);
        assert!((placed - Vec3::NEG_Y).length() < 1e-5, "{placed:?}");
        // As big as fits: the diagonal is the sphere's diameter.
        let diagonal = fit.transform_point(bounds.max) - fit.transform_point(bounds.min);
        assert!((diagonal.length() - 2.0).abs() < 1e-5, "{diagonal:?}");
        let top = fit.transform_point(Vec3::new(1.0, 140.0, 4.0));
        assert!(
            top.x.abs() < 1e-5 && top.z.abs() < 1e-5 && top.y <= 1.0,
            "{top:?}"
        );
    }

    #[test]
    fn fit_leaves_a_single_point_unscaled() {
        let point = Bounds {
            min: Vec3::new(2.0, 1.0, 0.0),
            max: Vec3::new(2.0, 1.0, 0.0),
        };
        let fit = fit(&point, 1.0);
        assert_eq!(fit.scale, Vec3::ONE);
        assert_eq!(fit.transform_point(point.min), Vec3::NEG_Y);
    }

    #[test]
    fn looks_lift_with_the_state_and_go_dark_when_down() {
        let base = StandardMaterial {
            base_color: Color::linear_rgba(0.2, 0.1, 0.4, 0.5),
            emissive: LinearRgba::new(0.0, 1.0, 0.0, 1.0),
            ..default()
        };
        let plain = look(&base, NodeState::None, false);
        assert_eq!(plain.base_color, base.base_color);
        assert_eq!(plain.emissive, base.emissive);

        let hovered = look(&base, NodeState::Hover, false);
        let lift = NodeState::Hover.lift();
        assert!(lift > 1.0);
        assert_eq!(hovered.base_color.to_linear().green, 0.1 * lift);
        assert_eq!(hovered.base_color.alpha(), 0.5);
        assert_eq!(hovered.emissive.green, lift);

        let down = look(&base, NodeState::None, true);
        assert_eq!(down.base_color.to_linear().blue, 0.4 * DOWN_SHADE);
        assert_eq!(down.emissive, LinearRgba::new(0.0, 0.0, 0.0, 1.0));
    }

    #[test]
    fn the_scanner_is_the_terminal_and_everyone_else_a_rack() {
        assert_eq!(ModelKind::of(Role::Scanner), ModelKind::Terminal);
        for role in [Role::Gateway, Role::Router, Role::Host] {
            assert_eq!(ModelKind::of(role), ModelKind::Rack);
        }
    }
}
