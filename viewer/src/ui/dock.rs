//! The scan dock, bottom centre: the `[ SCAN ]` block, with the scan's
//! features in a row of `[x]` toggles above it, the network field and the
//! interface at its sides, and an options drawer for the rest. While a
//! scan runs the block reads `[ STOP ]` and the stage and its progress are
//! written beside it as text (`arp  12/254  ████████░░░░░░░░`).
//!
//! The dock only edits the [`ScanForm`] and sends the same [`StartScan`]
//! message `--scan` does; scan.rs does the rest. Clear empties the map: a
//! scan adds to the map that is there, and nothing else takes a host off it.

use bevy::prelude::*;
use bevy::sprite::Anchor;
use bevy_lunex::prelude::*;

use crate::ViewerSet;
use crate::graph::MapDocument;
use crate::panel::{Panel, sidebar_width};
use crate::picking::SelectHost;
use crate::scan::{ScanJob, ScanSettings, StartScan};
use crate::scanform::{Feature, ScanForm};
use crate::theme;
use crate::ui::shape::{Look, ShapeAssets, ShapeMaterial};
use crate::ui::text_input::{Submitted, TextField, text_field};
use crate::ui::widgets::{
    ButtonSpec, Face, Fonts, Kit, Pin, Styled, TextStyle, Toggle, Tooltip, button, toggle, truncate,
};
use crate::ui::{PANEL_Z, Status, UiRoot};

/// The dock's box, and the Scan block in it.
const DOCK_W: f32 = 940.0;
const BLOCK_W: f32 = 200.0;
const BLOCK_H: f32 = 48.0;
/// The toggles sit in a row this far above the block, their centres this
/// far apart (a toggle is at most `[x] TRACE`, 86 px, so there is air
/// between them).
const TOGGLE_H: f32 = 24.0;
const TOGGLE_GAP: f32 = 22.0;
const TOGGLE_PITCH: f32 = 106.0;
/// The strip beside the block: the field and the buttons are this tall,
/// this far from the block, this far apart.
const STRIP_H: f32 = 34.0;
const STRIP_GAP: f32 = 14.0;
const ITEM_GAP: f32 = 8.0;
const NETWORK_W: f32 = 250.0;
const IFACE_W: f32 = 92.0;
const OPTIONS_W: f32 = 60.0;
const CLEAR_W: f32 = 64.0;
/// The options drawer opens above the toggles, centred on the block.
const DRAWER_W: f32 = 318.0;
const DRAWER_GAP: f32 = 10.0;
const DRAWER_ROW_H: f32 = 32.0;
const DRAWER_PAD: f32 = 10.0;
const DRAWER_KEY_W: f32 = 96.0;
const DRAWER_H: f32 = 4.0 * DRAWER_ROW_H + 2.0 * DRAWER_PAD;
/// The box holds the strip, the toggles and the open drawer.
const DOCK_H: f32 =
    2.0 + BLOCK_H + TOGGLE_GAP + TOGGLE_H + DRAWER_GAP + DRAWER_H + theme::PANEL_GLOW;
/// How much of the window's bottom the dock covers when the drawer is
/// shut, for the framing.
pub const DOCK_INSET: f32 = theme::EDGE + 2.0 + BLOCK_H + TOGGLE_GAP + TOGGLE_H + 8.0;
/// The block's glow at rest and under the pointer: the one glow that is
/// meant to be seen, and still a modest one.
const SCAN_GLOW: f32 = 10.0;
const SCAN_GLOW_HOVER: f32 = 16.0;
/// The progress bar is this many cells of `█` and `░`.
const BAR_CELLS: usize = 16;
/// The block's label.
const SCAN_LABEL: TextStyle = TextStyle {
    face: Face::MonoBold,
    px: 18.0,
};

