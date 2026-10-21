//! The JSON contract (`NetworkMap.to_dict()` in `pynmapper/graphics/model.py`)
//! as serde structs. This is the only thing the viewer knows about Python.
//!
//! Only the keys the viewer itself acts on are typed (`id`, `label`, `role`,
//! `position` on a host; `source`/`target` on a link). Everything else a host
//! carries (`ip`, `hostname`, `open_ports`, `extra`, and any field added to
//! `pynmapper/model.py` later) is kept verbatim in [`Host::fields`], in the
//! order Python wrote it, so the generic details card in panel.rs can render
//! it with no change here when the backend grows.

use std::error::Error;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value};

/// The newest contract version this viewer understands.
pub const CONTRACT_VERSION: u64 = 1;

#[derive(Debug, Clone, Deserialize)]
pub struct MapDoc {
    #[serde(default = "default_version")]
    pub version: u64,
    #[serde(default)]
    pub revision: u64,
    #[allow(dead_code)] // part of the contract mirror; nothing draws it yet
    #[serde(default)]
    pub generated_at: Option<String>,
    #[serde(default)]
    pub scan: Scan,
    #[serde(default)]
    pub hosts: Vec<Host>,
    #[serde(default)]
    pub links: Vec<Link>,
}

fn default_version() -> u64 {
    CONTRACT_VERSION
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Scan {
    /// `scan.type` ("ping_sweep", "demo", ...); `type` is a keyword in Rust.
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub network: Option<String>,
    #[allow(dead_code)] // part of the contract mirror; nothing draws it yet
    #[serde(default)]
    pub scanner_ip: Option<String>,
    /// Free text from `__main__.py` while a scan is in progress ("sweeping…").
    #[serde(default)]
    pub stage: Option<String>,
    #[allow(dead_code)] // part of the contract mirror; nothing draws it yet
    #[serde(flatten)]
    pub rest: Map<String, Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Host {
    pub id: String,
    #[serde(default)]
    pub label: Option<String>,
    /// The raw role string; use [`Host::role`] for the validated value.
    #[serde(default)]
    pub role: Option<String>,
    /// `None` unless the backend sent three finite numbers; graph.rs then
    /// places the host on the fallback ring.
    #[serde(default, deserialize_with = "lenient_position")]
    pub position: Option<[f32; 3]>,
    /// Every other key of the host object, in document order.
    #[serde(flatten)]
    pub fields: Map<String, Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Link {
    pub source: String,
    pub target: String,
    #[allow(dead_code)] // part of the contract mirror; nothing draws it yet
    #[serde(default)]
    pub kind: Option<String>,
    #[allow(dead_code)] // part of the contract mirror; nothing draws it yet
    #[serde(flatten)]
    pub rest: Map<String, Value>,
}

/// What a host is in the map. Unknown roles draw as plain hosts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    Scanner,
    Gateway,
    Host,
}

impl Role {
    pub fn parse(raw: Option<&str>) -> Role {
        match raw {
            Some("scanner") => Role::Scanner,
            Some("gateway") => Role::Gateway,
            _ => Role::Host,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Role::Scanner => "scanner",
            Role::Gateway => "gateway",
            Role::Host => "host",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Up,
    Down,
}

impl Host {
    /// The host's label, or its id when it has none or it is empty.
    pub fn display_label(&self) -> &str {
        match self.label.as_deref() {
            Some(label) if !label.is_empty() => label,
            _ => &self.id,
        }
    }

    pub fn role(&self) -> Role {
        Role::parse(self.role.as_deref())
    }

    /// The Host model has no status field yet. Until it does, "down" is read
    /// from a top-level `status` or from `extra.status`, so a backend that
    /// starts reporting it in either place greys those nodes out with no
    /// viewer change.
    pub fn status(&self) -> Status {
        let top_level = self.fields.get("status").filter(|v| !v.is_null());
        let in_extra = || {
            self.fields
                .get("extra")
                .and_then(|extra| extra.get("status"))
        };
        match top_level.or_else(in_extra).and_then(Value::as_str) {
            Some(s) if s.eq_ignore_ascii_case("down") => Status::Down,
            _ => Status::Up,
        }
    }
}

impl MapDoc {
    pub fn from_json(text: &str) -> Result<MapDoc, serde_json::Error> {
        serde_json::from_str(text)
    }

    pub fn load(path: &Path) -> Result<MapDoc, Box<dyn Error>> {
        Ok(MapDoc::from_json(&fs::read_to_string(path)?)?)
    }

    /// One line for the HUD / window title:
    /// `ping_sweep 192.168.1.0/24 · sweeping… · 12 hosts · 11 links · rev 3`.
    pub fn summary(&self) -> String {
        let place: Vec<&str> = [self.scan.kind.as_deref(), self.scan.network.as_deref()]
            .into_iter()
            .flatten()
            .filter(|s| !s.is_empty())
            .collect();
        let place = if place.is_empty() {
            "map".to_string()
        } else {
            place.join(" ")
        };
        let stage = match self.scan.stage.as_deref() {
            Some(stage) if !stage.is_empty() => format!(" · {stage}"),
            _ => String::new(),
        };
        format!(
            "{place}{stage} · {} hosts · {} links · rev {}",
            self.hosts.len(),
            self.links.len(),
            self.revision
        )
    }
}

/// Accepts `[x, y, z]` with finite numbers and turns anything else (missing,
/// null, wrong length, strings) into `None` instead of failing the whole document.
fn lenient_position<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<[f32; 3]>, D::Error> {
    let value = Option::<Value>::deserialize(deserializer)?;
    Ok(value.as_ref().and_then(position_from_value))
}

pub fn position_from_value(value: &Value) -> Option<[f32; 3]> {
    let items = value.as_array()?;
    if items.len() != 3 {
        return None;
    }
    let mut out = [0.0f32; 3];
    for (slot, item) in out.iter_mut().zip(items) {
        let number = item.as_f64()?;
        if !number.is_finite() {
            return None;
        }
        *slot = number as f32;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEMO: &str = include_str!("../tests/fixtures/demo.json");

    fn host(json: &str) -> Host {
        serde_json::from_str(json).expect("host JSON")
    }

    #[test]
    fn parses_the_demo_map() {
        let doc = MapDoc::from_json(DEMO).unwrap();
        assert_eq!(doc.version, 1);
        assert_eq!(doc.hosts.len(), 27);
        assert_eq!(doc.links.len(), 26);
        assert_eq!(doc.scan.kind.as_deref(), Some("demo"));
        assert_eq!(doc.scan.network.as_deref(), Some("192.168.1.0/24"));

        let scanner = &doc.hosts[0];
        assert_eq!(scanner.role(), Role::Scanner);
        assert_eq!(scanner.display_label(), "pynmapper-host.lan");
        assert_eq!(scanner.position, Some([0.0, 0.0, 0.0]));
        assert_eq!(scanner.status(), Status::Up);

        let gateway = &doc.hosts[1];
        assert_eq!(gateway.role(), Role::Gateway);
        assert_eq!(gateway.position, Some([8.0, 0.0, 0.0]));
        assert!(doc.hosts.iter().all(|h| h.position.is_some()));
        assert!(
            doc.links
                .iter()
                .all(|l| l.kind.as_deref() == Some("subnet"))
        );
    }

    #[test]
    fn keeps_every_other_field_in_document_order() {
        let doc = MapDoc::from_json(DEMO).unwrap();
        let keys: Vec<&str> = doc.hosts[0].fields.keys().map(String::as_str).collect();
        assert_eq!(keys, ["ip", "hostname", "open_ports", "is_local", "extra"]);

        // A field the backend adds tomorrow must survive the round trip untouched.
        let h = host(
            r#"{"id": "10.0.0.2", "ip": "10.0.0.2", "mac": "aa:bb", "role": "host", "position": [1, 2, 3]}"#,
        );
        assert_eq!(h.fields["mac"], "aa:bb");
        assert!(
            !h.fields.contains_key("position"),
            "typed keys are not duplicated"
        );
        assert_eq!(h.position, Some([1.0, 2.0, 3.0]));
    }

    #[test]
    fn unknown_roles_draw_as_hosts() {
        assert_eq!(host(r#"{"id": "a"}"#).role(), Role::Host);
        assert_eq!(host(r#"{"id": "a", "role": "printer"}"#).role(), Role::Host);
        assert_eq!(
            host(r#"{"id": "a", "role": "gateway"}"#).role(),
            Role::Gateway
        );
        assert_eq!(host(r#"{"id": "a"}"#).display_label(), "a");
        assert_eq!(host(r#"{"id": "a", "label": ""}"#).display_label(), "a");
    }

    #[test]
    fn status_is_read_from_the_top_level_or_from_extra() {
        assert_eq!(
            host(r#"{"id": "a", "status": "DOWN"}"#).status(),
            Status::Down
        );
        assert_eq!(
            host(r#"{"id": "a", "extra": {"status": "down"}}"#).status(),
            Status::Down
        );
        assert_eq!(
            host(r#"{"id": "a", "status": null, "extra": {"status": "down"}}"#).status(),
            Status::Down
        );
        assert_eq!(
            host(r#"{"id": "a", "status": "up", "extra": {"status": "down"}}"#).status(),
            Status::Up
        );
        assert_eq!(host(r#"{"id": "a", "status": 0}"#).status(), Status::Up);
        assert_eq!(host(r#"{"id": "a"}"#).status(), Status::Up);
    }

    #[test]
    fn malformed_positions_become_none_instead_of_errors() {
        for bad in [
            r#"{"id": "a"}"#,
            r#"{"id": "a", "position": null}"#,
            r#"{"id": "a", "position": [1, 2]}"#,
            r#"{"id": "a", "position": [1, "x", 3]}"#,
            r#"{"id": "a", "position": "1,2,3"}"#,
            r#"{"id": "a", "position": {"x": 1}}"#,
        ] {
            assert_eq!(host(bad).position, None, "{bad}");
        }
        assert_eq!(
            host(r#"{"id": "a", "position": [1.5, -2, 3e1]}"#).position,
            Some([1.5, -2.0, 30.0])
        );
    }

    #[test]
    fn summary_reads_kind_network_counts_and_revision() {
        let doc = MapDoc::from_json(DEMO).unwrap();
        assert_eq!(
            doc.summary(),
            "demo 192.168.1.0/24 · 27 hosts · 26 links · rev 0"
        );

        let doc = MapDoc::from_json(
            r#"{"revision": 3, "scan": {"type": "ping_sweep", "stage": "sweeping…"}}"#,
        )
        .unwrap();
        assert_eq!(
            doc.summary(),
            "ping_sweep · sweeping… · 0 hosts · 0 links · rev 3"
        );

        let doc = MapDoc::from_json("{}").unwrap();
        assert_eq!(doc.version, CONTRACT_VERSION);
        assert_eq!(doc.summary(), "map · 0 hosts · 0 links · rev 0");
    }
}
