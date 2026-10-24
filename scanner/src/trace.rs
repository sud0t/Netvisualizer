//! Traceroute: the routers on the way to a host beyond our link.
//!
//! Every IP packet carries a TTL ("time to live"): a hop count that each
//! router takes one off before passing the packet on, and when it reaches
//! zero the router drops the packet and sends back an ICMP "time exceeded"
//! message (type 11) from its own address. That is the whole trick: an echo
//! request sent with TTL 1 makes the first router complain, TTL 2 the
//! second, and so on, until the echo reply comes from the host itself.
//!
//! No root needed. The probes go out on the same kind of ICMP datagram
//! socket the ping sweep uses, with `IP_RECVERR` set: the kernel then keeps
//! the ICMP errors it receives for our packets on the socket's "error
//! queue", and `recvmsg(MSG_ERRQUEUE)` hands them over one at a time, each
//! with the address of the router that sent it attached (`SO_EE_OFFENDER`).
//! A raw socket would show the time-exceeded packets themselves, router
//! address and all, but needs `CAP_NET_RAW`; this way works for anyone the
//! sweep works for.

use std::hash::{BuildHasher, RandomState};
use std::io::{self, Read};
use std::net::{Ipv4Addr, SocketAddrV4};
use std::os::fd::{AsRawFd, RawFd};
use std::sync::Arc;
use std::time::Duration;

use socket2::{Domain, Protocol, Socket, Type};
use tokio::io::Interest;
use tokio::io::unix::AsyncFd;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio::time::Instant;

use crate::icmp::{self, Echo};

/// How far a trace goes at most; 30 is what `traceroute` uses too, and the
/// internet is rarely deeper than 20 hops.
pub const DEFAULT_MAX_HOPS: u8 = 30;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TraceOptions {
    pub max_hops: u8,
    /// How long each hop has to answer.
    pub timeout: Duration,
}

impl Default for TraceOptions {
    fn default() -> Self {
        TraceOptions {
            max_hops: DEFAULT_MAX_HOPS,
            timeout: Duration::from_secs(1),
        }
    }
}

/// One step of the way: the router that answered for this TTL, if any did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hop {
    pub ttl: u8,
    /// `None` when nothing came back in time (`traceroute` prints a `*`).
    pub router: Option<Ipv4Addr>,
    pub rtt: Option<Duration>,
}

/// The way to `target`: the routers in order, then whether the host itself
/// answered at the end.
#[derive(Debug, Clone, PartialEq)]
pub struct Path {
    pub target: Ipv4Addr,
    /// One per TTL tried, in order; empty when the host answered the very
    /// first probe (no router in between).
    pub hops: Vec<Hop>,
    /// The host's own echo reply came.
    pub reached: bool,
    /// The host's round trip, when it answered.
    pub rtt: Option<Duration>,
}

impl Path {
    /// The routers that answered, in order.
    pub fn routers(&self) -> impl Iterator<Item = Ipv4Addr> + '_ {
        self.hops.iter().filter_map(|hop| hop.router)
    }

    /// One entry per hop, the address or `*` for silence: what `traceroute`
    /// prints, for the host's details card.
    pub fn describe(&self) -> Vec<String> {
        self.hops
            .iter()
            .map(|hop| match hop.router {
                Some(router) => router.to_string(),
                None => "*".to_string(),
            })
            .collect()
    }
}

/// Find the way to `target`, one TTL at a time. `Err` only when no probe
/// socket can be opened; a trace that cannot get through ends early with
/// `reached` false.
pub async fn traceroute(target: Ipv4Addr, options: &TraceOptions) -> io::Result<Path> {
    let socket = ProbeSocket::open(target)?;
    let mut path = Path {
        target,
        hops: Vec::new(),
        reached: false,
        rtt: None,
    };
    for ttl in 1..=options.max_hops.max(1) {
        let Ok(answer) = socket.probe(ttl, options.timeout).await else {
            break; // the probe could not be sent: no route, say
        };
        match answer {
            Answer::Reply(rtt) => {
                path.reached = true;
                path.rtt = Some(rtt);
                break;
            }
            Answer::TimeExceeded(router, rtt) => path.hops.push(Hop {
                ttl,
                router: Some(router),
                rtt: Some(rtt),
            }),
            // "Unreachable" from a router: the way ends there.
            Answer::Unreachable(router, rtt) => {
                path.hops.push(Hop {
                    ttl,
                    router: Some(router),
                    rtt: Some(rtt),
                });
                break;
            }
            Answer::Silence => path.hops.push(Hop {
                ttl,
                router: None,
                rtt: None,
            }),
        }
    }
    Ok(path)
}