/// The block's centre in the dock, from the top left.
const BUTTON_CENTER: Vec2 = Vec2::new(DOCK_W / 2.0, DOCK_H - BLOCK_H / 2.0 - 2.0);
/// The toggle row's centre line.
const TOGGLE_Y: f32 = BUTTON_CENTER.y - BLOCK_H / 2.0 - TOGGLE_GAP - TOGGLE_H / 2.0;
// The row is above the block, and the open drawer above the row, inside the box.
const _: () = assert!(TOGGLE_Y + TOGGLE_H / 2.0 < BUTTON_CENTER.y - BLOCK_H / 2.0);
const _: () = assert!(TOGGLE_Y - TOGGLE_H / 2.0 - DRAWER_GAP - DRAWER_H >= 0.0);

#[derive(Component)]
struct Dock;
#[derive(Component)]
struct ScanButton;
#[derive(Component)]
struct ScanLabel;
#[derive(Component)]
struct StageText;
#[derive(Component)]
struct NetworkField;
#[derive(Component)]
struct IfaceLabel;
#[derive(Component)]
struct TimeoutLabel;
#[derive(Component)]
struct ConcurrencyLabel;
#[derive(Component)]
struct TcpField;
#[derive(Component)]
struct UdpField;
#[derive(Component)]
struct Drawer;
#[derive(Component)]
struct ClearButton;
/// Hidden while a scan runs (the stage text takes their place).
#[derive(Component)]
struct IdleOnly;
#[derive(Component)]
struct FeatureChip(Feature);

#[derive(Resource, Debug, Default)]
struct DockState {
    drawer_open: bool,
}

pub struct DockPlugin;

impl Plugin for DockPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DockState>()
            .add_systems(Startup, spawn_dock)
            .add_systems(
                Update,
                (
                    read_fields,
                    show_form,
                    show_scan_state,
                    show_drawer,
                    show_clear,
                    center_dock,
                    submit_network,
                )
                    .in_set(ViewerSet::Draw),
            );
    }
}

/// Where toggle `i` of `count` sits, from the dock's top left: the row is
/// centred on the block.
fn toggle_center(i: usize, count: usize) -> Vec2 {
    let offset = i as f32 - (count as f32 - 1.0) / 2.0;
    Vec2::new(BUTTON_CENTER.x + offset * TOGGLE_PITCH, TOGGLE_Y)
}

/// A window layout centred at `center` (from the parent's top left).
fn at(center: Vec2, size: Vec2) -> UiLayout {
    UiLayout::window()
        .pos((Ab(center.x), Ab(center.y)))
        .anchor(Anchor::CENTER)
        .size((Ab(size.x), Ab(size.y)))
        .pack()
}

/// The block's looks at rest and under the pointer: lit while idle, in the
/// stop colour while a scan runs.
fn scan_looks(running: bool) -> (Look, Look) {
    let (fill, fill_hover, stroke, glow) = if running {
        (
            theme::STOP_FILL,
            theme::STOP_FILL_HOVER,
            theme::STOP,
            theme::STOP_GLOW,
        )
    } else {
        (
            theme::ACCENT_FILL,
            theme::ACCENT_FILL_HOVER,
            theme::ACCENT,
            theme::ACCENT_GLOW,
        )
    };
    let base = Look {
        fill,
        stroke,
        border: 2.0,
        radius: theme::BUTTON_RADIUS,
        ..Look::default()
    }
    .with_glow(glow, SCAN_GLOW);
    let hover = Look {
        fill: fill_hover,
        ..base
    }
    .with_glow(glow, SCAN_GLOW_HOVER);
    (base, hover)
}

/// The stage as the dock writes it: the name, `done/total` and a bar of
/// `BAR_CELLS` cells; the name alone while a stage reports no steps.
pub fn progress_text(stage: &str, progress: Option<(usize, usize)>) -> String {
    match progress {
        Some((done, total)) if total > 0 => {
            let filled = (done * BAR_CELLS / total).min(BAR_CELLS);
            let bar: String = "█".repeat(filled) + &"░".repeat(BAR_CELLS - filled);
            format!("{stage}  {done}/{total}  {bar}")
        }
        _ => stage.to_string(),
    }
}

