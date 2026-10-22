//! Sidebar: a filterable host list and a details card for the selected host.
//! (The HUD, legend and help overlay live in ui.rs.)
//!
//! The details card is generic on purpose. Apart from a few keys that get a
//! friendlier heading, every field of the host JSON is rendered as it comes
//! (scalars as rows, lists of objects as tables, objects as nested rows), so a
//! field added to netvisualizer/model.py shows up here with no viewer change.
//!
//! The sidebar is an egui `Area` (a floating card) rather than an egui
//! `Panel`, so egui counts the pointer as over the UI there and clicks never
//! fall through to the 3D view.

use std::cmp::Ordering;

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use serde_json::Value;

use crate::ViewerSet;
use crate::graph::MapDocument;
use crate::map::{Host, Role};
use crate::picking::{Picker, SelectHost};
use crate::scene::Inset;
use crate::theme;
use crate::ui::{BODY_PX, MONO_PX, egui_color, mono, role_color, text};

/// Sidebar width, and the narrower one in windows up to `NARROW_WINDOW` wide.
const SIDEBAR_WIDTH: f32 = 300.0;
const NARROW_SIDEBAR_WIDTH: f32 = 220.0;
const NARROW_WINDOW: f32 = 720.0;
/// Gap between the sidebar and the window's top, right and bottom edges.
const SIDEBAR_MARGIN: f32 = 12.0;
/// 10 px of padding plus the 1 px border, per side.
const SIDEBAR_PADDING: f32 = 10.0 + 1.0;
/// Space between the head, the details card and the list.
const GAP: f32 = 8.0;
/// A host-list row: 14 px text at line-height 1.4, plus 5 px of padding above
/// and below and 8 px beside.
const ROW_HEIGHT: f32 = 30.0;
const ROW_PADDING_X: f32 = 8.0;
const DOT_SIZE: f32 = 9.0;
/// The host list keeps at least this much height.
const MIN_LIST_HEIGHT: f32 = 80.0;
/// The details card takes at most this fraction of the sidebar's height.
const DETAILS_MAX_FRACTION: f32 = 0.55;
/// A details field: a 92 px key column, then the value 10 px further right.
const KEY_COLUMN_PX: f32 = 92.0;
const KEY_GAP: f32 = 10.0;
/// Friendlier names for the fields the model has today.
fn heading(key: &str) -> String {
    match key {
        "ip" => "IP".to_string(),
        "open_ports" => "open ports".to_string(),
        "is_local" => "local".to_string(),
        other => humanize(other),
    }
}

pub fn humanize(key: &str) -> String {
    key.replace('_', " ")
}

/// One row of the host list: what the list and the filter need, sorted once
/// per map by [`compare_rows`], so the per-frame draw does no work but filtering.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub id: String,
    /// The host's label, or its id when it has none.
    pub label: String,
    /// The validated role, for the dot colour (unknown roles draw as hosts).
    pub role: Role,
    /// The role string as sent, for sorting and the tooltip.
    pub raw_role: String,
    /// Lower-cased text the filter box matches against ([`search_text`]).
    pub search: String,
}

#[derive(Resource, Debug)]
pub struct Panel {
    pub collapsed: bool,
    /// What is typed in the filter box, as typed.
    pub filter: String,
    /// `/` was pressed: give the filter box focus when it is next drawn.
    pub focus_filter: bool,
    pub rows: Vec<Row>,
    /// Selection seen last frame, to scroll its row into view once per change.
    last_selected: Option<String>,
    /// egui's zoom factor (logical pixels per egui point; Ctrl +/- changes
    /// it), so the framing inset matches the sidebar as drawn.
    pub zoom: f32,
}

impl Default for Panel {
    fn default() -> Self {
        Panel {
            collapsed: false,
            filter: String::new(),
            focus_filter: false,
            rows: Vec::new(),
            last_selected: None,
            zoom: 1.0,
        }
    }
}

