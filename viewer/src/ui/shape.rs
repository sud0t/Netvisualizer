//! The one material every box of the UI is painted with: a rectangle
//! (rounded if asked) with a border and a glow, drawn by shape.wgsl from a
//! signed distance field; the same shader darkens every third row of the
//! window for the scanlines. A [`Look`] is the
//! friendly description (Bevy colours, pixels); [`Shape`] is what the GPU
//! gets.
//!
//! A layout node does not draw itself: it gets a [`Skin`] child, a quad with
//! this material that follows the node's size. Keeping the quad on a child
//! means its scale never reaches the node's other children.

use bevy::asset::{AssetPath, embedded_asset, embedded_path};
use bevy::color::ColorToComponents;
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;
use bevy::sprite_render::{AlphaMode2d, Material2d, Material2dPlugin};
use bevy_lunex::prelude::*;

/// How a box looks, in Bevy colours and logical pixels. `Color::NONE` turns
/// a part off.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Look {
    pub fill: Color,
    pub stroke: Color,
    /// Corner radius; half the box's smaller side makes a pill or a circle.
    pub radius: f32,
    /// Border width inside the edge; 0 for none.
    pub border: f32,
    pub glow: Color,
    /// How far the glow reaches outside the edge; 0 for none.
    pub glow_radius: f32,
    /// Scanlines: how much every third row of the window is darkened, over
    /// whatever else the box paints; 0 for none.
    pub scanline: f32,
}

impl Default for Look {
    fn default() -> Self {
        Look {
            fill: Color::NONE,
            stroke: Color::NONE,
            radius: 0.0,
            border: 0.0,
            glow: Color::NONE,
            glow_radius: 0.0,
            scanline: 0.0,
        }
    }
}

impl Look {
    /// A filled box with a 1 px border.
    pub fn bordered(fill: Color, stroke: Color, radius: f32) -> Look {
        Look {
            fill,
            stroke,
            radius,
            border: 1.0,
            ..Look::default()
        }
    }

    /// A filled box without a border.
    pub fn filled(fill: Color, radius: f32) -> Look {
        Look {
            fill,
            radius,
            ..Look::default()
        }
    }

    /// Nothing but scanlines of `strength`, for a box over the whole window.
    pub fn scanlines(strength: f32) -> Look {
        Look {
            scanline: strength,
            ..Look::default()
        }
    }

    pub fn with_glow(mut self, color: Color, radius: f32) -> Look {
        self.glow = color;
        self.glow_radius = radius;
        self
    }

    /// Between two looks: colours and sizes blend, `t` from 0 (self) to 1.
    pub fn lerp(&self, other: &Look, t: f32) -> Look {
        let t = t.clamp(0.0, 1.0);
        let color = |a: Color, b: Color| -> Color {
            let (a, b) = (a.to_linear().to_vec4(), b.to_linear().to_vec4());
            LinearRgba::from_vec4(a.lerp(b, t)).into()
        };
        let number = |a: f32, b: f32| a + (b - a) * t;
        Look {
            fill: color(self.fill, other.fill),
            stroke: color(self.stroke, other.stroke),
            radius: number(self.radius, other.radius),
            border: number(self.border, other.border),
            glow: color(self.glow, other.glow),
            glow_radius: number(self.glow_radius, other.glow_radius),
            scanline: number(self.scanline, other.scanline),
        }
    }

    /// The uniform for a box of `size` whose quad has `pad` pixels of air
    /// around it (for the glow).
    pub fn shape(&self, size: Vec2, pad: f32) -> Shape {
        Shape {
            fill: self.fill.to_linear().to_vec4(),
            stroke: self.stroke.to_linear().to_vec4(),
            glow: self.glow.to_linear().to_vec4(),
            size,
            pad,
            radius: self.radius,
            border: self.border,
            glow_radius: self.glow_radius,
            scanline: self.scanline,
        }
    }
}

/// The shader's uniform; mirrors `struct Shape` in shape.wgsl field for field.
#[derive(ShaderType, Debug, Clone, Copy, PartialEq)]
pub struct Shape {
    pub fill: Vec4,
    pub stroke: Vec4,
    pub glow: Vec4,
    pub size: Vec2,
    pub pad: f32,
    pub radius: f32,
    pub border: f32,
    pub glow_radius: f32,
    pub scanline: f32,
}

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct ShapeMaterial {
    #[uniform(0)]
    pub shape: Shape,
}

