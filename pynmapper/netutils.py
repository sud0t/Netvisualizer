#!/usr/bin/env python3
"""Network discovery for Pynmapper.

- netutils.py finds hosts and fills in their values (``Host`` objects from model.py).
- __main__.py decides which of these steps run and in what order.
- graphics/ draws whatever it is given and never imports this module.

Everything here works without root: hosts are found with the system ``ping``
command, names come from reverse DNS, and open ports are found by completing a
TCP connection. Scapy is used only to read the routing table (which interface
we are on, its subnet and gateway); if it is missing, we fall back to guessing.

The long-running steps are coroutines so hundreds of pings or connections can
be in flight at once; ``asyncio.run(async_ping_sweep("192.168.1.0/24"))`` is
all a caller needs.
"""

from __future__ import annotations

import asyncio
import logging
import socket
import subprocess
import sys
import warnings
from collections.abc import Callable, Iterable
from dataclasses import dataclass
from ipaddress import IPv4Address, IPv4Network, ip_address, ip_network
from itertools import batched

from pynmapper.model import Host, Port

log = logging.getLogger(__name__)

Progress = Callable[[int, int], None]   # (done, total) callback for the long stages

# TCP ports worth trying when the caller gives no list: common LAN services.
COMMON_TCP_PORTS = (
    21, 22, 23, 25, 53, 80, 110, 111, 135, 139, 143, 389, 443, 445, 465, 515,
    548, 554, 587, 631, 636, 993, 995, 1433, 1521, 1883, 2049, 3000, 3306,
    3389, 5000, 5060, 5432, 5900, 6379, 8000, 8080, 8443, 8883, 8888, 9000,
    9090, 9100, 27017,
)


# ---------------------------------------------------------------------------
# Where are we? Interface, subnet and gateway from the routing table.
# ---------------------------------------------------------------------------

@dataclass
class LocalNetwork:
    """One of this machine's IPv4 networks: the interface, our address on it, the subnet, its gateway."""
    interface: str
    ip: IPv4Address
    network: IPv4Network
    gateway: IPv4Address | None = None


def get_local_ip() -> str:
    """The address of the interface that reaches the internet (or 127.0.0.1 when offline)."""
    # Source - https://stackoverflow.com/a/28950776
    # Retrieved 2026-09-21, License - CC BY-SA 4.0
    # Connecting a UDP socket sends nothing, but the kernel still picks the
    # interface it would use, and getsockname() tells us its address.
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    s.settimeout(0)
    try:
        s.connect(("10.254.254.254", 1))  # doesn't even have to be reachable
        ip = s.getsockname()[0]
    except OSError:
        ip = "127.0.0.1"
    finally:
        s.close()
    return ip


def _routes() -> list[tuple]:
    """The kernel routing table as scapy sees it: (net, mask, gateway, iface, source, metric) rows.

    ``net`` and ``mask`` are integers, the rest strings. Returns [] when scapy
    is not installed or cannot read the table on this platform.
    """
    logging.getLogger("scapy.runtime").setLevel(logging.ERROR)  # scapy warns about every odd interface
    with warnings.catch_warnings():
        warnings.simplefilter("ignore")  # its TLS layer imports deprecated crypto on load
        try:
            import scapy.route  # noqa: F401  (fills in conf.route for this platform)
            from scapy.config import conf
        except Exception as exc:  # noqa: BLE001  scapy raises all sorts when a platform is unsupported
            log.debug("routing table unavailable: %s", exc)
            return []
    if conf.route is None:
        return []
    return [(net, mask, str(gw), str(iface), str(src), metric) for net, mask, gw, iface, src, metric in conf.route.routes]


def list_local_networks() -> list[LocalNetwork]:
    """Every subnet this machine is directly on, loopback excluded.

    A route with no gateway ("0.0.0.0") is a directly connected network: packets
    for it leave the interface without a router, so it is a subnet we can
    sweep. The default route (destination 0.0.0.0/0) of the same interface
    tells us the gateway, which the map draws as its own kind of node.
    """
    routes = _routes()
    gateways: dict[str, IPv4Address] = {}
    for net, mask, gw, iface, _src, _metric in sorted(routes, key=lambda r: r[5]):  # lowest metric wins
        if net == 0 and mask == 0 and gw != "0.0.0.0" and iface not in gateways:
            gateways[iface] = IPv4Address(gw)

    found: list[LocalNetwork] = []
    seen: set[tuple[str, IPv4Network]] = set()
    for net, mask, gw, iface, src, _metric in routes:
        if gw != "0.0.0.0" or mask in (0, 0xFFFFFFFF):  # via a router, the default, or a single-address route
            continue
        network = ip_network(f"{IPv4Address(net)}/{IPv4Address(mask)}")
        source = IPv4Address(src)
        if network.is_loopback or network.is_multicast or network.is_link_local or source not in network:
            continue
        if (iface, network) in seen:
            continue
        seen.add((iface, network))
        found.append(LocalNetwork(iface, source, network, gateways.get(iface)))
    return found


