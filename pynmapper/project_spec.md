# 3D graphical network mapper in Python (maybe Rust as well in the future)

## Your role
You are going to be handling the graphics side of this project, do not touch the actual network mapping part as that is what I am using to learn, even if there are vast optimizations to be made for the actual network mapping still do not touch it. Your goal is to write the graphics so that they are compatible with what I write for the networking part.

### General instructions (open to recommendation)
- Create a 3D map of a given network based off the function that was run
- Scapy is not wired in yet so the first graphics target is the output of `async_ping_sweep` (plus `get_host_names`)
- This is very rough draft, so much is open to change and iteration from you or me, if there are better ways to do something (in terms of graphics) do not hesitate to recommend the better option


## Features
- Able to move around 3D space with wasd
- Host name should be preffered over IP address (if one was provided)
- Each host is a node that can be clicked on to reveal more information (such as open ports, the ports type, etc)
- The graphics should be modular and adaptable to change so that if the backend of this project changes (which it will)--the re-write to the graphics side is not overbearing
- The graphics need to be up to modern standards, if that means using something other than VPython then so be it


## Decisions made (2026-10-19)

### Rendering engine: three.js in the browser (VPython dropped)
- Python only produces data; a small buildless HTML/JS page (ES modules, vendored three.js pinned by version, no Node toolchain) renders it.
- Reasons: best visuals and free HTML info panels, keeps all graphics work separate from the networking code, VPython is browser based anyway but with dated visuals and global scene state (and it currently fails to import in `.venv` on Python 3.14 because setuptools is missing).
- Alternatives considered and rejected for now: Panda3D (pure Python engine, good but heavier), Ursina, VPython as originally spec'd.

### Launching
- `python -m pynmapper.graphics --demo` renders ~25 fake hosts with no network needed (day-to-day graphics development).
- `pynmapper.py` gets a small call after the sweep that builds the map and opens the browser. `netutils.py` is never touched by the graphics side.
- Also planned: `--json FILE` to load a saved map and `--save FILE` to dump one, so real scans can be replayed without a network.

### Hostnames
- `netutils.get_host_names(list[str])` returns the raw `socket.gethostbyaddr` tuples `(hostname, aliases, [ips])` and skips hosts that do not resolve, so its output is shorter than its input and not positionally aligned.
- The graphics adapter therefore keys hostnames by the IP inside each tuple's third element rather than by list position. This works as is; no change needed on the networking side.
- (Noted, left alone on purpose: `netutils.main()` uses `i` after the `except` branch, so a failed sweep raises `NameError`; `get_host_names` is serial and blocking so it is fine for a /24 but slow for larger ranges.)


## Shared data model: the one contract between backend and graphics

The only coupling between the networking code and the graphics code is a `Host` object (and the JSON it serializes to). The networking code fills it in, the graphics code reads it, and neither imports the other.

### `pynmapper/model.py` (written by Kyler, exists): `Host` and `Port`
Lives in a neutral module, not in `netutils.py` and not in `graphics/`. Current shape (the graphics code consumes exactly this; `label` is a property):

```python
from dataclasses import dataclass, field
from ipaddress import IPv4Address

@dataclass
class Port:
    number: int
    proto: str                # tcp / udp

@dataclass
class Host:
    ip: IPv4Address | IPv6Address
    hostname: str | None = None
    open_ports: list[Port] = field(default_factory=list)
    is_local: bool | None = None
    extra: dict = field(default_factory=dict)   # anything new goes here first

    @property
    def label(self) -> str:
        return self.hostname or str(self.ip)
```

Notes:
- `async_ping_sweep` now returns `list[Host]`; the graphics adapter `from_hosts` takes that list plus the scanner IP.
- Fields added to `Host` or `Port` later (service, state, mac, ...) show up in the JSON and the info panel automatically.
- `get_host_names` can stay as is (graphics matches its tuples by IP via `apply_hostnames`), or become a netutils function that sets `host.hostname` directly.

### Why
- A sweep result today is a bare list of IPs, with hostnames in a separate shorter list that has to be re-aligned by hand. `Host` keeps ip, hostname and ports together.
- When scapy port scanning lands, results append to `host.ports` instead of a third parallel list.
- The graphics side then needs to know exactly one shape, and its adapter becomes nearly empty.


## Graphics architecture (approved plan, starts after `model.py` exists)
Full plan file: `~/.claude/plans/read-the-project-spec-md-and-breezy-owl.md`