/// Trace the way to every target, `concurrency` at a time; the paths come
/// back in the targets' order, and `progress(done, total)` follows the
/// targets finished.
pub async fn trace_paths(
    targets: &[Ipv4Addr],
    options: &TraceOptions,
    concurrency: usize,
    mut progress: impl FnMut(usize, usize),
) -> io::Result<Vec<Path>> {
    let total = targets.len();
    let slots = Arc::new(Semaphore::new(concurrency.max(1)));
    let mut traces = JoinSet::new();
    for (index, &target) in targets.iter().enumerate() {
        let slots = Arc::clone(&slots);
        let options = *options;
        traces.spawn(async move {
            let _slot = slots
                .acquire_owned()
                .await
                .expect("the semaphore is never closed");
            (index, traceroute(target, &options).await)
        });
    }
    let mut paths: Vec<Option<Path>> = vec![None; total];
    let mut done = 0;
    while let Some(result) = traces.join_next().await {
        if let Ok((index, path)) = result {
            paths[index] = Some(path?);
        }
        done += 1;
        progress(done, total);
    }
    Ok(paths.into_iter().flatten().collect())
}

/// What one probe got back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answer {
    /// The host's echo reply: reached.
    Reply(Duration),
    /// A router's "time exceeded": one more hop to go.
    TimeExceeded(Ipv4Addr, Duration),
    /// A router's "destination unreachable": the way is blocked there.
    Unreachable(Ipv4Addr, Duration),
    Silence,
}

/// An ICMP datagram socket connected to one target, with the error queue on.
struct ProbeSocket {
    fd: AsyncFd<Socket>,
    /// Marks this socket's probes, so a late answer to someone else's is
    /// not mistaken for ours.
    token: [u8; 8],
}

impl ProbeSocket {
    fn open(target: Ipv4Addr) -> io::Result<ProbeSocket> {
        let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::ICMPV4))?;
        set_recv_error(&socket)?;
        // Connected: `send` needs no address, `read` only ever returns what
        // the target sends, and the kernel reports every ICMP error about
        // our packets to this socket (an unconnected one hears of none).
        socket.connect(&SocketAddrV4::new(target, 0).into())?;
        socket.set_nonblocking(true)?;
        // SAFETY: the `Socket` owns its descriptor and moves into the
        // `AsyncFd`, so the descriptor stays open while it is registered.
        let fd = unsafe { AsyncFd::register(socket) }.map_err(io::Error::from)?;
        Ok(ProbeSocket {
            fd,
            token: RandomState::new()
                .hash_one(std::process::id())
                .to_be_bytes(),
        })
    }

    /// The probe data: the token, then the TTL, which the sequence number
    /// cannot carry (the kernel keeps the ICMP header of a queued error,
    /// only the data comes back).
    fn data(&self, ttl: u8) -> Vec<u8> {
        let mut data = self.token.to_vec();
        data.push(ttl);
        data
    }

    /// One echo request with this TTL, and what came back for it within
    /// `timeout`.
    async fn probe(&self, ttl: u8, timeout: Duration) -> io::Result<Answer> {
        let socket = self.fd.get_ref();
        socket.set_ttl_v4(u32::from(ttl))?;
        let packet = Echo {
            identifier: 0, // the kernel fills in the socket's own
            sequence: u16::from(ttl),
            data: self.data(ttl),
        }
        .request_bytes();
        socket.send(&packet)?;
        let sent = Instant::now();
        let deadline = sent + timeout;
        let mut buffer = [0u8; 1500];
        loop {
            // Readable: a reply; "error": something on the error queue.
            let ready = self.fd.ready(Interest::READABLE | Interest::ERROR);
            let Ok(ready) = tokio::time::timeout_at(deadline, ready).await else {
                return Ok(Answer::Silence);
            };
            let mut guard = ready?;
            // The kernel matched the error to this socket by the echo's
            // identifier, and with one probe in flight it can only be about
            // that one. (The quoted packet cannot say more: a router quotes
            // the first 8 bytes of what it dropped, the ICMP header, and the
            // kernel keeps that header for itself.)
            if let Some(error) = read_queued_error(socket.as_raw_fd(), &mut buffer)? {
                let rtt = sent.elapsed();
                match (error.kind, error.router) {
                    (icmp::TIME_EXCEEDED, Some(router)) => {
                        return Ok(Answer::TimeExceeded(router, rtt));
                    }
                    (icmp::DESTINATION_UNREACHABLE, Some(router)) => {
                        return Ok(Answer::Unreachable(router, rtt));
                    }
                    _ => continue, // not ICMP's doing: keep waiting
                }
            }
            match guard.try_io(|fd| fd.get_ref().read(&mut buffer)) {
                Ok(Ok(len)) => {
                    if Echo::parse_reply(&buffer[..len])
                        .is_some_and(|echo| echo.data == self.data(ttl))
                    {
                        return Ok(Answer::Reply(sent.elapsed()));
                    }
                }
                // The error the queue just told us about, told once more the
                // old way (as a failed read); it is spent now.
                Ok(Err(_already_seen)) => {}
                Err(_would_block) => {}
            }
        }
    }
}

