//! The host rail, down the right edge: a header band with the count and the
//! filter, the hosts as a table (role, name, address), and, under it, the
//! selected host as a key-value dump with its own band. Collapses to a tab.
//!
//! Both lists are virtual: a fixed set of row slots is laid out once, and
//! scrolling only changes which rows the slots show, so a /16 sweep's list
//! costs the same per frame as a /24's, and nothing needs clipping.
//!
//! The dump is generic on purpose: every field of the host JSON is written
//! as it comes (scalars as `key : value`, lists of objects as tables,
//! objects as nested keys), under its own key, so a field added to the
//! model shows up here with no viewer change.

use bevy::input::mouse::MouseScrollUnit;
use bevy::prelude::*;
use bevy::sprite::Anchor;
use bevy_lunex::prelude::*;
use serde_json::Value;

use crate::ViewerSet;
use crate::graph::MapDocument;
use crate::map::{Host, Role};
use crate::panel::{Panel, humanize, scalar_text, sidebar_width, table_columns};
use crate::picking::{Picker, SelectHost};
use crate::theme;
use crate::ui::shape::{Look, ShapeAssets, ShapeMaterial};
use crate::ui::text_input::{FocusField, Submitted, TextField, text_field};
use crate::ui::widgets::{
    ButtonSpec, FitText, Fonts, Kit, Pin, Styled, TextStyle, button, hoverable, truncate,
};
use crate::ui::{PANEL_Z, UiRoot, role_color};

/// A band's padding; the rows' padding; a band's height; the rule under it.
const PAD: f32 = 10.0;
const ROW_PAD_X: f32 = 8.0;
const HEAD_H: f32 = 30.0;
const RULE_H: f32 = 1.0;
/// A table row and a dump line: 12 px monospace with a little air.
const ROW_H: f32 = 21.0;
const LINE_H: f32 = 20.0;
/// The table's columns: the role word at the left, the name after it, the
/// address `ADDR_CHARS` characters from the right edge (an IPv4 address is
/// at most 15). The narrow rail has no room for the address column.
const ROLE_COL: f32 = ROW_PAD_X;
const HOST_COL: f32 = 68.0;
const ADDR_CHARS: usize = 15;
const ADDR_MIN_WIDTH: f32 = 300.0;
/// A dump key is padded to this many characters before its colon; the
/// value starts two characters after that.
const KEY_CHARS: usize = 10;
/// The details take at most this fraction of the rail.
const DETAILS_MAX_FRACTION: f32 = 0.55;
/// Air above and below the dump lines, and the footer with the buttons.
const LINES_GAP: f32 = 6.0;
const FOOT_H: f32 = 26.0;
const TAB_W: f32 = 32.0;
const TAB_H: f32 = 24.0;
/// Above the other panels: in a narrow window the dock reaches under the rail.
const RAIL_Z: f32 = PANEL_Z + 5.0;
/// Rows per notch of the wheel.
const ROWS_PER_LINE: f32 = 2.0;

#[derive(Component)]
struct Rail;
#[derive(Component)]
struct RailTab;
#[derive(Component)]
struct FilterField;
#[derive(Component)]
struct CountText;
/// The `ADDR` of the table's header line: it moves with the address column.
#[derive(Component)]
struct AddrHead;
#[derive(Component)]
struct Details;
#[derive(Component)]
struct DetailsTitle;
#[derive(Component)]
struct DetailsRole;
#[derive(Component)]
struct HostList;
#[derive(Component)]
struct DetailLines;

/// A list whose rows are a fixed set of slots showing a window of the data.
#[derive(Component, Debug, Default)]
struct VirtualList {
    row_h: f32,
    /// The first row shown.
    offset: usize,
    /// How many rows there are.
    total: usize,
    slots: Vec<Entity>,
}

impl VirtualList {
    /// Clamp the offset so the window stays on the data.
    fn clamp(&mut self) {
        let max = self.total.saturating_sub(self.slots.len());
        self.offset = self.offset.min(max);
    }

    /// Scroll so row `row` is in the window, moving no further than needed.
    fn show(&mut self, row: usize) {
        if row < self.offset {
            self.offset = row;
        } else if row >= self.offset + self.slots.len() {
            self.offset = row + 1 - self.slots.len().max(1);
        }
        self.clamp();
    }
}

/// One row slot and its parts: the role word (host rows only), then two
/// texts (name and address, or key and value).
#[derive(Component, Debug)]
struct Slot {
    index: usize,
    role: Option<Entity>,
    left: Entity,
    right: Entity,
}

/// The host a host-row slot shows, for its click.
#[derive(Component, Debug, Default)]
struct SlotHost(Option<String>);

/// One line of the dump.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    /// A key, padded to the key column, and a value after it.
    Key(String, String),
    /// A key over a block (a table or a nested object).
    Head(String),
    /// A monospace line of a block.
    Mono(String),
}

/// The selected host as the dump shows it; empty without a selection.
#[derive(Resource, Debug)]
struct DetailsModel {
    id: Option<String>,
    label: String,
    role: Role,
    raw_role: String,
    lines: Vec<Line>,
}

