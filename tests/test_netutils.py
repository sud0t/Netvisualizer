"""Tests for the networking layer that never touch a real network.

Pings are replaced with a fake, DNS with a fake, the routing table with a copy
of a real one; the port scan runs against a server on 127.0.0.1 that the test
starts itself.
"""

from __future__ import annotations

import argparse
import asyncio
import socket
import sys
import unittest
from ipaddress import IPv4Address, ip_network
from unittest import mock

from pynmapper import __main__ as cli
from pynmapper import netutils
from pynmapper.model import Host, Port
from pynmapper.netutils import LocalNetwork

# A copy of this development machine's routing table as scapy reports it:
# (net, mask, gateway, iface, source, metric). wlan0 has the default route.
ROUTES = [
    (0x7F000000, 0xFF000000, "0.0.0.0", "lo", "127.0.0.1", 0),
    (0x7F000001, 0xFFFFFFFF, "0.0.0.0", "lo", "127.0.0.1", 0),
    (0xC0A8018F, 0xFFFFFFFF, "0.0.0.0", "wlan0", "192.168.1.143", 0),      # our own address, /32
    (0xC0A87A01, 0xFFFFFFFF, "0.0.0.0", "virbr0", "192.168.122.1", 0),
    (0, 0, "192.168.1.254", "wlan0", "192.168.1.143", 600),                 # default route
    (0xC0A80100, 0xFFFFFF00, "0.0.0.0", "wlan0", "192.168.1.143", 600),     # 192.168.1.0/24
    (0xC0A87A00, 0xFFFFFF00, "0.0.0.0", "virbr0", "192.168.122.1", 0),      # 192.168.122.0/24
    (0xE0000000, 0xF0000000, "0.0.0.0", "wlan0", "192.168.1.143", 250),     # multicast
]


class RoutingTests(unittest.TestCase):
    def test_connected_networks_with_their_gateways(self):
        with mock.patch.object(netutils, "_routes", return_value=ROUTES):
            found = netutils.list_local_networks()
        self.assertEqual(
            found,
            [
                LocalNetwork("wlan0", IPv4Address("192.168.1.143"), ip_network("192.168.1.0/24"), IPv4Address("192.168.1.254")),
                LocalNetwork("virbr0", IPv4Address("192.168.122.1"), ip_network("192.168.122.0/24"), None),
            ],
        )

    def test_default_network_is_the_one_our_address_is_on(self):
        with mock.patch.object(netutils, "_routes", return_value=ROUTES), \
             mock.patch.object(netutils, "get_local_ip", return_value="192.168.1.143"):
            local = netutils.get_local_network()
        self.assertEqual(local.interface, "wlan0")
        self.assertEqual(local.gateway, IPv4Address("192.168.1.254"))

    def test_interface_can_be_chosen(self):
        with mock.patch.object(netutils, "_routes", return_value=ROUTES):
            self.assertEqual(netutils.get_local_network("virbr0").network, ip_network("192.168.122.0/24"))
            with self.assertRaises(ValueError):
                netutils.get_local_network("eth9")

    def test_without_a_routing_table_a_slash_24_is_assumed(self):
        with mock.patch.object(netutils, "_routes", return_value=[]), \
             mock.patch.object(netutils, "get_local_ip", return_value="10.1.2.3"):
            local = netutils.get_local_network()
        self.assertEqual(local.network, ip_network("10.1.2.0/24"))
        self.assertIsNone(local.gateway)

    def test_mark_local(self):
        hosts = [Host(IPv4Address("192.168.1.9")), Host(IPv4Address("10.0.0.9"))]
        netutils.mark_local(hosts, ip_network("192.168.1.0/24"))
        self.assertEqual([h.is_local for h in hosts], [True, False])


