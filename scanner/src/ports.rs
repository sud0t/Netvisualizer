//! Open ports: the TCP connect scan, the UDP probe, the names of well-known
//! ports, and port lists the way people type them ("22,80,8000-8010").
//!
//! A connect scan completes the TCP handshake (SYN, SYN-ACK, ACK) the way any
//! client would, so it needs no privileges, at the cost of being visible to
//! the host. An open port accepts; a closed one answers with a reset
//! ("connection refused"); a filtered one stays silent until the timeout.
//! Only the first counts as open. (A SYN scan, which never finishes the
//! handshake, would need a raw socket.)
//!
//! UDP has no handshake, so a probe is a datagram and the answer, if any,
//! is up to the service. What a closed port does is the kernel's doing: it
//! sends back an ICMP "port unreachable", which the kernel on our side turns
//! into "connection refused" on the socket that sent the datagram, as long
//! as that socket is connected to the address. Silence means a service
//! that does not answer strangers, or a firewall dropping the probe: UDP
//! cannot tell those apart, so such a port is "open|filtered".

use std::collections::{BTreeSet, HashMap};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::model::{Host, Port, Proto};

/// TCP ports worth trying when nobody gives a list: common LAN services.
pub const COMMON_TCP_PORTS: [u16; 44] = [
    21, 22, 23, 25, 53, 80, 110, 111, 135, 139, 143, 389, 443, 445, 465, 515, 548, 554, 587, 631,
    636, 993, 995, 1433, 1521, 1883, 2049, 3000, 3306, 3389, 5000, 5060, 5432, 5900, 6379, 8000,
    8080, 8443, 8883, 8888, 9000, 9090, 9100, 27017,
];

/// UDP ports worth trying when nobody gives a list: the services a LAN
/// runs over UDP (DNS, DHCP, TFTP, RPC, NTP, NetBIOS, SNMP, IPsec, syslog,
/// SSDP, SIP, mDNS).
pub const COMMON_UDP_PORTS: [u16; 12] =
    [53, 67, 69, 111, 123, 137, 161, 500, 514, 1900, 5060, 5353];

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
                state: Some("open".to_string()),
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

/// What a UDP probe learned about a port (see the module comment).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UdpState {
    /// Something answered: a service is listening.
    Open,
    /// Nothing came back: a service that ignores strangers, or a firewall.
    OpenOrFiltered,
    /// The host said "port unreachable": nothing listens there.
    Closed,
}

impl UdpState {
    pub fn name(self) -> &'static str {
        match self {
            UdpState::Open => "open",
            UdpState::OpenOrFiltered => "open|filtered",
            UdpState::Closed => "closed",
        }
    }
}

/// Send one datagram to every (host, port) pair and add the ports that did
/// not refuse to `host.open_ports`, each with its [`UdpState`] as `state`.
/// One host's ports are probed one after another, with `concurrency` hosts
/// at a time: Linux sends at most about one "port unreachable" a second to
/// one address once a short burst is spent, and two probes at once would
/// make a closed port look silent. `progress(done, total)` is called as
/// each host's ports are settled.
pub async fn udp_port_scan(
    hosts: &mut [Host],
    ports: &[u16],
    options: &PortScanOptions,
    mut progress: impl FnMut(usize, usize),
) {
    let ports: BTreeSet<u16> = ports.iter().copied().collect();
    let total = hosts.len() * ports.len();
    let slots = Arc::new(Semaphore::new(options.concurrency.max(1)));
    let mut probes = JoinSet::new();
    for (index, host) in hosts.iter().enumerate() {
        let ip = host.ip;
        let ports = ports.clone();
        let slots = Arc::clone(&slots);
        let timeout = options.timeout;
        probes.spawn(async move {
            let _slot = slots
                .acquire_owned()
                .await
                .expect("the semaphore is never closed");
            let mut found = Vec::new();
            for port in ports {
                let state = udp_probe(SocketAddr::new(ip, port), timeout).await;
                if state != UdpState::Closed {
                    found.push((port, state));
                }
            }
            (index, found)
        });
    }
    let mut done = 0;
    while let Some(result) = probes.join_next().await {
        done += ports.len();
        progress(done, total);
        let Ok((index, found)) = result else {
            continue;
        };
        let host = &mut hosts[index];
        for (number, state) in found {
            if !host
                .open_ports
                .iter()
                .any(|p| p.number == number && p.proto == Proto::Udp)
            {
                host.open_ports.push(Port {
                    number,
                    proto: Proto::Udp,
                    service: service_name(number, Proto::Udp),
                    state: Some(state.name().to_string()),
                });
            }
        }
    }
    for host in hosts {
        host.open_ports.sort_by_key(|p| (p.proto, p.number));
    }
}

