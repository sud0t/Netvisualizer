//! Naming hosts: reverse DNS, i.e. asking the system's resolver which name
//! belongs to an address. The resolver tries whatever this machine is set up
//! for (`/etc/hosts`, a PTR query to the DNS server, mDNS, ...).
//!
//! The resolver call (`getnameinfo`) blocks, and for an address nobody has
//! named it blocks for the resolver's whole timeout: seconds each on a LAN
//! without reverse DNS, and ten or more when the DNS server drops the
//! question instead of saying no. So the lookups run on tokio's blocking
//! threads, `concurrency` at a time, instead of one after another, and
//! each is given up on after `timeout`.

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::model::Host;

/// How many lookups run at once by default.
pub const DEFAULT_CONCURRENCY: usize = 32;
/// How long one lookup may take by default.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(3);

/// Fill in `hostname`, in place, for every host the resolver can name
/// within `timeout`. Hosts that already have a name keep it unless
/// `overwrite`.
pub async fn resolve_hostnames(
    hosts: &mut [Host],
    concurrency: usize,
    timeout: Duration,
    overwrite: bool,
) {
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
            // The blocking call cannot be interrupted: past the timeout its
            // thread finishes on its own while the scan moves on without it.
            let lookup = tokio::task::spawn_blocking(move || reverse_lookup(ip));
            let name = tokio::time::timeout(timeout, lookup).await;
            (index, name.ok().and_then(Result::ok).flatten())
        });
    }
    while let Some(result) = lookups.join_next().await {
        if let Ok((index, Some(name))) = result {
            hosts[index].hostname = Some(name);
        }
    }
}

/// The name the system's resolver has for `ip`, if it has one. Blocks.
fn reverse_lookup(ip: IpAddr) -> Option<String> {
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
        resolve_hostnames(&mut hosts, DEFAULT_CONCURRENCY, DEFAULT_TIMEOUT, false).await;
        // /etc/hosts names 127.0.0.1 on every system this runs on ("localhost").
        assert!(
            hosts[0].hostname.as_deref().is_some_and(|n| !n.is_empty()),
            "{hosts:?}"
        );
        assert_eq!(hosts[1].hostname.as_deref(), Some("keep.me"));

        resolve_hostnames(&mut hosts, 1, DEFAULT_TIMEOUT, true).await;
        assert_eq!(
            hosts[1].hostname, hosts[0].hostname,
            "overwrite replaces it"
        );

        // An address nobody has named (reserved for documentation): the
        // resolver answers no, or the timeout does; either way, quickly.
        let mut hosts = vec![Host::new(Ipv4Addr::new(192, 0, 2, 1))];
        let started = std::time::Instant::now();
        resolve_hostnames(&mut hosts, 1, Duration::from_millis(100), false).await;
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn this_machine_has_a_name() {
        assert!(own_hostname().is_some());
    }
}
