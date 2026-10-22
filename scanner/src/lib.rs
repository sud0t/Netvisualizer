//! Finds the hosts on a network and fills in what is known about them.
//!
//! - model.rs: [`Host`] and [`Port`], the one shape shared with the viewer.
//! - local.rs: this machine's networks: interfaces, subnets, gateways.
//! - icmp.rs: ICMP echo packets, built and read by hand.
//! - sweep.rs: the ping sweep, finding which addresses answer.
//! - arp.rs: the ARP sweep, finding every host on our own link, MACs included.
//! - neighbours.rs: the kernel's ARP table, the MACs of the hosts found.
//! - trace.rs: traceroute, the routers on the way to hosts beyond our link.
//! - names.rs: reverse DNS, naming the hosts found.
//! - ports.rs: the TCP connect scan, service names, port lists.
//!
//! Nothing here draws anything or knows about Bevy or Python. Each module is
//! one tool (find the hosts, name them, try their ports); the viewer crate
//! decides which ones run and in what order (`viewer/src/scan.rs`).

pub mod arp;
pub mod icmp;
pub mod local;
pub mod model;
pub mod names;
pub mod neighbours;
pub mod ports;
pub mod sweep;
pub mod trace;

// The network type the scanner's API speaks, so its users need no ipnet of their own.
pub use ipnet::Ipv4Net;
pub use local::LocalNetwork;
pub use model::{Host, MacAddr, Port, Proto};
