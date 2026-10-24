//! The host labels: one line of text in a box over each node, following
//! it on screen, fading with the fog, brighter when hovered and framed when
//! selected. They live on a layer under every panel, so the rail covers
//! them, and nearer labels draw over farther ones.

use std::collections::HashMap;

use bevy::prelude::*;
use bevy::text::TextLayoutInfo;
use bevy_lunex::prelude::*;

use crate::ViewerSet;
use crate::graph::{ColorKey, Graph, LABEL_GAP, Node};
use crate::picking::Picker;
use crate::scene::{MainCamera, SceneSettings, smoothstep};
use crate::theme;
use crate::ui::shape::{Look, ShapeAssets, ShapeMaterial, Skin};
use crate::ui::widgets::{Fonts, Kit, Pin, TextStyle};
use crate::ui::{LABELS_Z, UiRoot, root_point};

/// A label's box: 7 px of padding beside the text, 2 px above and below,
/// hard corners like every other box.
const LABEL_PADDING: Vec2 = Vec2::new(7.0, 2.0);
const LABEL_ROUNDING: f32 = theme::BUTTON_RADIUS;
/// Labels fainter than this are not worth drawing.
const MIN_LABEL_OPACITY: f32 = 0.01;
/// A label whose anchor is further than this (logical pixels) outside the
/// viewport cannot reach into it, and is not placed at all.
const LABEL_CULL_MARGIN: f32 = 240.0;

/// The layer the labels hang from.
#[derive(Component)]
struct LabelsLayer;

/// One label and the sphere it follows.
#[derive(Component)]
struct HostLabel {
    node: Entity,
    skin: Entity,
    text: Entity,
}

/// Colours of one label, from its role, hover and selection.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LabelStyle {
    pub text: Color,
    pub background: Color,
    pub border: Color,
}

impl LabelStyle {
    pub fn new(key: ColorKey, hovered: bool, selected: bool) -> LabelStyle {
        let text = match key {
            ColorKey::Scanner => theme::ROLE_SCANNER,
            ColorKey::Gateway => theme::ROLE_GATEWAY,
            ColorKey::Router => theme::ROLE_ROUTER,
            ColorKey::Down => theme::ROLE_DOWN,
            ColorKey::Host => theme::TEXT,
        };
        let background = if selected {
            theme::HUD_BG_SOLID
        } else {
            theme::HUD_BG
        };
        let border = if selected {
            theme::ACCENT
        } else if hovered {
            theme::HUD_BORDER_HOVER
        } else {
            theme::HUD_BORDER
        };
        LabelStyle {
            text,
            background,
            border,
        }
    }

    /// The box's look at `opacity`, every part fading alike.
    fn look(&self, opacity: f32) -> Look {
        Look::bordered(
            fade(self.background, opacity),
            fade(self.border, opacity),
            LABEL_ROUNDING,
        )
    }
}

/// `color` with its alpha scaled by `opacity`.
pub fn fade(color: Color, opacity: f32) -> Color {
    let mut c = color.to_srgba();
    c.alpha *= opacity;
    c.into()
}

/// A label's opacity: fully visible up to the fog start, gone at the fog end.
pub fn label_opacity(distance: f32, settings: SceneSettings) -> f32 {
    1.0 - smoothstep(settings.fog_near, settings.fog_far, distance)
}

pub struct LabelsPlugin;

impl Plugin for LabelsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_layer).add_systems(
            Update,
            (sync_labels, place_labels).chain().in_set(ViewerSet::Draw),
        );
    }
}

fn spawn_layer(mut commands: Commands, root: Res<UiRoot>) {
    commands.entity(root.entity).with_children(|ui| {
        ui.spawn((
            Name::new("labels"),
            LabelsLayer,
            UiLayout::window().full().pack(),
            UiDepth::Set(LABELS_Z),
            Pickable::IGNORE,
        ));
    });
}

