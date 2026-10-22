//! This machine's IPv4 networks: the interfaces it has, its address and
//! subnet on each, and the gateway (router) each one sends the rest of the
//! world to. Everything here is read without privileges:
//!
//! - the interface list (`getifaddrs`, through the if-addrs crate) gives every
//!   address with its netmask, so the subnet it sits in;
//! - the kernel's routing table (`/proc/net/route` on Linux) gives the default
//!   route of each interface, which names its gateway.
//!
//! When nobody names an interface, "our" network is the one holding the
//! address our internet traffic leaves from (see [`outbound_ip`]).

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::net::{IpAddr, Ipv4Addr, UdpSocket};

use ipnet::Ipv4Net;

use crate::model::Host;

/// One of this machine's IPv4 networks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalNetwork {
    /// The interface name, e.g. "wlan0".
    pub interface: String,
    /// This machine's address on that interface.
    pub ip: Ipv4Addr,
    /// The subnet that address is in, e.g. 192.168.1.0/24.
    pub network: Ipv4Net,
    /// Where this interface sends traffic for everything outside the
    /// subnet, if it has a default route.
    pub gateway: Option<Ipv4Addr>,
}

/// Every subnet this machine is directly on, loopback and link-local
/// (169.254.x.x) left out, in the order the system lists its interfaces.
pub fn local_networks() -> io::Result<Vec<LocalNetwork>> {
    let gateways = default_gateways(&routes());
    let mut found: Vec<LocalNetwork> = Vec::new();
    for interface in if_addrs::get_if_addrs()? {
        let if_addrs::IfAddr::V4(addr) = &interface.addr else {
            continue; // IPv6: not swept (yet)
        };
        if addr.ip.is_loopback() || addr.ip.is_link_local() || addr.prefixlen >= 32 {
            continue; // a /32 (a VPN's point-to-point address) has no neighbours to find
        }
        let Ok(network) = Ipv4Net::new(addr.ip, addr.prefixlen) else {
            continue;
        };
        let network = network.trunc(); // 192.168.1.143/24 -> 192.168.1.0/24
        if found
            .iter()
            .any(|n| n.interface == interface.name && n.network == network)
        {
            continue; // a second address in the same subnet adds nothing to sweep
        }
        let gateway = gateways.get(&interface.name).copied();
        found.push(LocalNetwork {
            interface: interface.name,
            ip: addr.ip,
            network,
            gateway,
        });
    }
    Ok(found)
}

/// Why [`default_network`] found nothing to scan.
#[derive(Debug)]
pub enum LocalNetworkError {
    /// `-i NAME` names an interface without an IPv4 network.
    NoSuchInterface { name: String, known: Vec<String> },
    /// Nothing but loopback: not connected to any network.
    NotConnected,
    /// The interface list could not be read.
    Io(io::Error),
}

impl fmt::Display for LocalNetworkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LocalNetworkError::NoSuchInterface { name, known } if known.is_empty() => {
                write!(
                    f,
                    "no IPv4 network on interface {name:?} (no networks found at all)"
                )
            }
            LocalNetworkError::NoSuchInterface { name, known } => write!(
                f,
                "no IPv4 network on interface {name:?} (known: {})",
                known.join(", ")
            ),
            LocalNetworkError::NotConnected => {
                write!(f, "no network connection found (only loopback)")
            }
            LocalNetworkError::Io(err) => write!(f, "could not list the network interfaces: {err}"),
        }
    }
}

impl std::error::Error for LocalNetworkError {}

/// The network to scan by default: the one on `interface` when one is named,
/// else the one our internet traffic uses, else the first one there is.
pub fn default_network(interface: Option<&str>) -> Result<LocalNetwork, LocalNetworkError> {
    let networks = local_networks().map_err(LocalNetworkError::Io)?;
    if let Some(name) = interface {
        let found = networks.iter().find(|n| n.interface == name).cloned();
        return found.ok_or_else(|| {
            let mut known: Vec<String> = networks.into_iter().map(|n| n.interface).collect();
            known.sort();
            known.dedup();
            LocalNetworkError::NoSuchInterface {
                name: name.to_string(),
                known,
            }
        });
    }
    let outbound = outbound_ip();
    networks
        .iter()
        .find(|n| Some(n.ip) == outbound)
        .or(networks.first())
        .cloned()
        .ok_or(LocalNetworkError::NotConnected)
}