#[allow(clippy::too_many_lines)] // one dock, laid out once
fn spawn_dock(
    mut commands: Commands,
    root: Res<UiRoot>,
    fonts: Res<Fonts>,
    shapes: Res<ShapeAssets>,
    mut materials: ResMut<Assets<ShapeMaterial>>,
    form: Res<ScanForm>,
    panel: Res<Panel>,
) {
    let mut kit = Kit {
        fonts: &fonts,
        shapes: &shapes,
        materials: &mut materials,
    };
    let (rest, hover) = scan_looks(false);
    commands.entity(root.entity).with_children(|ui| {
        ui.spawn((
            Name::new("dock"),
            Dock,
            UiLayout::window()
                .pos((Rl(50.0) - Ab(dock_shift(&panel)), Rl(100.0) - Ab(theme::EDGE)))
                .anchor(Anchor::BOTTOM_CENTER)
                .size((Ab(DOCK_W), Ab(DOCK_H)))
                .pack(),
            UiDepth::Set(PANEL_Z),
            Pickable::IGNORE,
        ))
        .with_children(|dock| {
            // The Scan block.
            let mut scan = dock.spawn((
                Name::new("scan button"),
                ScanButton,
                at(BUTTON_CENTER, Vec2::new(BLOCK_W, BLOCK_H)),
                crate::ui::widgets::hoverable(),
                OnHoverSetCursor::new(bevy::window::SystemCursorIcon::Pointer),
                Tooltip("find the hosts; nothing touches the network before this".into()),
            ));
            let mut skin = Entity::PLACEHOLDER;
            let mut label = Entity::PLACEHOLDER;
            scan.with_children(|b| {
                skin = b.spawn(kit.skin(rest, SCAN_GLOW_HOVER)).id();
                label = b
                    .spawn((
                        kit.text("[ SCAN ]", SCAN_LABEL, theme::ACCENT, Pin::CENTER),
                        ScanLabel,
                    ))
                    .id();
            });
            scan.insert(Styled {
                skin,
                base: rest,
                hover,
                text: Some((label, theme::ACCENT, theme::ACCENT)),
            });
            scan.observe(hover_set::<PointerOver, true>)
                .observe(hover_set::<PointerOut, false>)
                .observe(scan_pressed);

            // The toggles, in a row above the block.
            for (i, feature) in Feature::ALL.into_iter().enumerate() {
                let entity = toggle(
                    dock,
                    &mut kit,
                    feature.label(),
                    at(
                        toggle_center(i, Feature::ALL.len()),
                        Vec2::new(80.0, TOGGLE_H),
                    ),
                    &feature.label().to_uppercase(),
                    form.enabled(feature),
                    feature.describe(),
                    move |click: On<PointerClick>, mut form: ResMut<ScanForm>| {
                        if click.button == PointerButton::Primary {
                            form.toggle(feature);
                        }
                    },
                );
                dock.commands().entity(entity).insert(FeatureChip(feature));
            }

            // Left of the block: the network.
            let network_center =
                BUTTON_CENTER - Vec2::new(BLOCK_W / 2.0 + STRIP_GAP + NETWORK_W / 2.0, 0.0);
            let field = text_field(
                dock,
                &mut kit,
                "network field",
                at(network_center, Vec2::new(NETWORK_W, STRIP_H)),
                "net>",
                &form.network,
                "the interface's subnet",
                TextStyle::MONO,
            );
            dock.commands().entity(field).insert((
                NetworkField,
                Tooltip("the network to sweep: 192.168.1.0/24, or one address; empty for the interface's own subnet".into()),
            ));

            // Right of the block: the interface, the options, Clear.
            let mut x = BUTTON_CENTER.x + BLOCK_W / 2.0 + STRIP_GAP;
            let iface_layout = at(
                Vec2::new(x + IFACE_W / 2.0, BUTTON_CENTER.y),
                Vec2::new(IFACE_W, STRIP_H),
            );
            let mut iface = dock.spawn((
                Name::new("interface"),
                IdleOnly,
                iface_layout,
                crate::ui::widgets::hoverable(),
                OnHoverSetCursor::new(bevy::window::SystemCursorIcon::Pointer),
                Tooltip("the interface to scan from; click to go round them".into()),
            ));
            let mut iface_skin = Entity::PLACEHOLDER;
            let plain = Look::bordered(theme::INPUT_BG, theme::HUD_BORDER, theme::BUTTON_RADIUS);
            let plain_hover =
                Look::bordered(theme::INPUT_BG, theme::HUD_BORDER_HOVER, theme::BUTTON_RADIUS);
            iface.with_children(|b| {
                iface_skin = b.spawn(kit.skin(plain, 0.0)).id();
                b.spawn(kit.text("if>", TextStyle::MONO, theme::MUTED, Pin::left(9.0)));
                b.spawn((
                    kit.text(
                        form.interface_label(),
                        TextStyle::MONO,
                        theme::TEXT,
                        Pin::left(9.0 + TextStyle::MONO.width_of("if> ")),
                    ),
                    IfaceLabel,
                ));
            });
            iface.insert(Styled {
                skin: iface_skin,
                base: plain,
                hover: plain_hover,
                text: None,
            });
            iface
                .observe(hover_set::<PointerOver, true>)
                .observe(hover_set::<PointerOut, false>)
                .observe(|click: On<PointerClick>, mut form: ResMut<ScanForm>| {
                    if click.button == PointerButton::Primary {
                        form.cycle_interface();
                    }
                });
            x += IFACE_W + ITEM_GAP;

            let options = ButtonSpec::plain(
                "options button",
                at(
                    Vec2::new(x + OPTIONS_W / 2.0, BUTTON_CENTER.y),
                    Vec2::new(OPTIONS_W, STRIP_H),
                ),
                "opts",
            )
            .style(TextStyle::MONO)
            .tooltip("timeout, probes at once, the port lists");
            let options_entity = button(
                dock,
                &mut kit,
                strip_button(options),
                |click: On<PointerClick>, mut state: ResMut<DockState>| {
                    if click.button == PointerButton::Primary {
                        state.drawer_open = !state.drawer_open;
                    }
                },
            );
            dock.commands().entity(options_entity).insert(IdleOnly);
            x += OPTIONS_W + ITEM_GAP;

            let clear = ButtonSpec::plain(
                "clear button",
                at(
                    Vec2::new(x + CLEAR_W / 2.0, BUTTON_CENTER.y),
                    Vec2::new(CLEAR_W, STRIP_H),
                ),
                "clear",
            )
            .style(TextStyle::MONO)
            .tooltip("forget every host; the next scan starts a map afresh");
            let clear_entity = button(dock, &mut kit, strip_button(clear), clear_pressed);
            dock.commands()
                .entity(clear_entity)
                .insert((IdleOnly, ClearButton));

            // While a scan runs: the stage, where the buttons were.
            dock.spawn((
                Name::new("stage"),
                UiLayout::window()
                    .pos((
                        Ab(BUTTON_CENTER.x + BLOCK_W / 2.0 + STRIP_GAP),
                        Ab(BUTTON_CENTER.y),
                    ))
                    .anchor(Anchor::CENTER_LEFT)
                    .size((Ab(320.0), Ab(STRIP_H)))
                    .pack(),
                Pickable::IGNORE,
                Visibility::Hidden,
            ))
            .with_children(|stage| {
                stage.spawn((
                    kit.text("", TextStyle::MONO, theme::TEXT, Pin::left(0.0)),
                    StageText,
                ));
            });

            // The options drawer, above the toggles.
            spawn_drawer(dock, &mut kit, &form);
        });
    });
}