impl Panel {
    /// Replace the rows with a sorted copy of the map's hosts.
    pub fn set_hosts(&mut self, hosts: &[Host]) {
        let mut rows: Vec<Row> = hosts
            .iter()
            .map(|host| Row {
                id: host.id.clone(),
                label: host.display_label().to_string(),
                role: host.role(),
                raw_role: host.role.clone().unwrap_or_default(),
                search: search_text(host),
            })
            .collect();
        rows.sort_by(compare_rows);
        self.rows = rows;
    }

    /// The normalised query: trimmed, lower-cased.
    pub fn query(&self) -> String {
        self.filter.trim().to_lowercase()
    }

    /// Every row, or those matching the query.
    pub fn visible(&self) -> Vec<&Row> {
        let query = self.query();
        self.rows
            .iter()
            .filter(|row| query.is_empty() || row.search.contains(&query))
            .collect()
    }

    /// `/` opens the sidebar if needed and puts the cursor in the filter box.
    pub fn focus_filter(&mut self) {
        self.collapsed = false;
        self.focus_filter = true;
    }

    /// The logical pixels the sidebar hides on the right of a window
    /// `window_width` logical pixels wide; none while collapsed.
    pub fn frame_inset(&self, window_width: f32) -> Inset {
        if self.collapsed {
            return Inset::default();
        }
        let width_points = sidebar_width(window_width / self.zoom) + SIDEBAR_MARGIN;
        Inset {
            right: (width_points * self.zoom).ceil(),
            ..Inset::default()
        }
    }
}

/// The sidebar's width for a window this many points wide.
pub fn sidebar_width(window_width: f32) -> f32 {
    if window_width <= NARROW_WINDOW {
        NARROW_SIDEBAR_WIDTH
    } else {
        SIDEBAR_WIDTH
    }
}

/// Dotted IPv4 sorts numerically; anything else (IPv6, names) falls back to text.
pub fn address_key(id: &str) -> String {
    let octets: Vec<&str> = id.split('.').collect();
    let is_ipv4 = octets.len() == 4
        && octets
            .iter()
            .all(|o| !o.is_empty() && o.bytes().all(|b| b.is_ascii_digit()));
    if is_ipv4 {
        octets
            .iter()
            .map(|o| format!("{o:0>3}"))
            .collect::<Vec<_>>()
            .join(".")
    } else {
        id.to_string()
    }
}

/// Sort position of a role: scanner, gateway, host, then roles this viewer
/// does not know.
fn role_order(raw_role: &str) -> u8 {
    match raw_role {
        "scanner" => 0,
        "gateway" => 1,
        "host" => 2,
        _ => 9,
    }
}

/// By role, then by address. Names compare without case first, with the
/// exact text as the tie-break.
pub fn compare_rows(a: &Row, b: &Row) -> Ordering {
    let (ka, kb) = (address_key(&a.id), address_key(&b.id));
    role_order(&a.raw_role)
        .cmp(&role_order(&b.raw_role))
        .then_with(|| ka.to_lowercase().cmp(&kb.to_lowercase()))
        .then_with(|| ka.cmp(&kb))
}

/// What the filter matches: label, id, hostname, MAC, vendor, role and
/// "number/proto" for each port, skipping the ones that are missing.
pub fn search_text(host: &Host) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut push = |s: &str| {
        if !s.is_empty() {
            parts.push(s.to_string());
        }
    };
    push(host.label.as_deref().unwrap_or_default());
    push(&host.id);
    for key in ["hostname", "mac", "vendor"] {
        push(
            host.fields
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or_default(),
        );
    }
    push(host.role.as_deref().unwrap_or_default());
    if let Some(ports) = host.fields.get("open_ports").and_then(Value::as_array) {
        for port in ports {
            let field = |key: &str| port.get(key).map(scalar_text).unwrap_or_default();
            push(&format!("{}/{}", field("number"), field("proto")));
        }
    }
    parts.join(" ").to_lowercase()
}

