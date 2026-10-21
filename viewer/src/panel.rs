//! Sidebar: a filterable host list and a details card for the selected host.
//! Port of panel.js. (The HUD, legend and help overlay live in ui.rs.)
//!
//! The details card is generic on purpose. Apart from a few keys that get a
//! friendlier heading, every field of the host JSON is rendered as it comes
//! (scalars as rows, lists of objects as tables, objects as nested rows), so a
//! field added to pynmapper/model.py shows up here with no viewer change.

use bevy::prelude::*;
use bevy_egui::egui;
use serde_json::Value;

use crate::controls::FlyControls;
use crate::graph::{Graph, MapDocument};
use crate::map::{Host, Role};
use crate::picking::{Picker, SelectHost};
use crate::theme;
use crate::ui::{egui_color, mono, text_color};

/// `--sidebar-width` and the 12 px the card keeps from the window edge.
pub const SIDEBAR_WIDTH: f32 = 300.0;
pub const SIDEBAR_MARGIN: f32 = 12.0;
/// Frontend-only keys, not shown in the details card.
const GRAPHICS_KEYS: [&str; 4] = ["id", "label", "role", "position"];
/// Column width of the key column in a `.field` row.
const KEY_COLUMN_PX: f32 = 92.0;

/// `HEADINGS` in panel.js: friendlier names for the fields the model has today.
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
/// per map (`compareHosts`), so the per-frame draw does no work but filtering.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// Index into `MapDocument.hosts`, for the details card.
    pub index: usize,
    pub id: String,
    pub label: String,
    pub role: Role,
    /// Lower-cased text the filter box matches against (`searchText`).
    pub search: String,
}

#[derive(Resource, Debug, Default)]
pub struct Panel {
    pub collapsed: bool,
    /// What is typed in the filter box, as typed.
    pub filter: String,
    /// `/` was pressed: give the filter box focus when it is next drawn.
    pub focus_filter: bool,
    pub rows: Vec<Row>,
    /// Selection seen last frame, to scroll the row into view once per change.
    last_selected: Option<String>,
    /// Logical pixels the sidebar takes from the right edge, for framing
    /// (`frameInset` in app.js); 0 while collapsed.
    pub inset_right: f32,
}

impl Panel {
    /// `setHosts`: a sorted copy of the list.
    pub fn set_hosts(&mut self, hosts: &[Host]) {
        let mut rows: Vec<Row> = hosts
            .iter()
            .enumerate()
            .map(|(index, host)| Row {
                index,
                id: host.id.clone(),
                label: host.display_label().to_string(),
                role: host.role(),
                search: search_text(host),
            })
            .collect();
        rows.sort_by(compare_rows);
        self.rows = rows;
    }

    /// The normalised query (`this.query`): trimmed, lower-cased.
    pub fn query(&self) -> String {
        self.filter.trim().to_lowercase()
    }

    /// `visibleHosts`: every row, or those matching the query.
    pub fn visible(&self) -> Vec<&Row> {
        let query = self.query();
        self.rows.iter().filter(|row| query.is_empty() || row.search.contains(&query)).collect()
    }

    /// `focusFilter`: `/` opens the sidebar if needed and puts the cursor in the box.
    pub fn focus_filter(&mut self) {
        self.collapsed = false;
        self.focus_filter = true;
    }
}

/// Dotted IPv4 sorts numerically; anything else (IPv6, names) falls back to text.
pub fn address_key(id: &str) -> String {
    let octets: Vec<&str> = id.split('.').collect();
    let is_ipv4 = octets.len() == 4 && octets.iter().all(|o| !o.is_empty() && o.bytes().all(|b| b.is_ascii_digit()));
    if is_ipv4 {
        octets.iter().map(|o| format!("{o:0>3}")).collect::<Vec<_>>().join(".")
    } else {
        id.to_string()
    }
}

fn role_order(role: Role) -> u8 {
    match role {
        Role::Scanner => 0,
        Role::Gateway => 1,
        Role::Host => 2,
    }
}

pub fn compare_rows(a: &Row, b: &Row) -> std::cmp::Ordering {
    role_order(a.role).cmp(&role_order(b.role)).then_with(|| address_key(&a.id).cmp(&address_key(&b.id)))
}

/// `searchText`: label, id, hostname, role and "number/proto" for each port.
pub fn search_text(host: &Host) -> String {
    let mut parts: Vec<String> = vec![host.display_label().to_string(), host.id.clone()];
    if let Some(name) = host.fields.get("hostname").and_then(Value::as_str) {
        parts.push(name.to_string());
    }
    parts.push(host.role().name().to_string());
    if let Some(ports) = host.fields.get("open_ports").and_then(Value::as_array) {
        for port in ports {
            let number = port.get("number").map(scalar_text).unwrap_or_default();
            let proto = port.get("proto").map(scalar_text).unwrap_or_default();
            parts.push(format!("{number}/{proto}"));
        }
    }
    parts.join(" ").to_lowercase()
}

