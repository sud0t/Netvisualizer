//! The kernel's neighbour table: which MAC address answers for which IP
//! address on the links this machine is on.
//!
//! An IP packet to an address on our own subnet goes out in an Ethernet (or
//! Wi-Fi) frame, and the frame needs the destination's MAC. The kernel finds
//! it with ARP: it broadcasts "who has 192.168.1.251?", the owner answers
//! with its MAC, and the pair is cached in the neighbour table for a while.
//! The ping sweep made the kernel do exactly that for every address that
//! answered, so once the sweep is over the table holds their MACs, and
//! reading it needs no privilege: `/proc/net/arp` is a text file.
//!
//! The table only knows our own links. A host behind the gateway is reached
//! by sending the frame to the gateway's MAC, so the kernel never asks for
//! the host's own; and the machine's own address is not in the table either
//! (nobody ARPs for themselves), it is in sysfs.
//!
//! Hosts that answer ARP but ignore ping are not in `/proc/net/arp` after a
//! ping sweep either: finding those takes ARP requests of our own (Milestone 7).

use std::net::{IpAddr, Ipv4Addr};

use crate::local::LocalNetwork;
use crate::model::{Host, MacAddr};

/// One row of the neighbour table: `mac` answers for `ip` on `interface`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Neighbour {
    pub ip: Ipv4Addr,
    pub mac: MacAddr,
    pub interface: String,
}

/// The kernel's table, or an empty one where it cannot be read (it is only
/// read from `/proc/net/arp`, so outside Linux no MACs are known).
pub fn arp_table() -> Vec<Neighbour> {
    std::fs::read_to_string("/proc/net/arp")
        .map(|text| parse_arp_table(&text))
        .unwrap_or_default()
}

/// Neighbour flags from `<net/if_arp.h>`.
const ATF_COM: u32 = 0x02; // completed: the MAC is known

/// Parse `/proc/net/arp`. Its rows look like
///
/// ```text
/// IP address       HW type     Flags       HW address            Mask     Device
/// 192.168.1.251    0x1         0x2         b8:27:eb:8e:cc:eb     *        wlan0
/// 192.168.1.77     0x1         0x0         00:00:00:00:00:00     *        wlan0
/// ```
///
/// Flags 0x2 means the entry is complete. 0x0 is one still waiting for an
/// answer (or that never got one), with an all-zero MAC; those rows, and
/// rows that do not parse, are skipped.
pub fn parse_arp_table(text: &str) -> Vec<Neighbour> {
    text.lines().skip(1).filter_map(parse_arp_line).collect()
}

fn parse_arp_line(line: &str) -> Option<Neighbour> {
    let fields: Vec<&str> = line.split_whitespace().collect();
    if fields.len() < 6 {
        return None;
    }
    let ip: Ipv4Addr = fields[0].parse().ok()?;
    let flags = u32::from_str_radix(fields[2].strip_prefix("0x")?, 16).ok()?;
    let mac: MacAddr = fields[3].parse().ok()?;
    if flags & ATF_COM == 0 || mac.is_nil() {
        return None;
    }
    Some(Neighbour {
        ip,
        mac,
        interface: fields[5].to_string(),
    })
}

/// This machine's own MAC on `interface`, from
/// `/sys/class/net/<interface>/address`. `None` when there is no such
/// interface, or it has no hardware address (loopback and tunnels report
/// all zeros).
pub fn own_mac(interface: &str) -> Option<MacAddr> {
    let text = std::fs::read_to_string(format!("/sys/class/net/{interface}/address")).ok()?;
    text.trim()
        .parse()
        .ok()
        .filter(|mac: &MacAddr| !mac.is_nil())
}

