//! Tooltips: a node with a [`Tooltip`] gets a small card by the pointer
//! once the pointer has rested on it (or on one of its children) a moment.

use bevy::picking::hover::HoverMap;
use bevy::picking::pointer::{PointerId, PointerLocation};
use bevy::prelude::*;
use bevy::text::TextLayoutInfo;
use bevy_lunex::prelude::*;

use crate::ViewerSet;
use crate::theme;
use crate::ui::shape::ShapeAssets;
use crate::ui::shape::ShapeMaterial;
use crate::ui::widgets::{Fonts, Kit, Pin, TextStyle, Tooltip};
use crate::ui::{TOOLTIP_Z, UiRoot, root_point};

/// How long the pointer rests before the tip shows.
const DELAY: f32 = 0.45;
/// Padding inside the card, and the card's offset from the pointer.
const PAD: Vec2 = Vec2::new(8.0, 5.0);
const OFFSET: Vec2 = Vec2::new(14.0, 20.0);

#[derive(Resource, Debug, Default)]
struct TooltipState {
    /// The node whose tip is due or showing, and since when.
    target: Option<Entity>,
    since: f32,
    /// The card, while it shows.
    card: Option<Entity>,
}

/// The card: a box with a text child (see widgets.rs for the pattern).
#[derive(Component)]
struct TooltipCard {
    text: Entity,
}

pub struct TooltipPlugin;

impl Plugin for TooltipPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TooltipState>()
            .add_systems(Update, tooltips.in_set(ViewerSet::Draw));
    }
}

/// Follow the pointer: note what it rests on, show the tip after the
/// delay, move the card with the pointer, hide it when the pointer leaves.
#[allow(clippy::too_many_arguments)] // a Bevy system lists what it reads
fn tooltips(
    mut commands: Commands,
    time: Res<Time>,
    hover: Res<HoverMap>,
    pointers: Query<(&PointerId, &PointerLocation)>,
    parents: Query<&ChildOf>,
    tips: Query<&Tooltip>,
    root: Res<UiRoot>,
    dimensions: Query<&Dimension, Without<TooltipCard>>,
    texts: Query<&TextLayoutInfo>,
    mut cards: Query<(&TooltipCard, &mut Transform, &mut Dimension), Without<UiLayout>>,
    mut state: ResMut<TooltipState>,
    fonts: Res<Fonts>,
    shapes: Res<ShapeAssets>,
    mut materials: ResMut<Assets<ShapeMaterial>>,
) {
    // The hovered node that carries a tip, or an ancestor of it that does.
    let target = hover
        .get(&PointerId::Mouse)
        .into_iter()
        .flat_map(|hits| hits.keys().copied())
        .find_map(|hit| {
            std::iter::once(hit)
                .chain(parents.iter_ancestors(hit))
                .find(|e| tips.get(*e).is_ok())
        });
    let now = time.elapsed_secs();
    if target != state.target {
        state.target = target;
        state.since = now;
        if let Some(card) = state.card.take() {
            commands.entity(card).despawn();
        }
    }
    let Some(target) = target else {
        return;
    };
    let Some(pointer) = pointers
        .iter()
        .find(|(id, _)| **id == PointerId::Mouse)
        .and_then(|(_, location)| location.location())
        .map(|location| location.position)
    else {
        return;
    };
    let Ok(root_size) = dimensions.get(root.entity) else {
        return;
    };

    if state.card.is_none() {
        if now - state.since < DELAY {
            return;
        }
        let Ok(tip) = tips.get(target) else {
            return;
        };
        let mut kit = Kit {
            fonts: &fonts,
            shapes: &shapes,
            materials: &mut materials,
        };
        let mut text = Entity::PLACEHOLDER;
        let card = commands
            .spawn((
                Name::new("tooltip"),
                Transform::from_xyz(0.0, 0.0, TOOLTIP_Z),
                Visibility::Hidden, // until it is measured
                Dimension::from(Vec2::ZERO),
                Pickable::IGNORE,
                ChildOf(root.entity),
            ))
            .with_children(|card| {
                card.spawn(kit.skin(theme::card(), 0.0));
                text = card
                    .spawn(kit.text(
                        tip.0.clone(),
                        TextStyle::MONO,
                        theme::TEXT,
                        Pin::left(PAD.x),
                    ))
                    .id();
            })
            .id();
        commands.entity(card).insert(TooltipCard { text });
        state.card = Some(card);
        return;
    }

    // Size the card to its text and keep it by the pointer, inside the window.
    let Some(card) = state.card else {
        return;
    };
    let Ok((parts, mut transform, mut dimension)) = cards.get_mut(card) else {
        return;
    };
    let Ok(info) = texts.get(parts.text) else {
        return;
    };
    if info.size.y == 0.0 {
        return; // not laid out yet
    }
    let size = info.size / info.scale_factor + 2.0 * PAD;
    if **dimension != size {
        **dimension = size;
        commands.entity(card).insert(Visibility::Inherited);
    }
    let top_left = (pointer + OFFSET).min(**root_size - size).max(Vec2::ZERO);
    let center = root_point(top_left + size / 2.0, **root_size);
    transform.translation.x = center.x;
    transform.translation.y = center.y;
}
