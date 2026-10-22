//! The ping sweep: one ICMP echo request to every address of a network, and
//! a [`Host`] for each address that answers.
//!
//! One socket sends every probe and reads every reply, instead of one `ping`
//! process per address. Which socket depends on what the system allows,
//! tried in this order:
//!
//! 1. An ICMP datagram socket (`SOCK_DGRAM` + `IPPROTO_ICMP`). Linux opens
//!    one for any process whose group is inside `net.ipv4.ping_group_range`
//!    (macOS for everyone), no root needed. The kernel fills in the
//!    identifier and checksum and hands the socket only the replies meant
//!    for it, starting at the ICMP header.
//! 2. A raw ICMP socket (`SOCK_RAW`), which needs root or `CAP_NET_RAW`. It
//!    receives every ICMP packet that reaches the machine, IPv4 header
//!    included, so the identifier is ours to choose and to check.
//! 3. When neither opens: the system `ping`, one process per address. It is
//!    allowed to do what we are not (setuid, a capability, or the same
//!    datagram socket under a group that may use it).
//!
//! At most `concurrency` probes wait for an answer at once, each for
//! `timeout`; an answer frees its slot for the next address straight away.

use std::collections::VecDeque;
use std::hash::{BuildHasher, RandomState};
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use ipnet::Ipv4Net;
use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::UdpSocket;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio::time::{Instant, sleep_until};

use crate::icmp::{self, Echo};
use crate::model::Host;

/// How patient the sweep is, and how many probes it keeps in flight.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SweepOptions {
    /// How long each address has to answer.
    pub timeout: Duration,
    /// How many addresses may be waiting for an answer at once.
    pub concurrency: usize,
}

impl Default for SweepOptions {
    fn default() -> Self {
        SweepOptions {
            timeout: Duration::from_secs(1),
            concurrency: 254,
        }
    }
}

/// How the probes went out (see the module comment).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    DatagramSocket,
    RawSocket,
    SystemPing,
}

impl Method {
    pub fn describe(self) -> &'static str {
        match self {
            Method::DatagramSocket => "ICMP datagram socket",
            Method::RawSocket => "raw ICMP socket",
            Method::SystemPing => "the system ping",
        }
    }
}

/// What a sweep found.
#[derive(Debug, Clone, PartialEq)]
pub struct Sweep {
    /// One per address that answered, in address order.
    pub hosts: Vec<Host>,
    /// How many addresses were probed.
    pub probed: usize,
    pub method: Method,
    /// Things worth telling the user: why a socket could not be used, probes
    /// the system refused to send.
    pub notes: Vec<String>,
}

/// Ping every address of `network` (all of them for a /31 or /32, else all
/// but the network and broadcast addresses). `progress(done, total)` is
/// called each time an address is settled, answered or not.
pub async fn ping_sweep(
    network: &Ipv4Net,
    options: &SweepOptions,
    mut progress: impl FnMut(usize, usize),
) -> io::Result<Sweep> {
    let addresses: Vec<Ipv4Addr> = network.hosts().collect();
    let mut notes = Vec::new();
    let (answered, method) = match IcmpSocket::open(&mut notes) {
        Some(socket) => {
            let method = socket.method;
            (
                socket
                    .sweep(&addresses, options, &mut notes, &mut progress)
                    .await?,
                method,
            )
        }
        None => {
            notes.push("no ICMP socket available, so one `ping` process per address".to_string());
            let answered =
                sweep_with_ping_command(&addresses, options, &mut notes, &mut progress).await?;
            (answered, Method::SystemPing)
        }
    };
    let hosts = addresses
        .iter()
        .zip(answered)
        .filter(|&(_, up)| up)
        .map(|(&address, _)| Host::new(address))
        .collect();
    Ok(Sweep {
        hosts,
        probed: addresses.len(),
        method,
        notes,
    })
}

/// Where each probe stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Probe {
    NotSent,
    Waiting,
    Answered,
    /// No answer within the timeout (yet: a late reply still counts).
    TimedOut,
    /// The system would not send it, e.g. "Network is unreachable".
    Refused,
}

/// Where every probe of a sweep stands, and the counts the sweep loop
/// steers by.
struct Tally {
    probes: Vec<Probe>,
    /// Probes sent and waiting for an answer: the slots in use.
    in_flight: usize,
    /// Probes settled: answered, timed out or refused.
    done: usize,
}