```
pynmapper/
  model.py                  # Host / Port (written by Kyler, shared contract)
  __main__.py               # entry point (argparse); small edit: after sweep, build map + serve
  netutils.py               # UNTOUCHED
  graphics/
    __init__.py             # public API: NetworkMap, Link, from_hosts, from_ping_sweep, serve, MapServer
    model.py                # NetworkMap/Link dataclasses + to_dict()/from_dict(); imports Host/Port from pynmapper.model
    adapters.py             # from_hosts(list[Host], scanner_ip, ...) and from_ping_sweep(ips, scanner_ip, hostnames=None, ...)
    layout.py               # positions: scanner at origin, hosts on a ring/shells; pluggable for tree/force later
    demo.py                 # fake NetworkMap for --demo
    server.py               # stdlib ThreadingHTTPServer: static files + /api/map (with revision); MapServer.update()
    __main__.py             # CLI: --demo | --json FILE, --port, --no-browser, --save FILE
    web/
      index.html, style.css
      app.js                # fetch /api/map, poll revision every 2 s, rebuild on change
      scene.js              # renderer, camera, lights, fog, grid floor, resize
      graph.js              # node spheres, link lines, CSS2D labels from map JSON
      controls.js           # WASD move, Q/E down/up, Shift fast, hold right mouse to look, wheel = speed
      picking.js            # raycaster hover highlight, click select, double-click fly-to
      panel.js              # info panel (label, ip, role, status, ports table, generic extra fields), host list with filter
      vendor/               # three.module.js, three.core.js, CSS2DRenderer.js, VERSION
tests/ (repo root)          # stdlib unittest: model round trip, adapters (hostname merge), layout
```

### JSON contract (`NetworkMap.to_dict()`)
```json
{ "version": 1, "revision": 3, "generated_at": "2026-10-19T12:00:00+00:00",
  "scan": {"type": "ping_sweep", "network": "192.168.1.0/24", "scanner_ip": "192.168.1.50"},
  "hosts": [ {"id": "192.168.1.10", "label": "nas.lan", "role": "host",
              "ip": "192.168.1.10", "hostname": "nas.lan",
              "open_ports": [{"number": 22, "proto": "tcp"}], "is_local": null, "extra": {},
              "position": [8.0, 0.0, 0.0]} ],
  "links": [ {"source": "192.168.1.50", "target": "192.168.1.10", "kind": "subnet"} ] }
```
- Each host entry is the `Host` dataclass serialised field by field (so `ip`, `hostname`, `open_ports`, `is_local`, `extra` mirror `pynmapper/model.py` exactly and any field added there appears automatically), plus four graphics keys: `id`, `label`, `role`, `position`.
- `label` = hostname if present else ip. `role` is one of scanner / gateway / host. `extra` is free-form and the panel renders it generically so new backend fields appear with zero frontend changes.
- A ping sweep implies no topology, so links are a star from the scanner to each host until traceroute exists.

### Frontend behaviour
- Dark scene, subtle fog, grid floor. Nodes are spheres coloured by role/status (scanner distinct and larger, gateway, host, grey for down). Labels are crisp HTML (CSS2DRenderer) that fade with distance. Hover brightens, selection shows a ring.
- Controls: W/A/S/D move, Q/E down/up, Shift fast, hold right mouse to look, scroll adjusts speed, R resets view, F flies to the selected host, double-click a node to fly to it, left click selects and opens the panel, `?` shows a help overlay.
- Panel: label, ip, role, status, ports table, extra key/values; host list sidebar with text filter, click = select + fly-to.
- Live updates: the page polls `/api/map` and rebuilds when `revision` changes, so the browser can open before the sweep finishes.

### Wiring into `__main__.py` (only entry file touched)
```python
hosts = asyncio.run(netutils.async_ping_sweep(myIP, prefix))
# ... turn into list[Host] (or pass hostnames from get_host_names) ...
net_map = graphics.from_hosts(hosts, scanner_ip=myIP, network=f"{myIP}/{prefix}")
graphics.serve(net_map)        # starts server, opens browser, blocks until Ctrl-C
```

### Milestones
1. **done** `graphics/model.py`, `adapters.py`, `layout.py`, `demo.py` + unit tests (`python -m unittest discover tests` from the repo root).
2. **done** `server.py` + `__main__.py`: `python -m pynmapper.graphics --demo` serves JSON + static files and opens the browser (placeholder page until milestone 3).
3. Vendor three.js, build `scene.js` + `graph.js`: nodes, links, labels render from `/api/map`.
4. `controls.js` + `picking.js` + `panel.js`: WASD fly, mouse look, hover/click, panel, host list, fly-to.
5. Polish: colours/legend/help overlay, label fade, revision polling, `--save/--json` round trip.
6. Wire `__main__.py`; real sweep end to end.

### Environment facts
- Repo root `Pynmapper/` has an empty `pyproject.toml` and `.venv` (Python 3.14, numpy, scapy 2.7 installed). `pynmapper/` is a package (`__init__.py`, `__main__.py`); run with `python -m pynmapper` from the repo root; internal imports are absolute (`from pynmapper.model import Host`).
- Graphics adds no Python dependencies (http.server, json, webbrowser, dataclasses, threading only).
- Dev machine: Wayland, NVIDIA GPU with OpenGL 4.6, Chrome available for testing.
