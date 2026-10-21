"""Graphics-side data model and the JSON contract.

A :class:`NetworkMap` wraps the backend's ``Host`` objects, which are never
modified here, with the extra information a renderer needs: a role per host,
a 3D position per host, links between hosts, and scan metadata.

``NetworkMap.to_dict()`` produces the document the frontend reads::

    {
      "version": 1, "revision": 0, "generated_at": "2026-10-19T12:00:00+00:00",
      "scan": {"type": "ping_sweep", "network": "192.168.1.0/24", "scanner_ip": "192.168.1.50"},
      "hosts": [
        {"id": "192.168.1.10", "label": "nas.lan", "role": "host",
         "ip": "192.168.1.10", "hostname": "nas.lan",
         "open_ports": [{"number": 22, "proto": "tcp", "service": "ssh"}], "is_local": null, "extra": {},
         "position": [8.0, 0.0, 0.0]}
      ],
      "links": [{"source": "192.168.1.50", "target": "192.168.1.10", "kind": "subnet"}]
    }

Every field of ``Host`` is serialised generically by walking its dataclass
fields, so a field added to the backend model appears in the JSON (and in the
frontend's info panel) with no change on this side. ``id``, ``label``, ``role``
and ``position`` are added on top and are the only keys the frontend relies on.
"""

from __future__ import annotations

import json
import types
import typing
from dataclasses import dataclass, field, fields, is_dataclass
from datetime import date, datetime, timezone
from ipaddress import (
    IPv4Address,
    IPv4Interface,
    IPv4Network,
    IPv6Address,
    IPv6Interface,
    IPv6Network,
    ip_address,
)
from pathlib import Path
from typing import Any, Callable, Iterator

from pynmapper.model import Host

CONTRACT_VERSION = 1
ROLES = ("scanner", "gateway", "host")
DEFAULT_ROLE = "host"

# Keys the graphics layer adds to each serialised host, on top of Host's own fields.
GRAPHICS_KEYS = ("id", "label", "role", "position")

Position = tuple[float, float, float]
Layout = Callable[["NetworkMap"], dict[str, Position]]

_IP_TYPES = (IPv4Address, IPv6Address, IPv4Network, IPv6Network, IPv4Interface, IPv6Interface)


def host_id(host: Host) -> str:
    """Stable identifier for a host: its address as a string."""
    return str(host.ip)


def host_label(host: Host) -> str:
    """Hostname if known, else the address. Tolerates ``label`` being a method."""
    label = getattr(host, "label", None)
    if callable(label):
        label = label()
    return label if label else host_id(host)


def _now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds")


@dataclass
class Link:
    source: str
    target: str
    kind: str = "subnet"


@dataclass
class ScanInfo:
    type: str = "ping_sweep"
    network: str | None = None
    scanner_ip: str | None = None
    stage: str | None = None     # free text shown in the HUD while a scan is in progress ("resolving names…")


