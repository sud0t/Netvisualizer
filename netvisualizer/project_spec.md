# 3D graphical network mapper in Python and Rust

## Your role
You are going to be handling the graphics side of this project (Rust), do not touch the actual network mapping part as that is what I am using to learn, even if there are vast optimizations to be made for the actual network mapping still do not touch it. Your goal is to write the graphics so that they are compatible with what I write for the networking part.


## Features
- Able to move around 3D space with wasd
- Host name should be preffered over IP address (if one was provided)
- Each host is a node that can be clicked on to reveal more information (such as open ports, the ports type, etc)

## TODO
### Python
- Remove scapy dependency 
- Craft packets using struct and socket for layer 3 enumeration
- Layer 2 host enumeration (ARP) with socket, struct, and async: finds hosts that ignore ping, but needs root.
- Download MAC vendor CSV and use `pandas` to give Hosts proper names if they weren't resolved by reverse DNS lookup
- Check if SSH or Telnet ports are open and build a console in the app that gives access with proper features and allows for SSH or Telnet connections to hosts that allow it traceroute, so the map can show real paths instead of a star from the scanner.

### Rust
- `PyO3` crate so python can be embedded
- Transfer networking backend to Rust keeping CSV parsing for MAC addresses and the SSH/Telnet setup to Python (asyncssh)

## Decisions made (2026-10-19)

### Native viewer in Rust: Bevy + bevy_egui (decided 2026-10-20, evening; see Milestone 4)
- Kyler wants the map to run in its own window, without a browser (and without Google in particular), so the graphics are a **native viewer in Rust** using Bevy (renders to its own window) and bevy_egui (sidebar, HUD, help overlay).
- Python only produces data: the viewer reads the JSON document from `/api/map` (polling `/api/health` for the revision) or from a `--save` file. This is exactly what the one-contract design was for. With `--host 0.0.0.0` a viewer on another machine can follow a scan.
- Alternatives considered: pywebview (native window, but still a web engine), Panda3D (pure Python, has a 3.14 wheel, weak GUI toolkit), three-d + egui (lighter Rust), kiss3d (too limited). Bevy chosen for capability and longevity; the spec always anticipated Rust.
- **Done 2026-10-21** (Milestone 4).

### Launching (implemented 2026-10-20, viewer 2026-10-21)
- `python -m netvisualizer` detects the interface with the default route, sweeps its subnet, starts the map server, starts the viewer on it, then publishes the map again after each stage (sweep, names, ports) so it redraws live. Options: a CIDR argument, `-i IFACE`, `--ports [LIST]`, `--no-names`, `--timeout`, `--concurrency`, `--save FILE`, `--json FILE`, `--no-open`, `--no-serve`, `--port`, `--host`, `--list-interfaces`.
- The viewer is `viewer/target/release/netvisualizer-viewer <server url>`; closing its window ends the program, and a viewer that exits non-zero (no usable GPU, say) makes the program exit with status 1. Without the binary, the program stops before scanning and prints the `cargo build --release` hint. `--no-open` serves until Ctrl-C without starting the viewer, and needs no binary; neither does `--no-serve`.
- `python -m netvisualizer --json viewer/tests/fixtures/demo.json` shows a saved fake LAN with no network needed; add `--no-open` to serve it for `cargo run --features dev` in `viewer/` to follow (day-to-day viewer development).

### Hostnames
- `netutils.resolve_hostnames(hosts)` (2026-10-20) writes `host.hostname` straight into the Host objects, running the blocking `gethostbyaddr` calls in threads, 32 at a time, because every miss waits for the resolver's full timeout. The scanner's own name falls back to `socket.gethostname()`.
- The older `netutils.get_host_names(list[str])` (raw `gethostbyaddr` tuples, shorter than its input) is kept; `graphics.apply_hostnames` still understands it by matching on the IP inside each tuple.


## Shared data model: the one contract between backend and graphics

The only coupling between the networking code and the graphics code is a `Host` object (and the JSON it serializes to). The networking code fills it in, the graphics code reads it, and neither imports the other.

