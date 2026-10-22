"""Tests for netvisualizer.graphics.adapters: backend output -> NetworkMap."""

import unittest
from ipaddress import ip_address

from netvisualizer.graphics import apply_hostnames, from_hosts, from_ping_sweep, host_id
from netvisualizer.model import Host


def hosts(*addrs):
    return [Host(ip=ip_address(a)) for a in addrs]


class FromHosts(unittest.TestCase):
    def test_scanner_is_added_when_absent_and_becomes_centre(self):
        net = from_hosts(hosts("10.0.0.2", "10.0.0.3"), scanner_ip="10.0.0.1")

        self.assertEqual(net.ids(), ["10.0.0.1", "10.0.0.2", "10.0.0.3"])
        self.assertEqual(net.role_of(net.get("10.0.0.1")), "scanner")
        self.assertEqual(net.scan.scanner_ip, "10.0.0.1")
        self.assertEqual(net.positions["10.0.0.1"], (0.0, 0.0, 0.0))

    def test_scanner_already_in_list_is_not_duplicated(self):
        net = from_hosts(hosts("10.0.0.1", "10.0.0.2"), scanner_ip=ip_address("10.0.0.1"))
        self.assertEqual(len(net), 2)
        self.assertEqual(net.hosts_with_role("scanner")[0].ip, ip_address("10.0.0.1"))

    def test_star_links_from_scanner_to_everyone_else(self):
        net = from_hosts(hosts("10.0.0.2", "10.0.0.3", "10.0.0.4"), scanner_ip="10.0.0.1")
        self.assertEqual(len(net.links), 3)
        self.assertTrue(all(link.source == "10.0.0.1" for link in net.links))
        self.assertEqual({link.target for link in net.links}, {"10.0.0.2", "10.0.0.3", "10.0.0.4"})
        self.assertFalse(any(link.target == "10.0.0.1" for link in net.links))

    def test_no_scanner_means_no_links_and_default_roles(self):
        net = from_hosts(hosts("10.0.0.2", "10.0.0.3"))
        self.assertEqual(net.links, [])
        self.assertEqual({net.role_of(h) for h in net}, {"host"})
        self.assertEqual(len(net.positions), 2)

    def test_gateway_role(self):
        net = from_hosts(hosts("10.0.0.1", "10.0.0.2"), scanner_ip="10.0.0.5", gateway_ip="10.0.0.1")
        self.assertEqual(net.role_of(net.get("10.0.0.1")), "gateway")
        self.assertEqual(net.role_of(net.get("10.0.0.2")), "host")

    def test_duplicates_are_dropped_keeping_first(self):
        first = Host(ip=ip_address("10.0.0.2"), hostname="first")
        second = Host(ip=ip_address("10.0.0.2"), hostname="second")
        net = from_hosts([first, second])
        self.assertEqual(len(net), 1)
        self.assertEqual(net.hosts[0].hostname, "first")

    def test_host_objects_are_not_copied(self):
        original = hosts("10.0.0.2")
        net = from_hosts(original)
        self.assertIs(net.hosts[0], original[0])

    def test_layout_can_be_skipped(self):
        net = from_hosts(hosts("10.0.0.2"), layout=None)
        self.assertEqual(net.positions, {})

    def test_scan_metadata(self):
        net = from_hosts([], scanner_ip="10.0.0.1", network="10.0.0.0/24", scan_type="arp")
        self.assertEqual(net.scan.type, "arp")
        self.assertEqual(net.scan.network, "10.0.0.0/24")


class FromPingSweep(unittest.TestCase):
    def test_accepts_addresses_strings_and_hosts(self):
        results = [ip_address("10.0.0.2"), "10.0.0.3", Host(ip=ip_address("10.0.0.4"), hostname="h4")]
        net = from_ping_sweep(results, scanner_ip="10.0.0.1")
        self.assertEqual(net.ids(), ["10.0.0.1", "10.0.0.2", "10.0.0.3", "10.0.0.4"])
        self.assertEqual(net.get("10.0.0.4").hostname, "h4")

    def test_hostnames_are_applied(self):
        resolved = [("two.lan", [], ["10.0.0.2"])]
        net = from_ping_sweep(["10.0.0.2", "10.0.0.3"], hostnames=resolved)
        self.assertEqual(net.get("10.0.0.2").hostname, "two.lan")
        self.assertIsNone(net.get("10.0.0.3").hostname)


class ApplyHostnames(unittest.TestCase):
    def test_gethostbyaddr_tuples_matched_by_ip_not_position(self):
        hs = hosts("10.0.0.2", "10.0.0.3", "10.0.0.4")
        # 10.0.0.2 failed to resolve, so it is simply absent: the list is shorter than the hosts.
        resolved = [
            ("four.lan", ["four"], ["10.0.0.4"]),
            ("three.lan", [], ["10.0.0.3"]),
        ]
        apply_hostnames(hs, resolved)
        self.assertEqual([h.hostname for h in hs], [None, "three.lan", "four.lan"])

    def test_mapping_form(self):
        hs = hosts("10.0.0.2")
        apply_hostnames(hs, {"10.0.0.2": "two.lan"})
        self.assertEqual(hs[0].hostname, "two.lan")

    def test_existing_hostname_kept_unless_overwrite(self):
        hs = [Host(ip=ip_address("10.0.0.2"), hostname="keep.me")]
        apply_hostnames(hs, {"10.0.0.2": "new.lan"})
        self.assertEqual(hs[0].hostname, "keep.me")
        apply_hostnames(hs, {"10.0.0.2": "new.lan"}, overwrite=True)
        self.assertEqual(hs[0].hostname, "new.lan")

    def test_ipv6_in_tuple_is_normalised(self):
        hs = [Host(ip=ip_address("fe80::1"))]
        apply_hostnames(hs, [("six.lan", [], ["FE80:0000:0000:0000:0000:0000:0000:0001"])])
        self.assertEqual(hs[0].hostname, "six.lan")


if __name__ == "__main__":
    unittest.main()
