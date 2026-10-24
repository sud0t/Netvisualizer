//! The HUD, top left: a two-line box with the name, the version and the
//! status line, then the fly speed and the key hints `[?] help`, `[/]
//! filter` and `[R] reset`, which are buttons too. A thin phosphor line
//! along the box's bottom edge fills while a scan runs. The legend, under
//! it: what the colours mean.

use bevy::prelude::*;
use bevy::sprite::Anchor;
use bevy_lunex::prelude::*;

use crate::ViewerSet;
use crate::controls::FlyControls;
use crate::map::Role;
use crate::panel::Panel;
use crate::scan::ScanJob;
use crate::scene::ResetView;
use crate::theme;
use crate::ui::shape::{Look, ShapeAssets, ShapeMaterial, Skin};
use crate::ui::widgets::{
    ButtonSpec, FitText, Fonts, Kit, Pin, TextStyle, Tooltip, button, truncate,
};
use crate::ui::{Help, PANEL_Z, Status, UiRoot, role_color, speed_text};

/// The box: two 20 px rows, 4 px apart, inside 12 × 8 px of padding.
const ROW_H: f32 = 20.0;
const ROW_GAP: f32 = 4.0;
const PAD: Vec2 = Vec2::new(12.0, 8.0);
const HUD_HEIGHT: f32 = 2.0 * PAD.y + 2.0 * ROW_H + ROW_GAP;
/// The status line is cut to the room between the brand and the rail,
/// within these bounds. The rest of its row (brand, version, separator,
/// padding and gaps) is about this wide.
const STATUS_MIN_PX: f32 = 120.0;
const STATUS_MAX_PX: f32 = 560.0;
const HUD_FIXED_PX: f32 = 200.0;
/// The legend's box is 28 px tall; its swatches are 8 px squares.
const LEGEND_HEIGHT: f32 = 28.0;
const SWATCH: f32 = 8.0;

#[derive(Component)]
struct StatusText;
#[derive(Component)]
struct SpeedText;
#[derive(Component)]
struct ProgressLine;

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, (spawn_hud, spawn_legend))
            .add_systems(
                Update,
                (show_status, show_speed, show_progress).in_set(ViewerSet::Draw),
            );
    }
}

/// A flow item holding one line of text, as wide as the text.
fn text_item(
    ui: &mut ChildSpawnerCommands,
    kit: &Kit,
    name: &'static str,
    text: &str,
    style: TextStyle,
    color: Color,
    marker: impl Bundle,
) {
    ui.spawn((
        Name::new(name),
        UiLayout::flow().width(Ab(1.0)).height(Ab(ROW_H)).pack(),
        FitText { pad: 0.0, min: 1.0 },
        Pickable::IGNORE,
    ))
    .with_children(|item| {
        item.spawn((kit.text(text, style, color, Pin::CENTER), marker));
    });
}

/// A row of the box: items side by side, centred on the row. The rows
/// carry the side padding: a hugging column ignores its own on the right.
fn row(name: &'static str, gap: f32) -> impl Bundle {
    (
        Name::new(name),
        UiLayout::flow()
            .direction(UiFlowDirection::LeftToRight)
            .height(Ab(ROW_H))
            .padding_x(Ab(PAD.x))
            .gap(Ab(gap))
            .align(Align::CENTER)
            .pack(),
        Pickable::IGNORE,
    )
}

/// A key hint that is also a button: `[?] help`, dim, lit under the pointer.
fn hint<'s>(name: &'static str, label: &'s str) -> ButtonSpec<'s> {
    ButtonSpec {
        style: TextStyle::MONO,
        text_hover: theme::ACCENT,
        fit: Some(4.0),
        ..ButtonSpec::ghost(
            name,
            UiLayout::flow().width(Ab(1.0)).height(Ab(ROW_H)).pack(),
            label,
        )
    }
}

