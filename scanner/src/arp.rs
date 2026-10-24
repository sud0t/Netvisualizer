//! The ARP sweep: one ARP request to every address of our own subnet, and a
//! [`Host`] with its MAC for each that answers.
//!
//! ARP ("who has 192.168.1.251? tell 192.168.1.143") is how the kernel
//! itself learns a neighbour's MAC before it can send it anything (see
//! neighbours.rs), so every host on the link answers it: a machine that
//! drops ping (a firewall's default on Windows) still answers ARP, or
//! nothing could ever reach it. That makes an ARP sweep the surest way to
//! find the hosts on the link, and the replies carry the MACs for free. It
//! goes no further than the link, though: ARP is not routed, so the rest of
//! a bigger network still needs the ping sweep.
//!
//! A request is an Ethernet frame with a 28-byte ARP message in it, 42
//! bytes in all, built here by hand:
//!
//! ```text
//! offset  bytes  field
//!  0       6     destination MAC   ff:ff:ff:ff:ff:ff, everyone on the link
//!  6       6     source MAC        ours
//! 12       2     EtherType         0x0806, "ARP follows"
//! 14       2     hardware type     1, Ethernet
//! 16       2     protocol type     0x0800, IPv4
//! 18       1     hardware length   6
//! 19       1     protocol length   4
//! 20       2     operation         1 request, 2 reply
//! 22       6     sender MAC        ours; in a reply, the answer
//! 28       4     sender IPv4       ours; in a reply, the address asked about
//! 32       6     target MAC        zeros in a request; ours in a reply
//! 38       4     target IPv4       the address asked about; ours in a reply
//! ```
//!
//! It goes out on a packet socket (`AF_PACKET`), which hands a process whole
//! frames, Ethernet header included, in both directions. Opening one needs
//! `CAP_NET_RAW`: root, or `setcap cap_net_raw+ep` on the binary. Without
//! it [`PacketSocket::open`] says so, and the scan falls back to ping.

use std::collections::BTreeMap;
use std::fmt;
use std::io::{self, Read, Write};
use std::net::Ipv4Addr;
use std::time::Duration;

use ipnet::Ipv4Net;
use socket2::{Domain, Protocol, SockAddr, Socket, Type, socklen_t};
use tokio::io::unix::AsyncFd;
use tokio::time::{Instant, sleep, sleep_until};

use crate::local::LocalNetwork;
use crate::model::{Host, MacAddr};
use crate::neighbours;

/// The `EtherType` that says an ARP message follows the Ethernet header.
const ETHERTYPE_ARP: u16 = 0x0806;
const HARDWARE_ETHERNET: u16 = 1;
const PROTOCOL_IPV4: u16 = 0x0800;
const REQUEST: u16 = 1;
/// An Ethernet/IPv4 ARP frame: 14 bytes of Ethernet header, 28 of ARP.
const FRAME_LEN: usize = 42;
const BROADCAST: MacAddr = MacAddr::new([0xff; 6]);

/// Addresses that stay silent are asked this many times in all. Wi-Fi
/// sends a broadcast once, unacknowledged, and now and then loses it.
const ROUNDS: usize = 2;
/// Requests sent between two reads of the replies that came in meanwhile,
/// so a /16's replies do not overflow the receive buffer while we send.
const SEND_BATCH: usize = 64;
/// How often a frame the interface's queue would not take is tried again,
/// a millisecond apart, before the sweep gives up on the interface.
const FULL_QUEUE_RETRIES: usize = 1000;

/// The fields of an ARP message (the Ethernet header around it is not kept).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Arp {
    pub operation: u16,
    pub sender_mac: MacAddr,
    pub sender_ip: Ipv4Addr,
    pub target_mac: MacAddr,
    pub target_ip: Ipv4Addr,
}

