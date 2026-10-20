"""Graphics layer for Pynmapper.

Everything here consumes ``pynmapper.model.Host`` objects (produced by the
networking code) and turns them into a :class:`NetworkMap`, the single JSON
document the browser frontend renders. Nothing in this package performs
network I/O; the only place that knows about the backend's output shape is
:mod:`pynmapper.graphics.adapters`.

Typical use from the entry point::

    hosts = asyncio.run(netutils.async_ping_sweep(my_ip, 24))
    net_map = graphics.from_hosts(hosts, scanner_ip=my_ip, network=f"{my_ip}/24")
    graphics.serve(net_map)          # opens the browser, blocks until Ctrl-C
"""

from .adapters import apply_hostnames, from_hosts, from_ping_sweep
from .demo import demo_map
from .layout import LAYOUTS, star_ring
from .model import CONTRACT_VERSION, Link, NetworkMap, ScanInfo, host_id
from .server import MapServer, serve

__all__ = [
    "CONTRACT_VERSION",
    "LAYOUTS",
    "Link",
    "MapServer",
    "NetworkMap",
    "ScanInfo",
    "apply_hostnames",
    "demo_map",
    "from_hosts",
    "from_ping_sweep",
    "host_id",
    "serve",
    "star_ring",
]
