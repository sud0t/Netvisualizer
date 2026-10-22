"""Graphics layer for Netvisualizer, Python side.

Everything here consumes ``netvisualizer.model.Host`` objects (produced by the
networking code) and turns them into a :class:`NetworkMap`, the single JSON
document the native viewer in ``viewer/`` (Rust) draws. Nothing in this
package performs network I/O other than serving that document; the only place
that knows about the backend's output shape is :mod:`netvisualizer.graphics.adapters`.

Typical use from the entry point::

    hosts = asyncio.run(netutils.async_ping_sweep(my_ip, 24))
    net_map = graphics.from_hosts(hosts, scanner_ip=my_ip, network=f"{my_ip}/24")
    server = graphics.MapServer(net_map).start()   # the viewer follows server.url
"""

from .adapters import apply_hostnames, from_hosts, from_ping_sweep
from .layout import star_ring
from .model import CONTRACT_VERSION, Link, NetworkMap, ScanInfo, host_id
from .server import MapServer

__all__ = [
    "CONTRACT_VERSION",
    "Link",
    "MapServer",
    "NetworkMap",
    "ScanInfo",
    "apply_hostnames",
    "from_hosts",
    "from_ping_sweep",
    "host_id",
    "star_ring",
]
