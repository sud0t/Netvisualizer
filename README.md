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

## Layout

```
pynmapper/
  __main__.py     which steps run, in what order; starts the map server first
  netutils.py     finds hosts and fills in their values (sweep, names, ports)
  model.py        Host and Port: the one shape shared by both sides
  graphics/       turns Hosts into a JSON map and serves the three.js page
tests/            python -m unittest discover tests
```
