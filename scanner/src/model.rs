//! What the scanner knows about a machine: [`Host`] and its open [`Port`]s.
//!
//! This is the one contract between the networking code and the viewer: the
//! scan stages fill a `Host` in, the viewer reads it, and neither knows how
//! the other works. Its JSON form is a host entry of the map document, field
//! for field what `netvisualizer/model.py` wrote, so maps saved by the Python
//! version still load (a field added since takes its default when missing).
//! A field added here appears in the JSON and in the viewer's details card
//! with no viewer change.

use std::fmt;
use std::net::IpAddr;
use std::str::FromStr;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

/// A MAC (hardware) address: the 48 bits that name a network interface on
/// its own link. Written `b8:27:eb:8e:cc:eb`, in JSON as that string.
///
/// The first three octets are the prefix the IEEE sold to the maker (the
/// OUI, `b8:27:eb` = Raspberry Pi Foundation); some blocks are split finer,
/// so a vendor lookup tries 36, 28 and then 24 bits. Two bits of the first
/// octet mean something on their own: 0x01 says multicast, and 0x02 says
/// "locally administered", i.e. not assigned by the IEEE: the address was
/// made up on the machine itself. Phones do that for privacy, so such an
/// address has no vendor.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MacAddr([u8; 6]);

impl MacAddr {
    /// All zeros: what an interface without hardware (loopback, a tunnel)
    /// reports, and what an unanswered ARP entry shows.
    pub const NIL: MacAddr = MacAddr([0; 6]);

    pub const fn new(octets: [u8; 6]) -> MacAddr {
        MacAddr(octets)
    }

    pub const fn octets(&self) -> [u8; 6] {
        self.0
    }

    pub fn is_nil(&self) -> bool {
        *self == MacAddr::NIL
    }

    /// The 0x02 bit of the first octet: the address was not assigned by the
    /// IEEE (a randomised or hand-set one). A few registered prefixes (the
    /// IEEE's "CID" blocks) set it too, so look the address up before
    /// calling it randomised.
    pub const fn is_locally_administered(&self) -> bool {
        self.0[0] & 0x02 != 0
    }
}

impl fmt::Display for MacAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, octet) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(":")?;
            }
            write!(f, "{octet:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for MacAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// `MacAddr::from_str` failed: not six two-digit hex groups.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacAddrError(String);

impl fmt::Display for MacAddrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "not a MAC address: {:?}", self.0)
    }
}

impl std::error::Error for MacAddrError {}

impl FromStr for MacAddr {
    type Err = MacAddrError;

    /// Six groups of two hex digits, any case, joined by `:` (the usual) or
    /// `-` (Windows): `B8-27-EB-8E-CC-EB` reads the same as `b8:27:eb:8e:cc:eb`.
    fn from_str(text: &str) -> Result<MacAddr, MacAddrError> {
        let err = || MacAddrError(text.to_string());
        let mut octets = [0u8; 6];
        let mut groups = text.split([':', '-']);
        for octet in &mut octets {
            let group = groups.next().ok_or_else(err)?;
            if group.len() != 2 {
                return Err(err());
            }
            *octet = u8::from_str_radix(group, 16).map_err(|_| err())?;
        }
        if groups.next().is_some() {
            return Err(err());
        }
        Ok(MacAddr(octets))
    }
}

impl Serialize for MacAddr {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for MacAddr {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<MacAddr, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(D::Error::custom)
    }
}

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
    /// How sure the scan is: "open" (a connection was accepted, or an
    /// answer came), or "open|filtered" for a UDP port that stayed silent,
    /// which UDP cannot tell apart from a dropped probe. Left out of the
    /// JSON when unknown, so maps from before it existed read back as is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
}

