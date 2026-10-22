# 3D graphical network mapper in Python and Rust

## Your role
You are going to be handling the graphics side of this project (Rust), do not touch the actual network mapping part as that is what I am using to learn, even if there are vast optimizations to be made for the actual network mapping still do not touch it. Your goal is to write the graphics so that they are compatible with what I write for the networking part.

*(2026-10-21: Kyler gave Claude full access to the network backend for the move to Rust.)*


## Features
- Able to move around 3D space with wasd
- Host name should be preffered over IP address (if one was provided)
- Each host is a node that can be clicked on to reveal more information (such as open ports, the ports type, etc)

## Feature requests

## TODO, in order
New features come after migration steps; every step leaves the program working. Requests under "Feature requests" get planned into this list, and each is deleted from there once it is done.

1. **Milestone 6: MAC addresses and vendor names.** MACs from the kernel's neighbour table after the sweep (no root); vendor names from `assets/mac-vendors-export.csv` (maclookup.app export, 24-, 28- and 36-bit prefixes) with pandas, through the Python bridge. Hosts that reverse DNS cannot name are labelled by vendor.
2. **Milestone 7: ARP sweep.** ARP requests built by hand on a raw `AF_PACKET` socket: finds hosts that ignore ping, with their MACs. Needs root or `setcap cap_net_raw+ep`; without either, the ICMP sweep runs as before.
3. **Milestone 8: traceroute.** TTL-limited probes, so the map shows real paths with routers as nodes instead of the "behind the gateway" guess; the two-level ring layout becomes a full tree.
4. **Milestone 9: UDP ports.** A closed UDP port answers with ICMP "port unreachable" (a connected UDP socket reports "connection refused"); silence means open or filtered.
5. **Milestone 10: scanning from the UI.** A Scan button that sends `StartScan` as `--scan` does, the scan options, progress and cancel; after 6–9 so the options panel is built once.
6. **Milestone 11: SSH/Telnet console.** For hosts with port 22 or 23 open, a console opened from the details card: asyncssh (and a telnet client) through the bridge, a terminal pane in egui.

Done: Milestones 1–4 (the viewer) and 5 (the networking backend in Rust), 2026-10-21.

## Architecture
```
Cargo.toml        workspace: scanner + viewer (Cargo.lock, rust-toolchain.toml, target/ at the root)
scanner/          netvisualizer-scanner: finds hosts; no Bevy, no Python
  model.rs        Host / Port: the one contract with the viewer
  local.rs        interfaces, subnets, gateways (getifaddrs, /proc/net/route)
  icmp.rs         ICMP echo packets, built and read by hand
  sweep.rs        the ping sweep
  names.rs        reverse DNS
  ports.rs        TCP connect scan, service names, port lists
viewer/           netvisualizer, the program (Bevy + bevy_egui)
  scan.rs         StartScan, the scan stages on their own thread, maps into the viewer
  map.rs          the map document: JSON contract, from_hosts, save/load
  layout.rs       positions
  python.rs       the embedded Python (PyO3); build.rs sets its rpath
  …               scene, graph, controls, picking, panel, ui, theme
netvisualizer/    the Python the program embeds, and this spec
tests/            Python tests
assets/           data files
```

## Contract
`scanner/src/model.rs`, filled in by the scan stages and read by the viewer:
```rust
pub struct Port { pub number: u16, pub proto: Proto /* "tcp" | "udp" */, pub service: Option<String> }
pub struct Host {
    pub ip: IpAddr,
    pub hostname: Option<String>,
    pub open_ports: Vec<Port>,
    pub is_local: Option<bool>,                 // on the scanner's own subnet; None = not checked
    pub extra: serde_json::Map<String, Value>,  // anything new goes here first
}
```
The map document (`viewer/src/map.rs`; also the `--save` / `--json` file, version 1, unchanged since the Python version):
```json
{ "version": 1, "revision": 3, "generated_at": "2026-10-21T12:00:00+00:00",
  "scan": {"type": "ping_sweep", "network": "192.168.1.0/24", "scanner_ip": "192.168.1.50", "stage": "done"},
  "hosts": [ {"id": "192.168.1.10", "label": "nas.lan", "role": "host",
              "ip": "192.168.1.10", "hostname": "nas.lan",
              "open_ports": [{"number": 22, "proto": "tcp", "service": "ssh"}], "is_local": true, "extra": {},
              "position": [8.0, 0.0, 0.0]} ],
  "links": [ {"source": "192.168.1.50", "target": "192.168.1.10", "kind": "subnet"} ] }
```
- A host entry is the `Host` serialised field by field, plus `id`, `label` (hostname, else ip), `role` (scanner / gateway / host) and `position`. A new `Host` field shows up in the details card with no viewer change.
- Links follow the packets as far as a sweep can tell: `"subnet"` from the scanner to each host on its own subnet, `"route"` from the gateway to each host outside it (from the scanner when the map has no gateway). The layout follows the links: the scanner at the origin, its hosts on rings around it (gateway first, then by address), hosts behind the gateway on rings of their own beyond it.
- `extra.status == "down"` draws a host grey; the scan uses it for a gateway that was swept and did not answer.

## Scanning
- Nothing touches the network until a `StartScan` message arrives: `--scan [CIDR]` sends one at start-up, and the Milestone 10 button will send the same. One scan at a time, with the settings in the `ScanSettings` resource.
- The stages run on a thread with its own tokio runtime and hand the viewer a new map after each: sweep → keep the gateway → mark local hosts → reverse DNS (the scanner's own name from `gethostname`) → TCP ports if asked → done (and `--save`). Bevy systems only read what has arrived over the channel.
- The sweep needs no root: an ICMP datagram socket (allowed when the process's group is in `net.ipv4.ping_group_range`), else a raw socket (root or `CAP_NET_RAW`), else the system `ping` per address. It asks for a 4 MiB send buffer, because probes waiting for ARP hold buffer space for up to ~3 s and the default 208 KiB stalls a /24; replies already received are read before any probe is timed out.

## Python (PyO3)
- The binary embeds the Python named by `PYO3_PYTHON` in `.cargo/config.toml` (uv's CPython 3.15; the system `python3` is 3.14 and unused) and links its libpython; `viewer/build.rs` records uv's lib directory as the binary's rpath.
- On first use the bridge puts the repo root (for `import netvisualizer`) and `.venv`'s site-packages on `sys.path`. `.venv` is made from the same Python and holds only what the program uses: pandas and asyncssh.
- After a Python upgrade, uv patch releases included: update `PYO3_PYTHON` if the minor version changed, recreate `.venv`, `cargo clean -p pyo3-build-config`, rebuild. A missing package only turns off the feature that needs it.

## Development
- Checks: `cargo test --workspace --features netvisualizer/dev`, `cargo clippy --all --workspace --all-targets --features netvisualizer/dev`, `.venv/bin/python -m unittest discover tests`.
- `--features dev` links Bevy dynamically, so rebuilds take seconds; a release build takes ~6 min.
- `--exit-after-frames N --screenshot FILE` (with `--select ID --fly` or `--help-overlay`) renders and quits: the smoke test. `--json viewer/tests/fixtures/demo.json` shows a fake 27-host LAN; `--scan 127.0.0.0/28` shows hosts behind the gateway without leaving the machine; `--no-window` prints instead of drawing.
- `rust-toolchain.toml` pins rustc 1.99.1 (Bevy 0.20 needs 1.97.1); bump Bevy and bevy_egui together. The dev machine's Hyprland exports `WGPU_BACKEND=gl`, which finds no GPU there; `main.rs` then lets wgpu choose (Vulkan).