impl Arp {
    /// The whole frame of a request from `sender` for `target`'s MAC.
    fn request(sender_mac: MacAddr, sender_ip: Ipv4Addr, target: Ipv4Addr) -> [u8; FRAME_LEN] {
        let mut frame = [0u8; FRAME_LEN];
        frame[0..6].copy_from_slice(&BROADCAST.octets());
        frame[6..12].copy_from_slice(&sender_mac.octets());
        frame[12..14].copy_from_slice(&ETHERTYPE_ARP.to_be_bytes());
        frame[14..16].copy_from_slice(&HARDWARE_ETHERNET.to_be_bytes());
        frame[16..18].copy_from_slice(&PROTOCOL_IPV4.to_be_bytes());
        frame[18] = 6;
        frame[19] = 4;
        frame[20..22].copy_from_slice(&REQUEST.to_be_bytes());
        frame[22..28].copy_from_slice(&sender_mac.octets());
        frame[28..32].copy_from_slice(&sender_ip.octets());
        // 32..38, the target MAC, stays zero: that is the question.
        frame[38..42].copy_from_slice(&target.octets());
        frame
    }

    /// The ARP message in a frame, or `None` when the frame is something
    /// else: another `EtherType`, other address sizes, or too short. (Longer
    /// is fine: Ethernet pads short frames to 60 bytes.)
    fn parse(frame: &[u8]) -> Option<Arp> {
        if frame.len() < FRAME_LEN {
            return None;
        }
        let word = |at: usize| u16::from_be_bytes([frame[at], frame[at + 1]]);
        if word(12) != ETHERTYPE_ARP
            || word(14) != HARDWARE_ETHERNET
            || word(16) != PROTOCOL_IPV4
            || frame[18] != 6
            || frame[19] != 4
        {
            return None;
        }
        let mac = |at: usize| MacAddr::new(frame[at..at + 6].try_into().expect("six bytes"));
        let ip = |at: usize| Ipv4Addr::new(frame[at], frame[at + 1], frame[at + 2], frame[at + 3]);
        Some(Arp {
            operation: word(20),
            sender_mac: mac(22),
            sender_ip: ip(28),
            target_mac: mac(32),
            target_ip: ip(38),
        })
    }
}

/// Why no ARP sweep could run; the scan then pings instead.
#[derive(Debug)]
pub enum ArpError {
    /// No `CAP_NET_RAW`: the packet socket could not be opened.
    NotPermitted(io::Error),
    /// The interface has no MAC address to send from (loopback, a tunnel),
    /// or no index in sysfs (not Linux, or no such interface).
    NoLink(String),
    Io(io::Error),
}

