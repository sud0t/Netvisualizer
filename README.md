# Netvisualizer

Finds the hosts on your network and shows them as a 3D map in its own window.

A ping sweep finds the hosts, reverse DNS names them, and an optional TCP
connect scan lists their open ports. It is one Rust program: the scanner
builds its own ICMP echo requests and sends them on an unprivileged ICMP
socket (no root needed, see below), and the map is drawn with Bevy: fly
around with the keyboard and mouse, click a host for its details.

## Build (once)

```sh
cargo build --release      # in the repo root; about 10 min the first time; needs Rust (rustup)
```

`rust-toolchain.toml` pins the compiler Bevy needs (rustc 1.99.1); rustup
installs it on the first `cargo` command in the repo. On Debian/Ubuntu the build also
needs `libasound2-dev libudev-dev libwayland-dev libxkbcommon-dev`.
The program is `target/release/netvisualizer` (or `cargo run --release --`
followed by its arguments).

The program embeds Python 3.15 for a few features (the `netvisualizer/` package):
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
netvisualizer                                   # the window; nothing is scanned
netvisualizer --scan                            # sweep the subnet of the interface with the default route
netvisualizer --scan 192.168.1.0/24             # this network instead
netvisualizer --scan --ports                    # then try common TCP ports on the hosts that answered
netvisualizer --scan --ports 22,80,8000-8010
netvisualizer --scan --save last.json           # keep the result; --json last.json shows it again
netvisualizer --scan --no-window                # scan and print the hosts, no window
netvisualizer --list-interfaces                 # which networks this machine is on
netvisualizer --help                            # everything else
```

Nothing touches the network without `--scan` (a Scan button comes with the UI
work). The window opens straight away and the map redraws as each stage of
the scan finishes; closing the window ends the program. To try it without
scanning, show the saved fake LAN: `netvisualizer --json viewer/tests/fixtures/demo.json`.

### Privileges

The sweep needs no root. Linux lets a process open an ICMP "datagram" socket
when its group is inside `net.ipv4.ping_group_range` (most distributions
allow every group: check with `sysctl net.ipv4.ping_group_range`). Without
that, run as root and the sweep uses a raw socket; failing both, it runs the
system `ping` once per address.

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
netvisualizer/    the project spec (and, from Milestone 5.7, Python the program embeds)
tests/            Python tests
assets/           data files: the MAC vendor CSV (Milestone 6)
```