/// One empty datagram to `address`, and what comes back within `timeout`.
/// The socket is connected first: that is what makes the kernel hand the
/// ICMP "port unreachable" to this socket, as a read that fails with
/// "connection refused". Anything else that goes wrong (no route, the host
/// gone) is not the port's doing and counts as silence.
pub async fn udp_probe(address: SocketAddr, timeout: Duration) -> UdpState {
    let unspecified = match address.ip() {
        IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::UNSPECIFIED),
    };
    let Ok(socket) = UdpSocket::bind((unspecified, 0)).await else {
        return UdpState::OpenOrFiltered;
    };
    if socket.connect(address).await.is_err() || socket.send(&[]).await.is_err() {
        return UdpState::OpenOrFiltered;
    }
    let mut buffer = [0u8; 512];
    match tokio::time::timeout(timeout, socket.recv(&mut buffer)).await {
        Ok(Ok(_answer)) => UdpState::Open,
        Ok(Err(err)) if err.kind() == std::io::ErrorKind::ConnectionRefused => UdpState::Closed,
        // Some other error (not the port's doing), or the time ran out.
        Ok(Err(_)) | Err(_) => UdpState::OpenOrFiltered,
    }
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
            state: None,
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

    /// Three UDP ports on this machine: one that answers, one that stays
    /// quiet, one nobody is on.
    #[tokio::test]
    async fn udp_ports_answer_stay_silent_or_refuse() {
        let echo = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let open = echo.local_addr().unwrap().port();
        tokio::spawn(async move {
            let mut buffer = [0u8; 64];
            while let Ok((len, from)) = echo.recv_from(&mut buffer).await {
                let _ = echo.send_to(&buffer[..len], from).await;
            }
        });
        let quiet = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let silent = quiet.local_addr().unwrap().port();
        let closed = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port(); // bound, noted, let go
        let timeout = Duration::from_millis(500);

        let at = |port| SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        assert_eq!(udp_probe(at(open), timeout).await, UdpState::Open);
        assert_eq!(
            udp_probe(at(silent), timeout).await,
            UdpState::OpenOrFiltered
        );
        assert_eq!(udp_probe(at(closed), timeout).await, UdpState::Closed);

        let mut hosts = vec![Host::new(Ipv4Addr::LOCALHOST)];
        let options = PortScanOptions {
            timeout,
            concurrency: 2,
        };
        let mut calls = Vec::new();
        udp_port_scan(
            &mut hosts,
            &[closed, open, silent],
            &options,
            |done, total| {
                calls.push((done, total));
            },
        )
        .await;
        assert_eq!(calls, [(3, 3)]);
        let found: Vec<(u16, &str)> = hosts[0]
            .open_ports
            .iter()
            .map(|p| (p.number, p.state.as_deref().unwrap_or("?")))
            .collect();
        let mut expected = vec![(open, "open"), (silent, "open|filtered")];
        expected.sort();
        assert_eq!(found, expected, "the closed port is not listed");
        assert!(hosts[0].open_ports.iter().all(|p| p.proto == Proto::Udp));
        drop(quiet);
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
