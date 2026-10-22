# Netvisualizer

Finds the hosts on your network and shows them as a 3D map in its own window.

An ARP sweep (with the right privilege, see below) or a ping sweep finds
the hosts, their MAC addresses and the IEEE's vendor list say who made
them, traceroute finds the routers on the way to hosts beyond your own
subnet, reverse DNS names them all, and optional TCP and UDP port scans
list their open ports. It is one Rust program: the scanner builds its own ARP
requests and ICMP echo requests and sends them on raw sockets, and the map
is drawn with Bevy: fly around with the keyboard and mouse, click a host
for its details.

## Build (once)

```sh
cargo build --release      # in the repo root; about 10 min the first time; needs Rust (rustup)
```

`rust-toolchain.toml` pins the compiler Bevy needs (rustc 1.99.1); rustup
installs it on the first `cargo` command in the repo. On Debian/Ubuntu the build also
needs `libasound2-dev libudev-dev libwayland-dev libxkbcommon-dev`.
The program is `target/release/netvisualizer` (or `cargo run --release --`
followed by its arguments).

The program embeds Python 3.15 for a few features (the `netvisualizer/`
package: the vendor lookup with pandas, later the SSH console):
`.cargo/config.toml` names the interpreter (`python3.15`, installed with
`uv python install 3.15`), and the build links its libpython, so after a Python
upgrade, rebuild. The packages it uses live in `.venv`, made from the same Python:

```sh
uv venv --python 3.15 --seed .venv
.venv/bin/pip install pandas asyncssh
```

`netvisualizer -v` says which Python it embeds and which packages it can import.

## Run

```sh
netvisualizer                                   # the window; press Scan there (nothing is scanned until then)
netvisualizer --scan                            # sweep the subnet of the interface with the default route
netvisualizer --scan 192.168.1.0/24             # this network instead
netvisualizer --scan --ports                    # then try common TCP ports on the hosts that answered
netvisualizer --scan --ports 22,80,8000-8010
netvisualizer --scan --udp                      # and common UDP ports (53/udp? means silent: open or filtered)
netvisualizer --scan --no-arp                   # ping only, even when ARP is allowed
netvisualizer --scan 1.1.1.1                    # one far host: the routers on the way become nodes
netvisualizer --scan 10.0.0.0/24 --no-trace     # without the traceroute to hosts beyond your subnet
netvisualizer --scan --save last.json           # keep the result; --json last.json shows it again
netvisualizer --scan --no-window                # scan and print the hosts, no window
netvisualizer --list-interfaces                 # which networks this machine is on
netvisualizer --help                            # everything else
```

Nothing touches the network without `--scan` or the Scan button in the
window's top-left card, which takes the same options (open "options") and
turns into the stage, a progress bar and Cancel while a scan runs. The window
opens straight away and the map redraws as each stage of the scan finishes;
closing the window ends the program. To try it without scanning, show the
saved fake LAN: `netvisualizer --json viewer/tests/fixtures/demo.json`.

### Privileges

With root, or the `CAP_NET_RAW` capability on the binary, the sweep sends
ARP requests to every address of the subnet. Every host on the link has to
answer those (that is how anything reaches it at all), so an ARP sweep also
finds the hosts that ignore ping, and the replies carry their MAC addresses.
To grant the capability once per build:

```sh
sudo setcap cap_net_raw+ep target/release/netvisualizer
```

Without it the ping sweep runs, which needs no root: Linux lets a process
open an ICMP "datagram" socket when its group is inside
`net.ipv4.ping_group_range` (most distributions allow every group: check
with `sysctl net.ipv4.ping_group_range`). Without that, run as root and the
sweep uses a raw socket; failing both, it runs the system `ping` once per
address. A network beyond your own subnet is always pinged: ARP does not
cross routers.

## Controls

| Key / mouse | Action |
|---|---|
| `W` `A` `S` `D`, `Q` `E` | fly; down / up |
| `Shift`, mouse wheel | faster; change speed |
| right mouse (hold) | look around |
| left click, double click | select a host; fly to it |
| `F`, `R` | fly to the selection; reset the view |
| `/`, `Esc`, `?` | filter the host list; deselect; help |

## Development

```sh
cargo test --workspace --features netvisualizer/dev   # everything
cargo test -p netvisualizer-scanner                   # the scanner alone: builds in seconds, no Bevy

cd viewer
cargo run --features dev -- --json tests/fixtures/demo.json   # a saved map
cargo run --features dev -- --scan --ports                    # a real scan
```

`--features dev` links Bevy dynamically so rebuilds take seconds; use the
release build for normal runs. `--exit-after-frames 120 --screenshot out.png`
renders two seconds and quits, which is the smoke test; add
`--select 192.168.1.1 --fly` to see a selection, or `--help-overlay` for the
help card, in that screenshot. `-v` shows Bevy's own messages too.
If the window never appears with "Unable to find a GPU", a `WGPU_BACKEND`
variable in your environment names a backend your GPU cannot use; the program
prints a line and falls back to another one on its own.

## Layout

```
Cargo.toml        the Cargo workspace: scanner + viewer
scanner/          netvisualizer-scanner: finds hosts and fills in their values (sweep, names, ports)
  src/model.rs    Host and Port: the one shape shared by both sides
viewer/           netvisualizer, the program: the window, and scan.rs, which runs the scanner
netvisualizer/    the project spec, and the Python the program embeds (the vendor lookup)
tests/            Python tests
assets/           data files: the MAC vendor list (maclookup.app's CSV export)
```
