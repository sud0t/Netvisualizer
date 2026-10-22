# 3D graphical network mapper in Python and Rust

## Your role
You are going to be handling the graphics side of this project (Rust), do not touch the actual network mapping part as that is what I am using to learn, even if there are vast optimizations to be made for the actual network mapping still do not touch it. Your goal is to write the graphics so that they are compatible with what I write for the networking part.

*(2026-10-21, evening: Kyler gave Claude full access to the network backend again, for the move to Rust in Milestone 5.)*


## Features
- Able to move around 3D space with wasd
- Host name should be preffered over IP address (if one was provided)
- Each host is a node that can be clicked on to reveal more information (such as open ports, the ports type, etc)

## TODO, in order (2026-10-21)
Migration first, then new features; every step leaves the program working. The milestone in progress has its own section at the end of this file.

1. **Milestone 5: move the networking backend to Rust** (migration, no new features; **done 2026-10-21**). One Rust program, `netvisualizer`, scans and draws; Python stays only for the CSV parsing and the SSH/Telnet setup, embedded with PyO3. This covers the old TODO items "Remove scapy dependency and use socket module instead", "Craft packets using struct and socket for layer 3 enumeration" (ICMP echo packets built by hand, in Rust), "`PyO3` crate so python can be embedded" and "Transfer networking backend to Rust". The scan no longer starts with the program: `--scan` starts it for now, a button later (Milestone 10). No root or setcap needed (see Milestone 5, Decisions).
2. **Milestone 6: MAC addresses and vendor names.** MACs from the kernel's neighbour table after the sweep (no root); vendor names from the MAC vendor CSV (`assets/mac-vendors-export.csv`, maclookup.app's export with 24-, 28- and 36-bit prefixes) read with `pandas`, in Python through the PyO3 bridge. Hosts that reverse DNS cannot name are labelled by vendor.
3. **Milestone 7: ARP sweep (layer 2).** ARP requests built by hand and sent on a raw `AF_PACKET` socket: finds hosts that ignore ping and reads their MACs directly. Needs root or `setcap cap_net_raw+ep` on the binary; without either, the ICMP sweep runs as before.
4. **Milestone 8: traceroute.** TTL-limited probes, so the map shows real paths with routers as nodes instead of a star from the scanner; the ring layout becomes a tree.
5. **Milestone 9: UDP ports.** Next to the TCP connect scan. A closed UDP port answers with ICMP "port unreachable" (a connected UDP socket reports it as "connection refused"); silence means open or filtered.
6. **Milestone 10: scanning from the UI.** A Scan button that sends the same `StartScan` message `--scan` sends, the scan options, progress and cancel. It comes after 6–9 so the options panel is built once, with every option.
7. **Milestone 11: SSH/Telnet console.** For hosts with port 22 or 23 open, a console opened from the details card: `asyncssh` (and a telnet client) in Python through the bridge, a terminal pane in egui. Last, because it needs the bridge, the port data and the UI work.

## Decisions made (2026-10-19)

### Native viewer in Rust: Bevy + bevy_egui (decided 2026-10-20, evening; see Milestone 4)
- Kyler wants the map to run in its own window, without a browser (and without Google in particular), so the graphics are a **native viewer in Rust** using Bevy (renders to its own window) and bevy_egui (sidebar, HUD, help overlay).
- Until Milestone 5, Python produced the data and the viewer read the JSON document from `/api/map` (polling `/api/health` for the revision) or from a `--save` file. Since 5.5 the scan runs inside the same program and maps cross a channel; the JSON contract stays, for `--save` and `--json`.
- Alternatives considered: pywebview (native window, but still a web engine), Panda3D (pure Python, has a 3.14 wheel, weak GUI toolkit), three-d + egui (lighter Rust), kiss3d (too limited). Bevy chosen for capability and longevity; the spec always anticipated Rust.
- **Done 2026-10-21** (Milestone 4).