/// A JSON scalar as the card shows it.
pub fn scalar_text(value: &Value) -> String {
    match value {
        Value::Null => "—".to_string(),
        Value::Bool(true) => "yes".to_string(),
        Value::Bool(false) => "no".to_string(),
        Value::Number(n) => number_text(n),
        Value::String(s) => s.clone(),
        Value::Array(_) | Value::Object(_) => value.to_string(),
    }
}

/// A number as the card shows it. JSON has one number type, so a whole float
/// such as Python's `1.0` prints as `1`.
fn number_text(n: &serde_json::Number) -> String {
    match n.as_f64() {
        Some(f) if n.is_f64() && f.fract() == 0.0 && f.abs() < 1e21 => format!("{f:.0}"),
        _ => n.to_string(),
    }
}

fn is_plain_object(value: &Value) -> bool {
    value.is_object()
}

/// Tables and nested objects take the full width below their key.
fn is_block(value: &Value) -> bool {
    match value {
        Value::Array(items) => !items.is_empty() && items.iter().all(is_plain_object),
        Value::Object(map) => !map.is_empty(),
        _ => false,
    }
}

pub struct PanelPlugin;

impl Plugin for PanelPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Panel>()
            .add_systems(Update, refresh_rows.in_set(ViewerSet::Build));
    }
}

/// Rebuild the host list whenever a new document arrives.
fn refresh_rows(doc: Res<MapDocument>, mut panel: ResMut<Panel>) {
    if doc.is_changed()
        && let Some(doc) = &doc.0
    {
        panel.set_hosts(&doc.hosts);
    }
}

/// Draw the sidebar (an egui pass system; ui.rs orders it before the input
/// flags are read, so they cover this frame's sidebar).
pub fn draw_sidebar(
    mut contexts: EguiContexts,
    mut panel: ResMut<Panel>,
    doc: Res<MapDocument>,
    picker: Res<Picker>,
    mut select: MessageWriter<SelectHost>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    let panel = &mut *panel;
    panel.zoom = ctx.zoom_factor();
    let screen = ctx.content_rect();
    let selected = picker.selected_id.clone();
    let selection_changed = selected != panel.last_selected;
    panel.last_selected.clone_from(&selected);
    let host = selected
        .as_deref()
        .and_then(|id| doc.0.as_ref()?.hosts.iter().find(|h| h.id == id));

    egui::Area::new(egui::Id::new("sidebar"))
        .anchor(
            egui::Align2::RIGHT_TOP,
            egui::vec2(-SIDEBAR_MARGIN, SIDEBAR_MARGIN),
        )
        .show(ctx, |ui| {
            card(10).show(ui, |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(GAP, GAP);
                if panel.collapsed {
                    // Collapsed: only the toggle, pointing the other way.
                    if toggle_button(ui, true).clicked() {
                        panel.collapsed = false;
                    }
                    return;
                }
                let width = sidebar_width(screen.width()) - 2.0 * SIDEBAR_PADDING;
                let height = (screen.height() - 2.0 * (SIDEBAR_MARGIN + SIDEBAR_PADDING)).max(0.0);
                ui.set_width(width);
                ui.set_height(height);
                draw_head(ui, panel, &mut select);
                if let Some(host) = host {
                    let max_height = (height * DETAILS_MAX_FRACTION)
                        .min(ui.available_height() - GAP - MIN_LIST_HEIGHT);
                    draw_details(ui, host, max_height, selection_changed, &mut select);
                }
                draw_list(
                    ui,
                    panel,
                    selected.as_deref(),
                    selection_changed,
                    &mut select,
                );
            });
        });
    Ok(())
}

