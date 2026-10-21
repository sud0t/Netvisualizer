"""Pynmapper: find the hosts on a network and show them as a 3D map.

    python -m pynmapper                      # detect the interface, sweep its subnet, open the map
    python -m pynmapper 192.168.1.0/24       # sweep this network instead
    python -m pynmapper --ports              # then try common TCP ports on the hosts that answered
    python -m pynmapper --ports 22,80,8000-8010
    python -m pynmapper --save last.json     # keep the result; --json last.json replays it later
    python -m pynmapper --demo               # a fake LAN, no network needed
    python -m pynmapper --list-interfaces    # which networks this machine is on

- __main__.py decides which steps run and in what order (this file).
- netutils.py finds hosts and fills in their values.
- graphics/ turns them into the map document and serves it.
- viewer/ (Rust) draws whatever map it is given, in its own window.

The map server starts before the scan, so the viewer opens at once and
redraws itself as each stage (sweep, names, ports) finishes.
"""

from __future__ import annotations

import argparse
import asyncio
import logging
import os
import socket
import subprocess
import sys
from collections.abc import Callable
from ipaddress import IPv4Network, ip_network
from pathlib import Path

from pynmapper import graphics, netutils
from pynmapper.graphics import MapServer, NetworkMap, demo_map
from pynmapper.graphics.server import DEFAULT_HOST, DEFAULT_PORT
from pynmapper.model import Host
from pynmapper.netutils import LocalNetwork

log = logging.getLogger("pynmapper")

MAX_SWEEP = 65536  # addresses; a /16 takes a few minutes, anything bigger is a mistake

# The viewer, built with `cargo build --release` in viewer/.
VIEWER_DIR = Path(__file__).resolve().parent.parent / "viewer"
VIEWER_BINARY = VIEWER_DIR / "target" / "release" / ("pynmapper-viewer.exe" if os.name == "nt" else "pynmapper-viewer")

Publish = Callable[[list[Host], str], None]  # (hosts so far, stage text) -> redraw the map


# ----- argument parsing -------------------------------------------------------------

def parse_ports(text: str) -> list[int]:
    """"22,80,8000-8010" -> [22, 80, 8000, ..., 8010]; "common" -> the built-in list."""
    if text.strip().lower() == "common":
        return list(netutils.COMMON_TCP_PORTS)
    ports: set[int] = set()
    for part in text.split(","):
        part = part.strip()
        if not part:
            continue
        low, _, high = part.partition("-")
        try:
            first = int(low)
            last = int(high) if high else first
        except ValueError:
            raise argparse.ArgumentTypeError(f"not a port or range: {part!r}") from None
        if not 1 <= first <= last <= 65535:
            raise argparse.ArgumentTypeError(f"port out of range: {part}")
        ports.update(range(first, last + 1))
    if not ports:
        raise argparse.ArgumentTypeError("no ports given")
    return sorted(ports)