class PingTests(unittest.TestCase):
    def test_ping_flags_per_platform(self):
        with mock.patch.object(sys, "platform", "linux"):
            self.assertEqual(netutils.ping_command("1.2.3.4", 0.5), ["ping", "-c", "1", "-W", "0.5", "1.2.3.4"])
        with mock.patch.object(sys, "platform", "darwin"):
            self.assertEqual(netutils.ping_command("1.2.3.4", 0.5)[3:5], ["-W", "500"])
        with mock.patch.object(sys, "platform", "win32"):
            self.assertEqual(netutils.ping_command("1.2.3.4", 2)[1:5], ["-n", "1", "-w", "2000"])

    def test_sweep_makes_a_host_for_every_address_that_answered(self):
        async def fake_ping(ip, timeout=1.0):
            return ip.endswith((".1", ".3"))

        seen = []
        with mock.patch.object(netutils, "async_ping", fake_ping):
            hosts = asyncio.run(netutils.async_ping_sweep("192.168.5.0/29", concurrency=4, progress=lambda d, t: seen.append((d, t))))
        self.assertEqual([str(h.ip) for h in hosts], ["192.168.5.1", "192.168.5.3"])
        self.assertTrue(all(isinstance(h, Host) for h in hosts))
        self.assertEqual(seen, [(4, 6), (6, 6)])  # two batches of four over the six usable addresses

    def test_sweep_accepts_the_older_address_plus_prefix_style(self):
        async def fake_ping(ip, timeout=1.0):
            return True

        with mock.patch.object(netutils, "async_ping", fake_ping):
            hosts = asyncio.run(netutils.async_ping_sweep("10.0.0.5", 30))
        self.assertEqual([str(h.ip) for h in hosts], ["10.0.0.5", "10.0.0.6"])

    def test_real_ping_of_loopback(self):
        self.assertTrue(asyncio.run(netutils.async_ping("127.0.0.1", timeout=1)))


class HostnameTests(unittest.TestCase):
    def test_names_are_filled_in_only_where_missing(self):
        def fake_gethostbyaddr(ip):
            if ip == "192.168.1.10":
                return ("nas.lan", [], [ip])
            raise socket.herror(1, "Unknown host")

        hosts = [Host(IPv4Address("192.168.1.10")), Host(IPv4Address("192.168.1.11")), Host(IPv4Address("192.168.1.12"), hostname="keep.me")]
        with mock.patch.object(socket, "gethostbyaddr", fake_gethostbyaddr):
            asyncio.run(netutils.resolve_hostnames(hosts))
        self.assertEqual([h.hostname for h in hosts], ["nas.lan", None, "keep.me"])


class PortScanTests(unittest.TestCase):
    def test_open_and_closed_ports_on_loopback(self):
        async def scenario():
            async def handle(reader, writer):
                writer.close()

            server = await asyncio.start_server(handle, "127.0.0.1", 0)
            open_port = server.sockets[0].getsockname()[1]
            # A port that was free a moment ago is as "closed" as we can arrange.
            probe = socket.socket()
            probe.bind(("127.0.0.1", 0))
            closed_port = probe.getsockname()[1]
            probe.close()
            try:
                host = Host(IPv4Address("127.0.0.1"))
                await netutils.async_port_scan([host], [open_port, closed_port, open_port], timeout=1)
                return host, open_port
            finally:
                server.close()
                await server.wait_closed()

        host, open_port = asyncio.run(scenario())
        self.assertEqual(host.open_ports, [Port(open_port, "tcp", netutils.service_name(open_port))])

    def test_service_name(self):
        self.assertEqual(netutils.service_name(22), "ssh")
        self.assertIsNone(netutils.service_name(65000))

    def test_scanning_twice_does_not_duplicate_ports(self):
        async def always_open(ip, port, timeout=1.0):
            return True

        host = Host(IPv4Address("10.0.0.1"), open_ports=[Port(80, "tcp")])
        with mock.patch.object(netutils, "tcp_connect", always_open):
            asyncio.run(netutils.async_port_scan([host], [22, 80]))
        self.assertEqual([(p.number, p.proto) for p in host.open_ports], [(22, "tcp"), (80, "tcp")])