impl Default for DetailsModel {
    fn default() -> Self {
        DetailsModel {
            id: None,
            label: String::new(),
            role: Role::Host,
            raw_role: String::new(),
            lines: Vec::new(),
        }
    }
}

pub struct RailPlugin;

impl Plugin for RailPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DetailsModel>()
            .add_systems(Startup, spawn_rail)
            .add_systems(
                Update,
                (
                    read_filter,
                    focus_filter,
                    submit_filter,
                    build_details,
                    show_collapsed,
                    show_details,
                    keep_selection_in_view,
                )
                    .chain()
                    .in_set(ViewerSet::Draw),
            )
            .add_systems(
                PostUpdate,
                (size_lists, fill_host_rows, fill_detail_lines)
                    .chain()
                    .after(UiSystems::PostCompute),
            );
    }
}

/// Where the address column starts in a rail `width` wide; none in the
/// narrow rail.
fn addr_col(width: f32) -> Option<f32> {
    (width >= ADDR_MIN_WIDTH)
        .then(|| width - ROW_PAD_X - TextStyle::MONO.average_advance() * ADDR_CHARS as f32)
}

/// Where a dump value starts: after the padded key, the colon and a space.
fn key_col() -> f32 {
    ROW_PAD_X + TextStyle::MONO.average_advance() * (KEY_CHARS + 2) as f32
}

/// A header band's row: items side by side, centred, with the band's padding.
fn band_row(name: &'static str) -> impl Bundle {
    (
        Name::new(name),
        UiLayout::flow()
            .direction(UiFlowDirection::LeftToRight)
            .width(Rl(100.0))
            .height(Ab(HEAD_H))
            .padding_x(Ab(PAD))
            .gap(Ab(8.0))
            .align(Align::CENTER)
            .pack(),
        Pickable::IGNORE,
    )
}

/// A flow item as wide as its text.
fn fit_item(name: &'static str) -> impl Bundle {
    (
        Name::new(name),
        UiLayout::flow().width(Ab(1.0)).height(Ab(20.0)).pack(),
        FitText { pad: 0.0, min: 1.0 },
        Pickable::IGNORE,
    )
}

/// A small bracketed button: `[x]`, `[>]`, `[fly to]`.
fn bracket<'s>(
    name: &'static str,
    layout: UiLayout,
    label: &'s str,
    tip: &'s str,
) -> ButtonSpec<'s> {
    ButtonSpec {
        style: TextStyle::MONO,
        text_hover: theme::ACCENT,
        fit: Some(4.0),
        ..ButtonSpec::ghost(name, layout, label)
    }
    .tooltip(tip)
}

