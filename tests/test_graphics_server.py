"""Tests for pynmapper.graphics.server: /api/map, revisions, static files."""

import json
import tempfile
import unittest
import urllib.error
import urllib.request
from pathlib import Path

from pynmapper.graphics import MapServer, NetworkMap, demo_map


def get(url):
    with urllib.request.urlopen(url, timeout=5) as res:
        return res.status, res.headers, res.read()


class MapServerTests(unittest.TestCase):
    def setUp(self):
        self.server = MapServer(demo_map(count=5), port=0).start()
        self.base = self.server.url

    def tearDown(self):
        self.server.stop()

    def test_binds_a_free_port_and_reports_url(self):
        self.assertNotEqual(self.server.port, 0)
        self.assertTrue(self.base.startswith("http://127.0.0.1:"))
        self.assertTrue(self.server.running)

    def test_api_map_returns_the_contract(self):
        status, headers, body = get(self.base + "api/map")
        self.assertEqual(status, 200)
        self.assertTrue(headers["Content-Type"].startswith("application/json"))
        self.assertEqual(headers["Cache-Control"], "no-store")
        doc = json.loads(body)
        self.assertEqual(doc, self.server.map.to_dict())
        self.assertEqual(len(doc["hosts"]), 7)   # 5 + scanner + gateway

    def test_update_bumps_revision_and_changes_payload(self):
        before = json.loads(get(self.base + "api/map")[2])
        self.server.update(demo_map(count=2, seed=9))
        after = json.loads(get(self.base + "api/map")[2])
        self.assertEqual(before["revision"], 0)
        self.assertEqual(after["revision"], 1)
        self.assertEqual(len(after["hosts"]), 4)

    def test_update_without_bump_keeps_revision(self):
        self.server.update(NetworkMap(), bump=False)
        self.assertEqual(json.loads(get(self.base + "api/map")[2])["revision"], 0)

    def test_health(self):
        doc = json.loads(get(self.base + "api/health")[2])
        self.assertEqual(doc, {"ok": True, "revision": 0})

    def test_unknown_api_is_404(self):
        with self.assertRaises(urllib.error.HTTPError) as ctx:
            get(self.base + "api/nope")
        self.assertEqual(ctx.exception.code, 404)
        ctx.exception.close()

    def test_index_is_served(self):
        status, headers, body = get(self.base)
        self.assertEqual(status, 200)
        self.assertTrue(headers["Content-Type"].startswith("text/html"))
        self.assertIn(b"<!doctype html>", body.lower())

    def test_stop_then_start_again(self):
        self.server.stop()
        self.assertFalse(self.server.running)
        self.server.start()
        self.assertTrue(self.server.running)
        self.assertEqual(get(self.server.url + "api/health")[0], 200)


class MapServerSetup(unittest.TestCase):
    def test_busy_port_falls_back_to_a_free_one(self):
        first = MapServer(port=0).start()
        try:
            second = MapServer(port=first.port).start()
            try:
                self.assertNotEqual(second.port, first.port)
            finally:
                second.stop()
        finally:
            first.stop()

    def test_busy_port_raises_when_fallback_disabled(self):
        first = MapServer(port=0).start()
        try:
            with self.assertRaises(OSError):
                MapServer(port=first.port, fallback_to_free_port=False).start()
        finally:
            first.stop()

    def test_missing_web_dir_is_an_error(self):
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaises(FileNotFoundError):
                MapServer(web_dir=Path(tmp) / "missing").start()

    def test_context_manager(self):
        with MapServer(port=0) as server:
            self.assertTrue(server.running)
        self.assertFalse(server.running)


if __name__ == "__main__":
    unittest.main()