class CliTests(unittest.TestCase):
    def test_parse_ports(self):
        self.assertEqual(cli.parse_ports("22, 80,8000-8002"), [22, 80, 8000, 8001, 8002])
        self.assertEqual(cli.parse_ports("common"), list(netutils.COMMON_TCP_PORTS))
        for bad in ("abc", "0", "70000", "90-10", ""):
            with self.subTest(bad=bad), self.assertRaises(argparse.ArgumentTypeError):
                cli.parse_ports(bad)

    def test_parse_network(self):
        self.assertEqual(cli.parse_network("192.168.1.7/24"), ip_network("192.168.1.0/24"))
        for bad in ("fe80::/64", "10.0.0.0/8", "not-a-network"):
            with self.subTest(bad=bad), self.assertRaises(argparse.ArgumentTypeError):
                cli.parse_network(bad)

    def test_ports_flag_with_and_without_a_list(self):
        parser = cli.build_parser()
        self.assertEqual(parser.parse_args(["--ports"]).ports, list(netutils.COMMON_TCP_PORTS))
        self.assertEqual(parser.parse_args(["--ports", "22,443"]).ports, [22, 443])
        self.assertIsNone(parser.parse_args([]).ports)
        self.assertEqual(parser.parse_args(["10.0.0.0/24"]).network, ip_network("10.0.0.0/24"))

    def test_gateway_that_ignored_ping_is_still_on_the_map_as_down(self):
        local = LocalNetwork("wlan0", IPv4Address("192.168.1.143"), ip_network("192.168.1.0/24"), IPv4Address("192.168.1.254"))
        hosts = cli.ensure_gateway([Host(IPv4Address("192.168.1.143"))], local)
        self.assertEqual(str(hosts[-1].ip), "192.168.1.254")
        self.assertEqual(hosts[-1].extra["status"], "down")
        self.assertEqual(len(cli.ensure_gateway(hosts, local)), 2)  # not added twice

    def test_build_map_marks_roles_and_stage(self):
        local = LocalNetwork("wlan0", IPv4Address("192.168.1.143"), ip_network("192.168.1.0/24"), IPv4Address("192.168.1.254"))
        hosts = [Host(IPv4Address("192.168.1.143")), Host(IPv4Address("192.168.1.254")), Host(IPv4Address("192.168.1.9"))]
        doc = cli.build_map(hosts, local, local.network, "resolving names…").to_dict()
        self.assertEqual({h["id"]: h["role"] for h in doc["hosts"]},
                         {"192.168.1.143": "scanner", "192.168.1.254": "gateway", "192.168.1.9": "host"})
        self.assertEqual(doc["scan"]["stage"], "resolving names…")

    def test_run_scan_publishes_each_stage(self):
        local = LocalNetwork("wlan0", IPv4Address("192.168.1.143"), ip_network("192.168.1.0/24"), IPv4Address("192.168.1.254"))
        args = cli.build_parser().parse_args(["--ports", "22", "--timeout", "0.2"])
        found = [Host(IPv4Address("192.168.1.143")), Host(IPv4Address("192.168.1.9"))]
        stages = []

        async def fake_sweep(network, prefix_length=None, **kwargs):
            return list(found)

        async def fake_names(hosts, **kwargs):
            return hosts

        async def fake_ports(hosts, ports, **kwargs):
            self.assertEqual(ports, [22])
            return hosts

        with mock.patch.object(netutils, "async_ping_sweep", fake_sweep), \
             mock.patch.object(netutils, "resolve_hostnames", fake_names), \
             mock.patch.object(netutils, "async_port_scan", fake_ports):
            hosts = asyncio.run(cli.run_scan(args, local, local.network, lambda h, stage: stages.append(stage)))

        self.assertEqual(stages, ["resolving names…", "trying 1 TCP ports…", "done"])
        self.assertEqual([str(h.ip) for h in hosts], ["192.168.1.143", "192.168.1.9", "192.168.1.254"])
        self.assertEqual([h.is_local for h in hosts[:2]], [True, True])
        self.assertTrue(hosts[0].hostname)  # our own name came from socket.gethostname()


if __name__ == "__main__":
    unittest.main()