/// The sidebar's frame: panel background, 1 px panel border.
pub fn card(radius: u8) -> egui::Frame {
    egui::Frame::new()
        .fill(egui_color(theme::HUD_BG, 1.0))
        .stroke(egui::Stroke::new(1.0, egui_color(theme::HUD_BORDER, 1.0)))
        .corner_radius(radius)
        .inner_margin(SIDEBAR_PADDING - 1.0)
}

fn toggle_button(ui: &mut egui::Ui, collapsed: bool) -> egui::Response {
    let arrow = if collapsed { "‹" } else { "›" };
    ui.add(
        egui::Button::new(text(arrow, theme::MUTED))
            .min_size(egui::vec2(28.0, 26.0))
            .corner_radius(6),
    )
    .on_hover_text("show / hide the host list")
}

/// The sidebar's head: filter box, count, toggle. Enter in the box selects and
/// flies to the first match; Escape leaves the box (egui does that itself).
fn draw_head(ui: &mut egui::Ui, panel: &mut Panel, select: &mut MessageWriter<SelectHost>) {
    let total = panel.rows.len();
    let count = if panel.query().is_empty() {
        total.to_string()
    } else {
        format!("{} / {total}", panel.visible().len())
    };
    // A row of its own: right-to-left so the box takes what the count and the
    // toggle leave, centred within the row's height.
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            draw_head_row(ui, panel, &count, select);
        });
    });
}

fn draw_head_row(
    ui: &mut egui::Ui,
    panel: &mut Panel,
    count: &str,
    select: &mut MessageWriter<SelectHost>,
) {
    if toggle_button(ui, false).clicked() {
        panel.collapsed = true;
    }
    ui.label(mono(count, theme::MUTED));
    let id = egui::Id::new("filter");
    let response = ui.add(
        egui::TextEdit::singleline(&mut panel.filter)
            .id(id)
            .hint_text("filter hosts  ( / )")
            .desired_width(ui.available_width())
            .margin(egui::Margin::symmetric(9, 5))
            .background_color(egui_color(theme::INPUT_BG, 1.0))
            .text_color(egui_color(theme::TEXT, 1.0))
            // Keep the focus on Enter: the box stays ready for the next query.
            .return_key(None::<egui::KeyboardShortcut>),
    );
    if std::mem::take(&mut panel.focus_filter) {
        // After the widget has run, so the `/` that asked for focus is not typed.
        response.request_focus();
        select_all(ui.ctx(), id, &panel.filter);
    }
    if response.has_focus()
        && ui.input(|i| i.key_pressed(egui::Key::Enter))
        && let Some(first) = panel.visible().first()
    {
        select.write(SelectHost {
            id: Some(first.id.clone()),
            fly: true,
        });
    }
}

/// Select the whole query, so the next keystroke replaces it.
fn select_all(ctx: &egui::Context, id: egui::Id, text: &str) {
    use egui::text::{CCursor, CCursorRange};
    let mut state = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
    state.cursor.set_char_range(Some(CCursorRange::two(
        CCursor::new(0),
        CCursor::new(text.chars().count()),
    )));
    egui::TextEdit::store_state(ctx, id, state);
}