def parse_network(text: str) -> IPv4Network:
    try:
        net = ip_network(text, strict=False)
    except ValueError as exc:
        raise argparse.ArgumentTypeError(str(exc)) from None
    if net.version != 4:
        raise argparse.ArgumentTypeError("only IPv4 networks can be swept")
    if net.num_addresses > MAX_SWEEP:
        raise argparse.ArgumentTypeError(f"{net} has {net.num_addresses} addresses; a /16 is the largest sweep allowed")
    return net


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="python -m pynmapper",
        description="Find the hosts on a network and show them as a 3D map in its own window.",
        epilog="examples:  python -m pynmapper --ports        python -m pynmapper 10.0.0.0/24 --save scan.json",
    )
    parser.add_argument("network", nargs="?", type=parse_network,
                        help="network to sweep in CIDR form, e.g. 192.168.1.0/24 "
                             "(default: the subnet of the interface with the default route)")
    parser.add_argument("-i", "--interface", help="sweep the subnet of this interface instead")
    parser.add_argument("--list-interfaces", action="store_true", help="show this machine's networks and exit")
    parser.add_argument("--ports", nargs="?", const="common", type=parse_ports, metavar="PORTS",
                        help="after the sweep, try TCP connections to these ports on every host that answered: "
                             "a list like 22,80,8000-8010, or 'common' (the default when no list is given)")
    parser.add_argument("--no-names", action="store_true", help="skip the reverse DNS lookups")
    parser.add_argument("--timeout", type=float, default=1.0, help="seconds to wait for each ping / connection (default 1)")
    parser.add_argument("--concurrency", type=int, default=254, help="pings or connections in flight at once (default 254)")

    instead = parser.add_argument_group("instead of scanning")
    instead.add_argument("--json", type=Path, metavar="FILE", help="show a map saved earlier with --save")
    instead.add_argument("--demo", action="store_true", help="show a generated fake LAN")

    output = parser.add_argument_group("output")
    output.add_argument("--save", type=Path, metavar="FILE", help="write the finished map as JSON")
    output.add_argument("--no-serve", action="store_true", help="exit after the scan instead of serving the map (pairs with --save)")
    output.add_argument("--no-open", action="store_true",
                        help="serve the map without starting the viewer (point one at the printed URL later)")
    output.add_argument("--port", type=int, default=DEFAULT_PORT, help=f"map server port (default {DEFAULT_PORT}, 0 = any free)")
    output.add_argument("--host", default=DEFAULT_HOST,
                        help=f"map server bind address (default {DEFAULT_HOST}; 0.0.0.0 lets other machines view it)")
    parser.add_argument("-v", "--verbose", action="store_true", help="show every step")
    return parser


# ----- the scan -------------------------------------------------------------------------

def build_map(hosts: list[Host], local: LocalNetwork, network: IPv4Network, stage: str | None) -> NetworkMap:
    """Wrap what we know so far into the map the viewer draws."""
    net_map = graphics.from_hosts(hosts, scanner_ip=local.ip, gateway_ip=local.gateway, network=str(network))
    net_map.scan.stage = stage
    return net_map


def ensure_gateway(hosts: list[Host], local: LocalNetwork) -> list[Host]:
    """Keep the gateway on the map even when it ignores ping (many routers do); it is drawn greyed out."""
    if local.gateway and all(h.ip != local.gateway for h in hosts):
        hosts.append(Host(ip=local.gateway, extra={"status": "down", "note": "did not answer ping"}))
    return hosts


async def run_scan(args: argparse.Namespace, local: LocalNetwork, network: IPv4Network, publish: Publish) -> list[Host]:
    """The scan stages in order; ``publish`` is called after each one so the map can redraw."""
    def progress(done: int, total: int) -> None:
        if done < total:
            log.info("  %d / %d", done, total)

    hosts = await netutils.async_ping_sweep(network, timeout=args.timeout, concurrency=args.concurrency, progress=progress)
    netutils.mark_local(hosts, local.network)
    ensure_gateway(hosts, local)

    if not args.no_names:
        publish(hosts, "resolving names…")
        await netutils.resolve_hostnames(hosts)
        me = next((h for h in hosts if h.ip == local.ip), None)
        if me and not me.hostname:
            me.hostname = socket.gethostname()  # reverse DNS rarely knows our own name

    if args.ports:
        publish(hosts, f"trying {len(args.ports)} TCP ports…")
        await netutils.async_port_scan(hosts, args.ports, timeout=args.timeout, concurrency=args.concurrency, progress=progress)

    publish(hosts, "done")
    return hosts


def serve(net_map: NetworkMap, args: argparse.Namespace) -> tuple[MapServer | None, subprocess.Popen | None]:
    """Start the map server (unless --no-serve) and open the map (unless --no-open).

    Returns the server and, when the viewer was started, its process.
    """
    if args.no_serve:
        return None, None
    server = MapServer(net_map, host=args.host, port=args.port).start()
    print(f"Pynmapper map: {server.url}  (Ctrl-C to stop)", flush=True)
    viewer = None if args.no_open else open_viewer(server, quiet=not args.verbose)
    return server, viewer


