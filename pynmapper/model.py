# - model.py defines what a host looks like.

from dataclasses import dataclass, field
from ipaddress import IPv4Address, IPv6Address, ip_address, ip_network


@dataclass
class Port:
    number: int
    proto: str



@dataclass
class Host:
    ip: IPv4Address | IPv6Address
    hostname: str | None = None
    open_ports: list[Port] = field(default_factory=list)
    is_local: bool | None = None # can use network address comparison **need subnet mask first**
    extra: dict = field(default_factory=dict)

    @property
    def label(self):
        return self.hostname or str(self.ip)