/// A new graph has new node entities: give each a label.
#[allow(clippy::too_many_arguments)] // a Bevy system lists what it reads
fn sync_labels(
    mut commands: Commands,
    graph: Res<Graph>,
    layer: Single<Entity, With<LabelsLayer>>,
    labels: Query<Entity, With<HostLabel>>,
    nodes: Query<(Entity, &Node)>,
    fonts: Res<Fonts>,
    shapes: Res<ShapeAssets>,
    mut materials: ResMut<Assets<ShapeMaterial>>,
) {
    if !graph.is_changed() {
        return;
    }
    for label in &labels {
        commands.entity(label).despawn();
    }
    let mut kit = Kit {
        fonts: &fonts,
        shapes: &shapes,
        materials: &mut materials,
    };
    let mut by_id: HashMap<&str, Entity> = HashMap::new();
    for (entity, node) in &nodes {
        by_id.insert(node.id.as_str(), entity);
    }
    for id in graph.nodes.keys() {
        let Some(&entity) = by_id.get(id.as_str()) else {
            continue; // spawned this frame, not yet in the query: next rebuild
        };
        let Ok((_, node)) = nodes.get(entity) else {
            continue;
        };
        let style = LabelStyle::new(node.key, false, false);
        let mut label = commands.spawn((
            Name::new("label"),
            Transform::from_xyz(0.0, 0.0, 0.0),
            Visibility::Hidden,
            Dimension::from(Vec2::ZERO),
            Pickable::IGNORE,
            ChildOf(*layer),
        ));
        let mut skin = Entity::PLACEHOLDER;
        let mut text = Entity::PLACEHOLDER;
        label.with_children(|l| {
            skin = l.spawn(kit.skin(style.look(1.0), 0.0)).id();
            text = l
                .spawn(kit.text(node.label.clone(), TextStyle::MONO, style.text, Pin::CENTER))
                .id();
        });
        label.insert(HostLabel {
            node: entity,
            skin,
            text,
        });
    }
}