### Launching (Rust program since Milestone 5, 2026-10-21)
- `netvisualizer` (built by `cargo build --release` in the repo root: `target/release/netvisualizer`) opens the map window. Nothing touches the network until a scan is asked for: `--scan [CIDR]` for now, a Scan button later (Milestone 10); both send the `StartScan` message.
- With `--scan` the window opens at once, and the map redraws after each stage (sweep, names, ports). Options: `-i IFACE`, `--ports [LIST]`, `--no-names`, `--timeout`, `--concurrency`, `--save FILE`; the scan options need `--scan`. `--no-window` scans and prints instead; `--list-interfaces` lists this machine's networks; `--json FILE` shows a saved map. Closing the window ends the program.
- Exit status: 0; 1 when a saved map cannot be loaded or a windowless scan fails; 2 when `-i` names an interface without an IPv4 network or there is no network at all (checked before the window opens).
- `netvisualizer --json viewer/tests/fixtures/demo.json` shows a saved fake LAN with no network needed (day-to-day viewer development: `cargo run --features dev -- --json tests/fixtures/demo.json` in `viewer/`).
- Logging: the program's own messages (what the scan finds) at "info"; Bevy's GPU set-up only with `-v`. `RUST_LOG` overrides both.

### Hostnames
- `scanner::names::resolve_hostnames(hosts, concurrency, overwrite)` writes `host.hostname` straight into the hosts, running the blocking `getnameinfo` calls (`dns-lookup`) on tokio's blocking threads, 32 at a time, because every miss waits for the resolver's full timeout. The scanner's own name falls back to `gethostname`.


## Shared data model: the one contract between backend and graphics

The only coupling between the networking code and the graphics code is a `Host` (and the JSON it serializes to). The networking code fills it in, the graphics code reads it, and neither knows how the other works.

### `scanner/src/model.rs`: `Host` and `Port`
Started as Kyler's `netvisualizer/model.py`; moved to Rust in 5.1 with the same fields, so it serializes field for field as the Python dataclasses did:

```rust
pub enum Proto { Tcp, Udp }                       // "tcp" / "udp"

pub struct Port {
    pub number: u16,
    pub proto: Proto,
    pub service: Option<String>,                  // "ssh" for 22/tcp, from /etc/services
}

pub struct Host {
    pub ip: IpAddr,
    pub hostname: Option<String>,
    pub open_ports: Vec<Port>,
    pub is_local: Option<bool>,                   // inside the scanner's own subnet; None = not checked
    pub extra: serde_json::Map<String, Value>,    // anything new goes here first
}
// Host::label(): the hostname, or else the address.
```

Notes:
- `scanner::sweep::ping_sweep` returns a `Host` per address that answered; the viewer's `MapDoc::from_hosts` takes those plus the scanner IP (and gateway IP).
- Fields added to `Host` or `Port` later show up in the JSON and the details card automatically. `Port.service` (2026-10-20, e.g. "ssh") was the first such addition and needed no viewer change.
- A host with `extra["status"] == "down"` is drawn grey; the scan uses this for a gateway that ignores ping.

### Why
- A sweep result used to be a bare list of IPs, with hostnames in a separate shorter list that had to be re-aligned by hand. `Host` keeps ip, hostname and ports together.
- Every later stage (names, ports, and later MACs and vendors) adds to the same `Host`s instead of a parallel list.
- The graphics side needs to know exactly one shape.


## Architecture

```
Cargo.toml                  # workspace: scanner + viewer; Cargo.lock, rust-toolchain.toml, target/ at the root
scanner/                    # netvisualizer-scanner: finds hosts (no Bevy, no Python); see Milestone 5
  src/model.rs              # Host / Port: the shared contract
  src/local.rs, icmp.rs, sweep.rs, names.rs, ports.rs
viewer/                     # netvisualizer, the program: the window (Milestone 4) and the glue below
  src/scan.rs               # StartScan, the scan stages on a background thread, maps into the viewer
  src/map.rs                # MapDoc: the JSON contract; from_hosts (the old graphics/adapters.py), save/load
  src/layout.rs             # ring positions around the scanner (the old graphics/layout.py)
  src/python.rs             # the embedded Python interpreter (PyO3)
netvisualizer/              # Python the program embeds (5.7 on), and this spec
tests/                      # Python tests
```
Until Milestone 5 the networking was Python (`netvisualizer/netutils.py`, `model.py`, `__main__.py`), and `netvisualizer/graphics/` turned its `Host`s into the JSON map and served it over HTTP; all of it was retired in 5.6 (it is in git history).

