# 3D graphical network mapper in Python (maybe Rust as well in the future)

## Your role
You are going to be handling the graphics side of this project (Rust), do not touch the actual network mapping part as that is what I am using to learn, even if there are vast optimizations to be made for the actual network mapping still do not touch it. Your goal is to write the graphics so that they are compatible with what I write for the networking part.


## Features
- Able to move around 3D space with wasd
- Host name should be preffered over IP address (if one was provided)
- Each host is a node that can be clicked on to reveal more information (such as open ports, the ports type, etc)

## TODO
- Enumerate ifnames and subnet mask with subprocess
- Add PyO3 so that all Python can be embedded in Rust and program can be started from one executable binary

## Decisions made (2026-10-19)

### Native viewer in Rust: Bevy + bevy_egui (decided 2026-10-20, evening; see Milestone 7)
- Kyler wants the map to run without a browser, and without Google in particular. The three.js page needs a browser engine (any browser: `BROWSER=firefox` works today), so the **next milestone, ahead of everything else, is a native viewer in Rust** using Bevy (renders to its own window) and bevy_egui (sidebar, HUD, help overlay).
- The Python side is untouched: the viewer reads the same JSON document from `/api/map` (polling `/api/health` for the revision) or from a `--save` file. This is exactly what the one-contract design was for.
- The web page stays in the repo as the reference for feature parity and as the remote-viewing option (`--host 0.0.0.0`); it becomes the fallback once the native viewer reaches parity.
- Alternatives considered: pywebview (native window, but still a web engine), Panda3D (pure Python, has a 3.14 wheel, weak GUI toolkit), three-d + egui (lighter Rust, closest port), kiss3d (too limited). Bevy chosen for capability and longevity; the spec always anticipated Rust.
- **Done 2026-10-21:** the native viewer reached parity and became the default, with the page as a fallback; later the same day the page was removed altogether (next section).

### Launching (implemented 2026-10-20, Rust only since 2026-10-21)
- `python -m pynmapper` detects the interface with the default route, sweeps its subnet, starts the map server, starts the viewer on it, then publishes the map again after each stage (sweep, names, ports) so it redraws live. Options: a CIDR argument, `-i IFACE`, `--ports [LIST]`, `--no-names`, `--timeout`, `--concurrency`, `--save FILE`, `--json FILE`, `--demo`, `--no-open`, `--no-serve`, `--port`, `--host`, `--list-interfaces`.
- The viewer is `viewer/target/release/pynmapper-viewer <server url>`; closing its window ends the program, and a viewer that exits non-zero (no usable GPU, say) makes the program exit with status 1. Without the binary, the program stops before scanning and prints the `cargo build --release` hint. `--no-open` (was `--no-browser`) serves until Ctrl-C without starting the viewer, and needs no binary; neither does `--no-serve`.
- `python -m pynmapper.graphics --demo` serves ~25 fake hosts with no network needed, for `cargo run --features dev` in `viewer/` to follow (day-to-day viewer development); `--json`/`--save`/`--dump` replay and dump maps there too.

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
    __init__.py             # public API: NetworkMap, Link, from_hosts, from_ping_sweep, MapServer
    model.py                # NetworkMap/Link dataclasses + to_dict()/from_dict(); imports Host/Port from pynmapper.model
    adapters.py             # from_hosts(list[Host], scanner_ip, ...) and from_ping_sweep(ips, scanner_ip, hostnames=None, ...)
    layout.py               # positions: scanner at origin, hosts on a ring/shells; pluggable for tree/force later
    demo.py                 # fake NetworkMap for --demo
    server.py               # stdlib ThreadingHTTPServer: /api/health + /api/map (with revision); MapServer.update()
    __main__.py             # CLI: --demo | --json FILE, --port, --save FILE, --dump
