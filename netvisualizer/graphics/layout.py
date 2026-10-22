"""Layouts assign a 3D position to every host in a :class:`NetworkMap`.

A layout is any callable ``(NetworkMap) -> dict[host_id, (x, y, z)]``. New
ones (a tree once traceroute exists, force-directed, ...) can be added here
and passed to ``from_hosts(layout=...)`` without touching anything else. The
viewer treats y as up.
"""

from __future__ import annotations

import math
from ipaddress import ip_address

from netvisualizer.model import Host

from .model import NetworkMap, Position, host_id

SPACING = 3.0      # world units between neighbours on a ring
MIN_RADIUS = 8.0   # radius of the innermost ring
RING_GAP = 6.0     # radial distance between concentric rings
RING_RISE = 1.5    # each further ring floats a little higher than the last


def _address_key(host: Host) -> tuple[int, int]:
    addr = ip_address(str(host.ip))
    return (addr.version, int(addr))


def ring_plan(count: int) -> list[int]:
    """How many hosts go on each concentric ring, innermost first."""
    plan: list[int] = []
    remaining = count
    ring = 0
    while remaining > 0:
        radius = MIN_RADIUS + ring * RING_GAP
        capacity = max(1, math.floor(2 * math.pi * radius / SPACING))
        take = min(capacity, remaining)
        plan.append(take)
        remaining -= take
        ring += 1
    return plan


def star_ring(net: NetworkMap) -> dict[str, Position]:
    """Scanner at the origin; everyone else on concentric rings in address order.

    The gateway, if any, is placed first on the innermost ring. Rings beyond the
    first are rotated by half a step so nodes do not line up radially.
    """
    positions: dict[str, Position] = {}
    scanners = net.hosts_with_role("scanner")
    others = [h for h in net.hosts if net.role_of(h) != "scanner"]

    if scanners:
        positions[host_id(scanners[0])] = (0.0, 0.0, 0.0)
        others.extend(scanners[1:])  # should not happen, but never lose a host

    others.sort(key=lambda h: (net.role_of(h) != "gateway", _address_key(h)))

    index = 0
    for ring, count in enumerate(ring_plan(len(others))):
        radius = MIN_RADIUS + ring * RING_GAP
        y = ring * RING_RISE
        step = 2 * math.pi / count
        offset = step / 2 if ring % 2 else 0.0
        for slot in range(count):
            angle = offset + slot * step
            positions[host_id(others[index])] = (
                round(radius * math.cos(angle), 3),
                y,
                round(radius * math.sin(angle), 3),
            )
            index += 1
    return positions


