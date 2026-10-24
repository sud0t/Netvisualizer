//! The host list's model: the sorted rows of the map's hosts, the filter
//! that picks among them, whether the rail is collapsed, and how much of
//! the window the rail and the dock cover (so the map is framed in what is
//! left). ui/rail.rs draws it.
//!
//! The helpers at the bottom turn host JSON into text the way the details
//! card shows it; ui/rail.rs builds its lines from them.

use bevy::prelude::*;
use serde_json::Value;

use crate::ViewerSet;
use crate::graph::MapDocument;
use crate::map::{Host, Role};
use crate::scene::Inset;
use crate::theme;
use crate::ui::dock::DOCK_INSET;

/// Rail width, and the narrower one in windows up to `NARROW_WINDOW` wide.
const SIDEBAR_WIDTH: f32 = 330.0;
const NARROW_SIDEBAR_WIDTH: f32 = 220.0;
const NARROW_WINDOW: f32 = 720.0;

/// A JSON key as a table heading: underscores become spaces.
pub fn humanize(key: &str) -> String {
    key.replace('_', " ")
}

/// One row of the host list: what the list and the filter need, sorted once
/// per map by [`sort_key`], so the per-frame draw does no work but filtering.
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

#[derive(Resource, Debug, Default)]
pub struct Panel {
    pub collapsed: bool,
    /// What is typed in the filter box, as typed.
    pub filter: String,
    /// `/` was pressed: give the filter box focus when it is next drawn.
    pub focus_filter: bool,
    pub rows: Vec<Row>,
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
        rows.sort_by_cached_key(sort_key);
        self.rows = rows;
    }

    /// The normalised query: trimmed, lower-cased.
    pub fn query(&self) -> String {
        self.filter.trim().to_lowercase()
    }

    /// The indices of every row, or of those matching the query.
    pub fn visible_indices(&self) -> Vec<usize> {
        let query = self.query();
        (0..self.rows.len())
            .filter(|&i| query.is_empty() || self.rows[i].search.contains(&query))
            .collect()
    }

    /// Every row, or those matching the query.
    #[cfg(test)]
    pub fn visible(&self) -> Vec<&Row> {
        self.visible_indices()
            .into_iter()
            .map(|i| &self.rows[i])
            .collect()
    }

    /// `/` opens the rail if needed and puts the cursor in the filter box.
    pub fn focus_filter(&mut self) {
        self.collapsed = false;
        self.focus_filter = true;
    }

    /// The logical pixels the UI hides at the edges of a window
    /// `window_width` logical pixels wide: the rail on the right (none while
    /// collapsed) and the dock along the bottom.
    pub fn frame_inset(&self, window_width: f32) -> Inset {
        let right = if self.collapsed {
            0.0
        } else {
            sidebar_width(window_width) + theme::EDGE
        };
        Inset {
            right,
            bottom: DOCK_INSET,
            ..Inset::default()
        }
    }
}

/// The rail's width for a window this many logical pixels wide.
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

/// Sort position of a role: scanner, gateway, router, host (which a host
/// without a role is drawn as), then roles this viewer does not know.
fn role_order(raw_role: &str) -> u8 {
    match raw_role {
        "scanner" => 0,
        "gateway" => 1,
        "router" => 2,
        "host" | "" => 3,
        _ => 9,
    }
}

/// What rows sort by: role, then address. Names compare without case
/// first, with the exact text as the tie-break. Computed once per row.
pub fn sort_key(row: &Row) -> (u8, String, String) {
    let key = address_key(&row.id);
    (role_order(&row.raw_role), key.to_lowercase(), key)
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

pub struct PanelPlugin;

impl Plugin for PanelPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Panel>()
            .add_systems(Update, refresh_rows.in_set(ViewerSet::Build));
    }
}

/// Rebuild the host list whenever a new document arrives.
fn refresh_rows(doc: Res<MapDocument>, mut panel: ResMut<Panel>) {
    if doc.is_changed() {
        match &doc.0 {
            Some(doc) => panel.set_hosts(&doc.hosts),
            None => panel.rows.clear(), // the map was cleared
        }
    }
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
            host(r#"{"id": "10.0.0.7"}"#), // no role: drawn as a host, sorted as one
        ]);
        let ids: Vec<&str> = panel.rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "10.0.0.50",
                "10.0.0.1",
                "10.0.0.3",
                "10.0.0.7",
                "10.0.0.20",
                "10.0.0.100",
                "10.0.0.2"
            ]
        );
        assert_eq!(panel.rows[2].label, "printer");
        let unknown = &panel.rows[6];
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
        assert_eq!(humanize("mac_address"), "mac address");
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
    fn the_framing_inset_follows_the_rail_and_the_dock() {
        let mut panel = Panel::default();
        let inset = panel.frame_inset(1280.0);
        assert_eq!(inset.right, 340.0, "330 px + 10 px margin");
        assert_eq!(inset.bottom, DOCK_INSET);
        assert_eq!(
            panel.frame_inset(700.0).right,
            230.0,
            "narrow window: 220 + 10"
        );
        panel.collapsed = true;
        assert_eq!(panel.frame_inset(1280.0).right, 0.0);
        assert_eq!(panel.frame_inset(1280.0).left, 0.0);
        assert_eq!(panel.frame_inset(1280.0).bottom, DOCK_INSET);
    }

    #[test]
    fn focusing_the_filter_reopens_a_collapsed_rail() {
        let mut panel = Panel {
            collapsed: true,
            ..Panel::default()
        };
        panel.focus_filter();
        assert!(!panel.collapsed && panel.focus_filter);
    }
}