/// The frame down the right edge, and the tab that stands in for it while
/// it is collapsed.
#[allow(clippy::too_many_lines)] // one rail, laid out once
fn spawn_rail(
    mut commands: Commands,
    root: Res<UiRoot>,
    fonts: Res<Fonts>,
    shapes: Res<ShapeAssets>,
    mut materials: ResMut<Assets<ShapeMaterial>>,
    panel: Res<Panel>,
) {
    let mut kit = Kit {
        fonts: &fonts,
        shapes: &shapes,
        materials: &mut materials,
    };
    let width = sidebar_width(1280.0);
    commands.entity(root.entity).with_children(|ui| {
        // The tab.
        let tab = ButtonSpec::plain(
            "rail tab",
            UiLayout::window()
                .pos((Rl(100.0) - Ab(theme::EDGE), Ab(theme::EDGE)))
                .anchor(Anchor::TOP_RIGHT)
                .size((Ab(TAB_W), Ab(TAB_H)))
                .pack(),
            "[<]",
        )
        .style(TextStyle::MONO)
        .tooltip("show the host list");
        let tab = button(
            ui,
            &mut kit,
            tab,
            |click: On<PointerClick>, mut panel: ResMut<Panel>| {
                if click.button == PointerButton::Primary {
                    panel.collapsed = false;
                }
            },
        );
        ui.commands()
            .entity(tab)
            .insert((RailTab, UiDepth::Set(RAIL_Z)));

        // The frame.
        ui.spawn((
            Name::new("rail"),
            Rail,
            UiLayout::window()
                .pos((Rl(100.0) - Ab(theme::EDGE), Ab(theme::EDGE)))
                .anchor(Anchor::TOP_RIGHT)
                .size((Ab(width), Rl(100.0) - Ab(2.0 * theme::EDGE)))
                .pack(),
            UiDepth::Set(RAIL_Z),
            if panel.collapsed {
                Visibility::Hidden
            } else {
                Visibility::Inherited
            },
        ))
        .with_children(|rail| {
            rail.spawn(kit.skin(theme::panel(), theme::PANEL_GLOW));
            rail.spawn((
                Name::new("column"),
                UiLayout::flow()
                    .direction(UiFlowDirection::TopToBottom)
                    .width(Rl(100.0))
                    .height(Rl(100.0))
                    .pack(),
                Pickable::IGNORE,
            ))
            .with_children(|column| {
                // The head band: HOSTS, the count, the filter, collapse.
                column.spawn(band_row("head")).with_children(|head| {
                    head.spawn(kit.skin(theme::band(), 0.0));
                    head.spawn(fit_item("title")).with_children(|item| {
                        item.spawn(kit.text("HOSTS", TextStyle::TITLE, theme::ACCENT, Pin::CENTER));
                    });
                    head.spawn(fit_item("count")).with_children(|item| {
                        item.spawn((
                            kit.text("0/0", TextStyle::MONO, theme::MUTED, Pin::CENTER),
                            CountText,
                        ));
                    });
                    let filter = text_field(
                        head,
                        &mut kit,
                        "filter field",
                        UiLayout::flow()
                            .width(UiFlowSize::Grow)
                            .height(Ab(22.0))
                            .pack(),
                        "filter>",
                        &panel.filter,
                        "(/)",
                        TextStyle::MONO,
                    );
                    head.commands().entity(filter).insert(FilterField);
                    button(
                        head,
                        &mut kit,
                        bracket(
                            "collapse",
                            UiLayout::flow().width(Ab(1.0)).height(Ab(20.0)).pack(),
                            "[>]",
                            "hide the host list",
                        ),
                        |click: On<PointerClick>, mut panel: ResMut<Panel>| {
                            if click.button == PointerButton::Primary {
                                panel.collapsed = true;
                            }
                        },
                    );
                });
                rule(column, &mut kit);

                // The table's header line.
                column
                    .spawn((
                        Name::new("table head"),
                        UiLayout::flow().width(Rl(100.0)).height(Ab(LINE_H)).pack(),
                        Pickable::IGNORE,
                    ))
                    .with_children(|line| {
                        line.spawn(kit.text("ROLE", TextStyle::MONO, theme::MUTED, Pin::left(ROLE_COL)));
                        line.spawn(kit.text("HOST", TextStyle::MONO, theme::MUTED, Pin::left(HOST_COL)));
                        line.spawn((
                            kit.text(
                                "ADDR",
                                TextStyle::MONO,
                                theme::MUTED,
                                Pin::left(addr_col(width).unwrap_or(HOST_COL)),
                            ),
                            AddrHead,
                        ));
                    });

                // The host rows take what is left.
                column
                    .spawn((
                        Name::new("host list"),
                        HostList,
                        VirtualList {
                            row_h: ROW_H,
                            ..VirtualList::default()
                        },
                        UiLayout::flow()
                            .width(Rl(100.0))
                            .height(UiFlowSize::Grow)
                            .pack(),
                    ))
                    .observe(scroll_list);

                // The details, under the table: a band, the lines, a footer.
                column
                    .spawn((
                        Name::new("details"),
                        Details,
                        UiLayout::flow().width(Rl(100.0)).height(Ab(0.0)).pack(),
                        Visibility::Hidden,
                    ))
                    .with_children(|card| {
                        card.spawn((
                            Name::new("details band"),
                            UiLayout::window()
                                .size((Rl(100.0), Ab(HEAD_H)))
                                .pack(),
                            Pickable::IGNORE,
                        ))
                        .with_children(|band| {
                            band.spawn(kit.skin(theme::band(), 0.0));
                            band.spawn(band_row("details head")).with_children(|head| {
                                head.spawn(fit_item("title")).with_children(|item| {
                                    item.spawn((
                                        kit.text("", TextStyle::TITLE, theme::ACCENT, Pin::CENTER),
                                        DetailsTitle,
                                    ));
                                });
                                head.spawn(fit_item("role")).with_children(|item| {
                                    item.spawn((
                                        kit.text("", TextStyle::MONO, theme::TEXT, Pin::CENTER),
                                        DetailsRole,
                                    ));
                                });
                                head.spawn((
                                    Name::new("spacer"),
                                    UiLayout::flow().width(UiFlowSize::Grow).height(Ab(1.0)).pack(),
                                    Pickable::IGNORE,
                                ));
                                button(
                                    head,
                                    &mut kit,
                                    bracket(
                                        "deselect",
                                        UiLayout::flow().width(Ab(1.0)).height(Ab(20.0)).pack(),
                                        "[x]",
                                        "deselect (Esc)",
                                    ),
                                    |click: On<PointerClick>, mut select: MessageWriter<SelectHost>| {
                                        if click.button == PointerButton::Primary {
                                            select.write(SelectHost {
                                                id: None,
                                                fly: false,
                                            });
                                        }
                                    },
                                );
                            });
                        });
                        card.spawn((
                            Name::new("rule"),
                            UiLayout::window()
                                .pos((Ab(0.0), Ab(HEAD_H)))
                                .size((Rl(100.0), Ab(RULE_H)))
                                .pack(),
                            Pickable::IGNORE,
                        ))
                        .with_children(|r| {
                            r.spawn(kit.skin(Look::filled(theme::HUD_BORDER_SOFT, 0.0), 0.0));
                        });
                        card.spawn((
                            Name::new("detail lines"),
                            DetailLines,
                            VirtualList {
                                row_h: LINE_H,
                                ..VirtualList::default()
                            },
                            UiLayout::window()
                                .pos((Ab(0.0), Ab(HEAD_H + RULE_H + LINES_GAP)))
                                .size((Rl(100.0), Ab(0.0)))
                                .pack(),
                            Pickable::IGNORE,
                        ))
                        .observe(scroll_list);
                        // The footer: fly to, forget.
                        card.spawn((
                            Name::new("footer"),
                            UiLayout::window()
                                .pos((Ab(ROW_PAD_X), Rl(100.0) - Ab(4.0)))
                                .anchor(Anchor::BOTTOM_LEFT)
                                .size((Rl(100.0) - Ab(2.0 * ROW_PAD_X), Ab(FOOT_H)))
                                .pack(),
                            Pickable::IGNORE,
                        ))
                        .with_children(|foot| {
                            let slot = |x: f32, w: f32| {
                                UiLayout::window()
                                    .pos((Ab(x), Rl(50.0)))
                                    .anchor(Anchor::CENTER_LEFT)
                                    .size((Ab(w), Ab(20.0)))
                                    .pack()
                            };
                            button(
                                foot,
                                &mut kit,
                                bracket("fly to", slot(0.0, 66.0), "[fly to]", "fly to this host (F)"),
                                |click: On<PointerClick>,
                                 model: Res<DetailsModel>,
                                 mut select: MessageWriter<SelectHost>| {
                                    if click.button == PointerButton::Primary {
                                        select.write(SelectHost {
                                            id: model.id.clone(),
                                            fly: true,
                                        });
                                    }
                                },
                            );
                            button(
                                foot,
                                &mut kit,
                                bracket(
                                    "forget",
                                    slot(74.0, 66.0),
                                    "[forget]",
                                    "take this host off the map; a scan may find it again",
                                ),
                                forget_pressed,
                            );
                        });
                    });
            });
        });
    });
}

