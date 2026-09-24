#!/usr/bin/env python3

# from ipaddress import ip_address, ip_network
# import socket
# import subprocess
# import os
# import asyncio




class scanner:
    """
    requires an IP address to function.
    """
    def __init__(self, addr: str):
        self.addr = addr


    def enumerate_hosts(self, prefix_length, ping=True):
        """
        Functions to enumerate active hosts
        on a given network
        """


    def port_scan(self, ports: range,):
        """
        The top port is 65,535
        User should either select specific ports to scan
        or a range of ports to scan, should also allow
        user to select a scan of just the common open
        ports.
        """
