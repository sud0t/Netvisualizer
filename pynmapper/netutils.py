#!/usr/bin/env python3
# - netutils.py creates hosts and fills in their values.
# - pynmapper will pass in the host dataclasses to the functions that need it

import asyncio
import os
import socket
import subprocess
from pynmapper.model import Host, Port
from ipaddress import IPv4Address, ip_address, ip_network
# from scapy.layers.inet import IP, UDP, ICMP

# TODO: Add Host dataclass so nodes in graphical representation don't have to
# pull directly from network mapping functions (model.py)

# TODO: Add port scanning functionality 

# TODO: Add layer 2 host enumeration
# Top level enumeration function
async def async_ping_sweep(ip_addr: str, prefix_length: int, count="1", task_count=254) -> list[Host]:
    """
    Runs a ping sweep based off given IP address and prefix length
    task_count is how many pings can be executing concurrently 
    """
    # TODO: Create one Host object (model.py) per responding host

    semaphore = asyncio.Semaphore(task_count)
    possible_hosts = list(ip_network(f"{ip_addr}/{prefix_length}", strict=False).hosts())
    task_list = [asyncio.create_task(async_ping(str(host), semaphore, count=count)) for host in possible_hosts]
    try:
        response_list = await asyncio.gather(
            *task_list
        )  # Gathering the tasks by unpacking task_list into response_list
    except Exception as e:
        print(e)

    active_hosts = []
    for index, tup in enumerate(
        response_list
    ):  # proc.communicate() returns a tuple with the first index being the response
        response = tup[0]
        if b"1 received" in response or b"1 packets received" in response:
            print(response.decode())

            active_hosts.append(
                Host(possible_hosts[index])
            )  # Append every host that responded to a list
            # make a Host (dataclass) from every host that responded
    return active_hosts


def get_local_ip():
    # Source - https://stackoverflow.com/a/28950776
    # Retrieved 2026-09-21, License - CC BY-SA 4.0
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    s.settimeout(0)
    try:
        # doesn't even have to be reachable
        s.connect(("10.254.254.254", 1))
        IP = s.getsockname()[0]
    except Exception:  # noqa: BLE001
        IP = "127.0.0.1"
    finally:
        s.close()
    return IP


def ping(ip_addr: str, count="1", ttl="64") -> bool:
    try:
        output = subprocess.check_output(["ping", "-c", count, "-t", ttl, ip_addr])
        print(output.decode())
        return True
    except subprocess.CalledProcessError:
        print(f"Could not ping {ip_addr}")
        return False


def ping_sweep(ip_addr: str, prefix_length: int):
    usable_hosts = list(ip_network(f"{ip_addr}/{prefix_length}", strict=False).hosts())
    active_hosts = []
    for host in usable_hosts:
        if ping(str(host)):
            active_hosts.append(str(host))
    return active_hosts


async def async_ping(ip_addr: str, semaphore: asyncio.Semaphore, count="1", timeout="1") -> tuple:
        async with semaphore:
            print(f"pinging {ip_addr}")
            proc = await asyncio.create_subprocess_exec(  # Await ping process response
                "ping",
                "-c",
                count, # how many packets to send
                "-W",
                timeout,
                ip_addr,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.STDOUT,
            )
            return await proc.communicate()


# TODO: Deprecate this in favour of just using gethostbyaddr in the host class
def get_host_names(hosts: list):
    hostnames = []
    for host in hosts:
        try:
            hostnames.append(socket.gethostbyaddr(host))
        except Exception as e:
            print(f"{host} cannot be resolved")
    return hostnames
    
        
# def traceroute(ip_addr: str, hops=30):
#     dest_ip = socket.gethostbyname(ip_addr)
#     port = 443
#
#
# async def async_traceroute(ip_addresses: list):
#     pass
