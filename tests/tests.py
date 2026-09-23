#!/usr/bin/env python3

import asyncio
import os
import re
from ipaddress import ip_network
from random import randint


def main():
    local = "127.0.0.1"
    ip = "207.81.215.6"
    google = "8.8.8.8"
    asyncio.run(async_ping_sweep(ip, 24))


# asyncio.create_task() will schedule a coro to run on the event loop
# immediately, and hands back a task object right away, that task
# object could be put in a list with many other task objects which
# can then be awaited 

# Genuinely how I learned async:
async def hello():
    print("hello")
    await asyncio.sleep(1)
    print("done")
    return randint(1, 100)


async def hello_run():
    task_list = []
    for _ in range(100):
        task = asyncio.create_task(hello()) # create task for each iteration
        task_list.append(task) # adds tasks to list

    test = await asyncio.gather(*task_list) # variable assignment awaits the tasks (print hello, done) and assigns the results to test

    print(test)


# My own implementation of an asynchronous ping sweep
async def async_ping_sweep(addr, prefix_length, count="1"):
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
        response = tup[0].decode()
        if bool(re.search(active_match, response)):
            print(response)
            active_hosts.append(possible_hosts[index])

    return active_hosts


if __name__ == "__main__":
    main()

