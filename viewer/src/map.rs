//! The map document: what the viewer draws. It comes from a scan
//! ([`MapDoc::from_hosts`]) or from a file saved earlier (`--json`), and it is
//! the same JSON either way: the contract the Python version wrote, so its
//! saved maps still load.
//!
//! Only the keys the viewer itself acts on are typed (`id`, `label`, `role`,
//! `position` on a host; `source`/`target` on a link). Everything else a host
//! carries (`ip`, `hostname`, `open_ports`, `extra`, and any field added to
//! the scanner's `Host` later) is kept verbatim in [`Host::fields`], in
//! document order, so the generic details card in panel.rs shows it with no
//! change here when the scanner learns something new.

use std::collections::HashSet;
use std::error::Error;
use std::fs;
use std::net::IpAddr;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

use crate::layout;

/// The newest contract version this viewer understands.
pub const CONTRACT_VERSION: u64 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MapDoc {
    #[serde(default = "default_version")]
    pub version: u64,
    /// Goes up every time a scan hands the viewer a new map.
    #[serde(default)]
    pub revision: u64,
    /// When the map was made, e.g. "2026-10-21T04:33:06+00:00".
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Scan {
    /// `scan.type` ("ping_sweep", "demo", ...); `type` is a keyword in Rust.
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub network: Option<String>,
    #[serde(default)]
    pub scanner_ip: Option<String>,
    /// What the scan is doing right now ("resolving names…"), shown in the HUD.
    #[serde(default)]
    pub stage: Option<String>,
    /// Keys a newer version may add, kept so a saved map round-trips.
    #[serde(flatten)]
    pub rest: Map<String, Value>,
}

/// One host of the map. Fields are in the order they are written: the map's
/// own keys, then the scanner's, then the position.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Host {
    pub id: String,
    #[serde(default)]
    pub label: Option<String>,
    /// The raw role string; use [`Host::role`] for the validated value.
    #[serde(default)]
    pub role: Option<String>,
    /// Every other key of the host object, in document order.
    #[serde(flatten)]
    pub fields: Map<String, Value>,
    /// `None` unless three finite numbers were given; graph.rs then places
    /// the host on the fallback ring.
    #[serde(default, deserialize_with = "lenient_position")]
    pub position: Option<[f32; 3]>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Link {
    pub source: String,
    pub target: String,
    #[serde(default)]
    pub kind: Option<String>,
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
    /// The map of what a scan has found so far:
    ///
    /// - every host once (of two with the same address, the first wins);
    /// - the scanner in the middle with role "scanner", added at the front if
    ///   the scan did not find it itself, and the gateway with role "gateway";
    /// - links in a star from the scanner to everyone else, the only topology
    ///   a ping sweep implies;
    /// - positions from [`layout::star_ring`].
    ///
    /// `scan.scanner_ip` is filled in from `scanner_ip`.
    pub fn from_hosts(
        hosts: &[scanner::Host],
        scanner_ip: Option<IpAddr>,
        gateway_ip: Option<IpAddr>,
        mut scan: Scan,
    ) -> MapDoc {
        let mut seen = HashSet::new();
        let unique: Vec<&scanner::Host> = hosts.iter().filter(|h| seen.insert(h.ip)).collect();
        let missing_scanner = scanner_ip
            .filter(|ip| !seen.contains(ip))
            .map(scanner::Host::new);

        let mut entries: Vec<Host> = missing_scanner
            .iter()
            .chain(unique)
            .map(|host| {
                let role = if Some(host.ip) == scanner_ip {
                    Role::Scanner
                } else if Some(host.ip) == gateway_ip {
                    Role::Gateway
                } else {
                    Role::Host
                };
                Host::from_scanner(host, role)
            })
            .collect();
        let positions = layout::star_ring(&entries);
        for (entry, position) in entries.iter_mut().zip(positions) {
            entry.position = Some(position);
        }

        let links = match scanner_ip.map(|ip| ip.to_string()) {
            Some(centre) => entries
                .iter()
                .filter(|h| h.id != centre)
                .map(|h| Link {
                    source: centre.clone(),
                    target: h.id.clone(),
                    kind: Some("subnet".to_string()),
                    rest: Map::new(),
                })
                .collect(),
            None => Vec::new(),
        };
        scan.scanner_ip = scanner_ip.map(|ip| ip.to_string());
        MapDoc {
            version: CONTRACT_VERSION,
            revision: 0,
            generated_at: Some(utc_timestamp(SystemTime::now())),
            scan,
            hosts: entries,
            links,
        }
    }

    pub fn from_json(text: &str) -> Result<MapDoc, serde_json::Error> {
        serde_json::from_str(text)
    }

    pub fn load(path: &Path) -> Result<MapDoc, Box<dyn Error>> {
        Ok(MapDoc::from_json(&fs::read_to_string(path)?)?)
    }

    /// Write the map as indented JSON; [`MapDoc::load`] reads it back.
    pub fn save(&self, path: &Path) -> Result<(), Box<dyn Error>> {
        let mut text = serde_json::to_string_pretty(self)?;
        text.push('\n');
        fs::write(path, text)?;
        Ok(())
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

impl Host {
    /// A scanner `Host` as a map entry: the map's keys on top of every field
    /// the scanner filled in, serialised generically. No position yet.
    fn from_scanner(host: &scanner::Host, role: Role) -> Host {
        let fields = match serde_json::to_value(host) {
            Ok(Value::Object(fields)) => fields,
            _ => Map::new(), // cannot happen: Host serialises to an object
        };
        Host {
            id: host.ip.to_string(),
            label: Some(host.label()),
            role: Some(role.name().to_string()),
            fields,
            position: None,
        }
    }
}

/// `time` as `2026-10-21T04:33:06+00:00`, in UTC, the way Python's
/// `datetime.isoformat(timespec="seconds")` wrote `generated_at`.
pub fn utc_timestamp(time: SystemTime) -> String {
    let seconds = time.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    let (year, month, day) = civil_from_days(days as i64);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}+00:00",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// Days since 1970-01-01 to (year, month, day) in the Gregorian calendar
/// (Howard Hinnant's `civil_from_days`). It counts from 0000-03-01 in
/// 400-year eras, so leap days fall at the end of each year it counts.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468; // days from 0000-03-01 to 1970-01-01
    let era = z.div_euclid(146_097); // 146 097 days in 400 years
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_from_march = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_from_march + 2) / 5 + 1) as u32;
    let month = if month_from_march < 10 {
        month_from_march + 3
    } else {
        month_from_march - 9
    } as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
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
    use std::net::Ipv4Addr;
    use std::time::Duration;

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
        assert_eq!(scanner.display_label(), "netvisualizer-host.lan");
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

    // ----- building a map from a scan (what graphics/adapters.py did) -----

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    fn scanned(addresses: &[&str]) -> Vec<scanner::Host> {
        addresses
            .iter()
            .map(|a| scanner::Host::new(ip(a)))
            .collect()
    }

    fn ids(doc: &MapDoc) -> Vec<&str> {
        doc.hosts.iter().map(|h| h.id.as_str()).collect()
    }

    fn role_of(doc: &MapDoc, id: &str) -> Role {
        doc.hosts.iter().find(|h| h.id == id).unwrap().role()
    }

    #[test]
    fn the_scanner_is_added_when_the_scan_missed_it_and_sits_in_the_middle() {
        let doc = MapDoc::from_hosts(
            &scanned(&["10.0.0.2", "10.0.0.3"]),
            Some(ip("10.0.0.1")),
            None,
            Scan::default(),
        );
        assert_eq!(ids(&doc), ["10.0.0.1", "10.0.0.2", "10.0.0.3"]);
        assert_eq!(role_of(&doc, "10.0.0.1"), Role::Scanner);
        assert_eq!(doc.scan.scanner_ip.as_deref(), Some("10.0.0.1"));
        assert_eq!(doc.hosts[0].position, Some([0.0, 0.0, 0.0]));
        assert_eq!(doc.hosts[0].fields["ip"], "10.0.0.1");
    }

    #[test]
    fn a_scanner_the_scan_found_is_not_added_twice() {
        let doc = MapDoc::from_hosts(
            &scanned(&["10.0.0.1", "10.0.0.2"]),
            Some(ip("10.0.0.1")),
            None,
            Scan::default(),
        );
        assert_eq!(ids(&doc), ["10.0.0.1", "10.0.0.2"]);
        assert_eq!(role_of(&doc, "10.0.0.1"), Role::Scanner);
    }

    #[test]
    fn links_are_a_star_from_the_scanner_to_everyone_else() {
        let doc = MapDoc::from_hosts(
            &scanned(&["10.0.0.2", "10.0.0.3", "10.0.0.4"]),
            Some(ip("10.0.0.1")),
            None,
            Scan::default(),
        );
        let links: Vec<(&str, &str)> = doc
            .links
            .iter()
            .map(|l| (l.source.as_str(), l.target.as_str()))
            .collect();
        assert_eq!(
            links,
            [
                ("10.0.0.1", "10.0.0.2"),
                ("10.0.0.1", "10.0.0.3"),
                ("10.0.0.1", "10.0.0.4")
            ]
        );
        assert!(
            doc.links
                .iter()
                .all(|l| l.kind.as_deref() == Some("subnet"))
        );
    }

    #[test]
    fn without_a_scanner_there_are_no_links_and_everyone_is_a_host() {
        let doc = MapDoc::from_hosts(
            &scanned(&["10.0.0.2", "10.0.0.3"]),
            None,
            None,
            Scan::default(),
        );
        assert!(doc.links.is_empty());
        assert!(doc.hosts.iter().all(|h| h.role() == Role::Host));
        assert!(doc.hosts.iter().all(|h| h.position.is_some()));
    }

    #[test]
    fn the_gateway_gets_its_role_unless_it_is_the_scanner() {
        let hosts = scanned(&["10.0.0.1", "10.0.0.2"]);
        let doc = MapDoc::from_hosts(
            &hosts,
            Some(ip("10.0.0.5")),
            Some(ip("10.0.0.1")),
            Scan::default(),
        );
        assert_eq!(role_of(&doc, "10.0.0.1"), Role::Gateway);
        assert_eq!(role_of(&doc, "10.0.0.2"), Role::Host);

        let doc = MapDoc::from_hosts(
            &hosts,
            Some(ip("10.0.0.1")),
            Some(ip("10.0.0.1")),
            Scan::default(),
        );
        assert_eq!(role_of(&doc, "10.0.0.1"), Role::Scanner);
    }

    #[test]
    fn of_two_hosts_with_one_address_the_first_wins() {
        let mut hosts = scanned(&["10.0.0.2", "10.0.0.2"]);
        hosts[0].hostname = Some("first".into());
        hosts[1].hostname = Some("second".into());
        let doc = MapDoc::from_hosts(&hosts, None, None, Scan::default());
        assert_eq!(ids(&doc), ["10.0.0.2"]);
        assert_eq!(doc.hosts[0].display_label(), "first");
    }

    #[test]
    fn host_entries_carry_every_scanner_field_after_the_map_keys() {
        let mut hosts = scanned(&["10.0.0.7"]);
        hosts[0].hostname = Some("nas.lan".into());
        hosts[0].open_ports.push(scanner::Port {
            number: 22,
            proto: scanner::Proto::Tcp,
            service: Some("ssh".into()),
        });
        let scan = Scan {
            kind: Some("ping_sweep".into()),
            network: Some("10.0.0.0/24".into()),
            stage: Some("done".into()),
            ..Scan::default()
        };
        let doc = MapDoc::from_hosts(&hosts, None, None, scan);
        let json = serde_json::to_value(&doc.hosts[0]).unwrap();
        let keys: Vec<&String> = json.as_object().unwrap().keys().collect();
        assert_eq!(
            keys,
            [
                "id",
                "label",
                "role",
                "ip",
                "hostname",
                "open_ports",
                "is_local",
                "extra",
                "position"
            ]
        );
        assert_eq!(json["label"], "nas.lan");
        assert_eq!(json["open_ports"][0]["service"], "ssh");
        assert_eq!(
            doc.summary(),
            "ping_sweep 10.0.0.0/24 · done · 1 hosts · 0 links · rev 0"
        );
    }

    #[test]
    fn a_saved_map_is_the_document_it_was_loaded_from() {
        let doc = MapDoc::from_json(DEMO).unwrap();
        let path =
            std::env::temp_dir().join(format!("netvisualizer-map-{}.json", std::process::id()));
        doc.save(&path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        let _ = fs::remove_file(&path);
        // Compared as text-parsed JSON: positions are f32 here, and an f32
        // like 13.856 is written back as "13.856", but widened to f64 in
        // memory it would be 13.855999…
        assert_eq!(
            serde_json::from_str::<Value>(&text).unwrap(),
            serde_json::from_str::<Value>(DEMO).unwrap(),
            "the same document the Python version wrote"
        );
        assert_eq!(MapDoc::from_json(&text).unwrap().summary(), doc.summary());
    }

    #[test]
    fn timestamps_are_utc_in_isoformat() {
        let at = |seconds: u64| utc_timestamp(UNIX_EPOCH + Duration::from_secs(seconds));
        assert_eq!(at(0), "1970-01-01T00:00:00+00:00");
        assert_eq!(at(951_782_400), "2000-02-29T00:00:00+00:00");
        assert_eq!(at(1_761_021_186), "2025-10-21T04:33:06+00:00");
        assert_eq!(at(4_102_444_799), "2099-12-31T23:59:59+00:00");
        let doc = MapDoc::from_hosts(
            &[],
            Some(IpAddr::V4(Ipv4Addr::LOCALHOST)),
            None,
            Scan::default(),
        );
        assert!(doc.generated_at.is_some_and(|t| t.ends_with("+00:00")));
    }
}