/// A JSON scalar as the card shows it (`renderValue` for non-objects).
pub fn scalar_text(value: &Value) -> String {
    match value {
        Value::Null => "—".to_string(),
        Value::Bool(true) => "yes".to_string(),
        Value::Bool(false) => "no".to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        Value::Array(_) | Value::Object(_) => value.to_string(),
    }
}

fn is_plain_object(value: &Value) -> bool {
    value.is_object()
}

/// `isBlock`: tables and nested objects take the full width below their key.
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
        app.init_resource::<Panel>().add_systems(Update, refresh_rows.in_set(crate::ViewerSet::Build));
    }
}

/// `panel.setHosts(doc.hosts)` whenever a new document arrives.
fn refresh_rows(doc: Res<MapDocument>, mut panel: ResMut<Panel>) {
    if doc.is_changed() {
        panel.set_hosts(&doc.0.hosts);
    }
}

/// Draw the sidebar. Called from ui.rs inside the egui pass, before the
/// labels, so `inset_right` and the input flags are fresh for this frame.
#[allow(clippy::too_many_arguments)] // a Bevy system lists what it reads
pub fn draw_panel(
    ctx: &egui::Context,
    panel: &mut Panel,
    doc: &MapDocument,
    graph: &Graph,
    picker: &Picker,
    controls: &FlyControls,
    select: &mut MessageWriter<SelectHost>,
) {
    let _ = (graph, controls);
    let selected = picker.selected_id.clone();
    let selection_changed = selected != panel.last_selected;
    panel.last_selected = selected.clone();

    let width = if panel.collapsed { 0.0 } else { SIDEBAR_WIDTH };
    let response = egui::SidePanel::right("sidebar")
        .resizable(false)
        .exact_width(width + SIDEBAR_MARGIN + if panel.collapsed { 40.0 } else { 0.0 })
        .frame(egui::Frame::NONE.inner_margin(egui::Margin {
            left: 0,
            right: SIDEBAR_MARGIN as i8,
            top: SIDEBAR_MARGIN as i8,
            bottom: SIDEBAR_MARGIN as i8,
        }))
        .show_separator_line(false)
        .show(ctx, |ui| {
            let card = egui::Frame::new()
                .fill(egui_color(theme::HUD_BG, 1.0))
                .stroke(egui::Stroke::new(1.0, egui_color(theme::HUD_BORDER, 1.0)))
                .corner_radius(10.0)
                .inner_margin(10.0);
            if panel.collapsed {
                // Just the toggle, top right, like `#sidebar.collapsed`.
                card.show(ui, |ui| {
                    if ui.add(toggle_button(true)).clicked() {
                        panel.collapsed = false;
                    }
                });
                return;
            }
            card.show(ui, |ui| {
                ui.set_min_height(ui.available_height());
                ui.set_width(ui.available_width());
                draw_head(ui, panel);
                ui.add_space(8.0);
                if let Some(id) = &selected
                    && let Some(host) = doc.0.hosts.iter().find(|h| &h.id == id)
                {
                    draw_details(ui, host, select);
                    ui.add_space(8.0);
                }
                draw_list(ui, panel, selected.as_deref(), selection_changed, select);
            });
        });
    panel.inset_right = if panel.collapsed { 0.0 } else { response.response.rect.width() };
}

fn toggle_button(collapsed: bool) -> egui::Button<'static> {
    egui::Button::new(egui::RichText::new(if collapsed { "⟨" } else { "⟩" }).color(text_color(theme::MUTED)))
}

/// `#sidebar-head`: filter box, count, toggle.
fn draw_head(ui: &mut egui::Ui, panel: &mut Panel) {
    let shown = panel.visible().len();
    let total = panel.rows.len();
    let count = if panel.query().is_empty() { total.to_string() } else { format!("{shown} / {total}") };
    ui.horizontal(|ui| {
        if ui.add(toggle_button(false)).on_hover_text("show / hide the host list").clicked() {
            panel.collapsed = true;
        }
        ui.label(mono(&count, theme::MUTED));
        let edit = egui::TextEdit::singleline(&mut panel.filter)
            .hint_text("filter hosts  ( / )")
            .desired_width(f32::INFINITY);
        let response = ui.add(edit);
        if panel.focus_filter {
            response.request_focus();
            panel.focus_filter = false;
        }
        panel.filter_response_lost_focus_on_escape(ui, &response);
    });
}