/// The host list: one row per visible host; click = select + fly. Rows have a
/// fixed height, so only the ones in view are laid out: a /16 sweep's list
/// costs the same per frame as a /24's.
fn draw_list(
    ui: &mut egui::Ui,
    panel: &Panel,
    selected: Option<&str>,
    selection_changed: bool,
    select: &mut MessageWriter<SelectHost>,
) {
    let rows = panel.visible();
    egui::ScrollArea::vertical()
        .id_salt("hosts")
        .auto_shrink([false, false])
        .max_height(ui.available_height().max(MIN_LIST_HEIGHT))
        .show_viewport(ui, |ui, viewport| {
            let origin = ui.max_rect().min;
            let width = ui.available_width();
            let row_rect = |i: usize| {
                egui::Rect::from_min_size(
                    origin + egui::vec2(0.0, i as f32 * ROW_HEIGHT),
                    egui::vec2(width, ROW_HEIGHT),
                )
            };
            ui.allocate_rect(
                egui::Rect::from_min_size(
                    origin,
                    egui::vec2(width, rows.len() as f32 * ROW_HEIGHT),
                ),
                egui::Sense::hover(),
            );
            let first = (viewport.min.y / ROW_HEIGHT).floor().max(0.0) as usize;
            let last = ((viewport.max.y / ROW_HEIGHT).ceil() as usize).min(rows.len());
            for (i, row) in rows.iter().enumerate().take(last).skip(first) {
                let is_selected = selected == Some(row.id.as_str());
                if draw_row(ui, row_rect(i), row, is_selected).clicked() {
                    select.write(SelectHost {
                        id: Some(row.id.clone()),
                        fly: true,
                    });
                }
            }
            // Scroll the selected row into view, no further than needed, when
            // the selection changes.
            if selection_changed
                && let Some(i) = rows.iter().position(|r| Some(r.id.as_str()) == selected)
            {
                ui.scroll_to_rect(row_rect(i), None);
            }
        });
}

/// One host-list row: role dot, name (ellipsised), ip on the right unless the
/// name already says it ("Raspberry Pi Foundation (192.168.1.251)").
fn draw_row(ui: &mut egui::Ui, rect: egui::Rect, row: &Row, selected: bool) -> egui::Response {
    let response = ui
        .interact(rect, ui.id().with(("host", &row.id)), egui::Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(format!("{} ({}) · {}", row.label, row.id, row.raw_role));
    let painter = ui.painter_at(rect);
    let fill = if selected {
        Some(theme::ROW_SELECTED)
    } else if response.hovered() {
        Some(theme::ROW_HOVER)
    } else {
        None
    };
    if let Some(fill) = fill {
        painter.rect_filled(rect, 6.0, egui_color(fill, 1.0));
    }
    let mut x = rect.left() + ROW_PADDING_X;
    painter.circle_filled(
        egui::pos2(x + DOT_SIZE / 2.0, rect.center().y),
        DOT_SIZE / 2.0,
        egui_color(role_color(row.role), 1.0),
    );
    x += DOT_SIZE + GAP;

    let mut right = rect.right() - ROW_PADDING_X;
    if !row.label.contains(&row.id) {
        let ip = painter.layout_no_wrap(
            row.id.clone(),
            egui::FontId::monospace(MONO_PX),
            egui_color(theme::MUTED, 1.0),
        );
        right -= ip.size().x;
        let pos = egui::pos2(right, rect.center().y - ip.size().y / 2.0);
        painter.galley(pos, ip, egui::Color32::PLACEHOLDER);
        right -= GAP;
    }
    let mut job = egui::text::LayoutJob::single_section(
        row.label.clone(),
        egui::TextFormat::simple(
            egui::FontId::proportional(BODY_PX),
            egui_color(theme::TEXT, 1.0),
        ),
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width((right - x).max(0.0));
    let name = painter.layout_job(job);
    let pos = egui::pos2(x, rect.center().y - name.size().y / 2.0);
    painter.galley(pos, name, egui::Color32::PLACEHOLDER);
    response
}

/// The details card: header, subtitle, then every field of the host JSON.
fn draw_details(
    ui: &mut egui::Ui,
    host: &Host,
    max_height: f32,
    selection_changed: bool,
    select: &mut MessageWriter<SelectHost>,
) {
    // Padding 10 10 12 and a 1 px border come off the card's own max height.
    let max_height = (max_height - 24.0).max(40.0);
    egui::Frame::new()
        .fill(egui_color(theme::HUD_BG_SOLID, 1.0))
        .stroke(egui::Stroke::new(1.0, egui_color(theme::HUD_BORDER, 1.0)))
        .corner_radius(8)
        .inner_margin(egui::Margin {
            left: 10,
            right: 10,
            top: 10,
            bottom: 12,
        })
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 4.0);
            let mut scroll = egui::ScrollArea::vertical()
                .id_salt("details")
                .max_height(max_height)
                .auto_shrink([false, true]);
            if selection_changed {
                scroll = scroll.vertical_scroll_offset(0.0); // `detailsEl.scrollTop = 0`
            }
            scroll.show(ui, |ui| {
                draw_details_header(ui, host, select);
                ui.add_space(6.0);
                for (key, value) in &host.fields {
                    if key == "ip" {
                        continue; // already in the subtitle
                    }
                    draw_field(ui, &heading(key), value);
                }
            });
        });
}