### JSON contract (`MapDoc`, `viewer/src/map.rs`)
```json
{ "version": 1, "revision": 3, "generated_at": "2026-10-19T12:00:00+00:00",
  "scan": {"type": "ping_sweep", "network": "192.168.1.0/24", "scanner_ip": "192.168.1.50"},
  "hosts": [ {"id": "192.168.1.10", "label": "nas.lan", "role": "host",
              "ip": "192.168.1.10", "hostname": "nas.lan",
              "open_ports": [{"number": 22, "proto": "tcp", "service": "ssh"}], "is_local": null, "extra": {},
              "position": [8.0, 0.0, 0.0]} ],
  "links": [ {"source": "192.168.1.50", "target": "192.168.1.10", "kind": "subnet"} ] }
```
- Each host entry is the `Host` serialised field by field (so `ip`, `hostname`, `open_ports`, `is_local`, `extra` mirror `scanner/src/model.rs` exactly and any field added there appears automatically), plus four graphics keys: `id`, `label`, `role`, `position`. Maps saved by the Python version load unchanged.
- `label` = hostname if present else ip. `role` is one of scanner / gateway / host. `extra` is free-form and the viewer's details card renders it generically so new backend fields appear with zero viewer changes.
- A ping sweep implies no topology, so links are a star from the scanner to each host until traceroute exists.

### Viewer behaviour
- Dark scene, subtle fog, grid floor. Nodes are spheres coloured by role/status (scanner distinct and larger, gateway, host, grey for down). Labels are drawn by egui and fade with distance. Hover brightens, selection shows a ring.
- Controls: W/A/S/D move, Q/E down/up, Shift fast, hold right mouse to look, scroll adjusts speed, R resets view, F flies to the selected host, double-click a node to fly to it, left click selects and opens the panel, `?` shows a help overlay.
- Panel: label, ip, role, status, ports table, extra key/values; host list sidebar with text filter, click = select + fly-to.
- Live updates: the scan hands the viewer a new map after each stage, so the window can open before the sweep finishes.

### Scan stages (`viewer/src/scan.rs`, 5.5)
`StartScan` → `ScanPlan::new` (where we are: `scanner::local::default_network`) → a thread with a tokio runtime runs: a scanner-only map whose `scan.stage` says "sweeping…" → `ping_sweep` → the gateway kept (grey) if it ignored the ping → `mark_local` → `resolve_hostnames` (+ `gethostname` for the scanner) → optional `tcp_port_scan` → "done" (and `--save`). Each map goes over an mpsc channel with a rising `revision`; the `receive` system puts it into `MapDocument`. The viewer re-frames the camera on each map until the user moves it.

### Networking (`scanner/`, 5.2–5.4)
- `local.rs`: `local_networks()` lists this machine's subnets from `getifaddrs` (if-addrs) with each interface's gateway from `/proc/net/route` (the default route with the lowest metric); `default_network(interface)` picks the one holding the outbound address (UDP-connect trick) or the named one; `mark_local`.
- `icmp.rs` + `sweep.rs`: ICMP echo requests built by hand (header, Internet checksum), sent on one ICMP socket: datagram (unprivileged), else raw (root), else the system `ping` per address. At most `concurrency` probes wait at once, each `timeout`.
- `names.rs`: reverse DNS, 32 lookups at a time. `ports.rs`: TCP connect scan with a semaphore of `concurrency`, service names from `/etc/services`, port lists ("22,80,8000-8010", "common").

