#!/bin/sh
# Try the ARP sweep without root or setcap, in namespaces of our own:
#
#   unshare -Urnm scripts/arp-sandbox.sh          # from the repo root, after `cargo build --features dev`
#
# `unshare -U` makes a user namespace where this user is root, with every
# capability (CAP_NET_RAW included) over the namespaces made with it: `-n` a
# network namespace, empty but for loopback, and `-m` a mount namespace so
# sysfs can be remounted to show that network namespace's interfaces. A
# veth pair is a virtual cable; its far end goes into a second network
# namespace, because the kernel will not answer an ARP request whose sender
# address is one of its own: the peer has to be another "machine". The
# sleeping process is the handle to that namespace.
set -e
mount -t sysfs sysfs /sys
ip link set lo up
unshare -n sleep 120 &
PEER=$!
sleep 0.3
ip link add veth0 type veth peer name veth1
ip link set veth1 netns "$PEER"
ip addr add 10.99.0.1/29 dev veth0
ip link set veth0 up
nsenter -t "$PEER" -n sh -c 'ip link set lo up; ip addr add 10.99.0.2/29 dev veth1; ip link set veth1 up'
echo "peer: $(nsenter -t "$PEER" -n ip -br link show veth1)"

# The dev build links Bevy and the standard library dynamically; `cargo run`
# would set this path itself.
BIN=${BIN:-target/debug/netvisualizer}
export LD_LIBRARY_PATH="target/debug/deps:$(rustc --print target-libdir):$LD_LIBRARY_PATH"
"$BIN" --scan 10.99.0.0/29 -i veth0 --no-window --no-names "$@"
kill "$PEER"