/// The card's header (name, "fly to", close) and subtitle (id · role badge).
/// The close button is "×" because "✕" is not in egui's bundled fonts.
fn draw_details_header(ui: &mut egui::Ui, host: &Host, select: &mut MessageWriter<SelectHost>) {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
        let small = |label: &str| {
            egui::Button::new(text(label, theme::TEXT).size(MONO_PX))
                .corner_radius(6)
                .min_size(egui::vec2(0.0, 22.0))
        };
        if ui.add(small("×")).on_hover_text("deselect (Esc)").clicked() {
            select.write(SelectHost {
                id: None,
                fly: false,
            });
        }
        if ui
            .add(small("fly to"))
            .on_hover_text("fly to this host (F)")
            .clicked()
        {
            select.write(SelectHost {
                id: Some(host.id.clone()),
                fly: true,
            });
        }
        ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.add(egui::Label::new(text(host.display_label(), theme::TEXT).size(16.0)).wrap());
        });
    });
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.label(mono(&host.id, theme::MUTED));
        ui.label(mono("·", theme::MUTED));
        let role = host.role.as_deref().unwrap_or("host");
        let badge = match host.role() {
            Role::Scanner => theme::ROLE_SCANNER,
            Role::Gateway => theme::ROLE_GATEWAY,
            Role::Host => theme::TEXT,
        };
        egui::Frame::new()
            .fill(egui_color(theme::ROW_HOVER, 1.0))
            .corner_radius(4)
            .inner_margin(egui::Margin::symmetric(6, 0))
            .show(ui, |ui| ui.label(mono(role, badge)));
    });
}

/// One field: a rule on top, then a key column and a value, or the key above a
/// block (table / nested object) that takes the full width.
fn draw_field(ui: &mut egui::Ui, key: &str, value: &Value) {
    let y = ui.cursor().top();
    ui.painter().hline(
        ui.max_rect().x_range(),
        y,
        egui::Stroke::new(1.0, egui_color(theme::HUD_BORDER, 1.0)),
    );
    ui.add_space(5.0);
    if is_block(value) {
        ui.label(mono(key, theme::MUTED));
        draw_value(ui, value, true);
    } else {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = KEY_GAP;
            ui.allocate_ui_with_layout(
                egui::vec2(KEY_COLUMN_PX, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(KEY_COLUMN_PX);
                    ui.add(egui::Label::new(mono(key, theme::MUTED)).wrap());
                },
            );
            ui.vertical(|ui| draw_value(ui, value, false));
        });
    }
    ui.add_space(5.0);
}