/// A strip button: a box like the fields, dim text that lights up.
fn strip_button(spec: ButtonSpec) -> ButtonSpec {
    ButtonSpec {
        text: theme::MUTED,
        text_hover: theme::TEXT,
        base: Look::bordered(theme::INPUT_BG, theme::HUD_BORDER, theme::BUTTON_RADIUS),
        hover: Look::bordered(
            theme::INPUT_BG,
            theme::HUD_BORDER_HOVER,
            theme::BUTTON_RADIUS,
        ),
        ..spec
    }
}

/// The drawer: timeout and probes-at-once steppers, the TCP and UDP port lists.
#[allow(clippy::too_many_lines)] // four rows, laid out once
fn spawn_drawer(dock: &mut ChildSpawnerCommands, kit: &mut Kit, form: &ScanForm) {
    let bottom = TOGGLE_Y - TOGGLE_H / 2.0 - DRAWER_GAP;
    dock.spawn((
        Name::new("options drawer"),
        Drawer,
        UiLayout::window()
            .pos((Ab(BUTTON_CENTER.x), Ab(bottom)))
            .anchor(Anchor::BOTTOM_CENTER)
            .size((Ab(DRAWER_W), Ab(DRAWER_H)))
            .pack(),
        // Over anything it meets, should a narrow window push things together.
        UiDepth::Add(2.0),
        Visibility::Hidden,
    ))
    .with_children(|drawer| {
        drawer.spawn(kit.skin(theme::card(), 0.0));
        let row_center = |i: f32| DRAWER_PAD + DRAWER_ROW_H * (i + 0.5);
        let key = |drawer: &mut ChildSpawnerCommands, kit: &mut Kit, i: f32, text: &str| {
            drawer
                .spawn((
                    Name::new("key"),
                    UiLayout::window()
                        .pos((Ab(DRAWER_PAD), Ab(row_center(i))))
                        .anchor(Anchor::CENTER_LEFT)
                        .size((Ab(DRAWER_KEY_W), Ab(DRAWER_ROW_H)))
                        .pack(),
                    Pickable::IGNORE,
                ))
                .with_children(|k| {
                    k.spawn(kit.text(text, TextStyle::MONO, theme::MUTED, Pin::left(2.0)));
                });
        };
        let control_x = DRAWER_PAD + DRAWER_KEY_W;
        let control_w = DRAWER_W - control_x - DRAWER_PAD;

        key(drawer, kit, 0.0, "timeout  :");
        stepper(
            drawer,
            kit,
            control_x,
            row_center(0.0),
            &form.timeout_label(),
            TimeoutLabel,
            "how long each ping or connection may take",
            |click: On<PointerClick>, mut form: ResMut<ScanForm>| {
                if click.button == PointerButton::Primary {
                    form.step_timeout(-1);
                }
            },
            |click: On<PointerClick>, mut form: ResMut<ScanForm>| {
                if click.button == PointerButton::Primary {
                    form.step_timeout(1);
                }
            },
        );
        key(drawer, kit, 1.0, "at once  :");
        stepper(
            drawer,
            kit,
            control_x,
            row_center(1.0),
            &form.concurrency_label(),
            ConcurrencyLabel,
            "probes in flight at once (a TCP scan opens at most 16 to one host)",
            |click: On<PointerClick>, mut form: ResMut<ScanForm>| {
                if click.button == PointerButton::Primary {
                    form.step_concurrency(-1);
                }
            },
            |click: On<PointerClick>, mut form: ResMut<ScanForm>| {
                if click.button == PointerButton::Primary {
                    form.step_concurrency(1);
                }
            },
        );
        key(drawer, kit, 2.0, "tcp ports:");
        let tcp = text_field(
            drawer,
            kit,
            "tcp field",
            UiLayout::window()
                .pos((Ab(control_x), Ab(row_center(2.0))))
                .anchor(Anchor::CENTER_LEFT)
                .size((Ab(control_w), Ab(26.0)))
                .pack(),
            "",
            &form.tcp_ports,
            "off; common or 22,80,8000-8010",
            TextStyle::MONO,
        );
        drawer.commands().entity(tcp).insert(TcpField);
        key(drawer, kit, 3.0, "udp ports:");
        let udp = text_field(
            drawer,
            kit,
            "udp field",
            UiLayout::window()
                .pos((Ab(control_x), Ab(row_center(3.0))))
                .anchor(Anchor::CENTER_LEFT)
                .size((Ab(control_w), Ab(26.0)))
                .pack(),
            "",
            &form.udp_ports,
            "off; common or 53,123",
            TextStyle::MONO,
        );
        drawer.commands().entity(udp).insert(UdpField);
    });
}