@dataclass
class NetworkMap:
    hosts: list[Host] = field(default_factory=list)
    links: list[Link] = field(default_factory=list)
    scan: ScanInfo = field(default_factory=ScanInfo)
    roles: dict[str, str] = field(default_factory=dict)          # host id -> role
    positions: dict[str, Position] = field(default_factory=dict)  # host id -> (x, y, z)
    generated_at: str = field(default_factory=_now)
    revision: int = 0

    # ----- queries -------------------------------------------------------
    def __len__(self) -> int:
        return len(self.hosts)

    def __iter__(self) -> Iterator[Host]:
        return iter(self.hosts)

    def ids(self) -> list[str]:
        return [host_id(h) for h in self.hosts]

    def get(self, id_: str) -> Host | None:
        for h in self.hosts:
            if host_id(h) == id_:
                return h
        return None

    def role_of(self, host: Host) -> str:
        return self.roles.get(host_id(host), DEFAULT_ROLE)

    def hosts_with_role(self, role: str) -> list[Host]:
        return [h for h in self.hosts if self.role_of(h) == role]

    # ----- mutation helpers -------------------------------------------------
    def set_role(self, host: Host, role: str) -> None:
        if role not in ROLES:
            raise ValueError(f"unknown role {role!r}; expected one of {ROLES}")
        self.roles[host_id(host)] = role

    def apply_layout(self, layout: Layout) -> NetworkMap:
        """Compute positions with ``layout`` and store them. Returns self."""
        self.positions = dict(layout(self))
        return self

    def touch(self) -> NetworkMap:
        """Bump the revision so a polling frontend knows to redraw."""
        self.revision += 1
        self.generated_at = _now()
        return self

    # ----- serialisation ----------------------------------------------------
    def to_dict(self) -> dict[str, Any]:
        return {
            "version": CONTRACT_VERSION,
            "revision": self.revision,
            "generated_at": self.generated_at,
            "scan": to_jsonable(self.scan),
            "hosts": [
                host_to_dict(h, self.role_of(h), self.positions.get(host_id(h)))
                for h in self.hosts
            ],
            "links": [to_jsonable(link) for link in self.links],
        }

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> NetworkMap:
        version = int(data.get("version", CONTRACT_VERSION))
        if version > CONTRACT_VERSION:
            raise ValueError(
                f"map was written with contract version {version}; "
                f"this code understands up to {CONTRACT_VERSION}"
            )
        net = cls(
            scan=dataclass_from_dict(ScanInfo, data.get("scan") or {}),
            links=[dataclass_from_dict(Link, link) for link in data.get("links", [])],
            generated_at=str(data.get("generated_at") or _now()),
            revision=int(data.get("revision", 0)),
        )
        for raw in data.get("hosts", []):
            host = host_from_dict(raw)
            net.hosts.append(host)
            id_ = host_id(host)
            if raw.get("role") and raw["role"] != DEFAULT_ROLE:
                net.roles[id_] = str(raw["role"])
            if raw.get("position") is not None:
                x, y, z = raw["position"]
                net.positions[id_] = (float(x), float(y), float(z))
        return net

    def to_json(self, indent: int | None = 2) -> str:
        return json.dumps(self.to_dict(), indent=indent)

    @classmethod
    def from_json(cls, text: str) -> NetworkMap:
        return cls.from_dict(json.loads(text))

    def save(self, path: str | Path) -> Path:
        path = Path(path)
        path.write_text(self.to_json(), encoding="utf-8")
        return path

    @classmethod
    def load(cls, path: str | Path) -> NetworkMap:
        return cls.from_json(Path(path).read_text(encoding="utf-8"))


# ----- generic dataclass <-> JSON helpers ----------------------------------------

def to_jsonable(value: Any) -> Any:
    """Recursively convert dataclasses, ip addresses, dates, sets... to JSON-safe values."""
    if is_dataclass(value) and not isinstance(value, type):
        return {f.name: to_jsonable(getattr(value, f.name)) for f in fields(value)}
    if isinstance(value, _IP_TYPES):
        return str(value)
    if isinstance(value, dict):
        return {str(k): to_jsonable(v) for k, v in value.items()}
    if isinstance(value, (list, tuple, set, frozenset)):
        return [to_jsonable(v) for v in value]
    if isinstance(value, (datetime, date)):
        return value.isoformat()
    if value is None or isinstance(value, (str, int, float, bool)):
        return value
    return str(value)


def host_to_dict(host: Host, role: str = DEFAULT_ROLE, position: Position | None = None) -> dict[str, Any]:
    """Serialise one host: graphics keys first, then every Host field, generically."""
    data: dict[str, Any] = {
        "id": host_id(host),
        "label": host_label(host),
        "role": role,
    }
    data.update(to_jsonable(host))
    data["position"] = [float(c) for c in position] if position is not None else None
    return data


def host_from_dict(data: dict[str, Any]) -> Host:
    """Rebuild a Host from its serialised form, ignoring graphics-only keys."""
    return dataclass_from_dict(Host, data)


def dataclass_from_dict(cls: type, data: dict[str, Any]) -> Any:
    """Construct ``cls`` from ``data`` using its field annotations to coerce values.

    Unknown keys are ignored so that extra keys (``id``, ``label``, ... or keys
    written by a newer version) never break loading.
    """
    hints = typing.get_type_hints(cls)
    kwargs = {}
    for f in fields(cls):
        if f.name in data:
            kwargs[f.name] = _coerce(data[f.name], hints.get(f.name, Any))
    return cls(**kwargs)


def _coerce(value: Any, annotation: Any) -> Any:
    """Best-effort conversion of a JSON value back to the annotated Python type."""
    if value is None:
        return None
    origin = typing.get_origin(annotation)
    args = typing.get_args(annotation)
    if origin in (types.UnionType, typing.Union):
        for arg in args:
            if arg is type(None):
                continue
            try:
                return _coerce(value, arg)
            except (TypeError, ValueError):
                continue
        return value
    if origin in (list, set, tuple, frozenset) and args and isinstance(value, (list, tuple)):
        return [_coerce(v, args[0]) for v in value]
    if isinstance(annotation, type):
        if issubclass(annotation, (IPv4Address, IPv6Address)) and isinstance(value, str):
            return ip_address(value)
        if is_dataclass(annotation) and isinstance(value, dict):
            return dataclass_from_dict(annotation, value)
    return value