tests/ (repo root)          # stdlib unittest: model round trip, adapters (hostname merge), layout, server, launcher
viewer/                     # the Rust viewer that draws the map (see Milestone 7)
```
The original plan had a three.js page in `graphics/web/` here (app, scene, graph, controls, picking, panel .js); it was removed on 2026-10-21.

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
1. **done** `graphics/model.py`, `adapters.py`, `layout.py`, `demo.py` + unit tests (`python -m unittest discover tests` from the repo root).
2. **done** `server.py` + `__main__.py`: `python -m pynmapper.graphics --demo` serves JSON + static files and opens the browser.
3. **done (2026-10-20)** three.js r186 vendored (`web/vendor/VERSION` says how to upgrade); `scene.js` + `graph.js` render nodes, links and labels from `/api/map`.
4. **done (2026-10-20)** `controls.js` (WASD/QE, Shift, right-mouse look, wheel = speed, fly-to and reset tweens), `picking.js` (hover, click, double-click, selection ring), `panel.js` (host list with filter, generic details card).
5. **done (2026-10-20)** legend, help overlay (`?`), labels fade with distance, "down" hosts drawn grey (read from `status` or `extra.status` once the backend reports it), revision polling, `--save/--json` round trip, shader-drawn grid floor, framing that leaves room for the sidebar.
6. **done (2026-10-20)** `__main__.py` with argparse; real sweep end to end verified on the dev LAN (10 hosts, gateway with 6 open ports, 3.3 s with `--ports`).
7. **done (2026-10-21):** native viewer in Rust with Bevy + bevy_egui, feature parity with the web page, now the default. 7.1 (skeleton) 2026-10-20; 7.2 (fly controls, picking, labels) and its review pass, 7.3 (sidebar, HUD, help, legend), 7.4 (live mode), 7.5 (Python wiring) and 7.6 (parity, default flipped) 2026-10-21. Plan and outcomes below.
8. **done (2026-10-21):** the three.js page removed; the Rust viewer is the only renderer and the Python server serves JSON only. See "Web viewer removed".
9. Later: ARP enumeration (root), traceroute topology, UDP ports.

### Frontend verification (2026-10-20)
_The page and `tests/test_graphics_web.py` were removed on 2026-10-21; kept as a record, since the Chrome quirk below is why the Rust grid is gizmos._
- `tests/test_graphics_web.py` checks, without a browser, that every module import, stylesheet reference and DOM id the JavaScript uses resolves, and that the server serves them with the right MIME types.
- The behaviour (hover, click, fly-to, keys, right-drag look, wheel, filter, help, selection surviving a map reload, label fade, "down" colouring, sidebar-aware framing) was exercised in headless Chrome over the DevTools protocol; 40 checks pass with no console errors.
- Known browser quirk: Chrome drops GL line segments that cross the camera's near plane, so a `GridHelper` loses all of its depth-running lines whenever the camera is over the grid. The floor is therefore a plane whose grid is drawn in the fragment shader.

## Milestone 7: native viewer (Rust, Bevy + bevy_egui)

### Goal
`python -m pynmapper` opens a native window instead of a browser. The window shows the same map, with the same controls and panel, and redraws live as the scan stages arrive. Python scans and serves exactly as today.

### Versions (checked on crates.io, 2026-10-20)
- `/usr/sbin/rustc` and `cargo` are rustup proxies (the pacman package `rustup` provides `rust`); the default toolchain was stable 1.96.1. **Bevy 0.20 declares `rust-version = 1.97.1`** and refuses to build on older compilers, so `viewer/rust-toolchain.toml` pins `1.99.1` (the newest stable that day); rustup installs it on the first `cargo` command inside `viewer/` and the default toolchain is left alone. bevy_egui 0.43.1 only needs 1.95. `rustup update` would make the pin redundant.
- `bevy = "0.20.0"` (features `wayland`, `png`), `bevy_egui = "0.43.1"` (must be the release built for Bevy 0.20; check its README compatibility table before bumping either; added in 7.2 with `default-features = false, features = ["render", "default_fonts", "manage_clipboard"]`, so no `open_url` browser launcher and no bevy_picking bridge), `serde`, `serde_json` with `preserve_order` (host fields stay in Python's order for the details card), `clap` 4.6 (derive), `wgpu = "30"` (Bevy's own wgpu, used directly only to probe for a GPU adapter), and from 7.4 `ureq = "3.4"` with `default-features = false` (blocking HTTP on a background thread, no async runtime; the server is plain HTTP, so ureq's default rustls/ring and gzip are left out, which adds only ureq, ureq-proto, httparse, base64 and a few small crates; 3.x is also the major bevy_asset's own `http` feature uses).
- `cargo run --features dev` links Bevy dynamically: the first build took 9 min 50 s on the dev laptop (12 threads), a rebuild after a source edit about 30 s, `cargo clippy` 2 min. The Python launcher (7.5) will run the static release build (`cargo build --release`, thin LTO; 6 min 20 s, `viewer/target/release/pynmapper-viewer`).

### Layout
```
viewer/                       # cargo crate, binary name pynmapper-viewer
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
All of these exist after 7.6, plus `theme.rs`, `Cargo.lock`, `rust-toolchain.toml` and `tests/fixtures/demo.json` (the 27-host demo map; regenerate with `python -m pynmapper.graphics --demo --dump > viewer/tests/fixtures/demo.json`).