impl Tally {
    fn new(total: usize) -> Tally {
        Tally {
            probes: vec![Probe::NotSent; total],
            in_flight: 0,
            done: 0,
        }
    }

    fn sent(&mut self, index: usize) {
        self.probes[index] = Probe::Waiting;
        self.in_flight += 1;
    }

    fn refused(&mut self, index: usize) {
        self.probes[index] = Probe::Refused;
        self.done += 1;
    }

    /// A reply came. A probe already timed out still counts as answered:
    /// the host is up, it was just slow (or we were slow to read it).
    fn answered(&mut self, index: usize) {
        match self.probes[index] {
            Probe::Waiting => {
                self.in_flight -= 1;
                self.done += 1;
            }
            Probe::TimedOut => {}
            Probe::NotSent | Probe::Answered | Probe::Refused => return,
        }
        self.probes[index] = Probe::Answered;
    }

    fn timed_out(&mut self, index: usize) {
        if self.probes[index] == Probe::Waiting {
            self.probes[index] = Probe::TimedOut;
            self.in_flight -= 1;
            self.done += 1;
        }
    }
}

/// Bytes of probe data: 8 random bytes that mark this sweep's probes, then
/// the address's index (the sequence number only holds 16 bits).
const DATA_LEN: usize = 12;

/// The send buffer the sweep asks for. A probe to a neighbour the kernel
/// has not resolved yet waits in the socket's send buffer, holding about a
/// kilobyte, until ARP finds the neighbour, or for about 3 s when nobody is
/// there. The default buffer (208 KiB) fills up after a couple of hundred
/// such probes and sending stalls until the kernel gives up on some. The
/// kernel caps the request at `net.core.wmem_max` (4 MiB on the dev machine).
const SEND_BUFFER_BYTES: usize = 4 << 20;

struct IcmpSocket {
    /// tokio's `UdpSocket` only ever calls `sendto` / `recvfrom` on its file
    /// descriptor, and those are all an ICMP socket needs too; wrapping the
    /// descriptor in it gets us tokio's waiting for free. Nothing UDP happens.
    socket: UdpSocket,
    method: Method,
    /// Ours on a raw socket; on a datagram socket the kernel overwrites it.
    identifier: u16,
}

impl IcmpSocket {
    /// The first ICMP socket the system lets us open, or `None`; the reasons
    /// for skipping a kind go into `notes`.
    fn open(notes: &mut Vec<String>) -> Option<IcmpSocket> {
        for (kind, method) in [
            (Type::DGRAM, Method::DatagramSocket),
            (Type::RAW, Method::RawSocket),
        ] {
            match Self::open_one(kind) {
                Ok(socket) => {
                    return Some(IcmpSocket {
                        socket,
                        method,
                        identifier: std::process::id() as u16,
                    });
                }
                Err(err) => notes.push(format!("cannot open a {}: {err}", method.describe())),
            }
        }
        None
    }

    fn open_one(kind: Type) -> io::Result<UdpSocket> {
        let socket = Socket::new(Domain::IPV4, kind, Some(Protocol::ICMPV4))?;
        // Best effort: a smaller send buffer only makes sending slower, and a
        // smaller receive buffer only risks replies when hundreds arrive at
        // once (the kernel drops what does not fit, without a word).
        let _ = socket.set_send_buffer_size(SEND_BUFFER_BYTES);
        let _ = socket.set_recv_buffer_size(SEND_BUFFER_BYTES);
        socket.set_nonblocking(true)?; // tokio needs it: it waits for readiness itself
        UdpSocket::from_std(socket.into())
    }

