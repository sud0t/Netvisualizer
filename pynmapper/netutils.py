#!/usr/bin/env python3

import asyncio
import os
import socket
import subprocess
from ipaddress import ip_address, ip_network


def main():
    try:
        asyncio.run(async_ping_sweep(get_local_ip(), 24, task_count=508))
    except Exception as e:
        print(f"error: {e}")

# Use the correct ping argument based off OS
if os.name == "nt":
    ping_count_arg = "-n"
elif os.name == "posix":
    ping_count_arg = "-c"
else:
    ping_count_arg = "-c"


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


def ping(ip_addr: str, count="1") -> bool:
    try:
        output = subprocess.check_output(["ping", ping_count_arg, count, ip_addr])
        print(output.decode())
        return True
    except subprocess.CalledProcessError:
        print(f"Could not ping {ip_addr}")
        return False


async def async_ping(ip_addr: str, semaphore: asyncio.Semaphore, count="1" ) -> tuple:
        async with semaphore:
            print(f"pinging {ip_addr}")
            proc = await asyncio.create_subprocess_exec(  # Await ping process response
                "ping",
                ping_count_arg,
                count,
                ip_addr,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.STDOUT,
            )
            return await proc.communicate()


def ping_sweep(ip_addr: str, prefix_length: int):
    usable_hosts = list(ip_network(f"{ip_addr}/{prefix_length}", strict=False).hosts())
    active_hosts = []
    for host in usable_hosts:
        if ping(str(host)):
            active_hosts.append(str(host))
    return active_hosts


async def async_ping_sweep(ip_addr, prefix_length, count="1", task_count=254):
    semaphore = asyncio.Semaphore(task_count)
    possible_hosts = list(ip_network(f"{ip_addr}/{prefix_length}", strict=False).hosts())
    task_list = [asyncio.create_task(async_ping(str(host), semaphore)) for host in possible_hosts]
    response_list = await asyncio.gather(
        *task_list
    )  # Gathering the tasks by unpacking task_list into response_list
    active_hosts = []
    for index, tup in enumerate(
        response_list
    ):  # proc.communicate() returns a tuple with the first index being the response
        response = tup[0]
        if b"1 received" in response:
            print(response.decode())
            active_hosts.append(
                possible_hosts[index]
            )  # Append every host that responded to a list

    return active_hosts


def traceroute(ip_addr: str, hops=30):
    with socket.socket(family=socket.AF_INET, type=socket.SOCK_STREAM, ) as tr:
        tr.bind(get_local_ip())
        for ttl in hops:
            pass





async def async_traceroute(hosts: list):
    pass




if __name__ == "__main__":
    main()