### `netvisualizer/model.py` (written by Kyler, exists): `Host` and `Port`
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
- Fields added to `Host` or `Port` later show up in the JSON and the info panel automatically. `Port.service` (2026-10-20, e.g. "ssh") was the first such addition and needed no viewer change.
- A host with `extra["status"] == "down"` is drawn grey; `__main__` uses this for a gateway that ignores ping.

### Why
- A sweep result today is a bare list of IPs, with hostnames in a separate shorter list that has to be re-aligned by hand. `Host` keeps ip, hostname and ports together.
- When scapy port scanning lands, results append to `host.ports` instead of a third parallel list.
- The graphics side then needs to know exactly one shape, and its adapter becomes nearly empty.


## Graphics architecture

```
netvisualizer/
  model.py                  # Host / Port (written by Kyler, shared contract)
  __main__.py               # entry point (argparse): scan, build the map, serve it, start the viewer
  netutils.py               # UNTOUCHED
  graphics/
    __init__.py             # public API: NetworkMap, Link, from_hosts, from_ping_sweep, MapServer
    model.py                # NetworkMap/Link dataclasses + to_dict()/from_dict(); imports Host/Port from netvisualizer.model
    adapters.py             # from_hosts(list[Host], scanner_ip, ...) and from_ping_sweep(ips, scanner_ip, hostnames=None, ...)
    layout.py               # positions: scanner at origin, hosts on a ring/shells; pluggable for tree/force later
    server.py               # stdlib ThreadingHTTPServer: /api/health + /api/map (with revision); MapServer.update()
tests/ (repo root)          # stdlib unittest: model round trip, adapters (hostname merge), layout, server, launcher
viewer/                     # the Rust viewer that draws the map (see Milestone 4)
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
- Each host entry is the `Host` dataclass serialised field by field (so `ip`, `hostname`, `open_ports`, `is_local`, `extra` mirror `netvisualizer/model.py` exactly and any field added there appears automatically), plus four graphics keys: `id`, `label`, `role`, `position`.
- `label` = hostname if present else ip. `role` is one of scanner / gateway / host. `extra` is free-form and the viewer's details card renders it generically so new backend fields appear with zero viewer changes.
- A ping sweep implies no topology, so links are a star from the scanner to each host until traceroute exists.

### Viewer behaviour
- Dark scene, subtle fog, grid floor. Nodes are spheres coloured by role/status (scanner distinct and larger, gateway, host, grey for down). Labels are drawn by egui and fade with distance. Hover brightens, selection shows a ring.
- Controls: W/A/S/D move, Q/E down/up, Shift fast, hold right mouse to look, scroll adjusts speed, R resets view, F flies to the selected host, double-click a node to fly to it, left click selects and opens the panel, `?` shows a help overlay.
- Panel: label, ip, role, status, ports table, extra key/values; host list sidebar with text filter, click = select + fly-to.
- Live updates: the viewer polls `/api/health` and downloads `/api/map` when `revision` changes, so its window can open before the sweep finishes.

### Wiring in `__main__.py` (done 2026-10-20)
`main()` resolves the local network (`netutils.get_local_network`), starts a `MapServer` with a scanner-only map whose `scan.stage` says "sweeping…", then runs `run_scan()`: ping sweep → `mark_local` → `ensure_gateway` → `resolve_hostnames` → optional `async_port_scan`. After each stage it calls `server.update(build_map(hosts, ...))`; the server carries the revision forward so the polling viewer redraws every time. The viewer re-frames the camera on each update until the user moves it.

### Networking functions (`netutils.py`, 2026-10-20)
- `get_local_network(interface=None) -> LocalNetwork(interface, ip, network, gateway)` and `list_local_networks()`: read scapy's routing table (`scapy.route`, ~0.15 s import, warnings silenced); connected routes with no gateway are the subnets, the default route gives the gateway. Without scapy: `get_local_ip()` and an assumed /24.
- `async_ping(ip, timeout)`: system `ping`, exit code 0 = alive (flags per OS in `ping_command`). `async_ping_sweep(network, *, timeout, concurrency, progress)`: batches of `concurrency` pings; a /24 is one batch of about one second.
- `async_port_scan(hosts, ports=COMMON_TCP_PORTS, *, timeout, concurrency, progress)`: TCP connect scan via `asyncio.open_connection`, TCP only, no root; `service_name()` adds the well-known name.
- Still TODO: layer-2 (ARP) enumeration with scapy for hosts that ignore ping (needs root); traceroute for real topology.

### Milestones
1. **done** `graphics/model.py`, `adapters.py`, `layout.py` + unit tests (`python -m unittest discover tests` from the repo root).
2. **done** `server.py`: the map as JSON at `/api/map`, with a revision that goes up on every update, and `/api/health` to poll that revision cheaply.
3. **done (2026-10-20)** `__main__.py` with argparse; real sweep end to end verified on the dev LAN (10 hosts, gateway with 6 open ports, 3.3 s with `--ports`).
4. **done (2026-10-21):** native viewer in Rust with Bevy + bevy_egui, started by `python -m netvisualizer`. 4.1 (skeleton) 2026-10-20; 4.2 (fly controls, picking, labels) and its review pass, 4.3 (sidebar, HUD, help, legend), 4.4 (live mode), 4.5 (Python wiring) and 4.6 (behaviour check) 2026-10-21. Plan and outcomes below.
5. Later: ARP enumeration (root), traceroute topology, UDP ports.

## Milestone 4: native viewer (Rust, Bevy + bevy_egui)

### Goal
`python -m netvisualizer` opens the map in a native window, with the controls and panel listed under "Viewer behaviour", and the map redraws live as the scan stages arrive. Python scans and serves the JSON.

### Versions (checked on crates.io, 2026-10-20)
- `/usr/sbin/rustc` and `cargo` are rustup proxies (the pacman package `rustup` provides `rust`); the default toolchain was stable 1.96.1. **Bevy 0.20 declares `rust-version = 1.97.1`** and refuses to build on older compilers, so `viewer/rust-toolchain.toml` pins `1.99.1` (the newest stable that day); rustup installs it on the first `cargo` command inside `viewer/` and the default toolchain is left alone. bevy_egui 0.43.1 only needs 1.95. `rustup update` would make the pin redundant.
- `bevy = "0.20.0"` (features `wayland`, `png`), `bevy_egui = "0.43.1"` (must be the release built for Bevy 0.20; check its README compatibility table before bumping either; added in 4.2 with `default-features = false, features = ["render", "default_fonts", "manage_clipboard"]`, so no `open_url` browser launcher and no bevy_picking bridge), `serde`, `serde_json` with `preserve_order` (host fields stay in Python's order for the details card), `clap` 4.6 (derive), `wgpu = "30"` (Bevy's own wgpu, used directly only to probe for a GPU adapter), and from 4.4 `ureq = "3.4"` with `default-features = false` (blocking HTTP on a background thread, no async runtime; the server is plain HTTP, so ureq's default rustls/ring and gzip are left out, which adds only ureq, ureq-proto, httparse, base64 and a few small crates; 3.x is also the major bevy_asset's own `http` feature uses).
- `cargo run --features dev` links Bevy dynamically: the first build took 9 min 50 s on the dev laptop (12 threads), a rebuild after a source edit about 30 s, `cargo clippy` 2 min. The Python launcher (4.5) runs the static release build (`cargo build --release`, thin LTO; 6 min 20 s, `viewer/target/release/netvisualizer-viewer`).

### Layout
```
viewer/                       # cargo crate, binary name netvisualizer-viewer
  Cargo.toml
  src/
    main.rs                   # clap args: URL (default http://127.0.0.1:8765) | --json FILE; builds the App, adds plugins
    map.rs                    # serde structs for the JSON contract; Host keeps unknown keys (#[serde(flatten)] extra map) so new backend fields reach the details card
    fetch.rs                  # std::thread: poll /api/health every 2 s, GET /api/map when the revision changes, send over an mpsc channel into a Bevy Resource
    scene.rs                  # camera, lights, clear colour, grid floor (Gizmos lines or a shader plane), framing with an inset for the sidebar
    graph.rs                  # spawn/despawn one entity per host (sphere mesh, material per role/status), link lines, bounds
    controls.rs               # fly camera: WASD/QE, Shift, right mouse held = look (cursor grab), wheel = speed, R, F, fly-to and reset tweens, userMoved flag
    picking.rs                # ray-sphere hover, click select, double-click fly-to, selection ring
    ui.rs                     # egui: HUD (status/stage/speed), legend, help overlay, labels drawn with the egui painter, UiInput flags
    panel.rs                  # egui sidebar: filter, host list, generic details card
