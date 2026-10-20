"""Adapters turn whatever the backend returns into a :class:`NetworkMap`.

This is the one module that is expected to change when the backend's output
shape changes. Everything downstream (layouts, server, frontend) only ever
sees a ``NetworkMap``.
"""

from __future__ import annotations

from collections.abc import Iterable, Mapping
from ipaddress import IPv4Address, IPv6Address, ip_address
from typing import Any

from pynmapper.model import Host

from .layout import star_ring
from .model import Layout, Link, NetworkMap, ScanInfo, host_id

AddressLike = str | IPv4Address | IPv6Address


def _normalise(addr: AddressLike | None) -> str | None:
    return None if addr is None else str(ip_address(str(addr)))


def _as_host(item: Host | AddressLike) -> Host:
    return item if isinstance(item, Host) else Host(ip=ip_address(str(item)))


def _dedupe(hosts: Iterable[Host]) -> list[Host]:
    seen: set[str] = set()
    unique: list[Host] = []
    for h in hosts:
        id_ = host_id(h)
        if id_ not in seen:
            seen.add(id_)
            unique.append(h)
    return unique


def from_hosts(
    hosts: Iterable[Host],
    scanner_ip: AddressLike | None = None,
    network: str | None = None,
    gateway_ip: AddressLike | None = None,
    scan_type: str = "ping_sweep",
    layout: Layout | None = star_ring,
) -> NetworkMap:
    """Build a map from ``Host`` objects as returned by the networking code.

    - ``scanner_ip``: the machine that ran the scan. It becomes the centre
      node (role ``scanner``); a Host is created for it if it is not in
      ``hosts``. Links form a star from it to every other host, which is the
      only topology a ping sweep implies.
    - ``gateway_ip``: optional; marks that host with role ``gateway``.
    - ``layout``: position function, or ``None`` to leave positions unset.
    """
    unique = _dedupe(hosts)
    scanner_id = _normalise(scanner_ip)
    gateway_id = _normalise(gateway_ip)

    if scanner_id and scanner_id not in {host_id(h) for h in unique}:
        unique.insert(0, Host(ip=ip_address(scanner_id)))

    net = NetworkMap(
        hosts=unique,
        scan=ScanInfo(type=scan_type, network=network, scanner_ip=scanner_id),
    )
    for h in unique:
        id_ = host_id(h)
        if id_ == scanner_id:
            net.roles[id_] = "scanner"
        elif id_ == gateway_id:
            net.roles[id_] = "gateway"

    if scanner_id:
        net.links = [Link(scanner_id, host_id(h)) for h in unique if host_id(h) != scanner_id]

    if layout is not None:
        net.apply_layout(layout)
    return net


def from_ping_sweep(
    results: Iterable[Host | AddressLike],
    scanner_ip: AddressLike | None = None,
    hostnames: Iterable[Any] | Mapping[str, str] | None = None,
    **kwargs: Any,
) -> NetworkMap:
    """Build a map from a ping sweep result.

    Accepts ``Host`` objects, ``IPv4Address``/``IPv6Address`` objects or plain
    strings, in any mix, so it works with both the current sweep (which
    returns Hosts) and the older one (which returned addresses).
    ``hostnames`` is passed to :func:`apply_hostnames`. Remaining keyword
    arguments go to :func:`from_hosts`.
    """
    hosts = [_as_host(item) for item in results]
    if hostnames:
        apply_hostnames(hosts, hostnames)
    return from_hosts(hosts, scanner_ip=scanner_ip, **kwargs)


def apply_hostnames(
    hosts: Iterable[Host],
    resolved: Iterable[Any] | Mapping[str, str],
    overwrite: bool = False,
) -> list[Host]:
    """Fill in ``hostname`` on hosts from reverse-DNS results, matched by IP.

    ``resolved`` is either a mapping ``{ip: hostname}`` or an iterable of
    ``socket.gethostbyaddr()`` tuples ``(hostname, aliases, [ips])``, which is
    what ``netutils.get_host_names`` returns. That function skips hosts that
    fail to resolve, so its output is shorter than its input and must never be
    matched by position; this matches on the addresses inside each tuple.

    Hosts that already have a hostname are left alone unless ``overwrite``.
    Modifies the Host objects in place and returns them as a list.
    """
    by_ip: dict[str, str] = {}
    if isinstance(resolved, Mapping):
        for ip, name in resolved.items():
            by_ip[_normalise(ip)] = str(name)
    else:
        for entry in resolved:
            name, _aliases, ips = entry
            for ip in ips:
                by_ip[_normalise(ip)] = str(name)

    hosts = list(hosts)
    for h in hosts:
        if h.hostname and not overwrite:
            continue
        name = by_ip.get(host_id(h))
        if name:
            h.hostname = name
    return hosts
