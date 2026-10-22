//! Naming hosts: reverse DNS, i.e. asking the system's resolver which name
//! belongs to an address. The resolver tries whatever this machine is set up
//! for (`/etc/hosts`, a PTR query to the DNS server, mDNS, ...).
//!
//! The resolver call (`getnameinfo`) blocks, and for an address nobody has
//! named it blocks for the resolver's whole timeout: seconds each on a LAN
//! without reverse DNS. So the lookups run on tokio's blocking threads,
//! `concurrency` at a time, instead of one after another.

use std::net::IpAddr;
use std::sync::Arc;

use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::model::Host;

/// How many lookups run at once by default.
pub const DEFAULT_CONCURRENCY: usize = 32;

/// Fill in `hostname`, in place, for every host the resolver can name.
/// Hosts that already have a name keep it unless `overwrite`.
pub async fn resolve_hostnames(hosts: &mut [Host], concurrency: usize, overwrite: bool) {
    let slots = Arc::new(Semaphore::new(concurrency.max(1)));
    let mut lookups = JoinSet::new();
    for (index, host) in hosts.iter().enumerate() {
        if host.hostname.is_some() && !overwrite {
            continue;
        }
        let ip = host.ip;
        let slots = Arc::clone(&slots);
        lookups.spawn(async move {
            let _slot = slots
                .acquire_owned()
                .await
                .expect("the semaphore is never closed");
            let name = tokio::task::spawn_blocking(move || reverse_lookup(ip)).await;
            (index, name.ok().flatten())
        });
    }
    while let Some(result) = lookups.join_next().await {
        if let Ok((index, Some(name))) = result {
            hosts[index].hostname = Some(name);
        }
    }
}

/// The name the system's resolver has for `ip`, if it has one. Blocks.
pub fn reverse_lookup(ip: IpAddr) -> Option<String> {
    // `lookup_addr` asks getnameinfo with NI_NAMEREQD: an error, rather than
    // the address back as text, when there is no name.
    dns_lookup::lookup_addr(&ip)
        .ok()
        .filter(|name| !name.is_empty())
}

/// This machine's own name (`gethostname`). Reverse DNS rarely knows the
/// scanner itself, so the scan falls back to this for it.
pub fn own_hostname() -> Option<String> {
    dns_lookup::get_hostname()
        .ok()
        .filter(|name| !name.is_empty())
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use super::*;

    #[tokio::test]
    async fn loopback_gets_a_name_and_existing_names_are_kept() {
        let mut hosts = vec![
            Host::new(Ipv4Addr::LOCALHOST),
            Host::new(Ipv4Addr::LOCALHOST),
        ];
        hosts[1].hostname = Some("keep.me".into());
        resolve_hostnames(&mut hosts, DEFAULT_CONCURRENCY, false).await;
        // /etc/hosts names 127.0.0.1 on every system this runs on ("localhost").
        assert!(
            hosts[0].hostname.as_deref().is_some_and(|n| !n.is_empty()),
            "{hosts:?}"
        );
        assert_eq!(hosts[1].hostname.as_deref(), Some("keep.me"));

        resolve_hostnames(&mut hosts, 1, true).await;
        assert_eq!(
            hosts[1].hostname, hosts[0].hostname,
            "overwrite replaces it"
        );
    }

    #[test]
    fn this_machine_has_a_name() {
        assert!(own_hostname().is_some());
    }
}
