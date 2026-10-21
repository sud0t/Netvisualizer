# 3D graphical network mapper in Python (maybe Rust as well in the future)

## Your role
You are going to be handling the graphics side of this project, do not touch the actual network mapping part as that is what I am using to learn, even if there are vast optimizations to be made for the actual network mapping still do not touch it. Your goal is to write the graphics so that they are compatible with what I write for the networking part.

> Update 2026-10-20: Kyler granted permission to edit the networking code as needed to reach the project's goals. The networking side is still explained in plain words when it changes, since the project is for learning.

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

### Launching (implemented 2026-10-20)
- `python -m pynmapper` detects the interface with the default route, sweeps its subnet, starts the map server, opens the browser, then publishes the map again after each stage (sweep, names, ports) so the page redraws live. Options: a CIDR argument, `-i IFACE`, `--ports [LIST]`, `--no-names`, `--timeout`, `--concurrency`, `--save FILE`, `--json FILE`, `--demo`, `--no-browser`, `--no-serve`, `--port`, `--host`, `--list-interfaces`.
- `python -m pynmapper.graphics --demo` renders ~25 fake hosts with no network needed (day-to-day graphics development); `--json`/`--save` replay and dump maps there too.

### Hostnames
- `netutils.resolve_hostnames(hosts)` (2026-10-20) writes `host.hostname` straight into the Host objects, running the blocking `gethostbyaddr` calls in threads, 32 at a time, because every miss waits for the resolver's full timeout. The scanner's own name falls back to `socket.gethostname()`.
- The older `netutils.get_host_names(list[str])` (raw `gethostbyaddr` tuples, shorter than its input) is kept; `graphics.apply_hostnames` still understands it by matching on the IP inside each tuple.


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
- `async_ping_sweep` returns `list[Host]`; the graphics adapter `from_hosts` takes that list plus the scanner IP (and gateway IP).
- Fields added to `Host` or `Port` later show up in the JSON and the info panel automatically. `Port.service` (2026-10-20, e.g. "ssh") was the first such addition and needed no frontend change.
- A host with `extra["status"] == "down"` is drawn grey; `__main__` uses this for a gateway that ignores ping.

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
              "open_ports": [{"number": 22, "proto": "tcp", "service": "ssh"}], "is_local": null, "extra": {},
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

### Wiring in `__main__.py` (done 2026-10-20)
`main()` resolves the local network (`netutils.get_local_network`), starts a `MapServer` with a scanner-only map whose `scan.stage` says "sweeping…", then runs `run_scan()`: ping sweep → `mark_local` → `ensure_gateway` → `resolve_hostnames` → optional `async_port_scan`. After each stage it calls `server.update(build_map(hosts, ...))`; the server carries the revision forward so the polling page redraws every time. The page re-frames the camera on each update until the user moves it.

### Networking functions (`netutils.py`, 2026-10-20)
- `get_local_network(interface=None) -> LocalNetwork(interface, ip, network, gateway)` and `list_local_networks()`: read scapy's routing table (`scapy.route`, ~0.15 s import, warnings silenced); connected routes with no gateway are the subnets, the default route gives the gateway. Without scapy: `get_local_ip()` and an assumed /24.
- `async_ping(ip, timeout)`: system `ping`, exit code 0 = alive (flags per OS in `ping_command`). `async_ping_sweep(network, *, timeout, concurrency, progress)`: batches of `concurrency` pings; a /24 is one batch of about one second.
- `async_port_scan(hosts, ports=COMMON_TCP_PORTS, *, timeout, concurrency, progress)`: TCP connect scan via `asyncio.open_connection`, TCP only, no root; `service_name()` adds the well-known name.
- Still TODO: layer-2 (ARP) enumeration with scapy for hosts that ignore ping (needs root); traceroute for real topology.

### Milestones
1. **done** `graphics/model.py`, `adapters.py`, `layout.py`, `demo.py` + unit tests (`python -m unittest discover tests` from the repo root).
2. **done** `server.py` + `__main__.py`: `python -m pynmapper.graphics --demo` serves JSON + static files and opens the browser.
3. **done (2026-10-20)** three.js r186 vendored (`web/vendor/VERSION` says how to upgrade); `scene.js` + `graph.js` render nodes, links and labels from `/api/map`.
4. **done (2026-10-20)** `controls.js` (WASD/QE, Shift, right-mouse look, wheel = speed, fly-to and reset tweens), `picking.js` (hover, click, double-click, selection ring), `panel.js` (host list with filter, generic details card).
5. **done (2026-10-20)** legend, help overlay (`?`), labels fade with distance, "down" hosts drawn grey (read from `status` or `extra.status` once the backend reports it), revision polling, `--save/--json` round trip, shader-drawn grid floor, framing that leaves room for the sidebar.
6. **done (2026-10-20)** `__main__.py` with argparse; real sweep end to end verified on the dev LAN (10 hosts, gateway with 6 open ports, 3.3 s with `--ports`).

### Frontend verification (2026-10-20)
- `tests/test_graphics_web.py` checks, without a browser, that every module import, stylesheet reference and DOM id the JavaScript uses resolves, and that the server serves them with the right MIME types.
- The behaviour (hover, click, fly-to, keys, right-drag look, wheel, filter, help, selection surviving a map reload, label fade, "down" colouring, sidebar-aware framing) was exercised in headless Chrome over the DevTools protocol; 40 checks pass with no console errors.
- Known browser quirk: Chrome drops GL line segments that cross the camera's near plane, so a `GridHelper` loses all of its depth-running lines whenever the camera is over the grid. The floor is therefore a plane whose grid is drawn in the fragment shader.

### Environment facts
- Repo root `Pynmapper/` has an empty `pyproject.toml` and `.venv` (Python 3.14, numpy, scapy 2.7 installed). `pynmapper/` is a package (`__init__.py`, `__main__.py`); run with `python -m pynmapper` from the repo root; internal imports are absolute (`from pynmapper.model import Host`).
- Graphics adds no Python dependencies (http.server, json, webbrowser, dataclasses, threading only). Networking uses the system `ping`, `socket`, `asyncio`, and scapy only for the routing table (optional).
- Dev machine: Wayland, NVIDIA GPU with OpenGL 4.6, Chrome available for testing.