def get_local_network(interface: str | None = None) -> LocalNetwork:
    """The network to scan by default: the one our internet traffic uses, or the one on ``interface``.

    Without a readable routing table we still know our address from
    :func:`get_local_ip`, and assume a /24 around it.
    """
    networks = list_local_networks()
    if interface:
        for candidate in networks:
            if candidate.interface == interface:
                return candidate
        known = sorted({n.interface for n in networks}) or ["none found"]
        raise ValueError(f"no IPv4 network on interface {interface!r} (known: {', '.join(known)})")

    ip = IPv4Address(get_local_ip())
    for candidate in networks:
        if candidate.ip == ip:
            return candidate
    if networks:
        return networks[0]
    log.warning("could not read the routing table; assuming %s is on a /24", ip)
    return LocalNetwork("?", ip, ip_network(f"{ip}/24", strict=False))


def mark_local(hosts: Iterable[Host], local: IPv4Network) -> list[Host]:
    """Set ``host.is_local``: True when the address is inside our own subnet, i.e. reachable without a router."""
    hosts = list(hosts)
    for host in hosts:
        host.is_local = host.ip in local
    return hosts


# ---------------------------------------------------------------------------
# Finding hosts: ping sweep.
# ---------------------------------------------------------------------------

def ping_command(ip: str, timeout: float = 1.0) -> list[str]:
    """The system ping invocation for one echo request; the flags differ per OS."""
    if sys.platform == "win32":
        return ["ping", "-n", "1", "-w", str(int(timeout * 1000)), ip]
    if sys.platform == "darwin":
        return ["ping", "-c", "1", "-W", str(int(timeout * 1000)), ip]   # macOS: -W is in milliseconds
    return ["ping", "-c", "1", "-W", f"{timeout:g}", ip]                  # iputils: seconds