### Milestones
1. **done** `graphics/model.py`, `adapters.py`, `layout.py` + unit tests (`python -m unittest discover tests` from the repo root).
2. **done** `server.py`: the map as JSON at `/api/map`, with a revision that goes up on every update, and `/api/health` to poll that revision cheaply.
3. **done (2026-10-20)** `__main__.py` with argparse; real sweep end to end verified on the dev LAN (10 hosts, gateway with 6 open ports, 3.3 s with `--ports`).
4. **done (2026-10-21):** native viewer in Rust with Bevy + bevy_egui, started by `python -m netvisualizer`. 4.1 (skeleton) 2026-10-20; 4.2 (fly controls, picking, labels) and its review pass, 4.3 (sidebar, HUD, help, legend), 4.4 (live mode), 4.5 (Python wiring) and 4.6 (behaviour check) 2026-10-21. Plan and outcomes below.
5. **done (2026-10-21):** the networking backend in Rust, one `netvisualizer` binary, Python embedded; see "Milestone 5" at the end. Milestones 6–11 are listed in order under "TODO, in order".

## Milestone 4: native viewer (Rust, Bevy + bevy_egui)
*The record of how the viewer was built, as a separate `netvisualizer-viewer` binary that followed the Python map server. Since Milestone 5 the binary is `netvisualizer` in a Cargo workspace (`rust-toolchain.toml`, `Cargo.lock` and `target/` moved to the repo root), it runs the scan itself, and `fetch.rs` and ureq are gone.*

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

## Milestone 5: networking backend in Rust (migration)

### Goal
One Rust program. `netvisualizer` opens the map window; `netvisualizer --scan` also sweeps the subnet of the interface with the default route, names the hosts and, with `--ports`, tries TCP ports, redrawing the map after each stage as `python -m netvisualizer` did. Without `--scan` nothing touches the network. A scan of the dev LAN finds the same hosts, names and ports as the Python scanner. No new features in this milestone; they start at Milestone 6.

### Layout
```
Cargo.toml                  # workspace: scanner + viewer; shared profiles; Cargo.lock and target/ at the root
rust-toolchain.toml
scanner/                    # netvisualizer-scanner (library): finds hosts. No Bevy, no Python.
  src/lib.rs
  src/model.rs              # Host, Port: the contract, serialised field for field like netvisualizer/model.py
  src/local.rs              # this machine's networks: interfaces, subnets, default gateway (5.2)
  src/icmp.rs               # ICMP echo packets, built and read by hand (5.3)
  src/sweep.rs              # the ping sweep (5.3)
  src/names.rs              # reverse DNS (5.4)
  src/ports.rs              # TCP connect scan, service names, port lists (5.4)
viewer/                     # package and binary `netvisualizer`: the window, plus the glue below
  src/scan.rs               # StartScan, the scan stages on a background thread, maps into the viewer (5.5)
  src/layout.rs             # ring positions around the scanner (5.5, from graphics/layout.py)
  src/python.rs             # the embedded interpreter (5.7)
  ...                       # the Milestone 4 modules
netvisualizer/              # the Python the binary embeds (5.7 on), and this spec
```

### Command line
```
netvisualizer                                       # the window, no scan
netvisualizer --scan                                # scan the default interface's subnet
netvisualizer --scan 10.0.0.0/24 --ports            # another network, then common TCP ports
netvisualizer --scan -i wlan0 --ports 22,80,8000-8010 --save last.json
netvisualizer --json last.json                      # a saved map
netvisualizer --scan --no-window --save last.json   # scan without opening a window
netvisualizer --list-interfaces
```
`--no-names`, `--timeout`, `--concurrency` and `-v` work as before; the scan options need `--scan`.

