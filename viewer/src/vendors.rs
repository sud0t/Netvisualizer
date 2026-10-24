//! Vendor names for the hosts' MACs: the one scan stage that runs in Python.
//!
//! The IEEE sells blocks of MAC addresses to makers and publishes who owns
//! which prefix; `assets/mac-vendors-export.csv` is that list. Looking a MAC
//! up in it is `netvisualizer/vendors.py`'s job (pandas), called here
//! through the bridge in python.rs; this side only decides which hosts to
//! ask about and what to write back.
//!
//! The call blocks the scan thread while Python runs, which is fine between
//! stages: nothing else is in flight then.

use pyo3::prelude::*;
use scanner::Host;

use crate::python;

/// Put in `extra.mac_note` when a MAC has no vendor and its locally
/// administered bit is set: it was made up on the device (phones do, for
/// privacy), so no registry knows it.
pub const RANDOMISED_NOTE: &str = "locally administered (randomised) address, so no vendor";

/// Fill in `vendor` for every host with a MAC, and note the randomised
/// addresses. Returns how many hosts got a vendor. `Err` is Python's
/// one-line reason (pandas missing, the CSV unreadable, ...), with the hosts
/// untouched.
pub fn look_up(hosts: &mut [Host]) -> Result<usize, String> {
    let with_mac: Vec<(usize, String)> = hosts
        .iter()
        .enumerate()
        .filter_map(|(index, host)| Some((index, host.mac?.to_string())))
        .collect();
    if with_mac.is_empty() {
        return Ok(0);
    }
    let macs: Vec<String> = with_mac.iter().map(|(_, mac)| mac.clone()).collect();
    let names = vendor_names(&macs)?;
    if names.len() != macs.len() {
        return Err(format!(
            "vendor_names gave {} names for {} addresses",
            names.len(),
            macs.len()
        ));
    }
    let mut with_vendor = 0;
    for ((index, _), name) in with_mac.into_iter().zip(names) {
        let host = &mut hosts[index];
        match name {
            Some(vendor) if !vendor.is_empty() => {
                host.vendor = Some(vendor);
                with_vendor += 1;
            }
            _ if host.mac.is_some_and(|mac| mac.is_locally_administered()) => {
                host.extra.insert("mac_note".into(), RANDOMISED_NOTE.into());
            }
            _ => {}
        }
    }
    Ok(with_vendor)
}

/// `netvisualizer.vendors.vendor_names(macs)`: one vendor name, or `None`,
/// per address, in the same order.
pub fn vendor_names(macs: &[String]) -> Result<Vec<Option<String>>, String> {
    python::with_package(|package| {
        package
            .py()
            .import("netvisualizer.vendors")?
            .call_method1("vendor_names", (macs.to_vec(),))?
            .extract()
    })
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use super::*;

    fn host(ip: u8, mac: Option<&str>) -> Host {
        let mut host = Host::new(Ipv4Addr::new(192, 168, 1, ip));
        host.mac = mac.map(|m| m.parse().unwrap());
        host
    }

    #[test]
    fn vendors_are_looked_up_and_randomised_addresses_noted() {
        let mut hosts = vec![
            host(251, Some("b8:27:eb:8e:cc:eb")), // a Raspberry Pi
            host(238, Some("aa:b7:5d:f6:d4:16")), // a phone, randomised
            host(9, None),                        // behind the gateway: no MAC
            host(66, Some("1c:87:76:f0:00:01")),  // a prefix sold in parts; this part not yet
        ];
        assert_eq!(look_up(&mut hosts), Ok(1));
        assert_eq!(hosts[0].vendor.as_deref(), Some("Raspberry Pi Foundation"));
        assert_eq!(hosts[0].label(), "Raspberry Pi Foundation (192.168.1.251)");
        assert!(hosts[0].extra.is_empty());

        assert_eq!(hosts[1].vendor, None);
        assert_eq!(hosts[1].extra["mac_note"], RANDOMISED_NOTE);

        assert_eq!(hosts[2].vendor, None);
        assert!(hosts[2].extra.is_empty());
        assert_eq!(hosts[3].vendor, None);
        assert!(hosts[3].extra.is_empty(), "registered bit clear: no note");
    }

    #[test]
    fn nothing_to_look_up_asks_python_nothing() {
        let mut hosts = vec![host(9, None)];
        assert_eq!(look_up(&mut hosts), Ok(0));
        assert_eq!(vendor_names(&[]), Ok(Vec::new()));
    }
}
