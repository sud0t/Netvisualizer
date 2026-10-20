"""Fake data for developing the frontend without touching the network."""

from __future__ import annotations

import random
from ipaddress import ip_network

from pynmapper.model import Host, Port

from .adapters import from_hosts
from .model import NetworkMap

_NAMES = [
    "nas", "printer", "desktop", "laptop", "phone", "tv", "pi", "cam",
    "switch", "ap", "server", "tablet", "console", "thermostat", "speaker",
]
_VENDORS = ["Intel", "Raspberry Pi", "Apple", "Samsung", "TP-Link", "HP", "Ubiquiti", "Sony"]
_SERVICES = [
    (22, "tcp"), (80, "tcp"), (443, "tcp"), (53, "udp"), (139, "tcp"),
    (445, "tcp"), (631, "tcp"), (8080, "tcp"), (5353, "udp"), (3389, "tcp"),
    (8443, "tcp"), (1900, "udp"),
]


def demo_map(count: int = 25, seed: int = 1, network: str = "192.168.1.0/24") -> NetworkMap:
    """A deterministic fake LAN: a scanner, a gateway and ``count`` other hosts."""
    rng = random.Random(seed)
    addresses = list(ip_network(network).hosts())
    gateway_ip, scanner_ip = addresses[0], addresses[min(49, len(addresses) - 1)]
    pool = [a for a in addresses[1:] if a != scanner_ip]
    picked = sorted(rng.sample(pool, k=min(count, len(pool))))

    hosts = [
        Host(
            ip=scanner_ip,
            hostname="pynmapper-host.lan",
            open_ports=[Port(22, "tcp")],
            is_local=True,
            extra={"note": "this machine ran the scan"},
        ),
        Host(
            ip=gateway_ip,
            hostname="gateway.lan",
            open_ports=[Port(53, "udp"), Port(80, "tcp"), Port(443, "tcp")],
            extra={"vendor": "TP-Link"},
        ),
    ]
    for n, addr in enumerate(picked, start=1):
        host = Host(ip=addr)
        if rng.random() < 0.7:
            host.hostname = f"{rng.choice(_NAMES)}-{n}.lan"
        for number, proto in rng.sample(_SERVICES, k=rng.randint(0, 4)):
            host.open_ports.append(Port(number, proto))
        if rng.random() < 0.6:
            host.extra["vendor"] = rng.choice(_VENDORS)
        if rng.random() < 0.8:
            host.extra["latency_ms"] = round(rng.uniform(0.3, 40.0), 1)
        hosts.append(host)

    return from_hosts(
        hosts,
        scanner_ip=scanner_ip,
        gateway_ip=gateway_ip,
        network=network,
        scan_type="demo",
    )