Each file began as a one-to-one port of a `web/*.js` module, and the JavaScript was the port guide. Since the page was removed (2026-10-21) the Rust files stand on their own: comments explain the code directly rather than by reference to the JavaScript.

### Steps
- 7.1 **done 2026-10-20.** `cargo run --features dev -- --json scan.json` opens a window, draws spheres and links at the JSON positions, frames the camera (and re-frames on resize). Hosts without a position go on a fallback ring, as in `graph.js`. Also in: `--exit-after-frames N` and `--screenshot FILE` (smoke test), the window title with the host count, the GPU-backend probe described below. A URL argument is accepted but exits with "not implemented yet (step 7.4)".
- 7.2 **done 2026-10-21.** `controls.rs` (WASD/QE, Shift, right-mouse look with `CursorGrabMode::Locked` + hidden cursor while held, wheel = speed, `Tween`, `fly_to`, `user_moved`), `picking.rs` (ray-sphere hover with 1.2× slack, click with 5 px slop, double click = second click within 0.4 s, selection by host id, ring as a gizmo circle facing the camera), `ui.rs` (labels through the egui painter, bottom-centred above each sphere, fading over the fog range, styled per role/hover/selection; `UiInput` flags so controls and picking yield to egui), `scene.rs` (later maps glide to a new framing while `user_moved` is false; `ResetView` message for R), `main.rs` (`ViewerSet` order Build → Frame → Controls → Picking → Draw; R/F/Escape; `--select HOST_ID [--fly]` debug flags for screenshots). Review pass (2026-10-21) fixed: the 300 px glitch filter ran on the frame's summed mouse motion, so a quick turn on a slow frame was dropped whole (now per `MouseMotion` event, as controls.js does per pointer event); movement keys ignored Ctrl/Alt/Super (`Ctrl+S` flew backwards); labels were painted in query order (now far to near, like CSS2DRenderer's `zOrder`).
- 7.3 **done 2026-10-21.** `panel.rs`: the sidebar is an egui `Area` anchored top right (a floating card like `position: fixed`, so egui knows the pointer is over UI and clicks never fall through to the scene); filter box (`/` focuses it and selects the old query, Enter selects and flies to the first match, Escape leaves it), count (`4 / 27`), collapse toggle; host list sorted like `compareHosts` (unknown roles last, case-insensitive like `localeCompare`), virtualised (only visible rows are laid out, so a /16 costs the same per frame as a /24), scrolls the selection into view; details card generic over `serde_json::Value` exactly as `panel.js` (scalars as rows, lists of objects as tables with the union of keys, objects nested, whole floats printed like JS `String(1.0)`), scrolled back to the top on each new selection, "fly to" and close buttons. `ui.rs`: HUD (brand, status line red while the server is unreachable, speed, `?` button), legend, help overlay (`egui::Modal`; `?`/`h`, Escape or a click on the backdrop closes it), page-like egui style, pointer cursor over hosts. Framing uses the sidebar's width as the right inset (none when collapsed), recomputed on R and on resize. `SelectHost` message: every selection that does not come from a 3D click (list, card buttons, F, Escape, `--select`) goes through one system.
- 7.4 **done 2026-10-21.** `fetch.rs`: a plain thread polls `/api/health` every 2 s and downloads `/api/map` only when the revision changes, sending a `FetchEvent` over `std::sync::mpsc`; the `Fetch` set (first in `Update`) moves it into `MapDocument`, which is now `Option<MapDoc>` (`None` until the first map arrives). Outages show `server unreachable, retrying… (reason)` like the page, and the summary comes back once the server answers. The selection survives because it is kept by id; re-framing stops once the user moves (`Graph.drawn` keeps the empty start-up graph from counting as the first map).
- 7.5 **done 2026-10-21.** `__main__.py --viewer native|web` (removed again with the page the same day), `--no-browser` renamed `--no-open`; see "Launching". README has the `cargo build --release` step and the Debian build packages.
- 7.6 **done 2026-10-21.** Parity walked through against the page's behaviour list (below); default flipped to `native`.