/// `[-]  value  [+]` at `x`, centred on `y`.
#[allow(clippy::too_many_arguments)]
fn stepper<M1, M2>(
    drawer: &mut ChildSpawnerCommands,
    kit: &mut Kit,
    x: f32,
    y: f32,
    value: &str,
    marker: impl Component,
    tip: &str,
    down: impl bevy::ecs::observer::IntoEntityObserver<M1>,
    up: impl bevy::ecs::observer::IntoEntityObserver<M2>,
) {
    let size = 24.0;
    let value_w = 76.0;
    let sign = |name: &'static str, cx: f32, label: &'static str| {
        ButtonSpec {
            text: theme::MUTED,
            text_hover: theme::ACCENT,
            ..ButtonSpec::ghost(name, at(Vec2::new(cx, y), Vec2::splat(size)), label)
        }
        .style(TextStyle::MONO)
    };
    button(drawer, kit, sign("step down", x + size / 2.0, "[-]"), down);
    drawer
        .spawn((
            Name::new("value"),
            at(
                Vec2::new(x + size + value_w / 2.0, y),
                Vec2::new(value_w, size),
            ),
            Tooltip(tip.to_string()),
        ))
        .with_children(|v| {
            v.spawn((
                kit.text(value, TextStyle::MONO, theme::TEXT, Pin::CENTER),
                marker,
            ));
        });
    button(
        drawer,
        kit,
        sign("step up", x + size + value_w + size / 2.0, "[+]"),
        up,
    );
}

