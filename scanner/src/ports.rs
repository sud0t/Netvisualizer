//! Open ports: the TCP connect scan, the names of well-known ports, and port
//! lists the way people type them ("22,80,8000-8010").
//!
//! A connect scan completes the TCP handshake (SYN, SYN-ACK, ACK) the way any
//! client would, so it needs no privileges, at the cost of being visible to
//! the host. An open port accepts; a closed one answers with a reset
//! ("connection refused"); a filtered one stays silent until the timeout.
//! Only the first counts as open. (A SYN scan, which never finishes the
//! handshake, would need a raw socket.)

use std::collections::{BTreeSet, HashMap};
use std::net::SocketAddr;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use tokio::net::TcpStream;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::model::{Host, Port, Proto};

/// TCP ports worth trying when nobody gives a list: common LAN services.
pub const COMMON_TCP_PORTS: [u16; 44] = [
    21, 22, 23, 25, 53, 80, 110, 111, 135, 139, 143, 389, 443, 445, 465, 515, 548, 554, 587, 631,
    636, 993, 995, 1433, 1521, 1883, 2049, 3000, 3306, 3389, 5000, 5060, 5432, 5900, 6379, 8000,
    8080, 8443, 8883, 8888, 9000, 9090, 9100, 27017,
];

/// How patient the scan is, and how many connections it keeps open at once.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PortScanOptions {
    /// How long a connection may take before the port counts as closed.
    pub timeout: Duration,
    /// How many connections are attempted at once.
    pub concurrency: usize,
}

impl Default for PortScanOptions {
    fn default() -> Self {
        PortScanOptions {
            timeout: Duration::from_secs(1),
            concurrency: 256,
        }
    }
}

/// Try a TCP connection to every (host, port) pair and add the open ones to
/// `host.open_ports`, with their service names. A port a host already lists
/// is not added twice; each host's list ends up sorted.
/// `progress(done, total)` is called as each pair is settled.
pub async fn tcp_port_scan(
    hosts: &mut [Host],
    ports: &[u16],
    options: &PortScanOptions,
    mut progress: impl FnMut(usize, usize),
) {
    let ports: BTreeSet<u16> = ports.iter().copied().collect();
    let slots = Arc::new(Semaphore::new(options.concurrency.max(1)));
    let mut attempts = JoinSet::new();
    for (index, host) in hosts.iter().enumerate() {
        for &port in &ports {
            let address = SocketAddr::new(host.ip, port);
            let slots = Arc::clone(&slots);
            let timeout = options.timeout;
            attempts.spawn(async move {
                let _slot = slots
                    .acquire_owned()
                    .await
                    .expect("the semaphore is never closed");
                (index, port, tcp_connect(address, timeout).await)
            });
        }
    }
    let total = attempts.len();
    let mut done = 0;
    while let Some(result) = attempts.join_next().await {
        done += 1;
        progress(done, total);
        let Ok((index, number, true)) = result else {
            continue;
        };
        let host = &mut hosts[index];
        if !host
            .open_ports
            .iter()
            .any(|p| p.number == number && p.proto == Proto::Tcp)
        {
            host.open_ports.push(Port {
                number,
                proto: Proto::Tcp,
                service: service_name(number, Proto::Tcp),
            });
        }
    }
    for host in hosts {
        host.open_ports.sort_by_key(|p| (p.proto, p.number));
    }
}

/// True if a TCP connection to `address` is accepted within `timeout`. The
/// connection is closed again straight away (dropping the stream does that).
pub async fn tcp_connect(address: SocketAddr, timeout: Duration) -> bool {
    matches!(
        tokio::time::timeout(timeout, TcpStream::connect(address)).await,
        Ok(Ok(_))
    )
}

/// The well-known name of a port ("ssh" for 22/tcp) from the system's
/// services table, `/etc/services`, if it lists one.
pub fn service_name(port: u16, proto: Proto) -> Option<String> {
    static TABLE: OnceLock<HashMap<(u16, Proto), String>> = OnceLock::new();
    TABLE
        .get_or_init(|| {
            std::fs::read_to_string("/etc/services")
                .map(|text| parse_services(&text))
                .unwrap_or_default()
        })
        .get(&(port, proto))
        .cloned()
}

/// Read a services table. Its lines look like
/// `ssh   22/tcp   # SSH Remote Login Protocol` (name, port/protocol,
/// aliases, comment); when a port is listed twice the first name wins, as
/// with `getservbyport`.
pub fn parse_services(text: &str) -> HashMap<(u16, Proto), String> {
    let mut table = HashMap::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or_default();
        let mut fields = line.split_whitespace();
        let (Some(name), Some(port_and_proto)) = (fields.next(), fields.next()) else {
            continue;
        };
        let Some((port, proto)) = port_and_proto.split_once('/') else {
            continue;
        };
        let proto = match proto {
            "tcp" => Proto::Tcp,
            "udp" => Proto::Udp,
            _ => continue,
        };
        if let Ok(port) = port.parse::<u16>() {
            table
                .entry((port, proto))
                .or_insert_with(|| name.to_string());
        }
    }
    table
}