/// Turn `IP_RECVERR` on: keep the ICMP errors about this socket's packets,
/// sender included, instead of dropping them (socket2 has no switch for it).
fn set_recv_error(socket: &Socket) -> io::Result<()> {
    let on: libc::c_int = 1;
    // SAFETY: a plain setsockopt with an int-sized value that outlives the call.
    let rc = unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            libc::SOL_IP,
            libc::IP_RECVERR,
            (&raw const on).cast(),
            size_of::<libc::c_int>() as libc::socklen_t,
        )
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// One entry of the error queue: which ICMP message came back about one of
/// our packets, and from which router.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct QueuedError {
    kind: u8,
    code: u8,
    router: Option<Ipv4Addr>,
}

/// Take the next entry off the socket's error queue, or `None` when it is
/// empty. `recvmsg` with `MSG_ERRQUEUE` returns what the router quoted of
/// the failed packet in the buffer (usually nothing past the headers) and,
/// as a control message of type `IP_RECVERR`, a `sock_extended_err` (the
/// ICMP type and code) followed by the sender's address; the `CMSG_*`
/// macros walk the control messages.
fn read_queued_error(fd: RawFd, buffer: &mut [u8]) -> io::Result<Option<QueuedError>> {
    let mut control = [0u8; 128]; // room for the one control message we expect
    let mut iov = libc::iovec {
        iov_base: buffer.as_mut_ptr().cast(),
        iov_len: buffer.len(),
    };
    // SAFETY: msghdr is plain data, and all zeros is a valid empty header.
    let mut header: libc::msghdr = unsafe { std::mem::zeroed() };
    header.msg_iov = &raw mut iov;
    header.msg_iovlen = 1;
    header.msg_control = control.as_mut_ptr().cast();
    header.msg_controllen = control.len() as _;
    // SAFETY: the header points at buffers that outlive the call.
    let len =
        unsafe { libc::recvmsg(fd, &raw mut header, libc::MSG_ERRQUEUE | libc::MSG_DONTWAIT) };
    if len < 0 {
        let err = io::Error::last_os_error();
        return if err.kind() == io::ErrorKind::WouldBlock {
            Ok(None)
        } else {
            Err(err)
        };
    }
    // The CMSG_* macros walk the control buffer the kernel just filled, and
    // only within the length it wrote back into the header; each pointer
    // they return is either null or points into that buffer.
    // SAFETY: `header` describes `control`, filled by the recvmsg above.
    let mut cmsg = unsafe { libc::CMSG_FIRSTHDR(&raw const header) };
    while !cmsg.is_null() {
        // SAFETY: `cmsg` is not null, so it points at a control message
        // header inside `control`.
        let (level, kind) = unsafe { ((*cmsg).cmsg_level, (*cmsg).cmsg_type) };
        if level == libc::SOL_IP && kind == libc::IP_RECVERR {
            // SAFETY: the data of an IP_RECVERR message is a sock_extended_err
            // (followed by the offender's address), still inside `control`;
            // `read_unaligned` because CMSG_DATA promises no alignment.
            let (ee, error) = unsafe {
                let ee = libc::CMSG_DATA(cmsg).cast::<libc::sock_extended_err>();
                (ee, ee.read_unaligned())
            };
            let router = (error.ee_origin == libc::SO_EE_ORIGIN_ICMP).then(|| {
                // SAFETY: with origin ICMP the kernel put the sender's
                // sockaddr_in right behind the struct, which is where
                // SO_EE_OFFENDER points; read unaligned for the same reason.
                let sender = unsafe {
                    libc::SO_EE_OFFENDER(ee)
                        .cast::<libc::sockaddr_in>()
                        .read_unaligned()
                };
                Ipv4Addr::from(sender.sin_addr.s_addr.to_ne_bytes())
            });
            return Ok(Some(QueuedError {
                kind: error.ee_type,
                code: error.ee_code,
                router,
            }));
        }
        // SAFETY: same header and buffer as CMSG_FIRSTHDR above; `cmsg` is a
        // control message inside it.
        cmsg = unsafe { libc::CMSG_NXTHDR(&raw const header, cmsg) };
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quick() -> TraceOptions {
        TraceOptions {
            max_hops: 3,
            timeout: Duration::from_millis(300),
        }
    }

    /// Our own machine: no router in between, reached on the first probe.
    #[tokio::test]
    async fn loopback_is_reached_without_a_router() {
        let path = traceroute(Ipv4Addr::LOCALHOST, &quick()).await.unwrap();
        assert!(path.reached, "{path:?}");
        assert!(path.hops.is_empty(), "{path:?}");
        assert!(path.rtt.is_some());
        assert_eq!(path.describe(), Vec::<String>::new());
    }

    /// An address reserved for documentation: never reached. With a route
    /// out, the first router answers "time exceeded" and the rest is
    /// silence or more routers; without one, the probe cannot be sent.
    #[tokio::test]
    async fn an_unreachable_address_is_not_reached() {
        let path = traceroute(Ipv4Addr::new(192, 0, 2, 1), &quick())
            .await
            .unwrap();
        assert!(!path.reached, "{path:?}");
        assert!(path.hops.len() <= 3, "{path:?}");
        for hop in &path.hops {
            assert_eq!(hop.router.is_some(), hop.rtt.is_some(), "{hop:?}");
        }
    }

    #[tokio::test]
    async fn paths_come_back_in_target_order() {
        let targets = [Ipv4Addr::new(192, 0, 2, 1), Ipv4Addr::LOCALHOST];
        let mut calls = Vec::new();
        let paths = trace_paths(&targets, &quick(), 2, |done, total| {
            calls.push((done, total));
        })
        .await
        .unwrap();
        assert_eq!(paths.len(), 2);
        assert_eq!(paths[0].target, targets[0]);
        assert_eq!(paths[1].target, targets[1]);
        assert!(paths[1].reached);
        assert_eq!(calls, [(1, 2), (2, 2)]);
    }

    #[test]
    fn describe_prints_a_star_for_silence() {
        let path = Path {
            target: Ipv4Addr::new(10, 1, 1, 1),
            hops: vec![
                Hop {
                    ttl: 1,
                    router: Some(Ipv4Addr::new(192, 168, 1, 254)),
                    rtt: Some(Duration::from_millis(2)),
                },
                Hop {
                    ttl: 2,
                    router: None,
                    rtt: None,
                },
            ],
            reached: true,
            rtt: None,
        };
        assert_eq!(path.describe(), ["192.168.1.254", "*"]);
        assert_eq!(
            path.routers().collect::<Vec<_>>(),
            [Ipv4Addr::new(192, 168, 1, 254)]
        );
    }
}
