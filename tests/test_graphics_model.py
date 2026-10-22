"""Tests for netvisualizer.graphics.model: the JSON contract and round-tripping."""

import json
import tempfile
import unittest
from dataclasses import dataclass
from ipaddress import IPv4Address, ip_address
from pathlib import Path

from netvisualizer.graphics import CONTRACT_VERSION, Link, NetworkMap, from_hosts, host_id
from netvisualizer.graphics.model import host_from_dict, host_to_dict
from netvisualizer.model import Host, Port

# A saved 27-host map, also the Rust viewer's test fixture: every role, ports, extras.
FIXTURE = Path(__file__).resolve().parent.parent / "viewer" / "tests" / "fixtures" / "demo.json"


def small_map():
    hosts = [Host(ip=ip_address(f"10.0.0.{n}"), open_ports=[Port(22, "tcp")]) for n in range(1, 6)]
    return from_hosts(hosts, scanner_ip="10.0.0.1", gateway_ip="10.0.0.2", network="10.0.0.0/24")


class HostSerialisation(unittest.TestCase):
    def test_graphics_keys_and_host_fields_are_present(self):
        host = Host(ip=ip_address("10.0.0.7"), hostname="nas.lan", open_ports=[Port(22, "tcp")])
        data = host_to_dict(host, role="host", position=(1, 2, 3))

        self.assertEqual(data["id"], "10.0.0.7")
        self.assertEqual(data["label"], "nas.lan")
        self.assertEqual(data["role"], "host")
        self.assertEqual(data["position"], [1.0, 2.0, 3.0])
        self.assertEqual(data["ip"], "10.0.0.7")          # address serialised as a string
        self.assertEqual(data["hostname"], "nas.lan")
        self.assertEqual(data["open_ports"], [{"number": 22, "proto": "tcp", "service": None}])
        json.dumps(data)  # must be JSON-safe

    def test_label_falls_back_to_ip(self):
        host = Host(ip=ip_address("10.0.0.8"))
        self.assertEqual(host_to_dict(host)["label"], "10.0.0.8")

    def test_position_may_be_missing(self):
        self.assertIsNone(host_to_dict(Host(ip=ip_address("10.0.0.9")))["position"])

    def test_new_backend_fields_are_serialised_automatically(self):
        @dataclass
        class RichHost(Host):
            mac: str = "aa:bb:cc:dd:ee:ff"

        data = host_to_dict(RichHost(ip=ip_address("10.0.0.1")))
        self.assertEqual(data["mac"], "aa:bb:cc:dd:ee:ff")

    def test_host_round_trip_restores_types(self):
        original = Host(
            ip=ip_address("10.0.0.7"),
            hostname="nas.lan",
            open_ports=[Port(22, "tcp"), Port(53, "udp")],
            is_local=False,
            extra={"vendor": "Synology"},
        )
        restored = host_from_dict(json.loads(json.dumps(host_to_dict(original, "gateway", (0, 0, 0)))))

        self.assertEqual(restored, original)
        self.assertIsInstance(restored.ip, IPv4Address)
        self.assertIsInstance(restored.open_ports[0], Port)

    def test_unknown_keys_are_ignored_when_loading(self):
        restored = host_from_dict({"ip": "10.0.0.1", "future_field": 42, "id": "x"})
        self.assertEqual(host_id(restored), "10.0.0.1")


class NetworkMapSerialisation(unittest.TestCase):
    def test_round_trip_is_lossless(self):
        net = NetworkMap.load(FIXTURE)
        net.touch()
        copy = NetworkMap.from_json(net.to_json())

        self.assertEqual(copy.to_dict(), net.to_dict())
        self.assertEqual(copy.ids(), net.ids())
        self.assertEqual(copy.roles, net.roles)
        self.assertEqual(copy.positions, net.positions)
        self.assertEqual(copy.links, net.links)
        self.assertEqual(copy.scan, net.scan)
        self.assertEqual(copy.revision, 1)

    def test_document_shape(self):
        doc = small_map().to_dict()
        self.assertEqual(doc["version"], CONTRACT_VERSION)
        self.assertEqual(set(doc), {"version", "revision", "generated_at", "scan", "hosts", "links"})
        self.assertEqual(doc["scan"]["type"], "ping_sweep")
        self.assertTrue(all({"id", "label", "role", "position"} <= set(h) for h in doc["hosts"]))

    def test_save_and_load(self):
        net = small_map()
        with tempfile.TemporaryDirectory() as tmp:
            path = net.save(Path(tmp) / "map.json")
            loaded = NetworkMap.load(path)
        self.assertEqual(loaded.to_dict(), net.to_dict())

    def test_newer_contract_version_is_rejected(self):
        with self.assertRaises(ValueError):
            NetworkMap.from_dict({"version": CONTRACT_VERSION + 1, "hosts": []})

    def test_touch_bumps_revision(self):
        net = NetworkMap()
        self.assertEqual(net.revision, 0)
        net.touch()
        net.touch()
        self.assertEqual(net.revision, 2)

    def test_role_validation(self):
        net = NetworkMap(hosts=[Host(ip=ip_address("10.0.0.1"))])
        with self.assertRaises(ValueError):
            net.set_role(net.hosts[0], "mainframe")
        net.set_role(net.hosts[0], "gateway")
        self.assertEqual(net.role_of(net.hosts[0]), "gateway")

    def test_links_round_trip(self):
        net = NetworkMap(links=[Link("a", "b"), Link("a", "c", kind="route")])
        self.assertEqual(NetworkMap.from_dict(net.to_dict()).links, net.links)


if __name__ == "__main__":
    unittest.main()