impl Material2d for ShapeMaterial {
    fn fragment_shader() -> ShaderRef {
        ShaderRef::Path(
            AssetPath::from_path_buf(embedded_path!("shape.wgsl")).with_source("embedded"),
        )
    }

    fn alpha_mode(&self) -> AlphaMode2d {
        AlphaMode2d::Blend
    }
}

/// The quad that paints its parent layout node. `look` is what it paints;
/// `pad` is how far the quad reaches past the node (room for the glow).
#[derive(Component, Debug, Clone)]
pub struct Skin {
    pub look: Look,
    pub pad: f32,
}

/// Everything a skin is spawned with, besides the material: the shared unit
/// quad is scaled to the node by [`fit_skins`].
#[derive(Resource)]
pub struct ShapeAssets {
    pub quad: Handle<Mesh>,
}

/// Just above the node it paints; a node's child nodes are a whole level
/// (1.0) higher, so they draw over it.
pub const SKIN_Z: f32 = 0.1;

pub struct ShapePlugin;

impl Plugin for ShapePlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "shape.wgsl");
        app.add_plugins(Material2dPlugin::<ShapeMaterial>::default())
            // Before the panels spawn their skins in Startup.
            .add_systems(PreStartup, setup)
            // After lunex has computed this frame's sizes.
            .add_systems(
                PostUpdate,
                (fit_skins, repaint_skins).after(UiSystems::PostCompute),
            );
    }
}

fn setup(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>) {
    commands.insert_resource(ShapeAssets {
        quad: meshes.add(Rectangle::new(1.0, 1.0)),
    });
}

/// A skin's quad follows its node's size: the mesh is scaled to the box
/// plus the padding, and the shader is told the box.
fn fit_skins(
    mut skins: Query<(
        &ChildOf,
        &Skin,
        &mut Transform,
        &MeshMaterial2d<ShapeMaterial>,
    )>,
    nodes: Query<&Dimension, Changed<Dimension>>,
    mut materials: ResMut<Assets<ShapeMaterial>>,
) {
    for (child_of, skin, mut transform, material) in &mut skins {
        let Ok(dimension) = nodes.get(child_of.parent()) else {
            continue;
        };
        let size = **dimension;
        let quad = size + Vec2::splat(2.0 * skin.pad);
        transform.scale = Vec3::new(quad.x, quad.y, 1.0);
        if let Some(mut material) = materials.get_mut(material) {
            material.shape = skin.look.shape(size, skin.pad);
        }
    }
}

/// A changed [`Skin`] (a hover blend, a progress arc) is painted anew with
/// the size the node has.
fn repaint_skins(
    skins: Query<(&ChildOf, &Skin, &MeshMaterial2d<ShapeMaterial>), Changed<Skin>>,
    nodes: Query<&Dimension>,
    mut materials: ResMut<Assets<ShapeMaterial>>,
) {
    for (child_of, skin, material) in &skins {
        let Ok(dimension) = nodes.get(child_of.parent()) else {
            continue;
        };
        if let Some(mut material) = materials.get_mut(material) {
            material.shape = skin.look.shape(**dimension, skin.pad);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_uniform_carries_the_box_and_its_padding() {
        let look = Look::filled(Color::WHITE, 6.0);
        let shape = look.shape(Vec2::new(40.0, 60.0), 4.0);
        assert_eq!(shape.radius, 6.0);
        assert_eq!(shape.pad, 4.0);
        assert_eq!(shape.size, Vec2::new(40.0, 60.0));
        assert_eq!(Look::scanlines(0.1).shape(Vec2::ONE, 0.0).scanline, 0.1);
    }

    #[test]
    fn lerp_blends_colours_and_sizes() {
        let a = Look::bordered(Color::BLACK, Color::NONE, 4.0);
        let b = Look::bordered(Color::WHITE, Color::WHITE, 8.0);
        let mid = a.lerp(&b, 0.5);
        assert_eq!(mid.radius, 6.0);
        assert_eq!(mid.border, 1.0);
        let fill = mid.fill.to_linear();
        assert!((fill.red - 0.5).abs() < 1e-6 && (fill.alpha - 1.0).abs() < 1e-6);
        assert!((mid.stroke.to_linear().alpha - 0.5).abs() < 1e-6);
        assert_eq!(a.lerp(&b, 0.0), a);
        assert_eq!(a.lerp(&b, 1.0), b);
        assert_eq!(a.lerp(&b, 7.0), b, "t is clamped");
    }
}
