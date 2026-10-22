"""Serve a :class:`NetworkMap` over HTTP as JSON, stdlib only.

The native viewer (``viewer/``) polls two endpoints:

- ``/api/health``  ``{"ok": true, "revision": N}``, cheap enough to ask every 2 s;
- ``/api/map``     the current map as JSON (see ``graphics.model``), with a
                   ``revision`` number that goes up on every :meth:`MapServer.update`
                   so the viewer knows when to download it again and redraw.

Any other path is a 404. Everything is served from 127.0.0.1 by default,
because the map describes the user's network; pass ``host="0.0.0.0"`` to let
a viewer on another machine follow it.
"""

from __future__ import annotations

import errno
import json
import logging
import threading
from functools import partial
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlsplit

from .model import NetworkMap

log = logging.getLogger(__name__)

DEFAULT_HOST = "127.0.0.1"
DEFAULT_PORT = 8765


class MapServer:
    """Holds the current map and serves it on a background thread.

    ::

        server = MapServer(net_map).start()
        ...                       # later, after another scan:
        server.update(new_map)    # bumps the revision, the viewer redraws
        server.stop()

    ``port=0`` picks any free port. With ``fallback_to_free_port`` (the
    default) a busy ``port`` falls back to a free one instead of failing.
    """

    def __init__(
        self,
        net_map: NetworkMap | None = None,
        host: str = DEFAULT_HOST,
        port: int = DEFAULT_PORT,
        fallback_to_free_port: bool = True,
    ) -> None:
        self.host = host
        self.port = port
        self.fallback_to_free_port = fallback_to_free_port
        self._lock = threading.Lock()
        self._map = net_map if net_map is not None else NetworkMap()
        self._payload: bytes | None = None
        self._httpd: ThreadingHTTPServer | None = None
        self._thread: threading.Thread | None = None

    # ----- map access ---------------------------------------------------------
    @property
    def map(self) -> NetworkMap:
        return self._map

    @property
    def revision(self) -> int:
        return self._map.revision

    def update(self, net_map: NetworkMap, bump: bool = True) -> MapServer:
        """Replace the served map. Bumps its revision unless ``bump`` is false.

        The revision continues from the map being replaced, so a caller that
        builds a fresh map for every stage of a scan (revision 0 each time)
        still produces 1, 2, 3, ... and the polling viewer notices every update.
        """
        with self._lock:
            if bump:
                net_map.revision = max(net_map.revision, self._map.revision)
                net_map.touch()
            self._map = net_map
            self._payload = None
        return self

    def payload(self) -> bytes:
        """The map as UTF-8 JSON bytes, cached until the next update."""
        with self._lock:
            if self._payload is None:
                self._payload = json.dumps(self._map.to_dict()).encode("utf-8")
            return self._payload

    # ----- lifecycle ------------------------------------------------------------
    @property
    def url(self) -> str:
        return f"http://{self.host}:{self.port}/"

    @property
    def running(self) -> bool:
        return self._thread is not None and self._thread.is_alive()

    def start(self) -> MapServer:
        if self.running:
            return self
        handler = partial(_Handler, self)
        try:
            self._httpd = ThreadingHTTPServer((self.host, self.port), handler)
        except OSError as exc:
            if exc.errno != errno.EADDRINUSE or not self.fallback_to_free_port:
                raise
            log.warning("port %d is busy, picking a free one", self.port)
            self._httpd = ThreadingHTTPServer((self.host, 0), handler)
        self._httpd.daemon_threads = True
        self.port = self._httpd.server_address[1]
        self._thread = threading.Thread(
            target=self._httpd.serve_forever, kwargs={"poll_interval": 0.1}, name="netvisualizer-http", daemon=True
        )
        self._thread.start()
        log.info("serving map at %s", self.url)
        return self

    def stop(self) -> None:
        if self._httpd is not None:
            self._httpd.shutdown()
            self._httpd.server_close()
            self._httpd = None
        if self._thread is not None:
            self._thread.join(timeout=5)
            self._thread = None

    def serve_forever(self) -> None:
        """Start (if needed) and block until Ctrl-C."""
        self.start()
        assert self._thread is not None
        try:
            while self._thread.is_alive():
                self._thread.join(timeout=0.5)
        except KeyboardInterrupt:
            log.info("stopping")
        finally:
            self.stop()

    def __enter__(self) -> MapServer:
        return self.start()

    def __exit__(self, *exc: object) -> None:
        self.stop()


class _Handler(BaseHTTPRequestHandler):
    """The two ``/api/`` endpoints; any other path is a 404."""

    def __init__(self, map_server: MapServer, *args, **kwargs) -> None:
        self.map_server = map_server
        super().__init__(*args, **kwargs)

    def do_GET(self) -> None:  # noqa: N802  (http.server naming)
        path = urlsplit(self.path).path
        if path == "/api/map":
            self._send_bytes(self.map_server.payload(), "application/json")
        elif path == "/api/health":
            body = json.dumps({"ok": True, "revision": self.map_server.revision}).encode()
            self._send_bytes(body, "application/json")
        else:
            self.send_error(HTTPStatus.NOT_FOUND, "no such endpoint")

    def _send_bytes(self, body: bytes, content_type: str) -> None:
        self.send_response(HTTPStatus.OK)
        self.send_header("Content-Type", f"{content_type}; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def end_headers(self) -> None:
        # The map changes behind the same URL, so nothing may cache it.
        self.send_header("Cache-Control", "no-store")
        super().end_headers()

    def log_message(self, format: str, *args: object) -> None:  # noqa: A002
        log.debug("%s %s", self.address_string(), format % args)

