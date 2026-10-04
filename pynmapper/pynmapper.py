import argparse
import asyncio

import netutils
# import scapy
# import vpython

# parser = argparse.ArgumentParser()
# parser.add_argument("echo", help="echo the string you use here")
# args = parser.parse_args()
# print(args.echo)

myIP = netutils.get_local_ip()
# Enumerate active hosts on the local network
def scanLAN():
    active_hosts = []
    prefix = "24"
    active_hosts.append(asyncio.run(netutils.async_ping_sweep(myIP, prefix)))
    print(active_hosts)

scanLAN()