def viewer_is_built() -> bool:
    """True when the viewer binary exists; otherwise say how to build it."""
    if VIEWER_BINARY.is_file():
        return True
    print(f"error: the viewer is not built yet ({VIEWER_BINARY} is missing); build it with "
          f"`cargo build --release` in {VIEWER_DIR}, or pass --no-open to only serve the map", file=sys.stderr)
    return False


def open_viewer(server: MapServer, quiet: bool = True) -> subprocess.Popen:
    """Start the viewer on ``server.url``; it polls the server, so it redraws as the scan goes on."""
    env = dict(os.environ)
    if quiet:
        env.setdefault("RUST_LOG", "warn")  # Bevy logs its GPU setup at info level
    return subprocess.Popen([str(VIEWER_BINARY), server.url], env=env)


def wait(server: MapServer, viewer: subprocess.Popen | None) -> int:
    """Keep serving until the viewer window is closed, or until Ctrl-C without a viewer.

    Returns the exit status for main(): 1 when the viewer failed (no usable GPU, say).
    """
    if viewer is None:
        server.serve_forever()
        return 0
    try:
        status = viewer.wait()
    except KeyboardInterrupt:  # Ctrl-C reaches the viewer too and closes it
        stop_viewer(viewer)
        status = 0
    server.stop()
    if status != 0:
        print(f"error: the viewer exited with status {status}", file=sys.stderr)
        return 1
    return 0


def stop_viewer(viewer: subprocess.Popen | None) -> None:
    if viewer is None:
        return
    try:
        viewer.wait(timeout=3)
    except subprocess.TimeoutExpired:
        viewer.terminate()


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    logging.basicConfig(
        level=logging.DEBUG if args.verbose else logging.INFO,
        format="%(levelname)s %(name)s: %(message)s" if args.verbose else "%(message)s",
    )
    logging.getLogger("pynmapper.graphics").setLevel(logging.WARNING)  # the HTTP server is chatty at INFO

    if args.list_interfaces:
        networks = netutils.list_local_networks()
        if not networks:
            print("no IPv4 networks found (is scapy installed?)", file=sys.stderr)
            return 1
        for n in networks:
            print(f"{n.interface:<12} {str(n.ip):<16} {str(n.network):<20} gateway {n.gateway or '-'}")
        return 0

    # Check before scanning: without a viewer the map has nowhere to go.
    if not (args.no_serve or args.no_open) and not viewer_is_built():
        return 1

    # ----- replay / demo: nothing to scan ---------------------------------
    if args.demo or args.json:
        try:
            net_map = demo_map() if args.demo else NetworkMap.load(args.json)
        except (OSError, ValueError) as exc:
            print(f"error: could not load map: {exc}", file=sys.stderr)
            return 1
        if args.save:
            net_map.save(args.save)
        server, viewer = serve(net_map, args)
        return wait(server, viewer) if server else 0

    # ----- a real scan -------------------------------------------------------
    try:
        local = netutils.get_local_network(args.interface)
    except ValueError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2
    if local.ip.is_loopback:
        print("error: no network connection found (only loopback)", file=sys.stderr)
        return 2
    network = args.network or local.network
    log.info("sweeping %s from %s on %s%s", network, local.ip, local.interface,
             f" (gateway {local.gateway})" if local.gateway else "")

    server, viewer = serve(build_map([], local, network, f"sweeping {network}…"), args)

    def publish(hosts: list[Host], stage: str) -> None:
        log.info("%s", stage)
        if server:
            server.update(build_map(hosts, local, network, stage))

    try:
        hosts = asyncio.run(run_scan(args, local, network, publish))
    except KeyboardInterrupt:
        print("\ninterrupted", file=sys.stderr)
        stop_viewer(viewer)
        if server:
            server.stop()
        return 130

    net_map = build_map(hosts, local, network, "done")
    if args.save:
        net_map.save(args.save)
        log.info("saved %d hosts to %s", len(net_map), args.save)
    return wait(server, viewer) if server else 0


if __name__ == "__main__":
    sys.exit(main())
