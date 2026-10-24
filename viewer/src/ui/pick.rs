//! Hit testing for the UI layer: which lunex nodes are under the pointer,
//! reported to `bevy_picking` like any other backend.
//!
//! `bevy_lunex` ships a backend of its own, but it takes the first active
//! camera that draws to the window, and a query's order is not promised:
//! when that camera is the 3D one (not orthographic) it reports nothing,
//! and the whole interface stops taking clicks. This backend looks only at
//! the UI camera. The hit test follows lunex's picking.rs (MIT or
//! Apache-2.0): the pointer's ray in the node's own space, against a box
//! of the node's `Dimension`, nearest (highest) node first, stopping at the
//! first one that blocks.

use bevy::camera::RenderTarget;
use bevy::camera::visibility::ViewVisibility;
use bevy::math::FloatExt;
use bevy::picking::PickingSystems;
use bevy::picking::backend::{HitData, PointerHits};
use bevy::picking::pointer::{PointerId, PointerLocation};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy_lunex::{Dimension, NoLunexPicking};

use crate::ui::UiCamera;

pub struct UiPickPlugin;

impl Plugin for UiPickPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PreUpdate,
            (silence_lunex, ui_picking)
                .chain()
                .in_set(PickingSystems::Backend),
        );
    }
}

/// Every node gets [`NoLunexPicking`], so lunex's backend stays quiet and
/// nothing is reported twice.
fn silence_lunex(
    mut commands: Commands,
    nodes: Query<Entity, (With<Dimension>, Without<NoLunexPicking>)>,
) {
    for node in &nodes {
        commands.entity(node).insert(NoLunexPicking);
    }
}

/// What the pointer is over, for each pointer on the window.
#[allow(clippy::type_complexity)] // the camera and the nodes, as lunex queries them
fn ui_picking(
    pointers: Query<(&PointerId, &PointerLocation)>,
    camera: Single<
        (
            Entity,
            &Camera,
            &RenderTarget,
            &GlobalTransform,
            &Projection,
        ),
        With<UiCamera>,
    >,
    primary_window: Query<Entity, With<PrimaryWindow>>,
    nodes: Query<(
        Entity,
        &Dimension,
        &GlobalTransform,
        Option<&Pickable>,
        &ViewVisibility,
    )>,
    mut output: MessageWriter<PointerHits>,
) {
    let (camera_entity, camera, target, camera_transform, projection) = camera.into_inner();
    let Projection::Orthographic(ortho) = projection else {
        return;
    };
    if !camera.is_active {
        return;
    }
    let target = target.normalize(primary_window.single().ok());
    // Highest first: the first node containing the pointer is the one on top.
    let mut sorted: Vec<_> = nodes
        .iter()
        .filter(|(_, _, transform, _, visibility)| !transform.affine().is_nan() && visibility.get())
        .map(|(entity, dimension, transform, pickable, _)| (entity, dimension, transform, pickable))
        .collect();
    sorted.sort_by(|a, b| b.2.translation().z.total_cmp(&a.2.translation().z));

    for (pointer, location) in pointers
        .iter()
        .filter_map(|(pointer, location)| location.location().map(|l| (pointer, l)))
    {
        if target.as_ref() != Some(&location.target) {
            continue;
        }
        let viewport_min = camera
            .logical_viewport_rect()
            .map(|rect| rect.min)
            .unwrap_or_default();
        let Ok(ray) = camera.viewport_to_world(camera_transform, location.position - viewport_min)
        else {
            continue;
        };
        let ray_end = ray.origin + ray.direction * (ortho.far - ortho.near);
        let mut picks = Vec::new();
        for &(entity, dimension, node_transform, pickable) in &sorted {
            // The ray in the node's space; the node is the plane z = 0 there.
            let world_to_node = node_transform.affine().inverse();
            let start = world_to_node.transform_point3(ray.origin);
            let end = world_to_node.transform_point3(ray_end);
            if (start.z - end.z).abs() < f32::EPSILON {
                continue; // edge on
            }
            let t = f32::inverse_lerp(start.z, end.z, 0.0);
            if !(0.0..=1.0).contains(&t) {
                continue; // outside the near and far planes
            }
            let at = start.lerp(end, t).xy();
            // A node that is still laid out may be a hair below zero wide.
            let half = dimension.max(Vec2::ZERO) / 2.0;
            if at.x.abs() > half.x || at.y.abs() > half.y {
                continue;
            }
            let hit_world = node_transform.transform_point(at.extend(0.0));
            let hit_camera = camera_transform
                .affine()
                .inverse()
                .transform_point3(hit_world);
            let depth = -ortho.near - hit_camera.z;
            picks.push((
                entity,
                HitData::new(
                    camera_entity,
                    depth,
                    Some(hit_world),
                    Some(*node_transform.back()),
                ),
            ));
            if pickable.is_none_or(|p| p.should_block_lower) {
                break;
            }
        }
        output.write(PointerHits::new(*pointer, picks, camera.order as f32));
    }
}