/// A 1 px rule across the column, under a band.
fn rule(column: &mut ChildSpawnerCommands, kit: &mut Kit) {
    column
        .spawn((
            Name::new("rule"),
            UiLayout::flow().width(Rl(100.0)).height(Ab(RULE_H)).pack(),
            Pickable::IGNORE,
        ))
        .with_children(|r| {
            r.spawn(kit.skin(Look::filled(theme::HUD_BORDER_SOFT, 0.0), 0.0));
        });
}

/// The wheel over a list moves its window.
fn scroll_list(
    scroll: On<PointerScroll>,
    parents: Query<&ChildOf>,
    mut lists: Query<&mut VirtualList>,
) {
    let target = scroll.event_target();
    let list = std::iter::once(target)
        .chain(parents.iter_ancestors(target))
        .find(|e| lists.get(*e).is_ok());
    let Some(list) = list else {
        return;
    };
    let Ok(mut list) = lists.get_mut(list) else {
        return;
    };
    let lines = match scroll.unit {
        MouseScrollUnit::Line => scroll.y,
        MouseScrollUnit::Pixel => scroll.y / 40.0,
    };
    // Scrolling down (negative y) shows later rows.
    let rows = (lines * ROWS_PER_LINE).round() as i64;
    let next = (list.offset as i64 - rows).max(0) as usize;
    if next != list.offset {
        list.offset = next;
        list.clamp();
    }
}

/// Each list has as many slots as rows fit in it.
fn size_lists(
    mut commands: Commands,
    mut lists: Query<(Entity, &mut VirtualList, &Dimension, Has<HostList>), Changed<Dimension>>,
    fonts: Res<Fonts>,
    shapes: Res<ShapeAssets>,
    mut materials: ResMut<Assets<ShapeMaterial>>,
) {
    for (entity, mut list, dimension, is_host_list) in &mut lists {
        let wanted = (dimension.y / list.row_h).floor().max(0.0) as usize;
        if wanted == list.slots.len() {
            continue;
        }
        let mut kit = Kit {
            fonts: &fonts,
            shapes: &shapes,
            materials: &mut materials,
        };
        while list.slots.len() > wanted {
            if let Some(slot) = list.slots.pop() {
                commands.entity(slot).despawn();
            }
        }
        while list.slots.len() < wanted {
            let index = list.slots.len();
            let slot = spawn_slot(
                &mut commands,
                &mut kit,
                entity,
                index,
                list.row_h,
                is_host_list,
            );
            list.slots.push(slot);
        }
        list.clamp();
    }
}