### Steps
- 5.1 **done 2026-10-21.** Cargo workspace at the repo root (members `scanner` and `viewer`; the profiles, `Cargo.lock`, `rust-toolchain.toml` and `target/` move to the root). The viewer's package and binary become `netvisualizer`. `scanner/src/model.rs`: `Host` and `Port` with serde, tested against the demo fixture's hosts. The Python launcher follows the binary's new path.
- 5.2 **done 2026-10-21.** `local.rs`: interfaces and their IPv4 subnets from `getifaddrs`, default gateways from `/proc/net/route`, the outbound address from the UDP-connect trick. Replaces scapy.
- 5.3 **done 2026-10-21.** `icmp.rs` and `sweep.rs`: the ping sweep on one ICMP socket (see Decisions), at most `--concurrency` probes waiting for an answer at once, each given `--timeout` seconds; the system `ping` as the fallback. Tested on loopback.
- 5.4 **done 2026-10-21.** `names.rs` and `ports.rs`: reverse DNS (`getnameinfo`) on blocking threads, 32 at a time; the TCP connect scan, `--concurrency` connections at once; service names from `/etc/services`; port lists ("22,80,8000-8010", "common").
- 5.5 **done 2026-10-21.** The program: `viewer/src/scan.rs` runs the stages (sweep, local flag, gateway kept, names, ports) on a background thread with a tokio runtime and hands the viewer a new map after each one; `layout.rs` and `MapDoc::from_hosts` replace `graphics/layout.py` and `adapters.py`; `--save` writes the same JSON; the HUD shows the stage; `--no-window` prints instead. The URL argument stays until 5.6 (the Python launcher passes it) but is no longer the default.
- 5.6 **done 2026-10-21.** Retire the Python path: scan the dev LAN with both and compare; then delete `netvisualizer/__main__.py`, `netutils.py`, `model.py`, `netvisualizer/graphics/`, their tests, and the viewer's HTTP polling (`fetch.rs`, ureq). README and this spec describe the Rust program.
- 5.7 **done 2026-10-21.** `viewer/src/python.rs`: PyO3 embeds Python (see Decisions) and `netvisualizer/` becomes the package of Python the binary calls (`optional_packages()` so far); `-v` says which interpreter it found, where the package and `.venv` are, and whether pandas and asyncssh import; tests call into the package. Milestone 6 is its first user.

### Decisions (2026-10-21)
- **The scanner is its own crate**, with no Bevy and no Python in it, so its tests build in seconds and the split of Milestones 1–4 carries over: the scanner fills in `Host`s, the viewer reads them, and only `model.rs` is shared. The order of the stages (what `__main__.run_scan` did) lives in the viewer's `scan.rs`, next to the Python bridge, so Python stages (vendor names) can go between Rust ones.
- **Tokio, on its own thread.** Hundreds of connections and timeouts in flight need an async runtime. It runs on the scan thread, and Bevy systems never wait on it: they read what the scan sends over a channel, as fetch.rs did for HTTP.
- **The scan starts on request.** `StartScan` (a Bevy message) starts a scan with the settings in the `ScanSettings` resource. `--scan` sends it once at start-up; the Milestone 10 button will send the same message. One scan at a time.
- **ICMP sockets instead of one `ping` process per address.** Linux lets a process open an ICMP datagram socket (`SOCK_DGRAM` + `IPPROTO_ICMP`) without privileges when its group is inside `net.ipv4.ping_group_range` (the dev machine allows every group, `0 2147483647`, and its `ping` has no setuid bit or capability: it uses the same socket). The kernel then fills in the identifier and only hands the socket its own replies. Root or `CAP_NET_RAW` gets a raw socket instead, where the program fills in everything and filters replies itself. When neither socket opens, the system `ping` is the fallback, so no root or setcap is needed anywhere in this milestone.
- **No HTTP.** Scanner and viewer are one process, so maps cross a channel; the map server, the viewer's polling and ureq go in 5.6. Following a scan from another machine (`--host 0.0.0.0`) goes with them; it can come back as a feature.
- **Python through PyO3.** The binary embeds the system's Python (`/usr/lib/libpython3.14.so`, the interpreter `.venv` was made from). When Python is first needed, the bridge puts the repo's `netvisualizer/` package and `.venv`'s site-packages on `sys.path`, so `pip install` into `.venv` is how Python dependencies (pandas, asyncssh) reach the binary. `libpython3.14.so` is a load-time dependency of the binary (a Python upgrade means a rebuild); a missing package only switches off the feature that needs it, with a message. Python errors reach Rust as one line ("ModuleNotFoundError: No module named 'pandas'").
- **The JSON contract is unchanged** (version 1): `--save` writes the document Python wrote, and `--json` reads old saves and the demo fixture.