/// One machine on the network. The sweep creates it from an address that
/// answered; later stages fill in the rest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Host {
    pub ip: IpAddr,
    #[serde(default)]
    pub hostname: Option<String>,
    /// The hardware address, known only for hosts on the scanner's own
    /// link: ARP works no further than the first router.
    #[serde(default)]
    pub mac: Option<MacAddr>,
    /// Who made the interface, from the MAC's registered prefix.
    #[serde(default)]
    pub vendor: Option<String>,
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
            mac: None,
            vendor: None,
            open_ports: Vec::new(),
            is_local: None,
            extra: Map::new(),
        }
    }

    /// What the map calls this host: its hostname; else its vendor with the
    /// address, "Raspberry Pi Foundation (192.168.1.251)", so two of a kind
    /// stay apart; else the address alone.
    pub fn label(&self) -> String {
        let given = |value: &Option<String>| value.clone().filter(|v| !v.is_empty());
        match (given(&self.hostname), given(&self.vendor)) {
            (Some(name), _) => name,
            (None, Some(vendor)) => format!("{vendor} ({})", self.ip),
            (None, None) => self.ip.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use serde_json::json;

    use super::*;

    /// Hosts as the scan stages leave them, one of each kind: every field
    /// filled in, a silent gateway, a randomised MAC, a traced far host, a
    /// router, an IPv6 address.
    const HOSTS: &str = include_str!("../tests/fixtures/hosts.json");

    #[test]
    fn serialises_field_for_field_like_the_python_model() {
        let mut host = Host::new(Ipv4Addr::new(10, 0, 0, 7));
        host.hostname = Some("nas.lan".into());
        host.open_ports.push(Port {
            number: 22,
            proto: Proto::Tcp,
            service: Some("ssh".into()),
            state: None,
        });
        host.mac = Some("00:11:32:ab:cd:ef".parse().unwrap());
        host.vendor = Some("Synology Incorporated".into());
        let value = serde_json::to_value(&host).unwrap();
        assert_eq!(
            value,
            json!({
                "ip": "10.0.0.7",
                "hostname": "nas.lan",
                "mac": "00:11:32:ab:cd:ef",
                "vendor": "Synology Incorporated",
                "open_ports": [{"number": 22, "proto": "tcp", "service": "ssh"}],
                "is_local": null,
                "extra": {},
            })
        );
        let keys: Vec<&String> = value.as_object().unwrap().keys().collect();
        assert_eq!(
            keys,
            [
                "ip",
                "hostname",
                "mac",
                "vendor",
                "open_ports",
                "is_local",
                "extra"
            ]
        );
        assert_eq!(serde_json::from_value::<Host>(value).unwrap(), host);

        // A port's state is written only once the scan has one.
        host.open_ports[0].state = Some("open|filtered".into());
        let value = serde_json::to_value(&host).unwrap();
        assert_eq!(
            value["open_ports"][0],
            json!({"number": 22, "proto": "tcp", "service": "ssh", "state": "open|filtered"})
        );
        assert_eq!(serde_json::from_value::<Host>(value).unwrap(), host);
    }

    #[test]
    fn mac_addresses_read_either_separator_and_write_lower_case_colons() {
        let mac: MacAddr = "B8-27-EB-8E-CC-EB".parse().unwrap();
        assert_eq!(mac, "b8:27:eb:8e:cc:eb".parse().unwrap());
        assert_eq!(mac.to_string(), "b8:27:eb:8e:cc:eb");
        assert_eq!(format!("{mac:?}"), "b8:27:eb:8e:cc:eb");
        assert_eq!(mac.octets(), [0xb8, 0x27, 0xeb, 0x8e, 0xcc, 0xeb]);
        for bad in [
            "",
            "b8:27:eb:8e:cc",
            "b8:27:eb:8e:cc:eb:00",
            "b827.eb8e.cceb",
            "b8:27:eb:8e:cc:zz",
            "b8:27:eb:8e:cc:e",
        ] {
            let err = bad.parse::<MacAddr>().unwrap_err();
            assert_eq!(err.to_string(), format!("not a MAC address: {bad:?}"));
        }
        assert_eq!(
            serde_json::to_string(&mac).unwrap(),
            "\"b8:27:eb:8e:cc:eb\""
        );
        assert_eq!(
            serde_json::from_str::<MacAddr>("\"B8:27:EB:8E:CC:EB\"").unwrap(),
            mac
        );
        assert!(serde_json::from_str::<MacAddr>("\"nope\"").is_err());
        assert!(serde_json::from_str::<MacAddr>("12").is_err());
    }

    #[test]
    fn the_locally_administered_bit_is_the_second_lowest_of_the_first_octet() {
        let randomised: MacAddr = "aa:b7:5d:f6:d4:16".parse().unwrap();
        assert!(randomised.is_locally_administered());
        let registered: MacAddr = "b8:27:eb:8e:cc:eb".parse().unwrap();
        assert!(!registered.is_locally_administered());
        assert!(MacAddr::new([0x02, 0, 0, 0, 0, 1]).is_locally_administered());
        assert!(!MacAddr::new([0x01, 0, 0, 0, 0, 1]).is_locally_administered());
        assert!(MacAddr::NIL.is_nil());
        assert!(!randomised.is_nil());
        assert_eq!(
            "00:00:00:00:00:00".parse::<MacAddr>().unwrap(),
            MacAddr::NIL
        );
    }

    /// The contract's JSON, read back and written again unchanged.
    #[test]
    fn every_host_of_the_fixture_round_trips() {
        let entries: Vec<Value> = serde_json::from_str(HOSTS).unwrap();
        assert_eq!(entries.len(), 7);
        let hosts: Vec<Host> = entries
            .iter()
            .map(|entry| serde_json::from_value(entry.clone()).unwrap())
            .collect();
        for (host, entry) in hosts.iter().zip(&entries) {
            assert_eq!(
                &serde_json::to_value(host).unwrap(),
                entry,
                "{}",
                entry["ip"]
            );
        }
        assert_eq!(hosts[0].label(), "kylr-laptop");
        assert_eq!(hosts[2].label(), "Raspberry Pi Foundation (192.168.1.251)");
        assert_eq!(hosts[1].extra["status"], "down");
        assert_eq!(
            hosts[1].open_ports[1].state.as_deref(),
            Some("open|filtered")
        );
        assert_eq!(hosts[4].open_ports[0].state, None, "an older map's port");
        assert_eq!(hosts[4].extra["path"][1], "*");
        assert!(hosts[6].ip.is_ipv6());
    }

    #[test]
    fn label_is_the_name_else_the_vendor_with_the_address_else_the_address() {
        let mut host = Host::new(Ipv4Addr::new(10, 0, 0, 8));
        assert_eq!(host.label(), "10.0.0.8");
        host.hostname = Some(String::new());
        host.vendor = Some(String::new());
        assert_eq!(host.label(), "10.0.0.8", "empty strings count as missing");
        host.vendor = Some("Raspberry Pi Foundation".into());
        assert_eq!(host.label(), "Raspberry Pi Foundation (10.0.0.8)");
        host.hostname = Some("printer.lan".into());
        assert_eq!(host.label(), "printer.lan", "the name comes first");
    }

    #[test]
    fn missing_fields_take_their_defaults() {
        let host: Host = serde_json::from_str(r#"{"ip": "fe80::1"}"#).unwrap();
        assert_eq!(host, Host::new("fe80::1".parse::<IpAddr>().unwrap()));
    }
}