def ping(ip: str, timeout: float = 1.0) -> bool:
    """True if ``ip`` answered one echo request. The simple, blocking version of :func:`async_ping`."""
    result = subprocess.run(ping_command(ip, timeout), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    return result.returncode == 0


async def async_ping(ip: str, timeout: float = 1.0) -> bool:
    """True if ``ip`` answered one echo request within ``timeout`` seconds.

    The system ping is used because sending ICMP ourselves needs root. Its exit
    code says everything: 0 = got a reply, 1 = no reply, 2 = error (bad address,
    no route), so the text output is not read at all.
    """
    proc = await asyncio.create_subprocess_exec(
        *ping_command(ip, timeout), stdout=asyncio.subprocess.DEVNULL, stderr=asyncio.subprocess.DEVNULL
    )
    try:
        await asyncio.wait_for(proc.wait(), timeout + 5)  # the +5 is only in case ping itself hangs
    except TimeoutError:
        proc.kill()
        await proc.wait()
        return False
    return proc.returncode == 0


def _as_network(network: str | IPv4Network, prefix_length: int | None = None) -> IPv4Network:
    """Accept "192.168.1.0/24", an IPv4Network, or the older ("192.168.1.5", 24) pair."""
    if prefix_length is not None:
        return ip_network(f"{network}/{prefix_length}", strict=False)
    return ip_network(network, strict=False)


async def async_ping_sweep(
    network: str | IPv4Network,
    prefix_length: int | None = None,
    *,
    timeout: float = 1.0,
    concurrency: int = 254,
    progress: Progress | None = None,
) -> list[Host]:
    """Ping every address in ``network`` and return a Host for each one that answered.

    ``concurrency`` is how many ping processes run at once. Addresses are probed
    in batches of that size, so even a /16 never creates thousands of tasks up
    front; a /24 is a single batch and takes about ``timeout`` seconds in total.
    """
    net = _as_network(network, prefix_length)
    addresses = list(net.hosts()) or [net.network_address]  # a /32 has no "hosts" but is still worth a ping
    alive: list[Host] = []
    done = 0
    for batch in batched(addresses, max(1, concurrency)):
        answers = await asyncio.gather(*(async_ping(str(addr), timeout) for addr in batch))
        alive.extend(Host(ip=addr) for addr, up in zip(batch, answers) if up)
        done += len(batch)
        if progress:
            progress(done, len(addresses))
    log.info("%s: %d of %d addresses answered", net, len(alive), len(addresses))
    return alive


def ping_sweep(network: str | IPv4Network, prefix_length: int | None = None, **kwargs) -> list[Host]:
    """Blocking convenience wrapper around :func:`async_ping_sweep`."""
    return asyncio.run(async_ping_sweep(network, prefix_length, **kwargs))


# ---------------------------------------------------------------------------
# Naming hosts: reverse DNS.
# ---------------------------------------------------------------------------

async def resolve_hostnames(hosts: Iterable[Host], *, concurrency: int = 32, overwrite: bool = False) -> list[Host]:
    """Fill in ``host.hostname`` from reverse DNS, in place, for every host that has none.

    ``socket.gethostbyaddr`` blocks for the resolver's full timeout on every
    miss (seconds each on a LAN without reverse DNS), so the lookups run in
    threads, ``concurrency`` at a time, instead of one after another.
    """
    hosts = list(hosts)
    semaphore = asyncio.Semaphore(concurrency)

    async def lookup(host: Host) -> None:
        async with semaphore:
            try:
                name, _aliases, _addresses = await asyncio.to_thread(socket.gethostbyaddr, str(host.ip))
            except OSError:  # socket.herror / gaierror: no name for this address
                return
            host.hostname = name

    await asyncio.gather(*(lookup(h) for h in hosts if overwrite or not h.hostname))
    log.info("%d of %d hosts have a name", sum(1 for h in hosts if h.hostname), len(hosts))
    return hosts


def get_host_names(hosts: list[str]) -> list[tuple]:
    """Older, blocking form: the raw ``gethostbyaddr`` tuples for the addresses that resolve.

    Kept for callers that still want it; ``graphics.apply_hostnames`` understands
    its output. New code should use :func:`resolve_hostnames`, which is concurrent
    and writes straight into the Host objects.
    """
    hostnames = []
    for host in hosts:
        try:
            hostnames.append(socket.gethostbyaddr(host))
        except OSError:
            log.debug("%s cannot be resolved", host)
    return hostnames


# ---------------------------------------------------------------------------
# Open ports: TCP connect scan.
# ---------------------------------------------------------------------------

def service_name(port: int, proto: str = "tcp") -> str | None:
    """The well-known name for a port ("ssh" for 22/tcp) from the system services table, if any."""
    try:
        return socket.getservbyport(port, proto)
    except OSError:
        return None


async def tcp_connect(ip: str, port: int, timeout: float = 1.0) -> bool:
    """True if a TCP connection to ``ip:port`` succeeds within ``timeout`` seconds.

    A "connect scan": the full handshake completes like any client's would, so it
    needs no privileges (a SYN scan with scapy would), at the cost of being
    visible to the host. A refused connection means the port is closed; a
    timeout means closed or filtered; either way it is not open.
    """
    try:
        _reader, writer = await asyncio.wait_for(asyncio.open_connection(ip, port), timeout)
    except (OSError, TimeoutError):
        return False
    writer.close()
    try:
        await writer.wait_closed()
    except OSError:
        pass
    return True


async def async_port_scan(
    hosts: Iterable[Host],
    ports: Iterable[int] = COMMON_TCP_PORTS,
    *,
    timeout: float = 1.0,
    concurrency: int = 256,
    progress: Progress | None = None,
) -> list[Host]:
    """Try a TCP connection to every (host, port) pair; record the open ones in ``host.open_ports``.

    Only TCP: there is no reliable way to tell an open UDP port from a silent one
    without root. Ports already listed on a host are not added twice.
    """
    hosts = list(hosts)
    ports = sorted(set(ports))
    pairs = [(host, port) for host in hosts for port in ports]
    done = 0
    for batch in batched(pairs, max(1, concurrency)):
        results = await asyncio.gather(*(tcp_connect(str(host.ip), port, timeout) for host, port in batch))
        for (host, port), is_open in zip(batch, results):
            if is_open and not any(p.number == port and p.proto == "tcp" for p in host.open_ports):
                host.open_ports.append(Port(port, "tcp", service_name(port)))
        done += len(batch)
        if progress:
            progress(done, len(pairs))
    for host in hosts:
        host.open_ports.sort(key=lambda p: (p.proto, p.number))
    log.info("%d open ports on %d hosts (%d ports tried each)", sum(len(h.open_ports) for h in hosts), len(hosts), len(ports))
    return hosts


# TODO: Layer 2 host enumeration (ARP) with scapy: finds hosts that ignore ping, but needs root.
# TODO: traceroute, so the map can show real paths instead of a star from the scanner.
