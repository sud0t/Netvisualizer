//! What the scanner knows about a machine: [`Host`] and its open [`Port`]s.
//!
//! This is the one contract between the networking code and the viewer: the
//! scan stages fill a `Host` in, the viewer reads it, and neither knows how
//! the other works. Its JSON form is a host entry of the map document, field
//! for field what `netvisualizer/model.py` wrote, so maps saved by the Python
//! version still load. A field added here appears in the JSON and in the
//! viewer's details card with no viewer change.

use std::net::IpAddr;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The transport protocol a port belongs to. Written as "tcp" / "udp".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Proto {
    Tcp,
    Udp,
}

/// An open port on a host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Port {
    pub number: u16,
    pub proto: Proto,
    /// The well-known name for the port, e.g. "ssh" for 22/tcp, if it has one.
    #[serde(default)]
    pub service: Option<String>,
}

/// One machine on the network. The sweep creates it from an address that
/// answered; later stages fill in the rest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Host {
    pub ip: IpAddr,
    #[serde(default)]
    pub hostname: Option<String>,
    #[serde(default)]
    pub open_ports: Vec<Port>,
    /// Inside the scanner's own subnet, i.e. reachable without a router.
    /// `None` until the scan has checked.
    #[serde(default)]
    pub is_local: Option<bool>,
    /// Anything new goes here first; the viewer shows it as is. A host the
    /// map keeps although it did not answer has `"status": "down"` here.
    #[serde(default)]
    pub extra: Map<String, Value>,
}

impl Host {
    /// A host known only by its address.
    pub fn new(ip: impl Into<IpAddr>) -> Host {
        Host {
            ip: ip.into(),
            hostname: None,
            open_ports: Vec::new(),
            is_local: None,
            extra: Map::new(),
        }
    }

    /// What the map calls this host: its hostname, or else its address.
    pub fn label(&self) -> String {
        match &self.hostname {
            Some(name) if !name.is_empty() => name.clone(),
            _ => self.ip.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use serde_json::json;

    use super::*;

    /// The viewer's saved demo map: a host entry there is a `Host` plus the
    /// four keys the map adds on top (id, label, role, position).
    const DEMO: &str = include_str!("../../viewer/tests/fixtures/demo.json");
    const MAP_KEYS: [&str; 4] = ["id", "label", "role", "position"];

    #[test]
    fn serialises_field_for_field_like_the_python_model() {
        let mut host = Host::new(Ipv4Addr::new(10, 0, 0, 7));
        host.hostname = Some("nas.lan".into());
        host.open_ports.push(Port {
            number: 22,
            proto: Proto::Tcp,
            service: Some("ssh".into()),
        });
        let value = serde_json::to_value(&host).unwrap();
        assert_eq!(
            value,
            json!({
                "ip": "10.0.0.7",
                "hostname": "nas.lan",
                "open_ports": [{"number": 22, "proto": "tcp", "service": "ssh"}],
                "is_local": null,
                "extra": {},
            })
        );
        let keys: Vec<&String> = value.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["ip", "hostname", "open_ports", "is_local", "extra"]);
    }

    #[test]
    fn every_host_of_the_demo_map_round_trips() {
        let doc: Value = serde_json::from_str(DEMO).unwrap();
        let entries = doc["hosts"].as_array().unwrap();
        assert_eq!(entries.len(), 27);
        for entry in entries {
            let mut fields = entry.as_object().unwrap().clone();
            for key in MAP_KEYS {
                fields.shift_remove(key);
            }
            let host: Host = serde_json::from_value(Value::Object(fields.clone())).unwrap();
            assert_eq!(serde_json::to_value(&host).unwrap(), Value::Object(fields));
            assert_eq!(host.label(), entry["label"], "{}", entry["id"]);
        }
    }

    #[test]
    fn label_falls_back_to_the_address() {
        let mut host = Host::new(Ipv4Addr::new(10, 0, 0, 8));
        assert_eq!(host.label(), "10.0.0.8");
        host.hostname = Some(String::new());
        assert_eq!(host.label(), "10.0.0.8");
        host.hostname = Some("printer.lan".into());
        assert_eq!(host.label(), "printer.lan");
    }

    #[test]
    fn missing_fields_take_their_defaults() {
        let host: Host = serde_json::from_str(r#"{"ip": "fe80::1"}"#).unwrap();
        assert_eq!(host, Host::new("fe80::1".parse::<IpAddr>().unwrap()));
    }
}
