# Pynmapper

Finds the hosts on your network and shows them as a 3D map in its own window.

A ping sweep finds the hosts, reverse DNS names them, and an optional TCP
connect scan lists their open ports. The map is drawn by a native viewer
(Rust, Bevy) that needs no browser; the same map is also served as a three.js
page for any browser or another machine. Fly around with the keyboard and
mouse, click a host for its details. No root needed, no Python dependencies
beyond the standard library (scapy, if installed, is used only to read the
routing table).

## Build the viewer (once)

```sh
cd viewer
cargo build --release      # about 10 min the first time; needs Rust (rustup)
```

`rust-toolchain.toml` pins the compiler Bevy needs (rustc 1.99.1); rustup
installs it on the first `cargo` command there. On Debian/Ubuntu the build also
needs `libasound2-dev libudev-dev libwayland-dev libxkbcommon-dev`. Until the
viewer is built, `python -m pynmapper` says so and opens the browser instead.

## Run

```sh
python -m pynmapper                      # detect the interface, sweep its subnet, open the map
python -m pynmapper 192.168.1.0/24       # sweep this network instead
python -m pynmapper --ports              # then try common TCP ports on the hosts that answered
python -m pynmapper --ports 22,80,8000-8010
python -m pynmapper --save last.json     # keep the result; --json last.json replays it later
python -m pynmapper --demo               # a fake LAN, no network needed
python -m pynmapper --viewer web         # draw the map in a browser tab instead
python -m pynmapper --list-interfaces    # which networks this machine is on
python -m pynmapper --help               # everything else
```

The map server starts at `http://127.0.0.1:8765/` before the scan does, and
the viewer redraws itself as each stage completes. Closing the viewer window
ends the program; with `--viewer web` (or `--no-open`) it runs until Ctrl-C.
If the native viewer cannot start (no usable GPU), the map opens in the
browser instead.

## Controls

| Key / mouse | Action |
|---|---|
| `W` `A` `S` `D`, `Q` `E` | fly; down / up |
| `Shift`, mouse wheel | faster; change speed |
| right mouse (hold) | look around |
| left click, double click | select a host; fly to it |
| `F`, `R` | fly to the selection; reset the view |
| `/`, `Esc`, `?` | filter the host list; deselect; help |

## Native viewer

`viewer/` is a Rust crate (Bevy 0.20 + bevy_egui) that follows the same map
server the page does, or draws a saved map:

```sh
cd viewer
cargo run --features dev                                      # follow http://127.0.0.1:8765
cargo run --features dev -- http://10.0.0.5:8765              # a map served elsewhere (--host 0.0.0.0)
cargo run --features dev -- --json tests/fixtures/demo.json   # a saved map, no server
cargo test --features dev
```

`--features dev` links Bevy dynamically so rebuilds take seconds; the Python
launcher runs the release build. `--exit-after-frames 120 --screenshot out.png`
renders two seconds and quits, which is the smoke test; add
`--select 192.168.1.1 --fly` to see a selection, or `--help-overlay` for the
help card, in that screenshot.
If the window never appears with "Unable to find a GPU", a `WGPU_BACKEND`
variable in your environment names a backend your GPU cannot use; the viewer
prints a line and falls back to another one on its own.

## Layout

```
pynmapper/
  __main__.py     which steps run, in what order; starts the map server, then the viewer
  netutils.py     finds hosts and fills in their values (sweep, names, ports)
  model.py        Host and Port: the one shape shared by both sides
  graphics/       turns Hosts into a JSON map, serves it and the three.js page
tests/            python -m unittest discover tests
viewer/           native Rust viewer for the same JSON (Bevy); cargo test
```