### Outcomes (2026-10-21)
- The moved `viewer/target` (21 GB) was reused as the workspace's `target/`: after the rename only the viewer crate rebuilt (release build 1 min 55 s).
- **The sweep bug found in 5.5, and its fix.** The first Rust sweep of the dev LAN found 3 hosts where Python found 9. A probe to a neighbour the kernel has not resolved yet waits in the socket's send buffer until ARP answers, or for ~3 s when nobody is there; with the default buffer (208 KiB) the 254 probes of a /24 stalled sending for 3 s (measured: all sent after 3.08 s with 208 KiB, after 0.8 ms with 2 MiB). Meanwhile replies piled up unread, and the timeout check then gave up on probes whose replies were already waiting. Fixed both ways: the sweep asks for a 4 MiB send buffer (`SEND_BUFFER_BYTES`; the kernel caps it at `net.core.wmem_max`, 4 MiB here, 208 KiB on a stock kernel), and it reads every waiting reply before timing anyone out, with a late reply still counting as answered. A stalled send now only costs time.
- **Rust against Python on the dev LAN** (5.6, back to back): three sweep pairs found the same addresses (once Rust found one more, a phone that answers on and off); a full pair with names and `--ports` matched host for host in role, name, open ports with service names, `is_local` and the grey gateway. Rust 2.1 s, Python 2.5 s; a sweep alone 1.0 s against 1.3 s.
- The gateway that ignored the ping is now added before `mark_local`, so it gets `is_local: true` instead of `null`.
- `--no-window` prints the stages, the notes ("192.168.1.0/24: 9 of 254 addresses answered (ICMP datagram socket)"), a progress line per second for stages longer than a second, and a table of hosts.

### Verification (2026-10-21)
- `cargo test --workspace --features netvisualizer/dev`: 28 scanner tests (model round trip against the demo fixture's hosts, routing table parsing, ICMP checksum against RFC 1071's example, packets, IPv4 header skipping, sweeps of 127.0.0.0/29 at 254 and 1 probes in flight, a /32, the timeout on 192.0.2.1, the system `ping` fallback, the tally's late replies, reverse DNS of 127.0.0.1, connect scan against a local listener, services table, port lists) and 80 viewer tests (the Milestone 4 ones less fetch.rs's, plus layout, `from_hosts`, save/load against the fixture, timestamps, a whole scan of 127.0.0.0/30 through the scan thread, the `receive` and `start_scan` systems, the host table, the command line, and the Python bridge: the package imports, errors come back as one line). `python -m unittest discover tests`: 1 test (`optional_packages`). `cargo clippy --all --workspace --all-targets --features netvisualizer/dev` clean, `cargo fmt` applied.
- Release build 6 min 10 s (`ldd`: `libpython3.14.so.1.0 => /usr/lib/...`); its smoke test on the demo map with `--select 192.168.1.1` renders the ring, the selection and the details card as in Milestone 4.
- Screenshots (dev build, real window): `--scan` on the dev LAN (HUD "ping_sweep 192.168.1.0/24 · done · 8 hosts · 7 links · rev 3", scanner "axtop" in the middle, grey gateway), and the start without `--scan` (empty scene, "no scan yet · start one with --scan").

### Environment facts
- Unprivileged ICMP: `net.ipv4.ping_group_range = 0 2147483647` on the dev machine; `/usr/sbin/ping` has no setuid bit or capability.
- `net.core.wmem_max = 4194304`, `wmem_default = 212992`, ARP gives up on a neighbour after `mcast_solicit` 3 × `retrans_time_ms` 1000.
- Crates added (crates.io, 2026-10-21): tokio 1.54, socket2 0.6.5 (feature `all`, for raw sockets), ipnet 2.12, if-addrs 0.15, dns-lookup 4.0, pyo3 0.30.1 (`auto-initialize`); ureq removed.
- `.venv` (Python 3.14.8) has pandas and asyncssh (installed by Kyler, 2026-10-21) next to scapy, which nothing uses any more. PyO3's build finds `python3` on `PATH` (/usr/bin/python3, the same 3.14.8).
