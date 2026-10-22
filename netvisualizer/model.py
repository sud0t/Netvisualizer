# - model.py defines what a host looks like. It is the one shape shared by the
#   networking code (which fills it in) and the graphics code (which reads it).

from dataclasses import dataclass, field
from ipaddress import IPv4Address, IPv6Address


@dataclass
class Port:
    number: int
    proto: str                       # "tcp" or "udp"
    service: str | None = None       # well-known name for the port, e.g. "ssh" for 22/tcp


@dataclass
class Host:
    ip: IPv4Address | IPv6Address
    hostname: str | None = None
    open_ports: list[Port] = field(default_factory=list)
    is_local: bool | None = None     # inside the scanner's own subnet (netutils.mark_local); None = unknown
    extra: dict = field(default_factory=dict)   # anything new goes here first; the map shows it as is

    @property
    def label(self):
        return self.hostname or str(self.ip)