/// "22,80,8000-8010" -> [22, 80, 8000, ..., 8010], sorted, each port once;
/// "common" -> [`COMMON_TCP_PORTS`].
pub fn parse_port_list(text: &str) -> Result<Vec<u16>, String> {
    if text.trim().eq_ignore_ascii_case("common") {
        return Ok(COMMON_TCP_PORTS.to_vec());
    }
    let mut ports = BTreeSet::new();
    for part in text.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let (low, high) = part.split_once('-').unwrap_or((part, part));
        let (Ok(first), Ok(last)) = (low.trim().parse::<u32>(), high.trim().parse::<u32>()) else {
            return Err(format!("not a port or range: {part:?}"));
        };
        if !(1 <= first && first <= last && last <= 65535) {
            return Err(format!("port out of range: {part}"));
        }
        ports.extend((first..=last).map(|p| p as u16));
    }
    if ports.is_empty() {
        return Err("no ports given".to_string());
    }
    Ok(ports.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, TcpListener};

    use super::*;

    /// A port nobody listens on: a socket bound to it but not listening
    /// refuses connections, and holding it keeps another test from taking
    /// the port meanwhile.
    fn closed_port() -> (socket2::Socket, u16) {
        let socket = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::STREAM,
            Some(socket2::Protocol::TCP),
        )
        .unwrap();
        socket
            .bind(&SocketAddr::from((Ipv4Addr::LOCALHOST, 0)).into())
            .unwrap();
        let port = socket.local_addr().unwrap().as_socket().unwrap().port();
        (socket, port)
    }

    #[tokio::test]
    async fn open_ports_are_found_and_closed_ones_are_not() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let open = listener.local_addr().unwrap().port();
        let (_held, closed) = closed_port();

        let mut hosts = vec![Host::new(Ipv4Addr::LOCALHOST)];
        hosts[0].open_ports.push(Port {
            number: 1,
            proto: Proto::Udp,
            service: None,
        });
        let options = PortScanOptions {
            timeout: Duration::from_millis(500),
            concurrency: 2,
        };
        let mut calls = 0;
        tcp_port_scan(&mut hosts, &[closed, open, open], &options, |_, total| {
            calls += 1;
            assert_eq!(total, 2, "each port is tried once");
        })
        .await;
        assert_eq!(calls, 2);
        let found: Vec<(u16, Proto)> = hosts[0]
            .open_ports
            .iter()
            .map(|p| (p.number, p.proto))
            .collect();
        assert_eq!(
            found,
            [(open, Proto::Tcp), (1, Proto::Udp)],
            "sorted by protocol, then number"
        );

        // A second scan adds nothing twice.
        tcp_port_scan(&mut hosts, &[open], &options, |_, _| {}).await;
        assert_eq!(hosts[0].open_ports.len(), 2);
    }

    #[tokio::test]
    async fn connect_is_true_only_for_a_listening_port() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let timeout = Duration::from_millis(500);
        assert!(tcp_connect(listener.local_addr().unwrap(), timeout).await);
        let (_held, closed) = closed_port();
        let refused = SocketAddr::from((Ipv4Addr::LOCALHOST, closed));
        assert!(!tcp_connect(refused, timeout).await);
    }

    #[test]
    fn services_table() {
        let table = parse_services(
            "# comment\n\
             ssh             22/tcp                          # SSH Remote Login Protocol\n\
             domain          53/udp\n\
             http            80/tcp          www             # WorldWideWeb HTTP\n\
             www-alt         80/tcp\n\
             odd             99/sctp\n\
             broken          x/tcp\n",
        );
        assert_eq!(
            table.get(&(22, Proto::Tcp)).map(String::as_str),
            Some("ssh")
        );
        assert_eq!(
            table.get(&(53, Proto::Udp)).map(String::as_str),
            Some("domain")
        );
        assert_eq!(
            table.get(&(80, Proto::Tcp)).map(String::as_str),
            Some("http"),
            "the first name wins"
        );
        assert_eq!(table.len(), 3);
    }

    #[test]
    fn this_machine_knows_ssh() {
        if std::path::Path::new("/etc/services").exists() {
            assert_eq!(service_name(22, Proto::Tcp).as_deref(), Some("ssh"));
        }
    }

    #[test]
    fn port_lists() {
        assert_eq!(
            parse_port_list("22,80,8000-8003"),
            Ok(vec![22, 80, 8000, 8001, 8002, 8003])
        );
        assert_eq!(parse_port_list(" 80 , 22,22, "), Ok(vec![22, 80]));
        assert_eq!(parse_port_list("Common"), Ok(COMMON_TCP_PORTS.to_vec()));
        assert_eq!(parse_port_list("65535"), Ok(vec![65535]));
        assert_eq!(
            parse_port_list("http"),
            Err("not a port or range: \"http\"".to_string())
        );
        assert_eq!(
            parse_port_list("0"),
            Err("port out of range: 0".to_string())
        );
        assert_eq!(
            parse_port_list("90-80"),
            Err("port out of range: 90-80".to_string())
        );
        assert_eq!(
            parse_port_list("70000"),
            Err("port out of range: 70000".to_string())
        );
        assert_eq!(parse_port_list(" , "), Err("no ports given".to_string()));
    }
}
