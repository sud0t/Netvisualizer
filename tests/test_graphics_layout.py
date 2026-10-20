"""Tests for pynmapper.graphics.layout."""

import math
import unittest
from ipaddress import ip_address

from pynmapper.graphics import from_hosts, star_ring
from pynmapper.graphics.layout import MIN_RADIUS, ring_plan
from pynmapper.model import Host


def lan(n, start=2):
    return [Host(ip=ip_address(f"10.0.{(start + i) // 256}.{(start + i) % 256}")) for i in range(n)]


class StarRing(unittest.TestCase):
    def test_scanner_at_origin_everyone_positioned_uniquely(self):
        net = from_hosts(lan(10), scanner_ip="10.0.0.1")
        pos = star_ring(net)

        self.assertEqual(set(pos), set(net.ids()))
        self.assertEqual(pos["10.0.0.1"], (0.0, 0.0, 0.0))
        self.assertEqual(len(set(pos.values())), len(net))

    def test_hosts_sit_on_the_inner_ring_for_small_networks(self):
        net = from_hosts(lan(6), scanner_ip="10.0.0.1")
        for id_, (x, y, z) in star_ring(net).items():
            if id_ == "10.0.0.1":
                continue
            self.assertAlmostEqual(math.hypot(x, z), MIN_RADIUS, places=2)
            self.assertEqual(y, 0.0)

    def test_large_networks_spill_onto_further_rings(self):
        net = from_hosts(lan(60), scanner_ip="10.0.0.1")
        pos = star_ring(net)
        radii = {round(math.hypot(x, z)) for id_, (x, y, z) in pos.items() if id_ != "10.0.0.1"}
        self.assertGreater(len(radii), 1)
        self.assertEqual(len(set(pos.values())), len(net))

    def test_gateway_is_first_on_the_inner_ring(self):
        net = from_hosts(lan(5), scanner_ip="10.0.0.1", gateway_ip="10.0.0.6")
        x, y, z = star_ring(net)["10.0.0.6"]
        self.assertAlmostEqual(x, MIN_RADIUS, places=2)
        self.assertAlmostEqual(z, 0.0, places=2)

    def test_address_order_around_the_ring(self):
        net = from_hosts(lan(4), scanner_ip="10.0.0.1")
        pos = star_ring(net)
        angles = [math.atan2(pos[i][2], pos[i][0]) % (2 * math.pi) for i in ["10.0.0.2", "10.0.0.3", "10.0.0.4", "10.0.0.5"]]
        self.assertEqual(angles, sorted(angles))

    def test_works_without_a_scanner(self):
        net = from_hosts(lan(3))
        pos = star_ring(net)
        self.assertEqual(len(pos), 3)
        self.assertNotIn((0.0, 0.0, 0.0), pos.values())

    def test_empty_map(self):
        self.assertEqual(star_ring(from_hosts([])), {})


class RingPlan(unittest.TestCase):
    def test_counts_add_up_and_inner_ring_fills_first(self):
        for n in (0, 1, 5, 16, 17, 60, 254):
            plan = ring_plan(n)
            self.assertEqual(sum(plan), n)
            if len(plan) > 1:
                self.assertEqual(plan[0], ring_plan(1000)[0])


if __name__ == "__main__":
    unittest.main()