/// How far left of the window's centre the dock sits, to be centred in the
/// part of the window the rail leaves free.
fn dock_shift(panel: &Panel) -> f32 {
    if panel.collapsed {
        0.0
    } else {
        f32::midpoint(sidebar_width(1280.0), theme::EDGE)
    }
}

/// Scan starts a scan with the form's settings; Stop cancels the running one.
fn scan_pressed(
    click: On<PointerClick>,
    job: Option<Res<ScanJob>>,
    mut form: ResMut<ScanForm>,
    mut settings: ResMut<ScanSettings>,
    mut start: MessageWriter<StartScan>,
    mut status: ResMut<Status>,
) {
    if click.button != PointerButton::Primary {
        return;
    }
    if let Some(job) = job {
        job.cancel();
        return;
    }
    start_scan(&mut form, &mut settings, &mut start, &mut status);
}

fn start_scan(
    form: &mut ScanForm,
    settings: &mut ScanSettings,
    start: &mut MessageWriter<StartScan>,
    status: &mut Status,
) {
    match form.settings(settings.save.clone()) {
        Ok(wanted) => {
            form.error = None;
            *settings = wanted;
            start.write(StartScan);
        }
        Err(err) => {
            status.show_error(err.clone());
            form.error = Some(err);
        }
    }
}

/// Enter in the network field is Scan.
fn submit_network(
    mut submitted: MessageReader<Submitted>,
    fields: Query<(), With<NetworkField>>,
    job: Option<Res<ScanJob>>,
    mut form: ResMut<ScanForm>,
    mut settings: ResMut<ScanSettings>,
    mut start: MessageWriter<StartScan>,
    mut status: ResMut<Status>,
) {
    let any = submitted.read().any(|s| fields.get(s.0).is_ok());
    if any && job.is_none() {
        start_scan(&mut form, &mut settings, &mut start, &mut status);
    }
}