/// One row slot: a skin, the role word (host rows), two texts.
fn spawn_slot(
    commands: &mut Commands,
    kit: &mut Kit,
    list: Entity,
    index: usize,
    row_h: f32,
    host_row: bool,
) -> Entity {
    let mut node = commands.spawn((
        Name::new(if host_row { "host row" } else { "detail line" }),
        UiLayout::window()
            .pos((Ab(0.0), Ab(index as f32 * row_h)))
            .anchor(Anchor::TOP_LEFT)
            .size((Rl(100.0), Ab(row_h)))
            .pack(),
        Visibility::Hidden,
        ChildOf(list),
    ));
    let mut skin = Entity::PLACEHOLDER;
    let mut role = None;
    let mut left = Entity::PLACEHOLDER;
    let mut right = Entity::PLACEHOLDER;
    node.with_children(|row| {
        skin = row
            .spawn(kit.skin(Look::filled(Color::NONE, 0.0), 0.0))
            .id();
        if host_row {
            role = Some(
                row.spawn(kit.text("", TextStyle::MONO, theme::ROLE_HOST, Pin::left(ROLE_COL)))
                    .id(),
            );
            left = row
                .spawn(kit.text("", TextStyle::MONO, theme::TEXT, Pin::left(HOST_COL)))
                .id();
            right = row
                .spawn(kit.text("", TextStyle::MONO, theme::TEXT, Pin::left(HOST_COL)))
                .id();
        } else {
            left = row
                .spawn(kit.text("", TextStyle::MONO, theme::MUTED, Pin::left(ROW_PAD_X)))
                .id();
            right = row
                .spawn(kit.text("", TextStyle::MONO, theme::TEXT, Pin::left(key_col())))
                .id();
        }
    });
    node.insert(Slot {
        index,
        role,
        left,
        right,
    });
    if host_row {
        node.insert((
            SlotHost::default(),
            hoverable(),
            OnHoverSetCursor::new(bevy::window::SystemCursorIcon::Pointer),
            Styled {
                skin,
                base: Look::filled(Color::NONE, 0.0),
                hover: Look::filled(theme::ROW_HOVER, 0.0),
                text: None,
            },
        ));
        node.observe(hover_set::<PointerOver, true>)
            .observe(hover_set::<PointerOut, false>)
            .observe(
                |click: On<PointerClick>,
                 slots: Query<&SlotHost>,
                 mut select: MessageWriter<SelectHost>| {
                    if click.button != PointerButton::Primary {
                        return;
                    }
                    if let Ok(SlotHost(Some(id))) = slots.get(click.event_target()) {
                        select.write(SelectHost {
                            id: Some(id.clone()),
                            fly: true,
                        });
                    }
                },
            );
    } else {
        node.insert(Pickable::IGNORE);
    }
    node.id()
}

/// The host rows show the window of the filtered, sorted hosts.
#[allow(clippy::too_many_arguments)] // a Bevy system lists what it reads
fn fill_host_rows(
    panel: Res<Panel>,
    picker: Res<Picker>,
    mut lists: Query<(&mut VirtualList, &Dimension), With<HostList>>,
    added: Query<(), Added<Slot>>,
    mut slots: Query<(&Slot, &mut SlotHost, &mut Visibility, &mut Styled)>,
    mut texts: Query<&mut Text2d, Without<CountText>>,
    mut colors: Query<&mut TextColor>,
    mut pins: Query<&mut Pin>,
    heads: Query<Entity, With<AddrHead>>,
    mut counts: Query<&mut Text2d, With<CountText>>,
) {
    let Ok((mut list, dimension)) = lists.single_mut() else {
        return;
    };
    let visible = panel.visible_indices();
    let total = visible.len();
    if list.total != total {
        list.total = total;
        list.clamp();
    }
    if !(panel.is_changed() || picker.is_changed() || list.is_changed() || !added.is_empty()) {
        return;
    }
    let count = format!("{}/{}", total, panel.rows.len());
    for mut text in &mut counts {
        if text.0 != count {
            text.0.clone_from(&count);
        }
    }
    let selected = picker.selected_id.as_deref();
    let width = dimension.x;
    let addr = addr_col(width);
    // The header's ADDR follows the column.
    for head in &heads {
        if let Ok(mut pin) = pins.get_mut(head) {
            let wanted = Pin::left(addr.unwrap_or(HOST_COL));
            if *pin != wanted {
                *pin = wanted;
            }
            if let Ok(mut text) = texts.get_mut(head) {
                let shown = if addr.is_some() { "ADDR" } else { "" };
                if text.0 != shown {
                    text.0 = shown.to_string();
                }
            }
        }
    }
    let name_w = addr.unwrap_or(width - ROW_PAD_X) - HOST_COL - ROW_PAD_X;
    for slot_entity in list.slots.clone() {
        let Ok((slot, mut host, mut visibility, mut styled)) = slots.get_mut(slot_entity) else {
            continue;
        };
        let Some(row) = visible
            .get(list.offset + slot.index)
            .map(|&i| &panel.rows[i])
        else {
            if *visibility != Visibility::Hidden {
                *visibility = Visibility::Hidden;
            }
            host.0 = None;
            continue;
        };
        if *visibility != Visibility::Inherited {
            *visibility = Visibility::Inherited;
        }
        if host.0.as_deref() != Some(row.id.as_str()) {
            host.0 = Some(row.id.clone());
        }
        if let Some(role) = slot.role {
            set(&mut texts, role, row.role.name());
            if let Ok(mut color) = colors.get_mut(role) {
                let wanted = role_color(row.role);
                if color.0 != wanted {
                    color.0 = wanted;
                }
            }
        }
        set(
            &mut texts,
            slot.left,
            &truncate(&row.label, name_w, TextStyle::MONO),
        );
        // The address in its column, unless the label already is the
        // address or the rail is too narrow for the column.
        let address = match addr {
            Some(_) if !row.label.contains(&row.id) => row.id.as_str(),
            _ => "",
        };
        set(&mut texts, slot.right, address);
        if let (Some(x), Ok(mut pin)) = (addr, pins.get_mut(slot.right)) {
            let wanted = Pin::left(x);
            if *pin != wanted {
                *pin = wanted;
            }
        }
        let base = if selected == Some(row.id.as_str()) {
            Look::bordered(theme::ROW_SELECTED, theme::ROW_SELECTED_BORDER, 0.0)
        } else {
            Look::filled(Color::NONE, 0.0)
        };
        if styled.base != base {
            styled.base = base;
        }
    }
}