/// Scalars as text, lists of objects as a table whose columns are the union
/// of their keys, other lists inline, objects nested. Inside tables and nested
/// objects everything is monospace.
fn draw_value(ui: &mut egui::Ui, value: &Value, in_block: bool) {
    let plain = |s: &str| {
        if in_block {
            mono(s, theme::TEXT)
        } else {
            text(s, theme::TEXT)
        }
    };
    match value {
        Value::Null => {
            ui.label(mono("—", theme::MUTED));
        }
        Value::Array(items) if items.is_empty() => {
            ui.label(mono("none", theme::MUTED));
        }
        Value::Array(items) if items.iter().all(is_plain_object) => draw_table(ui, items),
        Value::Array(items) => {
            let joined = items.iter().map(scalar_text).collect::<Vec<_>>().join(", ");
            ui.add(egui::Label::new(plain(&joined)).wrap());
        }
        Value::Object(map) if map.is_empty() => {
            ui.label(mono("none", theme::MUTED));
        }
        Value::Object(map) => {
            egui::Grid::new(ui.next_auto_id())
                .num_columns(2)
                .spacing([KEY_GAP, 2.0])
                .show(ui, |ui| {
                    for (key, inner) in map {
                        ui.label(mono(&humanize(key), theme::MUTED));
                        draw_value(ui, inner, true);
                        ui.end_row();
                    }
                });
        }
        scalar => {
            ui.add(egui::Label::new(plain(&scalar_text(scalar))).wrap());
        }
    }
}

/// A list of objects as a table; columns are the union of the rows' keys, in
/// first-seen order.
fn draw_table(ui: &mut egui::Ui, rows: &[Value]) {
    let columns = table_columns(rows);
    egui::Grid::new(ui.next_auto_id())
        .num_columns(columns.len())
        .striped(true)
        .spacing([12.0, 3.0])
        .show(ui, |ui| {
            for column in &columns {
                ui.label(mono(&humanize(column), theme::MUTED));
            }
            ui.end_row();
            for row in rows {
                for column in &columns {
                    match row.get(*column) {
                        Some(cell) => draw_value(ui, cell, true),
                        None => {
                            ui.label(mono("—", theme::MUTED));
                        }
                    }
                }
                ui.end_row();
            }
        });
}