```
All of these exist after 4.6, plus `theme.rs`, `Cargo.lock`, `rust-toolchain.toml` and `tests/fixtures/demo.json` (a saved 27-host fake LAN, also used by the Python tests and for trying the viewer with `python -m netvisualizer --json`; any `--save` output can replace it, as long as the Rust tests' host and link counts are updated with it).

### Steps
- 4.1 **done 2026-10-20.** `cargo run --features dev -- --json scan.json` opens a window, draws spheres and links at the JSON positions, frames the camera (and re-frames on resize). Hosts without a position go on a fallback ring. Also in: `--exit-after-frames N` and `--screenshot FILE` (smoke test), the window title with the host count, the GPU-backend probe described below. A URL argument was accepted but not followed yet (that came in 4.4).
- 4.2 **done 2026-10-21.** `controls.rs` (WASD/QE, Shift, right-mouse look with `CursorGrabMode::Locked` + hidden cursor while held, wheel = speed, `Tween`, `fly_to`, `user_moved`), `picking.rs` (ray-sphere hover with 1.2× slack, click with 5 px slop, double click = second click within 0.4 s, selection by host id, ring as a gizmo circle facing the camera), `ui.rs` (labels through the egui painter, bottom-centred above each sphere, fading over the fog range, styled per role/hover/selection; `UiInput` flags so controls and picking yield to egui), `scene.rs` (later maps glide to a new framing while `user_moved` is false; `ResetView` message for R), `main.rs` (`ViewerSet` order Build → Frame → Controls → Picking → Draw; R/F/Escape; `--select HOST_ID [--fly]` debug flags for screenshots). Review pass (2026-10-21) fixed: the 300 px glitch filter ran on the frame's summed mouse motion, so a quick turn on a slow frame was dropped whole (now per `MouseMotion` event); movement keys ignored Ctrl/Alt/Super (`Ctrl+S` flew backwards); labels were painted in query order (now far to near, so the nearer label is on top).
- 4.3 **done 2026-10-21.** `panel.rs`: the sidebar is an egui `Area` anchored top right (a floating card, so egui knows the pointer is over UI and clicks never fall through to the scene); filter box (`/` focuses it and selects the old query, Enter selects and flies to the first match, Escape leaves it), count (`4 / 27`), collapse toggle; host list sorted by role, then name (unknown roles last, names case-insensitive), virtualised (only visible rows are laid out, so a /16 costs the same per frame as a /24), scrolls the selection into view; details card generic over `serde_json::Value` (scalars as rows, lists of objects as tables with the union of keys, objects nested, whole floats printed without a decimal point), scrolled back to the top on each new selection, "fly to" and close buttons. `ui.rs`: HUD (brand, status line red while the server is unreachable, speed, `?` button), legend, help overlay (`egui::Modal`; `?`/`h`, Escape or a click on the backdrop closes it), a custom egui style, pointer cursor over hosts. Framing uses the sidebar's width as the right inset (none when collapsed), recomputed on R and on resize. `SelectHost` message: every selection that does not come from a 3D click (list, card buttons, F, Escape, `--select`) goes through one system.
- 4.4 **done 2026-10-21.** `fetch.rs`: a plain thread polls `/api/health` every 2 s and downloads `/api/map` only when the revision changes, sending a `FetchEvent` over `std::sync::mpsc`; the `Fetch` set (first in `Update`) moves it into `MapDocument`, which is now `Option<MapDoc>` (`None` until the first map arrives). Outages show `server unreachable, retrying… (reason)`, and the summary comes back once the server answers. The selection survives because it is kept by id; re-framing stops once the user moves (`Graph.drawn` keeps the empty start-up graph from counting as the first map).
- 4.5 **done 2026-10-21.** `__main__.py` starts the release binary on the server URL and waits for its window to close; `--no-open` serves without it; see "Launching". README has the `cargo build --release` step and the Debian build packages.
- 4.6 **done 2026-10-21.** Walked through the "Viewer behaviour" list above; `python -m netvisualizer` opens the viewer by default.

### Design decisions
- Labels through the egui painter rather than Bevy text billboards: no font assets to ship, trivial distance fade, and they sit in the same overlay as the sidebar.
- The grid floor as Gizmos line segments first (Bevy draws them as quads, so lines that cross the camera's near plane still draw); a shader plane only if the gizmo grid looks worse.
- HTTP on a plain thread with `ureq` and `std::sync::mpsc`; Bevy systems only ever read a `Resource` that holds the latest document.
- Unknown JSON keys are kept, never dropped, so the generic details card keeps working when `model.py` grows.
- Theme colours live in one `theme.rs` constant table.
- (4.2 outcomes) Input in Bevy 0.20: `ButtonInput<KeyCode>` / `ButtonInput<MouseButton>`, `AccumulatedMouseMotion` and `AccumulatedMouseScroll` resources, cursor grab and visibility on the window entity's `CursorOptions` component. One wheel notch arrives as one `Line` and a touchpad reports pixels; `WHEEL_LINE_PX = 100` turns lines into the pixels the speed curve works in. Pitch/yaw come from `Quat::to_euler(EulerRot::YXZ)`.
- egui 0.36 renamed the input queries: `Context::egui_wants_pointer_input`, `egui_wants_keyboard_input`, `is_pointer_over_egui` (`Rect::round_ui` is a trait method on `emath::GuiRounding`). UI systems run in bevy_egui's `EguiPrimaryContextPass` schedule, get the context with `EguiContexts::ctx_mut()?` (fallible systems returning `Result`), and `EguiPlugin::default()` creates the primary context by itself.
- The selection ring is `gizmos.circle(...)` in its own `SelectionGizmos` config group (2.5 px, `depth_bias = -1.0` so it shows through other nodes).
- Headless tests build an `App` with `MinimalPlugins` + `TimeUpdateStrategy::ManualDuration(16 ms)` and the input resources inserted by hand (no `InputPlugin`, whose PreUpdate clear would erase a `just_pressed` set from the test); `ButtonInput::clear()` runs after every simulated frame instead.
- (4.1 outcomes) Links are gizmo lines as well, redrawn every frame from the endpoints resolved into the `Graph` resource, so a map change only has node entities to despawn. The gizmo grid is line strips with a per-vertex alpha fade over the fog range, re-centred on the camera each frame, so it has no edge to fly past; it looked right on the first screenshot, so no shader plane.
- Framing fits the box to the usable region exactly (one lower bound on the camera distance per edge of the region) rather than sliding the look target by a pixel offset measured at the box centre's depth, which would let near corners poke out under a wide sidebar.
- The camera uses `Tonemapping::None` so the role colours on the spheres come out as the swatches in the legend and host list. Lights: ambient 400 with a pale sky tint, sun 6000 lux, tuned by eye on the demo map. `DebandDither::Disabled` goes with it (debanding only runs after tone mapping, and Bevy warned about the pair on every start).
- (4.3–4.5 outcomes) Shortcut keys (`?`, `h`, `r`, `f`, `/`, Escape) are read from `KeyboardInput` messages by logical key: `?` works on any layout, and a held key does not repeat. Movement keys stay physical (`KeyCode`), so WASD sits in the same place on every layout. Only a focused text box takes the keyboard (`Context::text_edit_focused`), so a button focused with Tab does not stop WASD; keys typed into the filter count as released, so the camera coasts to a stop.
- egui's bundled fonts have no "✕" (U+2715), so the details card's close button is "×"; a unit test lays out every character the UI draws and fails if any falls back to the replacement glyph. (`Fonts::has_glyph` cannot answer this: it reports every glyph of the face that also holds the replacement glyph as missing, including plain ASCII in the monospace font.)
- egui cannot blur what is behind a panel, so labels and spheres behind the translucent HUD and sidebar show through sharp.
- GPU backend: Bevy honours `WGPU_BACKEND`, and the Hyprland config (`~/.config/hypr/configs/environment.lua`) exports `WGPU_BACKEND=gl` desktop-wide. wgpu's OpenGL path finds no adapter on this machine (under Wayland or X11), so Bevy panicked with "Unable to find a GPU". `main.rs` now probes the requested backend the way Bevy would and, if it has no adapter, prints one line and lets wgpu choose (Vulkan on the Quadro T1000). Unsetting that variable, or `WGPU_BACKEND=vulkan`, works without the fallback.

### Verification
- `cargo build --release` and `cargo test` (map parsing from the real `scan.json`, fallback ring, port-table column union). After 4.1: 16 tests pass (`cargo test --features dev`: demo fixture parsing, unknown keys kept in order, lenient positions, status from `status`/`extra.status`, fallback ring, bounds, framing geometry with and without insets, smoothstep); `cargo clippy --all-targets` is clean and `cargo fmt` applied.
- A `--exit-after-frames N` flag so `netvisualizer-viewer --json scan.json --exit-after-frames 120` is a smoke test that opens, renders and quits with status 0; `--screenshot FILE` saves a PNG 30 frames before the exit, which is how a session without eyes on the window checks the picture. 150 frames of the demo map take about 2 s end to end. `--select 192.168.1.1 --fly` adds a selection and a fly-to, so one screenshot shows the ring, the selected label style and the fly-to pose.
- After 4.2: 32 tests pass, `cargo clippy --all-targets` clean, `cargo fmt` applied.
- After 4.6 (2026-10-21): 57 Rust tests pass (`cargo test --features dev`; new: the polling protocol against a fake server, a real HTTP round trip through ureq to a local socket that answers like Python's http.server, a refused port, the shortcut keys driven by `KeyboardInput` messages, right-drag look with per-event glitch filtering and cursor grab, keys ignored while typing or with Ctrl, sidebar sorting/filtering/framing inset, table columns, whole-float numbers, glyph coverage), `cargo clippy --features dev --all-targets` clean, `cargo fmt` applied, `cargo build --release` clean.
- End to end (2026-10-21, Xvfb + Mesa lavapipe, release binary, driven with xdotool): a fake scan published through the real `serve()` / `build_map()` / `open_viewer()` / `wait()` showed the scanner alone at "sweeping…", 27 unnamed hosts at rev 1 (camera glided to the new framing), then hostnames and ports at rev 2 with the selection and camera kept (the user had flown). Checked by screenshot: host-list click = select + fly, details card, R (sidebar-aware framing), `?` and Escape order (help first, then deselect), backdrop click, `/` + typing + Enter, WASD ignored while typing, click and double click on spheres, right-drag look, wheel, sidebar collapse, hover and "down" colouring, "server unreachable" then recovery, and closing the window stops the server.
- Wheel on X11 under xdotool: each synthetic notch counts as two lines, because winit 0.30 turns both the press and the release of button 4 into a wheel event unless the server flags them as emulated. Real wheels on libinput are flagged (winit reads the scroll valuators instead), and the dev machine runs the viewer on Wayland, so the viewer code is left alone.
- Python (2026-10-21): 79 tests pass on 3.14 (`python -m unittest discover tests`). `tests/test_viewer_launch.py` covers launching a stand-in binary with the server URL, the missing-binary stop, `--no-open`/`--no-serve` without a binary, waiting for the window to close, and a failing viewer exiting with status 1; the server tests check that only `/api/` is served; the model round trip uses the viewer's fixture.
- **Still to do by hand on the dev machine:** pointer lock and right-drag look under Hyprland (Wayland locks the pointer; X11 here only confines it), wheel feel and double-click timing on real hardware.

### Environment facts
- Repo root `Netvisualizer/` has an empty `pyproject.toml` and `.venv` (Python 3.14, numpy, scapy 2.7 installed). `netvisualizer/` is a package (`__init__.py`, `__main__.py`); run with `python -m netvisualizer` from the repo root; internal imports are absolute (`from netvisualizer.model import Host`).
- Graphics adds no Python dependencies (http.server, json, dataclasses, threading only). Networking uses the system `ping`, `socket`, `asyncio`, and scapy only for the routing table (optional).
- Dev machine: Hyprland (Wayland) on a hybrid Intel + NVIDIA Quadro T1000 laptop, 12 threads; the Hyprland config exports `WGPU_BACKEND=gl` and `WINIT_UNIX_BACKEND=x11` (the latter is ignored by winit 0.30). Hyprland tiles new windows, so the viewer rarely gets its requested 1280×720; framing re-runs on resize.