    /// Probe every address, at most `options.concurrency` waiting at once.
    /// Returns, for each address, whether it answered.
    async fn sweep(
        &self,
        addresses: &[Ipv4Addr],
        options: &SweepOptions,
        notes: &mut Vec<String>,
        progress: &mut impl FnMut(usize, usize),
    ) -> io::Result<Vec<bool>> {
        let total = addresses.len();
        let token = RandomState::new()
            .hash_one(std::process::id())
            .to_be_bytes();
        let mut tally = Tally::new(total);
        // Probes waiting, oldest first; deadlines only grow, because every
        // probe gets the same timeout.
        let mut waiting: VecDeque<(Instant, usize)> = VecDeque::new();
        let mut next = 0; // the next address to probe
        let mut refused = 0;
        let mut first_refusal: Option<io::Error> = None;
        let mut buffer = [0u8; 1500];

        while tally.done < total {
            let settled_before = tally.done;
            // Fill every free slot, reading the replies that arrive meanwhile:
            // a send can stall for seconds while the kernel waits on ARP, and
            // replies left unread that long could overflow the receive buffer.
            while next < total && tally.in_flight < options.concurrency.max(1) {
                let index = next;
                next += 1;
                match self.send_probe(addresses[index], index, &token).await {
                    Ok(()) => {
                        tally.sent(index);
                        waiting.push_back((Instant::now() + options.timeout, index));
                    }
                    Err(err) => {
                        tally.refused(index);
                        refused += 1;
                        first_refusal.get_or_insert(err);
                    }
                }
                self.read_waiting(&mut buffer, addresses, &token, &mut tally)?;
            }
            let Some(&(deadline, _)) = waiting.front() else {
                if tally.done != settled_before {
                    progress(tally.done, total);
                }
                continue; // nothing waiting: either all done or more to send
            };

            // Wait for a reply or for the oldest probe's time to run out.
            tokio::select! {
                biased; // a reply that is already here goes first
                received = self.socket.recv_from(&mut buffer) => {
                    let (len, from) = received?;
                    if let Some(index) = self.match_reply(&buffer[..len], from, addresses, &token) {
                        tally.answered(index);
                    }
                }
                () = sleep_until(deadline) => {}
            }
            // Read every reply that is already waiting before giving up on
            // anyone.
            self.read_waiting(&mut buffer, addresses, &token, &mut tally)?;
            // Then time out every probe whose deadline has passed.
            let now = Instant::now();
            while let Some(&(deadline, index)) = waiting.front() {
                if deadline > now {
                    break;
                }
                waiting.pop_front();
                tally.timed_out(index);
            }
            if tally.done != settled_before {
                progress(tally.done, total);
            }
        }

        if let Some(err) = first_refusal {
            notes.push(format!(
                "{refused} of {total} probes could not be sent: {err}"
            ));
        }
        Ok(tally
            .probes
            .into_iter()
            .map(|p| p == Probe::Answered)
            .collect())
    }

    /// Read every reply already waiting, without blocking, and mark the
    /// probes they answer.
    fn read_waiting(
        &self,
        buffer: &mut [u8],
        addresses: &[Ipv4Addr],
        token: &[u8; 8],
        tally: &mut Tally,
    ) -> io::Result<()> {
        loop {
            match self.socket.try_recv_from(buffer) {
                Ok((len, from)) => {
                    if let Some(index) = self.match_reply(&buffer[..len], from, addresses, token) {
                        tally.answered(index);
                    }
                }
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(err) => return Err(err),
            }
        }
    }

    async fn send_probe(&self, address: Ipv4Addr, index: usize, token: &[u8; 8]) -> io::Result<()> {
        let mut data = Vec::with_capacity(DATA_LEN);
        data.extend_from_slice(token);
        data.extend_from_slice(&(index as u32).to_be_bytes());
        let packet = Echo {
            identifier: self.identifier,
            sequence: index as u16, // wraps past 65535; the data carries the full index
            data,
        }
        .request_bytes();
        // ICMP has no ports; the 0 is ignored.
        let target = SocketAddr::V4(SocketAddrV4::new(address, 0));
        self.socket.send_to(&packet, target).await.map(|_| ())
    }

    /// Which address a packet answers, if it is a reply to one of this
    /// sweep's probes and comes from the address that probe went to.
    fn match_reply(
        &self,
        packet: &[u8],
        from: SocketAddr,
        addresses: &[Ipv4Addr],
        token: &[u8; 8],
    ) -> Option<usize> {
        let icmp = match self.method {
            Method::RawSocket => icmp::strip_ipv4_header(packet)?,
            _ => packet,
        };
        let echo = Echo::parse_reply(icmp)?;
        if self.method == Method::RawSocket && echo.identifier != self.identifier {
            return None; // someone else's ping
        }
        if echo.data.len() < DATA_LEN || echo.data[..8] != token[..] {
            return None;
        }
        let index = u32::from_be_bytes(echo.data[8..DATA_LEN].try_into().ok()?) as usize;
        let expected = addresses.get(index)?;
        (from.ip() == IpAddr::V4(*expected)).then_some(index)
    }
}