/// Every frame: each label sits bottom-centred just above its node, as
/// opaque as the fog leaves it, nearer ones above farther ones.
#[allow(clippy::too_many_arguments)] // a Bevy system lists what it reads
fn place_labels(
    camera: Single<(&Camera, &Transform), (With<MainCamera>, Without<HostLabel>)>,
    nodes: Query<(&Node, &Transform), Without<HostLabel>>,
    graph: Res<Graph>,
    picker: Res<Picker>,
    settings: Res<SceneSettings>,
    root: Res<UiRoot>,
    roots: Query<&Dimension, (With<UiLayoutRoot>, Without<HostLabel>)>,
    mut labels: Query<(&HostLabel, &mut Transform, &mut Visibility, &mut Dimension), Without<Node>>,
    infos: Query<&TextLayoutInfo>,
    mut skins: Query<&mut Skin>,
    mut colors: Query<&mut TextColor>,
) {
    let (camera, camera_transform) = camera.into_inner();
    let camera_global = GlobalTransform::from(*camera_transform);
    let Ok(root_size) = roots.get(root.entity) else {
        return;
    };
    let selected = picker.selected_entity(&graph);
    let on_screen = camera
        .logical_viewport_rect()
        .map(|rect| rect.inflate(LABEL_CULL_MARGIN));

    for (label, mut transform, mut visibility, mut dimension) in &mut labels {
        let mut hide = || {
            if *visibility != Visibility::Hidden {
                *visibility = Visibility::Hidden;
            }
        };
        let Ok((node, node_transform)) = nodes.get(label.node) else {
            hide();
            continue;
        };
        let Ok(info) = infos.get(label.text) else {
            hide();
            continue;
        };
        if info.size.y == 0.0 {
            hide();
            continue; // the text is not laid out yet
        }
        let distance = camera_transform
            .translation
            .distance(node_transform.translation);
        let opacity = label_opacity(distance, *settings);
        if opacity < MIN_LABEL_OPACITY {
            hide();
            continue; // lost in the fog
        }
        // A fixed gap above the top of the node's sphere (its model fits
        // inside it).
        let anchor = node_transform.translation + Vec3::Y * (node.radius + LABEL_GAP);
        let Ok(screen) = camera.world_to_viewport(&camera_global, anchor) else {
            hide();
            continue; // behind the camera
        };
        if on_screen.is_some_and(|rect| !rect.contains(screen)) {
            hide();
            continue;
        }

        let size = info.size / info.scale_factor + 2.0 * LABEL_PADDING;
        if **dimension != size {
            **dimension = size;
        }
        // Whole pixels keep the 1 px border crisp.
        let point = root_point(screen, **root_size).round();
        transform.translation.x = point.x;
        transform.translation.y = point.y + size.y / 2.0;
        // Nearer labels sit higher within the layer (0 to 0.9 above it).
        transform.translation.z = 0.9 * (1.0 - distance / settings.fog_far).clamp(0.0, 1.0);

        let style = LabelStyle::new(
            node.key,
            Some(label.node) == picker.hovered,
            Some(label.node) == selected,
        );
        if let Ok(mut skin) = skins.get_mut(label.skin) {
            let look = style.look(opacity);
            if skin.look != look {
                skin.look = look;
            }
        }
        if let Ok(mut color) = colors.get_mut(label.text) {
            let wanted = fade(style.text, opacity);
            if color.0 != wanted {
                color.0 = wanted;
            }
        }
        if *visibility != Visibility::Inherited {
            *visibility = Visibility::Inherited;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_fade_over_the_fog_range() {
        let settings = SceneSettings {
            fog_near: 60.0,
            fog_far: 160.0,
        };
        assert_eq!(label_opacity(10.0, settings), 1.0);
        assert_eq!(label_opacity(60.0, settings), 1.0);
        assert!((label_opacity(110.0, settings) - 0.5).abs() < 1e-6);
        assert_eq!(label_opacity(200.0, settings), 0.0);
    }

    #[test]
    fn label_styles_follow_the_stylesheet_cascade() {
        let plain = LabelStyle::new(ColorKey::Host, false, false);
        assert_eq!(plain.text, theme::TEXT);
        assert_eq!(plain.background, theme::HUD_BG);
        assert_eq!(plain.border, theme::HUD_BORDER);
        assert_eq!(
            LabelStyle::new(ColorKey::Scanner, false, false).text,
            theme::ROLE_SCANNER
        );
        assert_eq!(
            LabelStyle::new(ColorKey::Gateway, false, false).text,
            theme::ROLE_GATEWAY
        );
        assert_eq!(
            LabelStyle::new(ColorKey::Down, false, false).text,
            theme::ROLE_DOWN
        );
        let hovered = LabelStyle::new(ColorKey::Host, true, false);
        assert_eq!(hovered.border, theme::HUD_BORDER_HOVER);
        assert_eq!(hovered.background, theme::HUD_BG);
        let selected = LabelStyle::new(ColorKey::Host, true, true);
        assert_eq!(selected.border, theme::ACCENT, "selected wins over hovered");
        assert_eq!(selected.background, theme::HUD_BG_SOLID);
    }

    #[test]
    fn fading_scales_the_alpha_channel_only() {
        let faded = fade(theme::HUD_BG, 0.5).to_srgba();
        let full = theme::HUD_BG.to_srgba();
        assert_eq!(
            (faded.red, faded.green, faded.blue),
            (full.red, full.green, full.blue)
        );
        assert!((faded.alpha - full.alpha * 0.5).abs() < 1e-6);
        let lit = fade(theme::SELECTION, 1.0).to_srgba();
        assert!(lit.green > 0.99 && (lit.alpha - 1.0).abs() < 1e-6);
        assert!(fade(theme::SELECTION, 0.0).to_srgba().alpha.abs() < 1e-6);
    }
}