fn clear_pressed(
    click: On<PointerClick>,
    mut doc: ResMut<MapDocument>,
    mut status: ResMut<Status>,
    mut select: MessageWriter<SelectHost>,
) {
    if click.button != PointerButton::Primary {
        return;
    }
    doc.0 = None;
    status.show_message("map cleared");
    select.write(SelectHost {
        id: None,
        fly: false,
    });
}

/// What is typed in the fields goes into the form; typing clears an error.
fn read_fields(
    mut fields: Query<
        (
            &mut TextField,
            Has<NetworkField>,
            Has<TcpField>,
            Has<UdpField>,
        ),
        Changed<TextField>,
    >,
    mut form: ResMut<ScanForm>,
) {
    for (mut field, network, tcp, udp) in &mut fields {
        let target = if network {
            &mut form.network
        } else if tcp {
            &mut form.tcp_ports
        } else if udp {
            &mut form.udp_ports
        } else {
            continue;
        };
        if *target != field.value {
            target.clone_from(&field.value);
            form.error = None;
        }
        if field.error && form.error.is_none() {
            field.error = false;
        }
    }
}

/// The form shows in the toggles, the labels and the fields.
fn show_form(
    form: Res<ScanForm>,
    mut chips: Query<(&FeatureChip, &mut Toggle)>,
    mut iface: Query<
        &mut Text2d,
        (
            With<IfaceLabel>,
            Without<TimeoutLabel>,
            Without<ConcurrencyLabel>,
        ),
    >,
    mut timeout: Query<&mut Text2d, (With<TimeoutLabel>, Without<ConcurrencyLabel>)>,
    mut concurrency: Query<&mut Text2d, With<ConcurrencyLabel>>,
    mut fields: Query<(
        &mut TextField,
        Has<NetworkField>,
        Has<TcpField>,
        Has<UdpField>,
    )>,
) {
    if !form.is_changed() {
        return;
    }
    for (chip, mut toggle) in &mut chips {
        let on = form.enabled(chip.0);
        if toggle.on != on {
            toggle.on = on;
        }
    }
    set_text(&mut iface, &form.interface_label());
    set_text(&mut timeout, &form.timeout_label());
    set_text(&mut concurrency, &form.concurrency_label());
    for (mut field, network, tcp, udp) in &mut fields {
        let (value, error) = if network {
            (&form.network, form.error.is_some())
        } else if tcp {
            (&form.tcp_ports, false)
        } else if udp {
            (&form.udp_ports, false)
        } else {
            continue;
        };
        if field.value != *value {
            field.value.clone_from(value);
        }
        if field.error != error {
            field.error = error;
        }
    }
}

fn set_text<F: bevy::ecs::query::QueryFilter>(texts: &mut Query<&mut Text2d, F>, wanted: &str) {
    for mut text in texts {
        if text.0 != wanted {
            text.0 = wanted.to_string();
        }
    }
}

/// While a scan runs the block reads Stop; the buttons beside it make way
/// for the stage and its progress.
#[allow(clippy::too_many_arguments)] // a Bevy system lists what it reads
fn show_scan_state(
    job: Option<Res<ScanJob>>,
    mut button: Query<&mut Styled, With<ScanButton>>,
    mut label: Query<&mut Text2d, (With<ScanLabel>, Without<StageText>)>,
    mut stage: Query<(&ChildOf, &mut Text2d), With<StageText>>,
    mut visibility: Query<&mut Visibility>,
    idle_only: Query<Entity, With<IdleOnly>>,
    dock_state: Res<DockState>,
    mut was_running: Local<bool>,
) {
    let running = job.is_some();
    let (base, hover) = scan_looks(running);
    for mut styled in &mut button {
        if styled.base != base {
            styled.base = base;
            styled.hover = hover;
            let lit = if running { theme::STOP } else { theme::ACCENT };
            styled.text = styled.text.map(|(entity, _, _)| (entity, lit, lit));
        }
    }
    let wanted = if running { "[ STOP ]" } else { "[ SCAN ]" };
    for mut text in &mut label {
        if text.0 != wanted {
            text.0 = wanted.to_string();
        }
    }
    for (child_of, mut text) in &mut stage {
        let line = match job.as_deref() {
            Some(job) => progress_text(&job.stage, job.progress),
            None => String::new(),
        };
        let line = truncate(&line, 320.0, TextStyle::MONO);
        if text.0 != line {
            text.0 = line;
        }
        if let Ok(mut v) = visibility.get_mut(child_of.parent()) {
            let want = if running {
                Visibility::Inherited
            } else {
                Visibility::Hidden
            };
            if *v != want {
                *v = want;
            }
        }
    }
    if running != *was_running || dock_state.is_changed() {
        *was_running = running;
        for entity in &idle_only {
            if let Ok(mut v) = visibility.get_mut(entity) {
                *v = if running {
                    Visibility::Hidden
                } else {
                    Visibility::Inherited
                };
            }
        }
    }
}