### Decisions already made for the port
- Labels through the egui painter rather than Bevy text billboards: no font assets to ship, trivial distance fade, and they sit in the same overlay as the sidebar.
- The grid floor as Gizmos line segments first (Bevy draws them as quads, so the near-plane line bug from the browser does not apply); a shader plane only if the gizmo grid looks worse.
- HTTP on a plain thread with `ureq` and `std::sync::mpsc`; Bevy systems only ever read a `Resource` that holds the latest document.
- Unknown JSON keys are kept, never dropped, so the generic details card keeps working when `model.py` grows.
- Theme colours live in one `theme.rs` constant table. It mirrored `style.css` until the page was removed; now it is the only copy.
- (7.2 outcomes) Input in Bevy 0.20: `ButtonInput<KeyCode>` / `ButtonInput<MouseButton>`, `AccumulatedMouseMotion` and `AccumulatedMouseScroll` resources, cursor grab and visibility on the window entity's `CursorOptions` component. One wheel notch arrives as one `Line`; the browser's `deltaY` was ~100 px per notch, so `WHEEL_LINE_PX = 100` keeps the same speed curve. Pitch/yaw come from `Quat::to_euler(EulerRot::YXZ)`.
- egui 0.36 renamed the input queries: `Context::egui_wants_pointer_input`, `egui_wants_keyboard_input`, `is_pointer_over_egui` (`Rect::round_ui` is a trait method on `emath::GuiRounding`). UI systems run in bevy_egui's `EguiPrimaryContextPass` schedule, get the context with `EguiContexts::ctx_mut()?` (fallible systems returning `Result`), and `EguiPlugin::default()` creates the primary context by itself.
- The selection ring is `gizmos.circle(...)` in its own `SelectionGizmos` config group (2.5 px, `depth_bias = -1.0` so it shows through other nodes, like the web ring's `depthTest: false`).
- Headless tests build an `App` with `MinimalPlugins` + `TimeUpdateStrategy::ManualDuration(16 ms)` and the input resources inserted by hand (no `InputPlugin`, whose PreUpdate clear would erase a `just_pressed` set from the test); `ButtonInput::clear()` runs after every simulated frame instead.
- (7.1 outcomes) Links are gizmo lines as well, redrawn every frame from the endpoints resolved into the `Graph` resource, so a map change only has node entities to despawn. The gizmo grid is line strips with a per-vertex alpha fade over the fog range, re-centred on the camera each frame, so it has no edge to fly past; it looked right on the first screenshot, so no shader plane.
- Framing differs from `scene.js` on purpose: the port fits the box to the usable region exactly (one lower bound on the camera distance per edge of the region), whereas `scene.js` slides the look target by a pixel offset measured at the box centre's depth, which lets near corners poke out under a wide sidebar. Without insets the two formulas agree. Back-porting the exact version to `scene.js` is a small follow-up.
- The camera uses `Tonemapping::None` so role colours come out as the CSS swatches, like the browser (three.js tone mapping is off). Lights: ambient 400 with the hemisphere sky tint, sun 6000 lux; close to the page on the demo map. `DebandDither::Disabled` goes with it (debanding only runs after tone mapping, and Bevy warned about the pair on every start).
- (7.3–7.5 outcomes) Shortcut keys (`?`, `h`, `r`, `f`, `/`, Escape) are read from `KeyboardInput` messages by logical key, like `e.key` on the page: `?` works on any layout, and a held key does not repeat. Movement keys stay physical (`KeyCode`), like `e.code` in controls.js. Only a focused text box takes the keyboard (`Context::text_edit_focused`), so a button focused with Tab does not stop WASD; keys typed into the filter count as released, so the camera coasts to a stop.
- egui's bundled fonts have no "✕" (U+2715), so the details card's close button is "×"; a unit test lays out every character the UI draws and fails if any falls back to the replacement glyph. (`Fonts::has_glyph` cannot answer this: it reports every glyph of the face that also holds the replacement glyph as missing, including plain ASCII in the monospace font.)
- egui has no `backdrop-filter: blur()`, so labels and spheres behind the translucent HUD and sidebar show through sharp rather than blurred.
- GPU backend: Bevy honours `WGPU_BACKEND`, and the Hyprland config (`~/.config/hypr/configs/environment.lua`) exports `WGPU_BACKEND=gl` desktop-wide. wgpu's OpenGL path finds no adapter on this machine (under Wayland or X11), so Bevy panicked with "Unable to find a GPU". `main.rs` now probes the requested backend the way Bevy would and, if it has no adapter, prints one line and lets wgpu choose (Vulkan on the Quadro T1000). Unsetting that variable, or `WGPU_BACKEND=vulkan`, works without the fallback.

### Verification
- `cargo build --release` and `cargo test` (map parsing from the real `scan.json`, fallback ring, port-table column union). After 7.1: 16 tests pass (`cargo test --features dev`: demo fixture parsing, unknown keys kept in order, lenient positions, status from `status`/`extra.status`, fallback ring, bounds, framing geometry with and without insets, smoothstep); `cargo clippy --all-targets` is clean and `cargo fmt` applied.
- A `--exit-after-frames N` flag so `pynmapper-viewer --json scan.json --exit-after-frames 120` is a smoke test that opens, renders and quits with status 0; `--screenshot FILE` saves a PNG 30 frames before the exit, which is how a session without eyes on the window checks the picture. 150 frames of the demo map take about 2 s end to end. `--select 192.168.1.1 --fly` adds a selection and a fly-to, so one screenshot shows the ring, the selected label style and the fly-to pose.
- After 7.2: 32 tests pass, `cargo clippy --all-targets` clean, `cargo fmt` applied.
- After 7.6 (2026-10-21): 57 Rust tests pass (`cargo test --features dev`; new: the polling protocol against a fake server, a real HTTP round trip through ureq to a local socket that answers like Python's http.server, a refused port, the shortcut keys driven by `KeyboardInput` messages, right-drag look with per-event glitch filtering and cursor grab, keys ignored while typing or with Ctrl, sidebar sorting/filtering/framing inset, table columns, JS-style numbers, glyph coverage), `cargo clippy --features dev --all-targets` clean, `cargo fmt` applied, `cargo build --release` clean. Python: 96 tests pass on 3.14 (`tests/test_viewer_launch.py` covers `--viewer`, `--no-open`, the missing-binary fallback, launching a stand-in binary with the server URL, and the wait/fallback logic).
- End to end (2026-10-21, Xvfb + Mesa lavapipe, release binary, driven with xdotool): a fake scan published through the real `serve()` / `build_map()` / `open_viewer()` / `wait()` showed the scanner alone at "sweeping…", 27 unnamed hosts at rev 1 (camera glided to the new framing), then hostnames and ports at rev 2 with the selection and camera kept (the user had flown). Checked by screenshot: host-list click = select + fly, details card, R (sidebar-aware framing), `?` and Escape order (help first, then deselect), backdrop click, `/` + typing + Enter, WASD ignored while typing, click and double click on spheres, right-drag look, wheel, sidebar collapse, hover and "down" colouring, "server unreachable" then recovery, and closing the window stops the server. `python -m pynmapper --demo` opens the native viewer by default.
- Wheel on X11 under xdotool: each synthetic notch counts as two lines, because winit 0.30 turns both the press and the release of button 4 into a wheel event unless the server flags them as emulated. Real wheels on libinput are flagged (winit reads the scroll valuators instead), and the dev machine runs the viewer on Wayland, so the viewer code is left alone.
- After removing the page (2026-10-21): 86 Python tests pass (the 9 static checks on the page and the missing-`web/` test went; `tests/test_viewer_launch.py` now covers the missing-binary stop, `--no-open`/`--no-serve` without a binary, and a failing viewer exiting with status 1; the server test checks that only `/api/` is served).
- **Still to do by hand on the dev machine:** pointer lock and right-drag look under Hyprland (Wayland locks the pointer; X11 here only confines it), wheel feel and double-click timing on real hardware.

### Environment facts
- Repo root `Pynmapper/` has an empty `pyproject.toml` and `.venv` (Python 3.14, numpy, scapy 2.7 installed). `pynmapper/` is a package (`__init__.py`, `__main__.py`); run with `python -m pynmapper` from the repo root; internal imports are absolute (`from pynmapper.model import Host`).
- Graphics adds no Python dependencies (http.server, json, dataclasses, threading only). Networking uses the system `ping`, `socket`, `asyncio`, and scapy only for the routing table (optional).
- Dev machine: Hyprland (Wayland) on a hybrid Intel + NVIDIA Quadro T1000 laptop, 12 threads; the Hyprland config exports `WGPU_BACKEND=gl` and `WINIT_UNIX_BACKEND=x11` (the latter is ignored by winit 0.30). Chrome available for testing. Hyprland tiles new windows, so the viewer rarely gets its requested 1280×720; framing re-runs on resize.