impl Panel {
    /// Escape while typing leaves the box (`e.target.blur()` in app.js);
    /// Enter picks the first visible host (handled where the list is drawn).
    fn filter_response_lost_focus_on_escape(&self, ui: &egui::Ui, response: &egui::Response) {
        if response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            response.surrender_focus();
        }
    }
}

/// `#hosts`: one row per visible host; click = select + fly.
fn draw_list(
    ui: &mut egui::Ui,
    panel: &mut Panel,
    selected: Option<&str>,
    selection_changed: bool,
    select: &mut MessageWriter<SelectHost>,
) {
    let enter = ui.input(|i| i.key_pressed(egui::Key::Enter)) && ui.memory(|m| m.focused().is_some());
    let visible: Vec<Row> = panel.visible().into_iter().cloned().collect();
    if enter && let Some(first) = visible.first() {
        select.write(SelectHost { id: Some(first.id.clone()), fly: true });
    }
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        for row in &visible {
            let is_selected = selected == Some(row.id.as_str());
            let fill = if is_selected { Some(theme::ROW_SELECTED) } else { None };
            let response = egui::Frame::new()
                .fill(fill.map(|c| egui_color(c, 1.0)).unwrap_or(egui::Color32::TRANSPARENT))
                .corner_radius(6.0)
                .inner_margin(egui::Margin::symmetric(8, 5))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        let (rect, _) = ui.allocate_exact_size(egui::vec2(9.0, 9.0), egui::Sense::hover());
                        ui.painter().circle_filled(rect.center(), 4.5, egui_color(role_color(row.role), 1.0));
                        ui.add(egui::Label::new(egui::RichText::new(&row.label).color(text_color(theme::TEXT))).truncate());
                        if row.label != row.id {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.label(mono(&row.id, theme::MUTED));
                            });
                        }
                    });
                })
                .response
                .interact(egui::Sense::click())
                .on_hover_text(format!("{} ({}) · {}", row.label, row.id, row.role.name()));
            if response.hovered() && !is_selected {
                ui.painter().rect_filled(response.rect, 6.0, egui_color(theme::ROW_HOVER, 1.0));
            }
            if is_selected && selection_changed {
                response.scroll_to_me(None);
            }
            if response.clicked() {
                select.write(SelectHost { id: Some(row.id.clone()), fly: true });
            }
        }
    });
}

fn role_color(role: Role) -> Color {
    match role {
        Role::Scanner => theme::ROLE_SCANNER,
        Role::Gateway => theme::ROLE_GATEWAY,
        Role::Host => theme::ROLE_HOST,
    }
}

/// `#details`: header, subtitle, then every field of the host JSON.
fn draw_details(ui: &mut egui::Ui, host: &Host, select: &mut MessageWriter<SelectHost>) {
    egui::Frame::new()
        .fill(egui_color(theme::HUD_BG_SOLID, 1.0))
        .stroke(egui::Stroke::new(1.0, egui_color(theme::HUD_BORDER, 1.0)))
        .corner_radius(8.0)
        .inner_margin(egui::Margin { left: 10, right: 10, top: 10, bottom: 12 })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let max_height = ui.ctx().content_rect().height() * 0.55;
            egui::ScrollArea::vertical().max_height(max_height).auto_shrink([false, true]).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(host.display_label()).size(16.0).strong().color(text_color(theme::TEXT)),
                        )
                        .wrap(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                        if ui.small_button("✕").on_hover_text("deselect (Esc)").clicked() {
                            select.write(SelectHost { id: None, fly: false });
                        }
                        if ui.small_button("fly to").on_hover_text("fly to this host (F)").clicked() {
                            select.write(SelectHost { id: Some(host.id.clone()), fly: true });
                        }
                    });
                });
                ui.horizontal(|ui| {
                    ui.label(mono(&host.id, theme::MUTED));
                    ui.label(mono("·", theme::MUTED));
                    let badge_color = match host.role() {
                        Role::Scanner => theme::ROLE_SCANNER,
                        Role::Gateway => theme::ROLE_GATEWAY,
                        Role::Host => theme::TEXT,
                    };
                    egui::Frame::new()
                        .fill(egui_color(theme::ROW_HOVER, 1.0))
                        .corner_radius(4.0)
                        .inner_margin(egui::Margin::symmetric(6, 0))
                        .show(ui, |ui| ui.label(mono(host.role().name(), badge_color)));
                });
                ui.add_space(6.0);
                for (key, value) in &host.fields {
                    if GRAPHICS_KEYS.contains(&key.as_str()) || key == "ip" {
                        continue; // ip is already in the subtitle
                    }
                    draw_field(ui, &heading(key), value);
                }
            });
        });
}