/// The kernel's number for `interface` (`/sys/class/net/<interface>/ifindex`),
/// which is how a packet socket names the interface it binds to.
pub fn interface_index(interface: &str) -> Option<i32> {
    std::fs::read_to_string(format!("/sys/class/net/{interface}/ifindex"))
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// Everything known about MACs on `local`: the kernel's table, with the
/// entries on `local`'s interface first (an address in use on two links, a
/// VM network that reuses the LAN's addresses say, is taken from the link
/// being scanned, since [`attach_macs`] takes the first match), plus this
/// machine's own address on that interface.
pub fn known_macs(local: &LocalNetwork) -> Vec<Neighbour> {
    let mut table = arp_table();
    table.sort_by_key(|n| n.interface != local.interface); // stable: the rest keep their order
    if let Some(mac) = own_mac(&local.interface) {
        table.push(Neighbour {
            ip: local.ip,
            mac,
            interface: local.interface.clone(),
        });
    }
    table
}

/// Set `mac` on every host still without one whose address is in
/// `neighbours` (the first entry for it, when there are several). A MAC a
/// host already has (from an ARP reply just received) is fresher than the
/// kernel's cache and is kept. Returns how many hosts got one.
pub fn attach_macs(hosts: &mut [Host], neighbours: &[Neighbour]) -> usize {
    let mut attached = 0;
    for host in hosts.iter_mut().filter(|h| h.mac.is_none()) {
        let found = neighbours.iter().find(|n| IpAddr::V4(n.ip) == host.ip);
        if let Some(neighbour) = found {
            host.mac = Some(neighbour.mac);
            attached += 1;
        }
    }
    attached
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The dev machine's table (2026-10-22), plus an incomplete entry, a
    /// stale-looking complete one on another interface, and a broken row.
    const ARP_TABLE: &str = "\
IP address       HW type     Flags       HW address            Mask     Device
192.168.1.251    0x1         0x2         b8:27:eb:8e:cc:eb     *        wlan0
192.168.1.77     0x1         0x0         00:00:00:00:00:00     *        wlan0
192.168.1.254    0x1         0x2         08:c7:f5:ed:02:cb     *        wlan0
192.168.122.9    0x1         0x2         52:54:00:12:34:56     *        virbr0
192.168.1.5      0x1         0x2         not-a-mac             *        wlan0
garbage
";

    fn mac(text: &str) -> MacAddr {
        text.parse().unwrap()
    }

    #[test]
    fn parses_complete_entries_only() {
        let table = parse_arp_table(ARP_TABLE);
        assert_eq!(table.len(), 3, "{table:?}");
        assert_eq!(
            table[0],
            Neighbour {
                ip: Ipv4Addr::new(192, 168, 1, 251),
                mac: mac("b8:27:eb:8e:cc:eb"),
                interface: "wlan0".into(),
            }
        );
        assert_eq!(table[1].ip, Ipv4Addr::new(192, 168, 1, 254));
        assert_eq!(table[2].interface, "virbr0");
        assert!(parse_arp_table("header only\n").is_empty());
    }

    #[test]
    fn macs_are_attached_to_the_hosts_in_the_table() {
        let table = parse_arp_table(ARP_TABLE);
        let mut hosts = vec![
            Host::new(Ipv4Addr::new(192, 168, 1, 251)),
            Host::new(Ipv4Addr::new(192, 168, 1, 77)),
            Host::new(Ipv4Addr::new(10, 1, 2, 3)),
            Host::new(Ipv4Addr::new(192, 168, 1, 254)),
        ];
        hosts[3].mac = Some(mac("00:00:00:00:00:01"));
        assert_eq!(attach_macs(&mut hosts, &table), 1);
        assert_eq!(hosts[0].mac, Some(mac("b8:27:eb:8e:cc:eb")));
        assert_eq!(hosts[1].mac, None, "incomplete entry: no MAC");
        assert_eq!(hosts[2].mac, None, "behind a router: never in the table");
        assert_eq!(
            hosts[3].mac,
            Some(mac("00:00:00:00:00:01")),
            "a MAC already known is kept"
        );
    }

    #[test]
    fn of_two_entries_for_one_address_the_first_wins() {
        let mut table = parse_arp_table(
            "header\n\
             192.168.1.1  0x1  0x2  52:54:00:aa:bb:cc  *  virbr0\n\
             192.168.1.1  0x1  0x2  08:c7:f5:ed:02:cb  *  wlan0\n",
        );
        let mut hosts = vec![Host::new(Ipv4Addr::new(192, 168, 1, 1))];
        attach_macs(&mut hosts, &table);
        assert_eq!(hosts[0].mac, Some(mac("52:54:00:aa:bb:cc")));
        // What known_macs does for the interface being scanned.
        table.sort_by_key(|n| n.interface != "wlan0");
        hosts[0].mac = None;
        attach_macs(&mut hosts, &table);
        assert_eq!(hosts[0].mac, Some(mac("08:c7:f5:ed:02:cb")));
    }

    #[test]
    fn interfaces_have_an_index_and_loopback_is_first() {
        assert_eq!(interface_index("lo"), Some(1));
        assert_eq!(interface_index("no-such-interface0"), None);
    }

    #[test]
    fn this_machine_has_a_mac_on_a_real_interface_but_not_on_loopback() {
        assert_eq!(own_mac("lo"), None, "all zeros");
        assert_eq!(own_mac("no-such-interface0"), None);
        for network in crate::local::local_networks().unwrap_or_default() {
            let own = own_mac(&network.interface);
            // Not every interface has one (a VPN tunnel), but one that does
            // reports a real, registered-looking address.
            if let Some(mac) = own {
                assert!(!mac.is_nil(), "{network:?}");
            }
            let known = known_macs(&network);
            assert_eq!(
                known.iter().any(|n| n.ip == network.ip),
                own.is_some(),
                "{network:?}"
            );
        }
    }
}
