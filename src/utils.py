import asyncio
import os
import re
import socket
import subprocess
from ipaddress import ip_address, ip_network


def get_local_ip():
    # Source - https://stackoverflow.com/a/28950776
    # Retrieved 2026-09-21, License - CC BY-SA 4.0
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    s.settimeout(0)
    try:
        # doesn't even have to be reachable
        s.connect(('10.254.254.254', 1))
        IP = s.getsockname()[0]
    except Exception:  # noqa: BLE001
        IP = '127.0.0.1'
    finally:
        s.close()
    return IP


def ping(addr: str, count="1") -> bool:
    if os.name == "nt":
        ping_arg = "-n"
    elif os.name == "posix":
        ping_arg = "-c"
    try:
        output = subprocess.check_output(["ping", ping_arg, count, addr])
        print(output.decode())
        return True
    except subprocess.CalledProcessError:
        print(f"Could not ping {addr}")
        return False


def ping_sweep(addr: str, prefix_length: int):
    usable_hosts = list(ip_network(f'{addr}/{prefix_length}', strict=False).hosts())
    active_hosts = []
    for host in usable_hosts:
        if ping(str(host)):
            active_hosts.append(str(host))
    return active_hosts


async def async_ping_sweep(addr, prefix_length, count="1"):
    # TODO: Return values other than just active hosts
    if os.name == "nt":
        count_arg = "-n"
    elif os.name == "posix":
        count_arg = "-c"
    else:
        count_arg = "-c"

    async def async_ping(addr: str):
        print(f"pinging {addr}")
        proc = await asyncio.create_subprocess_exec(  # await a ping subprocess
            "ping", 
            count_arg,
            count,
            addr,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.STDOUT,
        )
        return await proc.communicate()

    possible_hosts = list(ip_network(f"{addr}/{prefix_length}", strict=False).hosts())
    task_list = []
    active_match = (
        r"[1-9]\d* received"  # Match for packets received > 0 in the ping output
    )
    active_hosts = []
    for host in possible_hosts:
        task = asyncio.create_task(async_ping(str(host)))  # Making a task for each ping in the given network
        task_list.append(task)  # Adding all the tasks to a list so they can be gathered

    gather_list = await asyncio.gather(*task_list)  # Gathering the tasks by unpacking task_list into asyncio.gather()
    for index, tup in enumerate(gather_list):  # communicate returns a tuple hence the name tup, need the index to add correct IP address to active_hosts list
        response = tup[0].decode() # Response is a tuple with bytes object and none, we decode the bytes object to str
        if bool(re.search(active_match, response)):
            print(response)
            active_hosts.append(possible_hosts[index])

    # To run this use asyncio.run(async_ping_sweep()) and pass in the address and prefix length
    return active_hosts
    

