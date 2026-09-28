#!/usr/bin/env python3

# from ipaddress import ip_address, ip_network
# import socket
# import subprocess
# import os
# import asyncio
import asyncio
import netutils

# One button to start the scanner and send all requests


class scanner:
    """
    requires an IP address to function.
    """
    def __init__(self, addr: str):
        self.addr = addr


    def host_enum(self, prefix_length, L3=True, L2=True):
        """
        enumerate active hosts on a given network
        """
        if L3:
            asyncio.run(netutils.async_ping_sweep(self.addr, prefix_length))


    def port_scan(self, ports: range,):
        """
        The top port is 65,535
        User should either select specific ports to scan
        or a range of ports to scan, should also allow
        user to select a scan of just the common open
        ports.
        """

local_scan = scanner(netutils.get_local_ip())
local_scan.host_enum(24)
