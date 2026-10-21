# Pynmapper

Finds the hosts on your network and shows them as a 3D map in the browser.

A ping sweep finds the hosts, reverse DNS names them, and an optional TCP
connect scan lists their open ports. The result is drawn with three.js: fly
around with the keyboard and mouse, click a host for its details. No root
needed, no build step, no Python dependencies beyond the standard library
(scapy, if installed, is used only to read the routing table).

## Run

```sh
python -m pynmapper                      # detect the interface, sweep its subnet, open the map
python -m pynmapper 192.168.1.0/24       # sweep this network instead
python -m pynmapper --ports              # then try common TCP ports on the hosts that answered
python -m pynmapper --ports 22,80,8000-8010
python -m pynmapper --save last.json     # keep the result; --json last.json replays it later
python -m pynmapper --demo               # a fake LAN, no network needed
python -m pynmapper --list-interfaces    # which networks this machine is on
python -m pynmapper --help               # everything else
```

The map opens at `http://127.0.0.1:8765/` before the scan finishes and
redraws itself as each stage completes.

## Controls

| Key / mouse | Action |
|---|---|
| `W` `A` `S` `D`, `Q` `E` | fly; down / up |
| `Shift`, mouse wheel | faster; change speed |
| right mouse (hold) | look around |
| left click, double click | select a host; fly to it |
| `F`, `R` | fly to the selection; reset the view |
| `/`, `Esc`, `?` | filter the host list; deselect; help |

## Native viewer (work in progress)

`viewer/` is a Rust crate (Bevy 0.20) that draws the same JSON map in its own
window, for machines where no browser should be involved. So far it renders a
saved scan with the same fly controls, picking and labels as the page; the
sidebar and live updates are the next steps (milestone 7 in
`pynmapper/project_spec.md`).

```sh
cd viewer
cargo run --features dev -- --json tests/fixtures/demo.json   # first build takes ~10 min
cargo run --features dev -- --json ../last.json               # a map saved with --save
cargo test --features dev
```

`rust-toolchain.toml` pins the compiler Bevy needs (rustc 1.99.1); rustup
installs it on the first `cargo` command here. `--exit-after-frames 120
--screenshot out.png` renders two seconds and quits, which is the smoke test;
add `--select 192.168.1.1 --fly` to see a selection in that screenshot.
If the window never appears with "Unable to find a GPU", a `WGPU_BACKEND`
variable in your environment names a backend your GPU cannot use; the viewer
prints a line and falls back to another one on its own.

## Layout

```
pynmapper/
  __main__.py     which steps run, in what order; starts the map server first
  netutils.py     finds hosts and fills in their values (sweep, names, ports)
  model.py        Host and Port: the one shape shared by both sides
  graphics/       turns Hosts into a JSON map and serves the three.js page
tests/            python -m unittest discover tests
viewer/           native Rust viewer for the same JSON (Bevy); cargo test
```