/// The last resort: the system `ping`, one process per address, at most
/// `concurrency` at once. Its exit status says it all (0: a reply came), so
/// its output is thrown away. `Err` when `ping` could not be run for any
/// address (not installed): silence from every address would look the same
/// as an empty network otherwise.
async fn sweep_with_ping_command(
    addresses: &[Ipv4Addr],
    options: &SweepOptions,
    notes: &mut Vec<String>,
    progress: &mut impl FnMut(usize, usize),
) -> io::Result<Vec<bool>> {
    let total = addresses.len();
    let slots = Arc::new(Semaphore::new(options.concurrency.max(1)));
    let mut tasks = JoinSet::new();
    for (index, &address) in addresses.iter().enumerate() {
        let slots = Arc::clone(&slots);
        let timeout = options.timeout;
        tasks.spawn(async move {
            let _slot = slots
                .acquire_owned()
                .await
                .expect("the semaphore is never closed");
            (index, system_ping(address, timeout).await)
        });
    }
    let mut answered = vec![false; total];
    let mut done = 0;
    let mut not_run = 0;
    let mut first_failure: Option<io::Error> = None;
    while let Some(result) = tasks.join_next().await {
        match result {
            Ok((index, Ok(up))) => answered[index] = up,
            Ok((_, Err(err))) => {
                not_run += 1;
                first_failure.get_or_insert(err);
            }
            Err(_panicked) => {}
        }
        done += 1;
        progress(done, total);
    }
    if let Some(err) = first_failure {
        if not_run == total {
            return Err(io::Error::new(
                err.kind(),
                format!("cannot run ping: {err}"),
            ));
        }
        notes.push(format!(
            "{not_run} of {total} pings could not be run: {err}"
        ));
    }
    Ok(answered)
}

/// Whether `address` answers the system `ping`; `Err` when `ping` could not
/// be started at all (not installed, not allowed).
async fn system_ping(address: Ipv4Addr, timeout: Duration) -> io::Result<bool> {
    let mut command = tokio::process::Command::new("ping");
    command
        .args(ping_arguments(address, timeout))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    // The extra 5 s only matter if ping itself hangs; dropping it kills it.
    match tokio::time::timeout(timeout + Duration::from_secs(5), command.status()).await {
        Ok(Ok(status)) => Ok(status.success()),
        Ok(Err(err)) => Err(err),
        Err(_hung) => Ok(false),
    }
}

