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

1. **Milestone 8: traceroute.** TTL-limited probes, so the map shows real paths with routers as nodes instead of the "behind the gateway" guess; the two-level ring layout becomes a full tree.
2. **Milestone 9: UDP ports.** A closed UDP port answers with ICMP "port unreachable" (a connected UDP socket reports "connection refused"); silence means open or filtered.
3. **Milestone 10: scanning from the UI.** A Scan button that sends `StartScan` as `--scan` does, the scan options, progress and cancel; after 8–9 so the options panel is built once.
4. **Milestone 11: SSH/Telnet console.** For hosts with port 22 or 23 open, a console opened from the details card: asyncssh (and a telnet client) through the bridge, a terminal pane in egui.

Done: Milestones 1–4 (the viewer) and 5 (the networking backend in Rust), 2026-10-21; 6 (MACs from the kernel's ARP table, vendor names through pandas, vendor labels) and 7 (the ARP sweep on a packet socket, ping for what it cannot reach), 2026-10-22.

## Architecture
```
Cargo.toml        workspace: scanner + viewer (Cargo.lock, rust-toolchain.toml, target/ at the root)
scanner/          netvisualizer-scanner: finds hosts; no Bevy, no Python
  model.rs        Host / Port: the one contract with the viewer
  local.rs        interfaces, subnets, gateways (getifaddrs, /proc/net/route)
  icmp.rs         ICMP echo packets, built and read by hand
  sweep.rs        the ping sweep
  arp.rs          the ARP sweep: requests built by hand on a packet socket
  neighbours.rs   the kernel's ARP table: MACs of the hosts found
  names.rs        reverse DNS
  ports.rs        TCP connect scan, service names, port lists
viewer/           netvisualizer, the program (Bevy + bevy_egui)
  scan.rs         StartScan, the scan stages on their own thread, maps into the viewer
  vendors.rs      the vendor stage: MACs to vendor names through python.rs
  map.rs          the map document: JSON contract, from_hosts, save/load
  layout.rs       positions
  python.rs       the embedded Python (PyO3); build.rs sets its rpath
  …               scene, graph, controls, picking, panel, ui, theme
netvisualizer/    the Python the program embeds (vendors.py), and this spec
tests/            Python tests
assets/           data files: mac-vendors-export.csv, the IEEE's MAC blocks (maclookup.app export)
```

## Contract
`scanner/src/model.rs`, filled in by the scan stages and read by the viewer:
```rust
pub struct Port { pub number: u16, pub proto: Proto /* "tcp" | "udp" */, pub service: Option<String> }
pub struct Host {
    pub ip: IpAddr,
    pub hostname: Option<String>,
    pub mac: Option<MacAddr>,                   // "b8:27:eb:8e:cc:eb"; only hosts on the scanner's own link
    pub vendor: Option<String>,                 // who owns the MAC's registered prefix
    pub open_ports: Vec<Port>,
    pub is_local: Option<bool>,                 // on the scanner's own subnet; None = not checked
    pub extra: serde_json::Map<String, Value>,  // one-off notes; anything new goes here until it settles
}
```
Every field but `ip` has a `#[serde(default)]`, so a map saved before a field existed still loads. The map document (`viewer/src/map.rs`; also the `--save` / `--json` file, version 1 since the Python version, which only ever gains host fields):
```json
{ "version": 1, "revision": 3, "generated_at": "2026-10-21T12:00:00+00:00",
  "scan": {"type": "ping_sweep", "network": "192.168.1.0/24", "scanner_ip": "192.168.1.50", "stage": "done"},
  "hosts": [ {"id": "192.168.1.10", "label": "nas.lan", "role": "host",
              "ip": "192.168.1.10", "hostname": "nas.lan", "mac": "00:11:32:ab:cd:ef", "vendor": "Synology Incorporated",
              "open_ports": [{"number": 22, "proto": "tcp", "service": "ssh"}], "is_local": true, "extra": {},
              "position": [8.0, 0.0, 0.0]} ],
  "links": [ {"source": "192.168.1.50", "target": "192.168.1.10", "kind": "subnet"} ] }
```
- A host entry is the `Host` serialised field by field, plus `id`, `label` (hostname; else "Vendor (ip)", so two of a kind stay apart; else ip), `role` (scanner / gateway / host) and `position`. A new `Host` field shows up in the details card with no viewer change; the sidebar filter matches label, ip, hostname, mac, vendor, role and ports.
- `extra.mac_note` marks a MAC with no vendor whose locally administered bit (0x02 of the first octet) is set: made up on the device, as phones do for privacy. The IEEE's CID blocks set that bit too, hence the lookup first.
- `scan.type` is `arp_sweep`, `ping_sweep` or `arp_ping_sweep` (see Scanning), `demo` in the fixture.
- Links follow the packets as far as a sweep can tell: `"subnet"` from the scanner to each host on its own subnet, `"route"` from the gateway to each host outside it (from the scanner when the map has no gateway). The layout follows the links: the scanner at the origin, its hosts on rings around it (gateway first, then by address), hosts behind the gateway on rings of their own beyond it.
- `extra.status == "down"` draws a host grey; the scan uses it for a gateway that was swept and did not answer.

## Scanning
- Nothing touches the network until a `StartScan` message arrives: `--scan [CIDR]` sends one at start-up, and the Milestone 10 button will send the same. One scan at a time, with the settings in the `ScanSettings` resource.
- The stages run on a thread with its own tokio runtime and hand the viewer a new map after each: sweep → keep the scanner and the gateway → mark local hosts → MACs → vendors → reverse DNS (the scanner's own name from `gethostname`) → TCP ports if asked → done (and `--save`). Bevy systems only read what has arrived over the channel.
- The sweep is ARP for the part of the network on our own link, and ping for the whole network when it reaches beyond the link (ARP is not routed, but every host on the link must answer it, ping or no ping; the hosts ARP found keep their entries). `scan.type` says which ran: `arp_sweep`, `ping_sweep`, or `arp_ping_sweep` for both. The ARP sweep needs a packet socket (`AF_PACKET`/`SOCK_RAW`, bound to the interface and EtherType 0x0806), so root or `CAP_NET_RAW` (`sudo setcap cap_net_raw+ep target/release/netvisualizer`, again after each build); without it, with `--no-arp`, or when the ARP sweep fails midway, ping covers everything. It asks every address but ours, asks the silent ones once more, and listens `timeout` after the last request; any ARP frame from an address in the network, reply or overheard request, counts. The sweep never finds the scanner itself, so the scan adds it. Without root it can still be tried in a user namespace: `scripts/arp-sandbox.sh`.
- MACs come with the ARP replies; for pinged hosts, from `/proc/net/arp` (rows with flags 0x2, complete), which the pings filled: to ping an address on our link the kernel first ARPs for it. No root; Linux only. The scanner's own is `/sys/class/net/<iface>/address`. Hosts behind the gateway have none (the kernel ARPs for the gateway instead).
- The vendor stage runs in Python (`viewer/src/vendors.rs` → `netvisualizer/vendors.py`), blocking the scan thread between the Rust stages; without pandas it skips itself with a note.
- The sweep needs no root: an ICMP datagram socket (allowed when the process's group is in `net.ipv4.ping_group_range`), else a raw socket (root or `CAP_NET_RAW`), else the system `ping` per address. It asks for a 4 MiB send buffer, because probes waiting for ARP hold buffer space for up to ~3 s and the default 208 KiB stalls a /24; replies already received are read before any probe is timed out.

## Python (PyO3)
- The binary embeds the Python named by `PYO3_PYTHON` in `.cargo/config.toml` (uv's CPython 3.15; the system `python3` is 3.14 and unused) and links its libpython; `viewer/build.rs` records uv's lib directory as the binary's rpath.
- On first use the bridge puts the repo root (for `import netvisualizer`) and `.venv`'s site-packages on `sys.path`. `.venv` is made from the same Python and holds only what the program uses: pandas and asyncssh.
- After a Python upgrade, uv patch releases included: update `PYO3_PYTHON` if the minor version changed, recreate `.venv`, `cargo clean -p pyo3-build-config`, rebuild. A missing package only turns off the feature that needs it.
- `vendors.vendor_names(macs) -> list[str | None]`: the CSV (59k IEEE blocks of 24, 28 and 36 bits) read once with pandas; the longest matching prefix wins; a Private block has no name and counts as unknown.

## Development
- Checks: `cargo test --workspace --features netvisualizer/dev`, `cargo clippy --all --workspace --all-targets --features netvisualizer/dev`, `.venv/bin/python -m unittest discover tests`.
- `--features dev` links Bevy dynamically, so rebuilds take seconds; a release build takes ~6 min.
- `--exit-after-frames N --screenshot FILE` (with `--select ID --fly` or `--help-overlay`) renders and quits: the smoke test. `--json viewer/tests/fixtures/demo.json` shows a fake 27-host LAN; `--scan 127.0.0.0/28` shows hosts behind the gateway without leaving the machine; `--no-window` prints instead of drawing.
- `rust-toolchain.toml` pins rustc 1.99.1 (Bevy 0.20 needs 1.97.1); bump Bevy and bevy_egui together. The dev machine's Hyprland exports `WGPU_BACKEND=gl`, which finds no GPU there; `main.rs` then lets wgpu choose (Vulkan).