/// Every key that appears in any row, each once, in first-seen order.
pub fn table_columns(rows: &[Value]) -> Vec<&str> {
    let mut columns: Vec<&str> = Vec::new();
    for key in rows
        .iter()
        .filter_map(Value::as_object)
        .flat_map(|m| m.keys())
    {
        if !columns.contains(&key.as_str()) {
            columns.push(key);
        }
    }
    columns
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(json: &str) -> Host {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn ipv4_sorts_numerically_and_names_sort_as_text() {
        assert_eq!(address_key("192.168.1.9"), "192.168.001.009");
        assert!(address_key("192.168.1.9") < address_key("192.168.1.100"));
        assert_eq!(address_key("fe80::1"), "fe80::1");
        assert_eq!(address_key("nas"), "nas");
        assert_eq!(address_key("1.2.3"), "1.2.3");
    }

    #[test]
    fn rows_sort_scanner_gateway_hosts_then_unknown_roles_by_address() {
        let mut panel = Panel::default();
        panel.set_hosts(&[
            host(r#"{"id": "10.0.0.20", "role": "host"}"#),
            host(r#"{"id": "10.0.0.3", "role": "host", "label": "printer"}"#),
            host(r#"{"id": "10.0.0.2", "role": "printer"}"#),
            host(r#"{"id": "10.0.0.1", "role": "gateway"}"#),
            host(r#"{"id": "10.0.0.50", "role": "scanner"}"#),
            host(r#"{"id": "10.0.0.100", "role": "host"}"#),
        ]);
        let ids: Vec<&str> = panel.rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "10.0.0.50",
                "10.0.0.1",
                "10.0.0.3",
                "10.0.0.20",
                "10.0.0.100",
                "10.0.0.2"
            ]
        );
        assert_eq!(panel.rows[2].label, "printer");
        let unknown = &panel.rows[5];
        assert_eq!(
            (unknown.role, unknown.raw_role.as_str()),
            (Role::Host, "printer")
        );
    }

    #[test]
    fn the_filter_matches_label_id_hostname_mac_vendor_role_and_ports() {
        let h = host(
            r#"{"id": "10.0.0.3", "label": "nas.lan", "role": "host", "hostname": "nas.lan",
                "mac": "00:11:32:AB:CD:EF", "vendor": "Synology Incorporated",
                "open_ports": [{"number": 22, "proto": "tcp"}, {"number": 445, "proto": "tcp"}]}"#,
        );
        assert_eq!(
            search_text(&h),
            "nas.lan 10.0.0.3 nas.lan 00:11:32:ab:cd:ef synology incorporated host 22/tcp 445/tcp"
        );
        // Missing keys are skipped, not written as "undefined" or "null".
        assert_eq!(
            search_text(&host(r#"{"id": "10.0.0.9", "hostname": null}"#)),
            "10.0.0.9"
        );
        let mut panel = Panel::default();
        panel.set_hosts(&[h, host(r#"{"id": "10.0.0.4", "role": "gateway"}"#)]);
        panel.filter = "  445/TCP ".into();
        assert_eq!(panel.visible().len(), 1);
        panel.filter = "gate".into();
        assert_eq!(panel.visible()[0].id, "10.0.0.4");
        panel.filter.clear();
        assert_eq!(panel.visible().len(), 2);
    }

    #[test]
    fn scalars_render_as_the_card_shows_them() {
        assert_eq!(scalar_text(&Value::Null), "—");
        assert_eq!(scalar_text(&Value::Bool(true)), "yes");
        assert_eq!(scalar_text(&Value::Bool(false)), "no");
        assert_eq!(scalar_text(&serde_json::json!(3.5)), "3.5");
        assert_eq!(scalar_text(&serde_json::json!(22)), "22");
        assert_eq!(
            scalar_text(&serde_json::json!(1.0)),
            "1",
            "a whole float prints without a decimal point"
        );
        assert_eq!(scalar_text(&serde_json::json!(-40.0)), "-40");
        assert_eq!(scalar_text(&serde_json::json!("ssh")), "ssh");
        assert_eq!(heading("open_ports"), "open ports");
        assert_eq!(heading("ip"), "IP");
        assert_eq!(heading("is_local"), "local");
        assert_eq!(heading("mac_address"), "mac address");
    }

    #[test]
    fn blocks_are_tables_and_non_empty_objects() {
        assert!(is_block(&serde_json::json!([{"a": 1}, {"b": 2}])));
        assert!(!is_block(&serde_json::json!([1, 2])));
        assert!(!is_block(&serde_json::json!([])));
        assert!(is_block(&serde_json::json!({"vendor": "x"})));
        assert!(!is_block(&serde_json::json!({})));
        assert!(!is_block(&serde_json::json!("text")));
    }

    #[test]
    fn table_columns_are_the_union_of_keys_in_first_seen_order() {
        let rows = serde_json::json!([
            {"number": 22, "proto": "tcp", "service": "ssh"},
            {"number": 9999, "proto": "tcp"},
            {"number": 53, "proto": "udp", "banner": "dnsmasq"}
        ]);
        assert_eq!(
            table_columns(rows.as_array().unwrap()),
            ["number", "proto", "service", "banner"]
        );
    }

    #[test]
    fn the_framing_inset_follows_the_sidebar() {
        let mut panel = Panel::default();
        assert_eq!(
            panel.frame_inset(1280.0).right,
            312.0,
            "300 px + 12 px margin"
        );
        assert_eq!(
            panel.frame_inset(700.0).right,
            232.0,
            "narrow window: 220 + 12"
        );
        panel.zoom = 1.5; // Ctrl +: 1280 logical px is 853 points, still wide
        assert_eq!(panel.frame_inset(1280.0).right, 468.0);
        panel.collapsed = true;
        assert_eq!(panel.frame_inset(1280.0).right, 0.0);
        assert_eq!(panel.frame_inset(1280.0).left, 0.0);
    }

    #[test]
    fn focusing_the_filter_reopens_a_collapsed_sidebar() {
        let mut panel = Panel {
            collapsed: true,
            ..Panel::default()
        };
        panel.focus_filter();
        assert!(!panel.collapsed && panel.focus_filter);
    }
}