fn set<F: bevy::ecs::query::QueryFilter>(
    texts: &mut Query<&mut Text2d, F>,
    entity: Entity,
    wanted: &str,
) {
    if let Ok(mut text) = texts.get_mut(entity)
        && text.0 != wanted
    {
        text.0 = wanted.to_string();
    }
}

/// The dump lines show the window of the selected host's lines.
fn fill_detail_lines(
    model: Res<DetailsModel>,
    mut lists: Query<(&mut VirtualList, &Dimension), With<DetailLines>>,
    added: Query<(), Added<Slot>>,
    mut slots: Query<(&Slot, &mut Visibility)>,
    mut texts: Query<(&mut Text2d, &mut TextColor)>,
) {
    let Ok((mut list, dimension)) = lists.single_mut() else {
        return;
    };
    if list.total != model.lines.len() {
        list.total = model.lines.len();
        list.clamp();
    }
    if !(model.is_changed() || list.is_changed() || !added.is_empty()) {
        return;
    }
    let width = dimension.x;
    for slot_entity in list.slots.clone() {
        let Ok((slot, mut visibility)) = slots.get_mut(slot_entity) else {
            continue;
        };
        let Some(line) = model.lines.get(list.offset + slot.index) else {
            if *visibility != Visibility::Hidden {
                *visibility = Visibility::Hidden;
            }
            continue;
        };
        if *visibility != Visibility::Inherited {
            *visibility = Visibility::Inherited;
        }
        let (left, left_color, right) = match line {
            Line::Key(key, value) => (
                key_text(key),
                theme::MUTED,
                truncate(value, width - key_col() - ROW_PAD_X, TextStyle::MONO),
            ),
            Line::Head(key) => (format!("{key}:"), theme::MUTED, String::new()),
            Line::Mono(text) => (
                truncate(text, width - 2.0 * ROW_PAD_X, TextStyle::MONO),
                theme::TEXT,
                String::new(),
            ),
        };
        if let Ok((mut text, mut color)) = texts.get_mut(slot.left) {
            if text.0 != left {
                text.0 = left;
            }
            if color.0 != left_color {
                color.0 = left_color;
            }
        }
        if let Ok((mut text, _)) = texts.get_mut(slot.right)
            && text.0 != right
        {
            text.0 = right;
        }
    }
}

/// A dump key as written: padded to the key column, then the colon
/// (`hostname  :`); a key too long for the column is cut.
fn key_text(key: &str) -> String {
    let key = if key.chars().count() > KEY_CHARS {
        let mut cut: String = key.chars().take(KEY_CHARS - 1).collect();
        cut.push('…');
        cut
    } else {
        key.to_string()
    };
    format!("{key:<KEY_CHARS$}:")
}

/// The selected host's dump: rebuilt when the selection or the map changes.
fn build_details(picker: Res<Picker>, doc: Res<MapDocument>, mut model: ResMut<DetailsModel>) {
    if !(picker.is_changed() || doc.is_changed()) {
        return;
    }
    let host = picker
        .selected_id
        .as_deref()
        .and_then(|id| doc.0.as_ref()?.hosts.iter().find(|h| h.id == id));
    let Some(host) = host else {
        if model.id.is_some() {
            *model = DetailsModel::default();
        }
        return;
    };
    let lines = detail_lines(host);
    if model.id.as_deref() == Some(host.id.as_str()) && model.lines == lines {
        return;
    }
    *model = DetailsModel {
        id: Some(host.id.clone()),
        label: host.display_label().to_string(),
        role: host.role(),
        raw_role: host.role.clone().unwrap_or_else(|| "host".to_string()),
        lines,
    };
}

/// Every field of the host, as lines, under the JSON's own keys: scalars as
/// key and value, lists of objects as a key and a monospace table, objects
/// as a key and indented keys.
pub fn detail_lines(host: &Host) -> Vec<Line> {
    let mut lines = Vec::new();
    for (key, value) in &host.fields {
        push_value(&mut lines, key, value, 0);
    }
    lines
}

fn push_value(lines: &mut Vec<Line>, key: &str, value: &Value, depth: usize) {
    let indent = "  ".repeat(depth);
    match value {
        Value::Array(items) if items.is_empty() => {
            lines.push(Line::Key(format!("{indent}{key}"), "none".to_string()));
        }
        Value::Array(items) if items.iter().all(Value::is_object) => {
            lines.push(Line::Head(format!("{indent}{key}")));
            for row in table_lines(items) {
                lines.push(Line::Mono(format!("{indent}  {row}")));
            }
        }
        Value::Array(items) => {
            let joined = items.iter().map(scalar_text).collect::<Vec<_>>().join(", ");
            lines.push(Line::Key(format!("{indent}{key}"), joined));
        }
        Value::Object(map) if map.is_empty() => {
            lines.push(Line::Key(format!("{indent}{key}"), "none".to_string()));
        }
        Value::Object(map) => {
            lines.push(Line::Head(format!("{indent}{key}")));
            for (inner_key, inner) in map {
                push_value(lines, inner_key, inner, depth + 1);
            }
        }
        scalar => lines.push(Line::Key(format!("{indent}{key}"), scalar_text(scalar))),
    }
}