/// `.field`: a key column and a value, or key above a block (table / nested).
fn draw_field(ui: &mut egui::Ui, key: &str, value: &Value) {
    ui.separator();
    if is_block(value) {
        ui.label(mono(key, theme::MUTED));
        draw_value(ui, value);
    } else {
        ui.horizontal_top(|ui| {
            ui.add_sized([KEY_COLUMN_PX, 0.0], egui::Label::new(mono(key, theme::MUTED)).halign(egui::Align::LEFT));
            draw_value(ui, value);
        });
    }
}

/// `renderValue`: scalars as text, lists of objects as a table whose columns
/// are the union of their keys, other lists inline, objects nested.
fn draw_value(ui: &mut egui::Ui, value: &Value) {
    match value {
        Value::Array(items) if items.is_empty() => {
            ui.label(mono("none", theme::MUTED));
        }
        Value::Array(items) if items.iter().all(is_plain_object) => draw_table(ui, items),
        Value::Array(items) => {
            let text = items.iter().map(scalar_text).collect::<Vec<_>>().join(", ");
            ui.add(egui::Label::new(egui::RichText::new(text).color(text_color(theme::TEXT))).wrap());
        }
        Value::Object(map) if map.is_empty() => {
            ui.label(mono("none", theme::MUTED));
        }
        Value::Object(map) => {
            egui::Grid::new(ui.next_auto_id()).num_columns(2).spacing([10.0, 2.0]).show(ui, |ui| {
                for (key, inner) in map {
                    ui.label(mono(&humanize(key), theme::MUTED));
                    draw_value(ui, inner);
                    ui.end_row();
                }
            });
        }
        Value::Null => {
            ui.label(mono("—", theme::MUTED));
        }
        scalar => {
            ui.add(egui::Label::new(egui::RichText::new(scalar_text(scalar)).color(text_color(theme::TEXT))).wrap());
        }
    }
}

/// `renderTable`: columns are the union of the rows' keys, in first-seen order.
fn draw_table(ui: &mut egui::Ui, rows: &[Value]) {
    let mut columns: Vec<&str> = Vec::new();
    for row in rows {
        if let Some(map) = row.as_object() {
            for key in map.keys() {
                if !columns.contains(&key.as_str()) {
                    columns.push(key);
                }
            }
        }
    }
    egui::Grid::new(ui.next_auto_id()).num_columns(columns.len()).striped(true).spacing([12.0, 3.0]).show(ui, |ui| {
        for column in &columns {
            ui.label(mono(&humanize(column), theme::MUTED));
        }
        ui.end_row();
        for row in rows {
            for column in &columns {
                match row.get(*column) {
                    Some(cell) => draw_value(ui, cell),
                    None => {
                        ui.label(mono("—", theme::MUTED));
                    }
                }
            }
            ui.end_row();
        }
    });
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
    fn rows_sort_scanner_gateway_then_hosts_by_address() {
        let mut panel = Panel::default();
        panel.set_hosts(&[
            host(r#"{"id": "10.0.0.20", "role": "host"}"#),
            host(r#"{"id": "10.0.0.3", "role": "host", "label": "printer"}"#),
            host(r#"{"id": "10.0.0.1", "role": "gateway"}"#),
            host(r#"{"id": "10.0.0.50", "role": "scanner"}"#),
            host(r#"{"id": "10.0.0.100", "role": "host"}"#),
        ]);
        let ids: Vec<&str> = panel.rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["10.0.0.50", "10.0.0.1", "10.0.0.3", "10.0.0.20", "10.0.0.100"]);
        assert_eq!(panel.rows[2].label, "printer");
        assert_eq!(panel.rows[2].index, 1, "index points back into the document order");
    }

    #[test]
    fn the_filter_matches_label_id_hostname_role_and_ports() {
        let h = host(
            r#"{"id": "10.0.0.3", "label": "nas.lan", "role": "host", "hostname": "nas.lan",
                "open_ports": [{"number": 22, "proto": "tcp"}, {"number": 445, "proto": "tcp"}]}"#,
        );
        assert_eq!(search_text(&h), "nas.lan 10.0.0.3 nas.lan host 22/tcp 445/tcp");
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
    fn scalars_render_like_the_web_card() {
        assert_eq!(scalar_text(&Value::Null), "—");
        assert_eq!(scalar_text(&Value::Bool(true)), "yes");
        assert_eq!(scalar_text(&Value::Bool(false)), "no");
        assert_eq!(scalar_text(&serde_json::json!(3.5)), "3.5");
        assert_eq!(scalar_text(&serde_json::json!("ssh")), "ssh");
        assert_eq!(heading("open_ports"), "open ports");
        assert_eq!(heading("ip"), "IP");
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
}