impl fmt::Display for ArpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ArpError::NotPermitted(err) => write!(
                f,
                "cannot open a packet socket ({err}): it needs root or CAP_NET_RAW \
                 (`sudo setcap cap_net_raw+ep` on the program)"
            ),
            ArpError::NoLink(interface) => {
                write!(
                    f,
                    "interface {interface:?} has no MAC address to send ARP from"
                )
            }
            ArpError::Io(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for ArpError {}

/// What an ARP sweep found.
#[derive(Debug, Clone, PartialEq)]
pub struct ArpSweep {
    /// One per address that answered, with its MAC, in address order.
    pub hosts: Vec<Host>,
    /// How many addresses were asked about.
    pub probed: usize,
}

/// A packet socket bound to one interface, receiving only ARP frames.
#[derive(Debug)]
pub struct PacketSocket {
    /// tokio waits for the descriptor to be readable or writable; the reads
    /// and writes themselves are socket2's.
    fd: AsyncFd<Socket>,
    /// This machine's addresses on the interface: the sender of every request.
    mac: MacAddr,
    ip: Ipv4Addr,
}

impl PacketSocket {
    /// Open and bind a socket on `local`'s interface. Instant; the sweep
    /// comes after. Must run inside a tokio runtime, which the socket is
    /// registered with.
    pub fn open(local: &LocalNetwork) -> Result<PacketSocket, ArpError> {
        let no_link = || ArpError::NoLink(local.interface.clone());
        let mac = neighbours::own_mac(&local.interface).ok_or_else(no_link)?;
        let index = neighbours::interface_index(&local.interface).ok_or_else(no_link)?;
        // The socket's protocol is the EtherType it receives, in network byte
        // order (`htons`): ARP frames only, not everything on the wire.
        let protocol = Protocol::from(i32::from(ETHERTYPE_ARP.to_be()));
        let socket =
            Socket::new(Domain::PACKET, Type::RAW, Some(protocol)).map_err(|err| {
                match err.kind() {
                    io::ErrorKind::PermissionDenied => ArpError::NotPermitted(err),
                    _ => ArpError::Io(err),
                }
            })?;
        // Binding to the interface sends on it and receives from it alone.
        socket.bind(&link_address(index)).map_err(ArpError::Io)?;
        socket.set_nonblocking(true).map_err(ArpError::Io)?;
        // SAFETY: the `Socket` owns its descriptor and moves into the
        // `AsyncFd`, so the descriptor stays open for as long as it is
        // registered with tokio (what `register` asks the caller to ensure).
        let fd = unsafe { AsyncFd::register(socket) }.map_err(|err| ArpError::Io(err.into()))?;
        Ok(PacketSocket {
            fd,
            mac,
            ip: local.ip,
        })
    }

    /// Ask for every address of `network` but our own, again for the ones
    /// that stay silent, and wait `timeout` after the last request each
    /// time. `progress(sent, total)` follows the first round's requests.
    pub async fn sweep(
        &self,
        network: &Ipv4Net,
        timeout: Duration,
        mut progress: impl FnMut(usize, usize),
    ) -> io::Result<ArpSweep> {
        let addresses: Vec<Ipv4Addr> = network.hosts().filter(|&ip| ip != self.ip).collect();
        let total = addresses.len();
        let mut found: BTreeMap<Ipv4Addr, MacAddr> = BTreeMap::new();
        let mut buffer = [0u8; 1500];
        let mut sent = 0;
        for _ in 0..ROUNDS {
            let silent: Vec<Ipv4Addr> = addresses
                .iter()
                .copied()
                .filter(|ip| !found.contains_key(ip))
                .collect();
            if silent.is_empty() {
                break;
            }
            for batch in silent.chunks(SEND_BATCH) {
                for &target in batch {
                    self.send(&Arp::request(self.mac, self.ip, target)).await?;
                    sent += 1;
                    progress(sent.min(total), total);
                }
                self.read_waiting(*network, &mut found, &mut buffer)?;
            }
            // Then listen until the timeout since the last request runs out.
            let deadline = Instant::now() + timeout;
            loop {
                tokio::select! {
                    biased;
                    ready = self.fd.readable() => {
                        let mut guard = ready?;
                        // `Read` for `&Socket` is `recv`: one frame per call.
                        match guard.try_io(|fd| fd.get_ref().read(&mut buffer)) {
                            Ok(len) => self.record(&buffer[..len?], *network, &mut found),
                            Err(_would_block) => {}
                        }
                    }
                    () = sleep_until(deadline) => break,
                }
            }
        }
        progress(total, total);
        let hosts = found
            .into_iter()
            .map(|(ip, mac)| {
                let mut host = Host::new(ip);
                host.mac = Some(mac);
                host
            })
            .collect();
        Ok(ArpSweep {
            hosts,
            probed: total,
        })
    }

    /// Send one frame, waiting when the system cannot take it yet: tokio
    /// says when the socket's buffer has room, and `ENOBUFS` ("No buffer
    /// space available", the interface's own queue is full) is tried again
    /// after a moment, up to [`FULL_QUEUE_RETRIES`] times: a queue that
    /// never drains means the interface is stuck, and the error says so.
    async fn send(&self, frame: &[u8]) -> io::Result<()> {
        let mut retries = 0;
        loop {
            let mut guard = self.fd.writable().await?;
            match guard.try_io(|fd| fd.get_ref().write(frame)) {
                Ok(Ok(_)) => return Ok(()),
                Ok(Err(err))
                    if err.raw_os_error() == Some(libc::ENOBUFS)
                        && retries < FULL_QUEUE_RETRIES =>
                {
                    retries += 1;
                    sleep(Duration::from_millis(1)).await;
                }
                Ok(Err(err)) => return Err(err),
                Err(_would_block) => {}
            }
        }
    }

    /// Read every frame already waiting, without blocking.
    fn read_waiting(
        &self,
        network: Ipv4Net,
        found: &mut BTreeMap<Ipv4Addr, MacAddr>,
        buffer: &mut [u8],
    ) -> io::Result<()> {
        loop {
            match self.fd.get_ref().read(buffer) {
                Ok(len) => self.record(&buffer[..len], network, found),
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(err) => return Err(err),
            }
        }
    }

    /// A frame from the link. Any ARP message whose sender is in `network`
    /// proves that sender is up and says its MAC: a reply to us, or a
    /// request of its own that we overheard. Our own requests come back
    /// too (a packet socket sees outgoing frames) and are skipped.
    fn record(&self, frame: &[u8], network: Ipv4Net, found: &mut BTreeMap<Ipv4Addr, MacAddr>) {
        let Some(arp) = Arp::parse(frame) else {
            return;
        };
        if arp.sender_ip == self.ip
            || !network.contains(&arp.sender_ip)
            || arp.sender_mac.is_nil()
            || arp.sender_mac == BROADCAST
        {
            return;
        }
        found.entry(arp.sender_ip).or_insert(arp.sender_mac);
    }
}

/// A packet socket's address (`struct sockaddr_ll`): the interface, by
/// index, and the `EtherType`. The other fields only matter for sending with
/// `sendto` on a `SOCK_DGRAM` packet socket, where the kernel builds the
/// Ethernet header; ours is `SOCK_RAW` and the frame carries its own.
fn link_address(index: i32) -> SockAddr {
    let address = libc::sockaddr_ll {
        sll_family: libc::AF_PACKET as u16,
        sll_protocol: ETHERTYPE_ARP.to_be(),
        sll_ifindex: index,
        sll_hatype: 0,
        sll_pkttype: 0,
        sll_halen: 0,
        sll_addr: [0; 8],
    };
    // SAFETY: `try_init` hands out zeroed storage big enough for any socket
    // address; a `sockaddr_ll` is one, written whole, and its length is
    // reported back.
    let ((), address) = unsafe {
        SockAddr::try_init(|storage, len| {
            storage.cast::<libc::sockaddr_ll>().write(address);
            *len = size_of::<libc::sockaddr_ll>() as socklen_t;
            Ok(())
        })
    }
    .expect("writing a struct cannot fail");
    address
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local;

    /// The operation of a reply (the code only ever sends requests).
    const REPLY: u16 = 2;

    fn mac(text: &str) -> MacAddr {
        text.parse().unwrap()
    }

    #[test]
    fn a_request_is_42_bytes_with_every_field_in_network_order() {
        let frame = Arp::request(
            mac("02:00:00:aa:bb:cc"),
            Ipv4Addr::new(10, 0, 0, 1),
            Ipv4Addr::new(10, 0, 0, 2),
        );
        #[rustfmt::skip]
        let expected: [u8; FRAME_LEN] = [
            0xff, 0xff, 0xff, 0xff, 0xff, 0xff, // to everyone
            0x02, 0x00, 0x00, 0xaa, 0xbb, 0xcc, // from us
            0x08, 0x06, // ARP
            0x00, 0x01, 0x08, 0x00, 6, 4, // Ethernet, IPv4, 6-byte and 4-byte addresses
            0x00, 0x01, // request
            0x02, 0x00, 0x00, 0xaa, 0xbb, 0xcc, 10, 0, 0, 1, // sender
            0, 0, 0, 0, 0, 0, 10, 0, 0, 2, // target: MAC unknown
        ];
        assert_eq!(frame, expected);
    }

    #[test]
    fn requests_and_replies_parse_and_other_frames_do_not() {
        let request = Arp::request(
            mac("02:00:00:aa:bb:cc"),
            Ipv4Addr::new(10, 0, 0, 1),
            Ipv4Addr::new(10, 0, 0, 2),
        );
        assert_eq!(
            Arp::parse(&request),
            Some(Arp {
                operation: REQUEST,
                sender_mac: mac("02:00:00:aa:bb:cc"),
                sender_ip: Ipv4Addr::new(10, 0, 0, 1),
                target_mac: MacAddr::NIL,
                target_ip: Ipv4Addr::new(10, 0, 0, 2),
            })
        );

        // What 10.0.0.2 sends back: the sender and target swapped, its own
        // MAC filled in, and operation 2. Padded to 60 bytes, as Ethernet does.
        let mut reply = request.to_vec();
        reply[0..6].copy_from_slice(&request[6..12]);
        reply[6..12].copy_from_slice(&mac("b8:27:eb:8e:cc:eb").octets());
        reply[20..22].copy_from_slice(&REPLY.to_be_bytes());
        reply[22..32].copy_from_slice(&[0xb8, 0x27, 0xeb, 0x8e, 0xcc, 0xeb, 10, 0, 0, 2]);
        reply[32..42].copy_from_slice(&[0x02, 0x00, 0x00, 0xaa, 0xbb, 0xcc, 10, 0, 0, 1]);
        reply.resize(60, 0);
        let parsed = Arp::parse(&reply).unwrap();
        assert_eq!(parsed.operation, REPLY);
        assert_eq!(parsed.sender_mac, mac("b8:27:eb:8e:cc:eb"));
        assert_eq!(parsed.sender_ip, Ipv4Addr::new(10, 0, 0, 2));
        assert_eq!(parsed.target_ip, Ipv4Addr::new(10, 0, 0, 1));

        assert_eq!(Arp::parse(&request[..41]), None, "too short");
        let mut ipv4 = request;
        ipv4[12..14].copy_from_slice(&[0x08, 0x00]);
        assert_eq!(Arp::parse(&ipv4), None, "not ARP");
        let mut token_ring = request;
        token_ring[14..16].copy_from_slice(&[0x00, 0x06]);
        assert_eq!(Arp::parse(&token_ring), None, "not Ethernet");
        let mut odd_sizes = request;
        odd_sizes[19] = 16;
        assert_eq!(Arp::parse(&odd_sizes), None, "not IPv4 addresses");
    }

    #[test]
    fn the_link_address_names_the_interface_and_the_protocol() {
        let address = link_address(3);
        assert_eq!(address.family(), libc::AF_PACKET as libc::sa_family_t);
        assert_eq!(address.len() as usize, size_of::<libc::sockaddr_ll>());
        // SAFETY: the storage holds the sockaddr_ll written by link_address.
        let raw = unsafe { &*address.as_ptr().cast::<libc::sockaddr_ll>() };
        assert_eq!(raw.sll_ifindex, 3);
        assert_eq!(u16::from_be(raw.sll_protocol), ETHERTYPE_ARP);
    }

    /// Whatever this machine allows: a socket, or the error that says how to
    /// be allowed one. Loopback has no MAC, so it is the other error.
    #[tokio::test]
    async fn opening_says_what_is_missing() {
        let loopback = LocalNetwork {
            interface: "lo".into(),
            ip: Ipv4Addr::LOCALHOST,
            network: "127.0.0.0/8".parse().unwrap(),
            gateway: None,
        };
        let err = PacketSocket::open(&loopback).unwrap_err();
        assert!(matches!(err, ArpError::NoLink(_)), "{err}");
        assert!(err.to_string().contains("\"lo\""), "{err}");

        let Some(network) = local::local_networks()
            .unwrap_or_default()
            .into_iter()
            .next()
        else {
            return; // not on any network: nothing more to try
        };
        match PacketSocket::open(&network) {
            Ok(socket) => assert_eq!(socket.ip, network.ip), // running with CAP_NET_RAW
            // No CAP_NET_RAW, or a VPN tunnel first in the list.
            Err(ArpError::NotPermitted(_) | ArpError::NoLink(_)) => {}
            Err(ArpError::Io(err)) => panic!("{err}"),
        }
        let unprivileged = ArpError::NotPermitted(io::Error::from_raw_os_error(libc::EPERM));
        assert!(unprivileged.to_string().contains("CAP_NET_RAW"));
    }
}