#[allow(clippy::too_many_lines)] // one box, laid out once
fn spawn_hud(
    mut commands: Commands,
    root: Res<UiRoot>,
    fonts: Res<Fonts>,
    shapes: Res<ShapeAssets>,
    mut materials: ResMut<Assets<ShapeMaterial>>,
    status: Res<Status>,
    controls: Res<FlyControls>,
) {
    let mut kit = Kit {
        fonts: &fonts,
        shapes: &shapes,
        materials: &mut materials,
    };
    commands.entity(root.entity).with_children(|ui| {
        // A wide, invisible frame at the top left; the box inside hugs its
        // content (a flow inside an absolute node sits at its top left).
        ui.spawn((
            Name::new("hud frame"),
            UiLayout::window()
                .pos((Ab(theme::EDGE), Ab(theme::EDGE)))
                .anchor(Anchor::TOP_LEFT)
                .size((Ab(900.0), Ab(HUD_HEIGHT)))
                .pack(),
            UiDepth::Set(PANEL_Z),
            Pickable::IGNORE,
        ))
        .with_children(|frame| {
            frame
                .spawn((
                    Name::new("hud"),
                    UiLayout::flow()
                        .direction(UiFlowDirection::TopToBottom)
                        .padding_y(Ab(PAD.y))
                        .gap(Ab(ROW_GAP))
                        .pack(),
                ))
                .with_children(|hud| {
                    hud.spawn(kit.skin(theme::panel(), theme::PANEL_GLOW));

                    // Line 1: NETVISUALIZER v0.1 // the status.
                    hud.spawn(row("title row", 8.0)).with_children(|line| {
                        text_item(
                            line,
                            &kit,
                            "brand",
                            "NETVISUALIZER",
                            TextStyle::TITLE,
                            theme::ACCENT,
                            (),
                        );
                        text_item(
                            line,
                            &kit,
                            "version",
                            concat!("v", env!("CARGO_PKG_VERSION")),
                            TextStyle::MONO,
                            theme::MUTED,
                            (),
                        );
                        text_item(
                            line,
                            &kit,
                            "separator",
                            "//",
                            TextStyle::MONO,
                            theme::MUTED,
                            (),
                        );
                        let color = if status.error {
                            theme::ERROR
                        } else {
                            theme::TEXT
                        };
                        text_item(
                            line,
                            &kit,
                            "status",
                            &truncate(&status.text, STATUS_MAX_PX, TextStyle::MONO),
                            TextStyle::MONO,
                            color,
                            StatusText,
                        );
                    });

                    // Line 2: the fly speed and the key hints.
                    hud.spawn(row("hint row", 12.0)).with_children(|line| {
                        line.spawn((
                            Name::new("speed item"),
                            UiLayout::flow().width(Ab(1.0)).height(Ab(ROW_H)).pack(),
                            FitText { pad: 0.0, min: 1.0 },
                            Tooltip("fly speed: the mouse wheel changes it".into()),
                        ))
                        .with_children(|item| {
                            item.spawn((
                                kit.text(
                                    fly_text(controls.speed),
                                    TextStyle::MONO,
                                    theme::MUTED,
                                    Pin::CENTER,
                                ),
                                SpeedText,
                            ));
                        });
                        button(
                            line,
                            &mut kit,
                            hint("help button", "[?] help").tooltip("the controls"),
                            |click: On<PointerClick>, mut help: ResMut<Help>| {
                                if click.button == PointerButton::Primary {
                                    help.open = !help.open;
                                }
                            },
                        );
                        button(
                            line,
                            &mut kit,
                            hint("filter button", "[/] filter").tooltip("filter the host list"),
                            |click: On<PointerClick>, mut panel: ResMut<Panel>| {
                                if click.button == PointerButton::Primary {
                                    panel.focus_filter();
                                }
                            },
                        );
                        button(
                            line,
                            &mut kit,
                            hint("reset button", "[R] reset").tooltip("fly back to the whole map"),
                            |click: On<PointerClick>, mut reset: MessageWriter<ResetView>| {
                                if click.button == PointerButton::Primary {
                                    reset.write(ResetView { animate: true });
                                }
                            },
                        );
                    });

                    // The progress line: a track along the bottom edge, inside
                    // the border, and the fill from its left. (The fill is a
                    // fraction of the track, never a width worked out by
                    // subtraction: a negative width panics lunex's picking.)
                    hud.spawn((
                        Name::new("progress track"),
                        UiLayout::window()
                            .pos((Ab(1.0), Rl(100.0) - Ab(1.0)))
                            .anchor(Anchor::BOTTOM_LEFT)
                            .size((Rl(100.0) - Ab(2.0), Ab(2.0)))
                            .pack(),
                        Pickable::IGNORE,
                    ))
                    .with_children(|track| {
                        track
                            .spawn((
                                Name::new("progress"),
                                UiLayout::window().size((Rl(0.0), Rl(100.0))).pack(),
                                ProgressLine,
                                Pickable::IGNORE,
                            ))
                            .with_children(|line| {
                                line.spawn(kit.skin(Look::filled(theme::ACCENT, 0.0), 0.0));
                            });
                    });
                });
        });
    });
}

/// The speed as the HUD writes it: `fly 18 u/s`.
fn fly_text(speed: f32) -> String {
    format!("fly {}", speed_text(speed))
}