/// The address of the interface our internet traffic leaves from, or `None`
/// when there is no route out.
///
/// "Connecting" a UDP socket sends nothing: it only makes the kernel choose
/// the route, and so the source address, it would use for that destination.
/// `local_addr()` then tells us which address that is. The destination does
/// not even have to exist.
pub fn outbound_ip() -> Option<Ipv4Addr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect((Ipv4Addr::new(10, 254, 254, 254), 1)).ok()?;
    match socket.local_addr().ok()?.ip() {
        IpAddr::V4(ip) if !ip.is_unspecified() => Some(ip),
        _ => None,
    }
}

/// Set `is_local` on every host: true when its address is inside `network`,
/// i.e. reachable without going through a router.
pub fn mark_local(hosts: &mut [Host], network: &Ipv4Net) {
    for host in hosts {
        host.is_local = Some(match host.ip {
            IpAddr::V4(ip) => network.contains(&ip),
            IpAddr::V6(_) => false,
        });
    }
}

// ---------------------------------------------------------------------------
// The routing table.
// ---------------------------------------------------------------------------

/// One row of the kernel's IPv4 routing table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    pub interface: String,
    /// The addresses this row is for; 0.0.0.0/0 is the default route.
    pub destination: Ipv4Net,
    /// The router to hand the packets to, or `None` when the destination is
    /// directly connected (the packets go straight out of the interface).
    pub gateway: Option<Ipv4Addr>,
    /// Lower wins when two rows match the same address.
    pub metric: u32,
}

/// The routing table, or an empty one where it cannot be read (it is only
/// read from `/proc/net/route`, so outside Linux there are no gateways).
pub fn routes() -> Vec<Route> {
    std::fs::read_to_string("/proc/net/route")
        .map(|text| parse_routes(&text))
        .unwrap_or_default()
}

/// Route flags from `<linux/route.h>`.
const RTF_UP: u32 = 0x0001; // the route is usable
const RTF_GATEWAY: u32 = 0x0002; // the destination is reached through a gateway

/// Parse `/proc/net/route`. Its rows look like
///
/// ```text
/// Iface  Destination  Gateway   Flags  RefCnt  Use  Metric  Mask      MTU  Window  IRTT
/// wlan0  00000000     FE01A8C0  0003   0       0    600     00000000  0    0       0
/// wlan0  0001A8C0     00000000  0001   0       0    600     00FFFFFF  0    0       0
/// ```
///
/// Addresses and masks are the four address bytes printed as one hex number
/// in the machine's own byte order, so on a little-endian PC `FE01A8C0` is
/// the bytes C0 A8 01 FE, 192.168.1.254. Rows that are not up, or do not
/// parse, are skipped.
pub fn parse_routes(text: &str) -> Vec<Route> {
    text.lines().skip(1).filter_map(parse_route_line).collect()
}

fn parse_route_line(line: &str) -> Option<Route> {
    let fields: Vec<&str> = line.split_whitespace().collect();
    if fields.len() < 8 {
        return None;
    }
    let destination = hex_address(fields[1])?;
    let gateway = hex_address(fields[2])?;
    let flags = u32::from_str_radix(fields[3], 16).ok()?;
    let metric = fields[6].parse().ok()?;
    let mask = hex_address(fields[7])?;
    if flags & RTF_UP == 0 {
        return None;
    }
    let prefix_len = mask.to_bits().count_ones() as u8;
    Some(Route {
        interface: fields[0].to_string(),
        destination: Ipv4Net::new(destination, prefix_len).ok()?.trunc(),
        gateway: (flags & RTF_GATEWAY != 0).then_some(gateway),
        metric,
    })
}

/// `FE01A8C0` -> 192.168.1.254 on a little-endian machine (see [`parse_routes`]).
fn hex_address(field: &str) -> Option<Ipv4Addr> {
    let number = u32::from_str_radix(field, 16).ok()?;
    Some(Ipv4Addr::from(number.to_ne_bytes()))
}

