//! The help overlay: the controls as a table of key caps and gestures, on a
//! card over a dimmed backdrop. `?`, `h` and the HUD button open it; Escape
//! (main.rs) or a click on the backdrop closes it.

use bevy::prelude::*;
use bevy::sprite::Anchor;
use bevy_lunex::prelude::*;

use crate::ViewerSet;
use crate::theme;
use crate::ui::shape::{Look, ShapeAssets, ShapeMaterial};
use crate::ui::widgets::{Fonts, Kit, Pin, TextStyle};
use crate::ui::{Help, OVERLAY_Z, UiRoot};

/// What triggers an action: keys (drawn as key caps) or a mouse gesture.
enum Trigger {
    Keys(&'static [&'static str]),
    Mouse(&'static str),
}

/// The rows of the table.
const HELP_ROWS: [(Trigger, &str); 12] = [
    (
        Trigger::Keys(&["W", "A", "S", "D"]),
        "fly forward / left / back / right",
    ),
    (Trigger::Keys(&["Q", "E"]), "down / up"),
    (Trigger::Keys(&["Shift"]), "fly faster"),
    (Trigger::Mouse("right mouse (hold)"), "look around"),
    (Trigger::Mouse("mouse wheel"), "change fly speed"),
    (
        Trigger::Mouse("left click"),
        "select a host, open its details",
    ),
    (Trigger::Mouse("double click"), "fly to a host"),
    (Trigger::Keys(&["F"]), "fly to the selected host"),
    (Trigger::Keys(&["R"]), "reset the view"),
    (Trigger::Keys(&["/"]), "filter the host list"),
    (Trigger::Keys(&["Esc"]), "deselect / close"),
    (Trigger::Keys(&["?"]), "this help"),
];

const CARD_W: f32 = 460.0;
const CARD_PAD: Vec2 = Vec2::new(22.0, 18.0);
const ROW_H: f32 = 28.0;
/// The action column starts here, from the card's left padding.
const ACTION_X: f32 = 170.0;
/// A key cap: monospace text with 6 px either side, 22 px tall, 4 px apart.
const CAP_H: f32 = 22.0;
const CAP_PAD: f32 = 6.0;
const CAP_GAP: f32 = 4.0;

#[derive(Component)]
struct HelpOverlay;

pub struct HelpPlugin;

impl Plugin for HelpPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, show_help.in_set(ViewerSet::Draw));
    }
}

/// The card's height: title, rows, footer, padding.
fn card_height() -> f32 {
    CARD_PAD.y + 22.0 + 10.0 + HELP_ROWS.len() as f32 * ROW_H + 10.0 + CAP_H + CARD_PAD.y
}

/// A key cap's width, from the monospace advance.
fn cap_width(key: &str) -> f32 {
    key.chars().count() as f32 * TextStyle::MONO.average_advance() + 2.0 * CAP_PAD
}