/// The drawer shows while it is open and no scan runs.
fn show_drawer(
    state: Res<DockState>,
    job: Option<Res<ScanJob>>,
    mut drawers: Query<&mut Visibility, With<Drawer>>,
) {
    let open = state.drawer_open && job.is_none();
    for mut v in &mut drawers {
        let want = if open {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *v != want {
            *v = want;
        }
    }
}

/// Clear shows while there is a map to clear (and no scan runs).
fn show_clear(
    doc: Res<MapDocument>,
    job: Option<Res<ScanJob>>,
    mut clears: Query<&mut Visibility, With<ClearButton>>,
) {
    let has_map = doc.0.as_ref().is_some_and(|map| !map.hosts.is_empty());
    let show = has_map && job.is_none();
    for mut v in &mut clears {
        let want = if show {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *v != want {
            *v = want;
        }
    }
}

/// The dock stays centred in the part of the window the rail leaves free.
fn center_dock(panel: Res<Panel>, mut docks: Query<&mut UiLayout, With<Dock>>) {
    if !panel.is_changed() {
        return;
    }
    for mut layout in &mut docks {
        if let Some(window) = layout.get_mut_window(UiBase::id()) {
            window.set_x(Rl(50.0) - Ab(dock_shift(&panel)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_toggles_sit_in_a_row_centred_on_the_block() {
        let count = Feature::ALL.len();
        let first = toggle_center(0, count);
        let middle = toggle_center(2, count);
        let last = toggle_center(count - 1, count);
        assert!((middle.x - BUTTON_CENTER.x).abs() < 1e-3, "{middle:?}");
        assert!(
            (first.x + last.x - 2.0 * BUTTON_CENTER.x).abs() < 1e-3,
            "symmetric"
        );
        assert!((last.x - first.x - 4.0 * TOGGLE_PITCH).abs() < 1e-3);
        // The widest toggle, `[x] TRACE`, leaves air to the next one.
        assert!(TextStyle::LABEL.width_of("[x] TRACE") + 2.0 * theme::CHIP_PAD_X < TOGGLE_PITCH);
    }

    #[test]
    fn the_block_turns_to_stop_while_running() {
        let (idle, idle_hover) = scan_looks(false);
        assert_eq!(idle.stroke, theme::ACCENT);
        assert!(idle_hover.glow_radius > idle.glow_radius);
        let (running, _) = scan_looks(true);
        assert_eq!(running.stroke, theme::STOP);
        assert_eq!(running.border, 2.0);
    }

    #[test]
    fn progress_is_written_as_a_bar() {
        assert_eq!(
            progress_text("arp", Some((64, 256))),
            "arp  64/256  ████░░░░░░░░░░░░"
        );
        assert_eq!(
            progress_text("tcp ports", Some((10, 10))),
            "tcp ports  10/10  ████████████████"
        );
        assert_eq!(progress_text("names", None), "names");
        assert_eq!(progress_text("names", Some((3, 0))), "names");
    }
}