/// Each interface's gateway: the default route (0.0.0.0/0) with the lowest
/// metric on that interface.
pub fn default_gateways(routes: &[Route]) -> HashMap<String, Ipv4Addr> {
    let mut best: HashMap<String, (u32, Ipv4Addr)> = HashMap::new();
    for route in routes {
        let Some(gateway) = route.gateway else {
            continue;
        };
        if route.destination.prefix_len() != 0 {
            continue; // a route to one particular network, not the default
        }
        let entry = best
            .entry(route.interface.clone())
            .or_insert((route.metric, gateway));
        if route.metric < entry.0 {
            *entry = (route.metric, gateway);
        }
    }
    best.into_iter()
        .map(|(interface, (_, gateway))| (interface, gateway))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The dev machine's table (2026-10-21), plus a second default route on
    /// wlan0 with a worse metric and a route that is down.
    const ROUTE_TABLE: &str = "\
Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT
wlan0\t00000000\tFE01A8C0\t0003\t0\t0\t600\t00000000\t0\t0\t0
lo\t0000007F\t0100007F\t0003\t0\t0\t0\t000000FF\t0\t0\t0
wlan0\t0001A8C0\t00000000\t0001\t0\t0\t600\t00FFFFFF\t0\t0\t0
virbr0\t007AA8C0\t00000000\t0001\t0\t0\t0\t00FFFFFF\t0\t0\t0
wlan0\t00000000\t0101A8C0\t0003\t0\t0\t700\t00000000\t0\t0\t0
eth9\t00000000\t0100000A\t0002\t0\t0\t0\t00000000\t0\t0\t0
";

    fn net(text: &str) -> Ipv4Net {
        text.parse().unwrap()
    }

    #[test]
    #[cfg(target_endian = "little")] // the table is written the way a little-endian kernel prints it
    fn parses_the_routing_table() {
        let routes = parse_routes(ROUTE_TABLE);
        assert_eq!(routes.len(), 5, "the route that is not up is skipped");
        assert_eq!(
            routes[0],
            Route {
                interface: "wlan0".into(),
                destination: net("0.0.0.0/0"),
                gateway: Some(Ipv4Addr::new(192, 168, 1, 254)),
                metric: 600,
            }
        );
        assert_eq!(routes[1].destination, net("127.0.0.0/8"));
        assert_eq!(
            routes[2],
            Route {
                interface: "wlan0".into(),
                destination: net("192.168.1.0/24"),
                gateway: None,
                metric: 600,
            }
        );
        assert_eq!(routes[3].destination, net("192.168.122.0/24"));
    }

    #[test]
    #[cfg(target_endian = "little")]
    fn the_default_route_with_the_lowest_metric_names_the_gateway() {
        let gateways = default_gateways(&parse_routes(ROUTE_TABLE));
        assert_eq!(
            gateways.len(),
            1,
            "lo's route is not a default route; eth9's is down"
        );
        assert_eq!(gateways["wlan0"], Ipv4Addr::new(192, 168, 1, 254));
    }

    #[test]
    fn garbage_rows_are_skipped() {
        let routes = parse_routes("header\nwlan0 zz 00000000 0001 0 0 0 00FFFFFF\nshort row\n");
        assert!(routes.is_empty());
    }

    #[test]
    fn mark_local_checks_the_subnet() {
        let mut hosts = vec![
            Host::new(Ipv4Addr::new(192, 168, 1, 7)),
            Host::new(Ipv4Addr::new(10, 0, 0, 1)),
        ];
        mark_local(&mut hosts, &net("192.168.1.0/24"));
        assert_eq!(hosts[0].is_local, Some(true));
        assert_eq!(hosts[1].is_local, Some(false));
    }

    /// Whatever machine this runs on: every network listed holds its own
    /// address, is a real subnet, and is not loopback.
    #[test]
    fn this_machines_networks_are_sane() {
        let networks = local_networks().unwrap();
        for n in &networks {
            assert!(n.network.contains(&n.ip), "{n:?}");
            assert_eq!(n.network, n.network.trunc(), "{n:?}");
            assert!(!n.ip.is_loopback(), "{n:?}");
        }
        if let Some(first) = networks.first() {
            let named = default_network(Some(&first.interface)).unwrap();
            assert_eq!(named.interface, first.interface);
        }
    }

    #[test]
    fn an_unknown_interface_is_an_error_naming_the_known_ones() {
        let err = default_network(Some("no-such-interface0")).unwrap_err();
        assert!(
            matches!(err, LocalNetworkError::NoSuchInterface { .. }),
            "{err:?}"
        );
        assert!(
            err.to_string()
                .starts_with("no IPv4 network on interface \"no-such-interface0\"")
        );
    }
}