/// A list of objects as monospace lines: a header, then the rows, each
/// column as wide as its widest cell.
pub fn table_lines(rows: &[Value]) -> Vec<String> {
    let columns = table_columns(rows);
    let cell = |row: &Value, column: &str| row.get(column).map_or("—".to_string(), scalar_text);
    let widths: Vec<usize> = columns
        .iter()
        .map(|column| {
            rows.iter()
                .map(|row| cell(row, column).chars().count())
                .chain(std::iter::once(humanize(column).chars().count()))
                .max()
                .unwrap_or(0)
        })
        .collect();
    let line = |cells: Vec<String>| -> String {
        cells
            .iter()
            .zip(&widths)
            .map(|(text, width)| format!("{text:<width$}"))
            .collect::<Vec<_>>()
            .join("  ")
            .trim_end()
            .to_string()
    };
    let mut out = vec![line(columns.iter().map(|c| humanize(c)).collect())];
    for row in rows {
        out.push(line(columns.iter().map(|c| cell(row, c)).collect()));
    }
    out
}

/// The details' band and height follow the model; they hide without a
/// selection.
#[allow(clippy::too_many_arguments)] // a Bevy system lists what it reads
fn show_details(
    model: Res<DetailsModel>,
    rails: Query<Ref<Dimension>, With<Rail>>,
    mut cards: Query<(&mut UiLayout, &mut Visibility), With<Details>>,
    mut lines: Query<(&mut UiLayout, &mut VirtualList), (With<DetailLines>, Without<Details>)>,
    mut title: Query<&mut Text2d, (With<DetailsTitle>, Without<DetailsRole>)>,
    mut role: Query<(&mut Text2d, &mut TextColor), With<DetailsRole>>,
    mut last_height: Local<f32>,
) {
    let Ok(rail) = rails.single() else {
        return;
    };
    let fixed = HEAD_H + RULE_H + 2.0 * LINES_GAP + FOOT_H + 4.0;
    let max_lines = ((rail.y * DETAILS_MAX_FRACTION - fixed) / LINE_H)
        .floor()
        .max(1.0) as usize;
    let shown = model.lines.len().min(max_lines);
    let height = if model.id.is_some() {
        fixed + shown as f32 * LINE_H
    } else {
        0.0
    };
    // The rail's first frame has no width yet: the title is sized again
    // when the rail is.
    let refresh = model.is_changed() || rail.is_changed();
    if !refresh && (height - *last_height).abs() < 0.5 {
        return;
    }
    *last_height = height;
    for (mut layout, mut visibility) in &mut cards {
        if let Some(flow) = layout.get_mut_flow(UiBase::id()) {
            flow.height = UiFlowAxis::from(UiFlowSize::from(Ab(height)));
        }
        let want = if model.id.is_some() {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *visibility != want {
            *visibility = want;
        }
    }
    for (mut layout, mut list) in &mut lines {
        if let Some(window) = layout.get_mut_window(UiBase::id()) {
            window.set_height(Ab(shown as f32 * LINE_H));
        }
        if model.is_changed() {
            list.offset = 0; // a new host: its dump starts at the top
        }
    }
    if refresh {
        // The title gets what the role word, the [x] and the gaps leave.
        let width = if rail.x > 1.0 {
            rail.x
        } else {
            sidebar_width(1280.0)
        };
        let room = width
            - 2.0 * PAD
            - 3.0 * 8.0
            - TextStyle::MONO.width_of(&model.raw_role)
            - TextStyle::MONO.width_of("[x]")
            - 8.0;
        for mut text in &mut title {
            text.0 = truncate(&model.label, room, TextStyle::TITLE);
        }
        for (mut text, mut color) in &mut role {
            text.0.clone_from(&model.raw_role);
            color.0 = role_color(model.role);
        }
    }
}

/// Collapsed: the frame hides and the tab shows.
fn show_collapsed(
    panel: Res<Panel>,
    mut rails: Query<&mut Visibility, (With<Rail>, Without<RailTab>)>,
    mut tabs: Query<&mut Visibility, With<RailTab>>,
) {
    if !panel.is_changed() {
        return;
    }
    for mut v in &mut rails {
        *v = if panel.collapsed {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        };
    }
    for mut v in &mut tabs {
        *v = if panel.collapsed {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }
}

/// What is typed in the filter box filters the list.
fn read_filter(
    fields: Query<&TextField, (With<FilterField>, Changed<TextField>)>,
    mut panel: ResMut<Panel>,
) {
    for field in &fields {
        if panel.filter != field.value {
            panel.filter.clone_from(&field.value);
        }
    }
}

/// `/` asked for the filter box.
fn focus_filter(
    mut panel: ResMut<Panel>,
    fields: Query<Entity, With<FilterField>>,
    mut focus: MessageWriter<FocusField>,
    mut texts: Query<&mut TextField>,
) {
    if !panel.focus_filter {
        return;
    }
    panel.focus_filter = false;
    if let Ok(field) = fields.single() {
        focus.write(FocusField(Some(field)));
        // The next keystroke replaces the query.
        if let Ok(mut text) = texts.get_mut(field) {
            text.value.clear();
        }
    }
}

/// Enter in the filter box selects and flies to the first match.
fn submit_filter(
    mut submitted: MessageReader<Submitted>,
    fields: Query<(), With<FilterField>>,
    panel: Res<Panel>,
    mut select: MessageWriter<SelectHost>,
) {
    if !submitted.read().any(|s| fields.get(s.0).is_ok()) {
        return;
    }
    if let Some(&first) = panel.visible_indices().first() {
        select.write(SelectHost {
            id: Some(panel.rows[first].id.clone()),
            fly: true,
        });
    }
}

/// A selection made elsewhere (a click on a node, F, the filter) scrolls
/// its row into view, no further than needed.
fn keep_selection_in_view(
    picker: Res<Picker>,
    panel: Res<Panel>,
    mut lists: Query<&mut VirtualList, With<HostList>>,
) {
    if !picker.is_changed() {
        return;
    }
    let Some(selected) = picker.selected_id.as_deref() else {
        return;
    };
    let visible = panel.visible_indices();
    let Some(row) = visible.iter().position(|&i| panel.rows[i].id == selected) else {
        return;
    };
    for mut list in &mut lists {
        list.total = visible.len();
        list.show(row);
    }
}

fn forget_pressed(
    click: On<PointerClick>,
    model: Res<DetailsModel>,
    mut doc: ResMut<MapDocument>,
    mut select: MessageWriter<SelectHost>,
) {
    if click.button != PointerButton::Primary {
        return;
    }
    let Some(id) = &model.id else {
        return;
    };
    if let Some(map) = doc.0.as_mut() {
        map.forget(id);
    }
    select.write(SelectHost {
        id: None,
        fly: false,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(json: &str) -> Host {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn details_render_scalars_tables_and_nested_objects_as_lines() {
        let h = host(
            r#"{"id": "10.0.0.3", "ip": "10.0.0.3", "hostname": "nas.lan", "mac": null,
                "open_ports": [{"number": 22, "proto": "tcp", "service": "ssh"}, {"number": 445, "proto": "tcp"}],
                "is_local": true, "extra": {"note": "x", "latency_ms": 1.5}, "tags": ["a", "b"], "empty": []}"#,
        );
        let lines = detail_lines(&h);
        assert_eq!(
            lines,
            vec![
                Line::Key("ip".into(), "10.0.0.3".into()),
                Line::Key("hostname".into(), "nas.lan".into()),
                Line::Key("mac".into(), "—".into()),
                Line::Head("open_ports".into()),
                Line::Mono("  number  proto  service".into()),
                Line::Mono("  22      tcp    ssh".into()),
                Line::Mono("  445     tcp    —".into()),
                Line::Key("is_local".into(), "yes".into()),
                Line::Head("extra".into()),
                Line::Key("  note".into(), "x".into()),
                Line::Key("  latency_ms".into(), "1.5".into()),
                Line::Key("tags".into(), "a, b".into()),
                Line::Key("empty".into(), "none".into()),
            ]
        );
    }

    #[test]
    fn dump_keys_are_padded_to_the_key_column() {
        assert_eq!(key_text("ip"), "ip        :");
        assert_eq!(key_text("open_ports"), "open_ports:");
        assert_eq!(key_text("generated_at"), "generated…:");
        assert_eq!(key_text("  note"), "  note    :");
    }

    #[test]
    fn the_address_column_exists_only_in_a_wide_rail() {
        assert_eq!(addr_col(220.0), None);
        // 330 wide: 15 characters of 7.2 px and the row padding from the right.
        let col = addr_col(330.0).unwrap();
        assert!((col - (330.0 - 8.0 - 108.0)).abs() < 1e-3, "{col}");
        assert!((key_col() - (8.0 + 12.0 * 7.2)).abs() < 1e-3);
    }

    #[test]
    fn a_virtual_list_keeps_its_window_on_the_data() {
        let mut list = VirtualList {
            row_h: 30.0,
            offset: 50,
            total: 20,
            slots: vec![Entity::PLACEHOLDER; 5],
        };
        list.clamp();
        assert_eq!(list.offset, 15);
        list.show(2);
        assert_eq!(list.offset, 2, "scroll up to the row");
        list.show(4);
        assert_eq!(list.offset, 2, "already in view: stay");
        list.show(10);
        assert_eq!(list.offset, 6, "scroll down just enough");
        list.show(19);
        assert_eq!(list.offset, 15);
        list.total = 3;
        list.clamp();
        assert_eq!(list.offset, 0, "fewer rows than slots: from the top");
    }
}
