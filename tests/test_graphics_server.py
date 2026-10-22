"""Tests for netvisualizer.graphics.server: /api/map, /api/health, revisions."""

import json
import unittest
import urllib.error
import urllib.request
from ipaddress import ip_address

from netvisualizer.graphics import MapServer, NetworkMap, from_hosts
from netvisualizer.model import Host


def get(url):
    with urllib.request.urlopen(url, timeout=5) as res:
        return res.status, res.headers, res.read()


def lan(count):
    """A scanner, a gateway and ``count`` other hosts."""
    hosts = [Host(ip=ip_address(f"192.168.1.{n}")) for n in range(1, count + 3)]
    return from_hosts(hosts, scanner_ip="192.168.1.1", gateway_ip="192.168.1.2", network="192.168.1.0/24")


class MapServerTests(unittest.TestCase):
    def setUp(self):
        self.server = MapServer(lan(5), port=0).start()
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
        self.server.update(lan(2))
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

    def test_only_the_api_is_served(self):
        for path in ("", "map.json", "api/"):
            with self.subTest(path=path), self.assertRaises(urllib.error.HTTPError) as ctx:
                get(self.base + path)
            self.assertEqual(ctx.exception.code, 404)
            ctx.exception.close()

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

    def test_context_manager(self):
        with MapServer(port=0) as server:
            self.assertTrue(server.running)
        self.assertFalse(server.running)


class RevisionTests(unittest.TestCase):
    def test_fresh_maps_still_get_increasing_revisions(self):
        # __main__ builds a brand new NetworkMap (revision 0) after every scan
        # stage; the viewer only reloads when the revision changes.
        server = MapServer(NetworkMap())
        seen = []
        for _ in range(3):
            server.update(NetworkMap())
            seen.append(server.revision)
        self.assertEqual(seen, [1, 2, 3])
        server.update(NetworkMap(revision=10))
        self.assertEqual(server.revision, 11)


if __name__ == "__main__":
    unittest.main()