/// Spawn the overlay when help opens, take it down when it closes.
#[allow(clippy::too_many_lines)] // one card, laid out when it opens
fn show_help(
    mut commands: Commands,
    help: Res<Help>,
    root: Res<UiRoot>,
    overlays: Query<Entity, With<HelpOverlay>>,
    fonts: Res<Fonts>,
    shapes: Res<ShapeAssets>,
    mut materials: ResMut<Assets<ShapeMaterial>>,
) {
    if !help.is_changed() {
        return;
    }
    for overlay in &overlays {
        commands.entity(overlay).despawn();
    }
    if !help.open {
        return;
    }
    let mut kit = Kit {
        fonts: &fonts,
        shapes: &shapes,
        materials: &mut materials,
    };
    commands.entity(root.entity).with_children(|ui| {
        ui.spawn((
            Name::new("help"),
            HelpOverlay,
            UiLayout::window().full().pack(),
            UiDepth::Set(OVERLAY_Z),
            Pickable::IGNORE,
        ))
        .with_children(|overlay| {
            // The backdrop: a click on it closes the help.
            overlay
                .spawn((Name::new("backdrop"), UiLayout::window().full().pack()))
                .with_children(|b| {
                    b.spawn(kit.skin(Look::filled(theme::BACKDROP, 0.0), 0.0));
                })
                .observe(|click: On<PointerClick>, mut help: ResMut<Help>| {
                    if click.button == PointerButton::Primary {
                        help.open = false;
                    }
                });
            // The card, in the middle.
            overlay
                .spawn((
                    Name::new("help card"),
                    UiLayout::window()
                        .pos(Rl(50.0))
                        .anchor(Anchor::CENTER)
                        .size((Ab(CARD_W), Ab(card_height())))
                        .pack(),
                ))
                .with_children(|card| {
                    card.spawn(kit.skin(theme::card(), 0.0));
                    text_at(
                        card,
                        &kit,
                        CARD_PAD.x,
                        CARD_PAD.y + 11.0,
                        "CONTROLS",
                        TextStyle::TITLE,
                        theme::ACCENT,
                    );
                    let rows_top = CARD_PAD.y + 22.0 + 10.0;
                    for (i, (trigger, action)) in HELP_ROWS.iter().enumerate() {
                        let y = rows_top + ROW_H * (i as f32 + 0.5);
                        match trigger {
                            Trigger::Keys(keys) => {
                                let mut x = CARD_PAD.x;
                                for key in *keys {
                                    key_cap(card, &mut kit, x, y, key);
                                    x += cap_width(key) + CAP_GAP;
                                }
                            }
                            Trigger::Mouse(gesture) => {
                                text_at(
                                    card,
                                    &kit,
                                    CARD_PAD.x,
                                    y,
                                    gesture,
                                    TextStyle::BODY,
                                    theme::MUTED,
                                );
                            }
                        }
                        text_at(
                            card,
                            &kit,
                            CARD_PAD.x + ACTION_X,
                            y,
                            action,
                            TextStyle::BODY,
                            theme::TEXT,
                        );
                    }
                    // The footer: "Press ? or Esc to close."
                    let y = rows_top + ROW_H * HELP_ROWS.len() as f32 + 10.0 + CAP_H / 2.0;
                    let mut x = CARD_PAD.x;
                    for part in ["Press", "?", "or", "Esc", "to close."] {
                        if part == "?" || part == "Esc" {
                            key_cap(card, &mut kit, x, y, part);
                            x += cap_width(part) + CAP_GAP;
                        } else {
                            text_at(card, &kit, x, y, part, TextStyle::SMALL, theme::MUTED);
                            x += part.chars().count() as f32 * TextStyle::SMALL.average_advance()
                                + CAP_GAP;
                        }
                    }
                });
        });
    });
}

/// A line of text with its left edge at (`x`, `y`) from the card's top left.
fn text_at(
    card: &mut ChildSpawnerCommands,
    kit: &Kit,
    x: f32,
    y: f32,
    text: &str,
    style: TextStyle,
    color: Color,
) {
    card.spawn((
        Name::new("text"),
        UiLayout::window()
            .pos((Ab(x), Ab(y)))
            .anchor(Anchor::CENTER_LEFT)
            .size((Ab(300.0), Ab(22.0)))
            .pack(),
        Pickable::IGNORE,
    ))
    .with_children(|t| {
        t.spawn(kit.text(text, style, color, Pin::left(0.0)));
    });
}

/// A key cap with its left edge at (`x`, `y`).
fn key_cap(card: &mut ChildSpawnerCommands, kit: &mut Kit, x: f32, y: f32, key: &str) {
    card.spawn((
        Name::new("key cap"),
        UiLayout::window()
            .pos((Ab(x), Ab(y)))
            .anchor(Anchor::CENTER_LEFT)
            .size((Ab(cap_width(key)), Ab(CAP_H)))
            .pack(),
        Pickable::IGNORE,
    ))
    .with_children(|cap| {
        cap.spawn(kit.skin(
            Look::bordered(theme::INPUT_BG, theme::HUD_BORDER, theme::BUTTON_RADIUS),
            0.0,
        ));
        cap.spawn(kit.text(key, TextStyle::MONO, theme::TEXT, Pin::CENTER));
    });
}