fn spawn_legend(
    mut commands: Commands,
    root: Res<UiRoot>,
    fonts: Res<Fonts>,
    shapes: Res<ShapeAssets>,
    mut materials: ResMut<Assets<ShapeMaterial>>,
) {
    let mut kit = Kit {
        fonts: &fonts,
        shapes: &shapes,
        materials: &mut materials,
    };
    commands.entity(root.entity).with_children(|ui| {
        // Under the HUD, so the bottom-left corner stays free for the dock.
        ui.spawn((
            Name::new("legend frame"),
            UiLayout::window()
                .pos((Ab(theme::EDGE), Ab(theme::EDGE + HUD_HEIGHT + 8.0)))
                .anchor(Anchor::TOP_LEFT)
                .size((Ab(400.0), Ab(LEGEND_HEIGHT)))
                .pack(),
            UiDepth::Set(PANEL_Z),
            Pickable::IGNORE,
        ))
        .with_children(|frame| {
            frame
                .spawn((
                    Name::new("legend"),
                    UiLayout::flow()
                        .direction(UiFlowDirection::LeftToRight)
                        .height(Ab(LEGEND_HEIGHT))
                        .padding_x(Ab(PAD.x))
                        .gap(Ab(16.0))
                        .align(Align::CENTER)
                        .pack(),
                    Pickable::IGNORE,
                ))
                .with_children(|legend| {
                    legend.spawn(kit.skin(theme::panel(), theme::PANEL_GLOW));
                    for role in [Role::Scanner, Role::Gateway, Role::Router, Role::Host] {
                        legend
                            .spawn((
                                Name::new(role.name()),
                                UiLayout::flow()
                                    .direction(UiFlowDirection::LeftToRight)
                                    .height(Ab(ROW_H))
                                    .gap(Ab(6.0))
                                    .align(Align::CENTER)
                                    .pack(),
                                Pickable::IGNORE,
                            ))
                            .with_children(|item| {
                                item.spawn((
                                    Name::new("swatch"),
                                    UiLayout::flow().width(Ab(SWATCH)).height(Ab(SWATCH)).pack(),
                                    Pickable::IGNORE,
                                ))
                                .with_children(|swatch| {
                                    swatch
                                        .spawn(kit.skin(Look::filled(role_color(role), 0.0), 0.0));
                                });
                                text_item(
                                    item,
                                    &kit,
                                    "role name",
                                    role.name(),
                                    TextStyle::MONO,
                                    role_color(role),
                                    (),
                                );
                            });
                    }
                });
        });
    });
}

/// The status line, cut to the room between the brand and the rail.
fn show_status(
    status: Res<Status>,
    panel: Res<Panel>,
    root: Res<UiRoot>,
    roots: Query<&Dimension, Changed<Dimension>>,
    mut texts: Query<(&mut Text2d, &mut TextColor), With<StatusText>>,
    mut width: Local<f32>,
) {
    if let Ok(size) = roots.get(root.entity) {
        *width = size.x;
    }
    if !(status.is_changed() || panel.is_changed() || roots.get(root.entity).is_ok()) {
        return;
    }
    let space = *width - panel.frame_inset(*width).right - 2.0 * theme::EDGE - HUD_FIXED_PX;
    let max = space.clamp(STATUS_MIN_PX, STATUS_MAX_PX);
    for (mut text, mut color) in &mut texts {
        let wanted = truncate(&status.text, max, TextStyle::MONO);
        if text.0 != wanted {
            text.0 = wanted;
        }
        color.0 = if status.error {
            theme::ERROR
        } else {
            theme::TEXT
        };
    }
}

fn show_speed(controls: Res<FlyControls>, mut texts: Query<&mut Text2d, With<SpeedText>>) {
    if !controls.is_changed() {
        return;
    }
    let wanted = fly_text(controls.speed);
    for mut text in &mut texts {
        if text.0 != wanted {
            text.0.clone_from(&wanted);
        }
    }
}

/// The phosphor line along the HUD's bottom edge: as long as the running
/// stage is far, a quiet pulse while a stage reports no steps, gone when no
/// scan runs.
fn show_progress(
    job: Option<Res<ScanJob>>,
    time: Res<Time>,
    mut lines: Query<(&mut UiLayout, &Children), With<ProgressLine>>,
    mut skins: Query<&mut Skin>,
) {
    let fraction = job.as_deref().map(|job| match job.progress {
        Some((done, total)) if total > 0 => done as f32 / total as f32,
        _ => 0.5 + 0.5 * (time.elapsed_secs() * 2.0).sin(),
    });
    for (mut layout, children) in &mut lines {
        let Some(window) = layout.get_mut_window(UiBase::id()) else {
            continue;
        };
        let percent = fraction.map_or(0.0, |f| f.clamp(0.0, 1.0) * 100.0);
        window.set_width(Rl(percent));
        // The pulse fades the line instead of moving it when there are no steps.
        for child in children.iter() {
            if let Ok(mut skin) = skins.get_mut(child) {
                let alpha = if fraction.is_some() { 1.0 } else { 0.0 };
                let look = Look::filled(theme::ACCENT.with_alpha(alpha), 0.0);
                if skin.look != look {
                    skin.look = look;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_speed_is_written_as_a_fly_line() {
        assert_eq!(fly_text(18.0), "fly 18 u/s");
        assert_eq!(fly_text(2.5), "fly 2.5 u/s");
    }
}
