"""Command line for developing and inspecting the graphics layer.

    python -m pynmapper.graphics --demo                 # serve a fake LAN until Ctrl-C
    python -m pynmapper.graphics --demo --count 60      # bigger fake LAN
    python -m pynmapper.graphics --json scan.json       # replay a saved map
    python -m pynmapper.graphics --demo --save scan.json
    python -m pynmapper.graphics --demo --dump          # print the JSON and exit

Nothing is opened: follow the served map with ``cargo run --features dev`` in
``viewer/``. The real entry point (``python -m pynmapper``) builds a map from a
scan, serves it the same way and starts the release viewer itself.
"""

from __future__ import annotations

import argparse
import logging
import sys
from pathlib import Path

from .demo import demo_map
from .layout import LAYOUTS
from .model import NetworkMap
from .server import DEFAULT_HOST, DEFAULT_PORT, MapServer


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="python -m pynmapper.graphics",
        description="Serve a network map for the native viewer to follow.",
    )
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--demo", action="store_true", help="serve a generated fake LAN")
    source.add_argument("--json", type=Path, metavar="FILE", help="serve a map saved earlier with --save")

    parser.add_argument("--count", type=int, default=25, help="hosts in the demo LAN (default 25)")
    parser.add_argument("--seed", type=int, default=1, help="random seed for the demo LAN")
    parser.add_argument("--layout", choices=sorted(LAYOUTS), help="re-run this layout before serving")

    parser.add_argument("--host", default=DEFAULT_HOST, help=f"bind address (default {DEFAULT_HOST})")
    parser.add_argument("--port", type=int, default=DEFAULT_PORT, help=f"port (default {DEFAULT_PORT}, 0 = any free)")
    parser.add_argument("--save", type=Path, metavar="FILE", help="also write the map JSON to FILE")
    parser.add_argument("--dump", action="store_true", help="print the map JSON to stdout and exit")
    parser.add_argument("-v", "--verbose", action="store_true", help="log every HTTP request")
    return parser


def load_map(args: argparse.Namespace) -> NetworkMap:
    if args.demo:
        net_map = demo_map(count=args.count, seed=args.seed)
    else:
        net_map = NetworkMap.load(args.json)
    if args.layout:
        net_map.apply_layout(LAYOUTS[args.layout])
    return net_map


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    logging.basicConfig(
        level=logging.DEBUG if args.verbose else logging.INFO,
        format="%(levelname)s %(name)s: %(message)s",
    )

    try:
        net_map = load_map(args)
    except (OSError, ValueError) as exc:
        print(f"error: could not load map: {exc}", file=sys.stderr)
        return 1

    if args.save:
        net_map.save(args.save)
        print(f"saved {len(net_map)} hosts to {args.save}", file=sys.stderr)

    if args.dump:
        print(net_map.to_json())
        return 0

    server = MapServer(net_map, host=args.host, port=args.port).start()
    print(f"Pynmapper map: {server.url}  ({len(net_map)} hosts, Ctrl-C to stop)", flush=True)
    server.serve_forever()
    return 0


if __name__ == "__main__":
    sys.exit(main())