/// One echo request, waiting `timeout`; the flags differ per system.
fn ping_arguments(address: Ipv4Addr, timeout: Duration) -> Vec<String> {
    let address = address.to_string();
    let millis = timeout.as_millis().max(1).to_string();
    if cfg!(windows) {
        vec!["-n".into(), "1".into(), "-w".into(), millis, address]
    } else if cfg!(target_os = "macos") {
        vec!["-c".into(), "1".into(), "-W".into(), millis, address] // milliseconds
    } else {
        let seconds = timeout.as_secs_f64().to_string(); // iputils: seconds, fractions allowed
        vec!["-c".into(), "1".into(), "-W".into(), seconds, address]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn net(text: &str) -> Ipv4Net {
        text.parse().unwrap()
    }

    fn quick(concurrency: usize) -> SweepOptions {
        SweepOptions {
            timeout: Duration::from_millis(500),
            concurrency,
        }
    }

    fn addresses(sweep: &Sweep) -> Vec<String> {
        sweep.hosts.iter().map(|h| h.ip.to_string()).collect()
    }

    /// Linux answers on the whole of 127.0.0.0/8, so a sweep there finds every
    /// address, whichever way the probes went out.
    #[tokio::test]
    async fn every_loopback_address_answers() {
        let mut calls = Vec::new();
        let sweep = ping_sweep(&net("127.0.0.0/29"), &quick(254), |done, total| {
            calls.push((done, total));
        })
        .await
        .unwrap();
        assert_eq!(sweep.probed, 6);
        assert_eq!(
            addresses(&sweep),
            [
                "127.0.0.1",
                "127.0.0.2",
                "127.0.0.3",
                "127.0.0.4",
                "127.0.0.5",
                "127.0.0.6"
            ],
            "{sweep:?}"
        );
        assert_eq!(calls.last(), Some(&(6, 6)));
        assert!(calls.windows(2).all(|w| w[0].0 < w[1].0), "{calls:?}");
    }

    #[tokio::test]
    async fn one_probe_at_a_time_still_reaches_everyone() {
        let sweep = ping_sweep(&net("127.0.0.8/30"), &quick(1), |_, _| {})
            .await
            .unwrap();
        assert_eq!(addresses(&sweep), ["127.0.0.9", "127.0.0.10"]);
    }

    #[tokio::test]
    async fn a_single_address_is_swept_too() {
        let sweep = ping_sweep(&net("127.0.0.1/32"), &quick(254), |_, _| {})
            .await
            .unwrap();
        assert_eq!(addresses(&sweep), ["127.0.0.1"]);
    }

    /// 192.0.2.0/24 is reserved for documentation: nothing ever answers
    /// there, so the probe is given up after its timeout (or never leaves,
    /// on a machine without a route).
    #[tokio::test]
    async fn silence_is_given_up_on_after_the_timeout() {
        let options = SweepOptions {
            timeout: Duration::from_millis(200),
            concurrency: 254,
        };
        let started = std::time::Instant::now();
        let sweep = ping_sweep(&net("192.0.2.1/32"), &options, |_, _| {})
            .await
            .unwrap();
        assert!(sweep.hosts.is_empty(), "{sweep:?}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    /// The fallback on its own, if this machine has a `ping` to fall back on.
    #[tokio::test]
    async fn the_system_ping_fallback_finds_loopback() {
        if std::process::Command::new("ping")
            .arg("-V")
            .output()
            .is_err()
        {
            return; // no ping installed: nothing to test
        }
        let addresses = [Ipv4Addr::new(127, 0, 0, 1), Ipv4Addr::new(192, 0, 2, 1)];
        let mut notes = Vec::new();
        let answered = sweep_with_ping_command(&addresses, &quick(2), &mut notes, &mut |_, _| {})
            .await
            .unwrap();
        assert_eq!(answered, [true, false]);
        assert!(notes.is_empty(), "{notes:?}");
    }

    /// A `ping` that cannot start is an error, not an empty network.
    #[tokio::test]
    async fn a_ping_that_cannot_run_is_reported() {
        let err = system_ping(Ipv4Addr::LOCALHOST, Duration::from_millis(100))
            .await
            .map(|_| ()); // whatever this machine's ping says
        let _ = err;
        let mut command = tokio::process::Command::new("/no/such/ping");
        command.arg("-c").arg("1").kill_on_drop(true);
        let status = command.status().await;
        assert_eq!(
            status.map(|_| ()).unwrap_err().kind(),
            io::ErrorKind::NotFound,
            "a spawn failure comes back as an error, which system_ping passes on"
        );
    }

    #[test]
    fn the_tally_frees_slots_and_late_replies_still_count() {
        let mut tally = Tally::new(3);
        for index in 0..3 {
            tally.sent(index);
        }
        assert_eq!((tally.in_flight, tally.done), (3, 0));
        tally.answered(0);
        tally.answered(0); // a duplicate reply changes nothing
        tally.timed_out(1);
        tally.timed_out(0); // answered already: not timed out
        assert_eq!((tally.in_flight, tally.done), (1, 2));
        tally.answered(1); // late, but here: the host is up
        assert_eq!((tally.in_flight, tally.done), (1, 2));
        assert_eq!(
            tally.probes,
            [Probe::Answered, Probe::Answered, Probe::Waiting]
        );
    }

    #[test]
    fn ping_arguments_for_this_system() {
        let args = ping_arguments(Ipv4Addr::new(10, 0, 0, 1), Duration::from_millis(1500));
        assert_eq!(args.last().map(String::as_str), Some("10.0.0.1"));
        if cfg!(target_os = "linux") {
            assert_eq!(args, ["-c", "1", "-W", "1.5", "10.0.0.1"]);
        }
    }
}
