//! The command line: [`Args`] (clap), the scan it asks for, and the parsers
//! the scan card shares with it, so a value typed into the card is checked
//! the same way as one typed after `--scan`.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::Parser;
use scanner::Ipv4Net;
use scanner::local;
use scanner::ports::parse_port_list;

use crate::scan::ScanSettings;

/// The biggest sweep allowed: a /16. It takes a few minutes; anything bigger
/// is a typo.
const MAX_SWEEP_ADDRESSES: u64 = 65_536;

#[derive(Parser, Debug)]
#[command(
    name = "netvisualizer",
    version,
    about = "Find the hosts on a network and show them as a 3D map in its own window.",
    after_help = "Examples:\n  \
        netvisualizer --scan                          scan the subnet of the default interface\n  \
        netvisualizer --scan 10.0.0.0/24 --ports      another network, then common TCP ports\n  \
        netvisualizer --scan --no-window --save s.json   scan without a window, keep the map\n  \
        netvisualizer --json s.json                   show a saved map"
)]
// The doc comments below are `--help` text, printed as they are, so no
// markdown backticks in them.
#[allow(clippy::doc_markdown)]
pub struct Args {
    /// Scan a network: CIDR like 192.168.1.0/24, or without one the subnet of
    /// the interface with the default route. Nothing touches the network
    /// without this.
    // Option<Option<T>> is clap's shape for a flag that may stand alone or
    // take a value: absent, present, or present with a value.
    #[allow(clippy::option_option)]
    #[arg(long, value_name = "CIDR", num_args = 0..=1, value_parser = parse_network, help_heading = "Scanning")]
    pub scan: Option<Option<Ipv4Net>>,

    /// Scan from this interface (and its subnet, unless --scan names a network).
    #[arg(
        short,
        long,
        value_name = "IFACE",
        requires = "scan",
        help_heading = "Scanning"
    )]
    pub interface: Option<String>,

    /// After the sweep, try TCP connections to these ports on every host that
    /// answered: a list like 22,80,8000-8010, or "common" (the default).
    #[allow(clippy::option_option)]
    #[arg(long, value_name = "PORTS", num_args = 0..=1, value_parser = parse_ports, requires = "scan", help_heading = "Scanning")]
    pub ports: Option<Option<PortList>>,

    /// Also probe these UDP ports on every host: a list like 53,123,5000-5010,
    /// or "common" (the default). A closed port refuses; a silent one is
    /// open or filtered, which UDP cannot tell apart.
    #[allow(clippy::option_option)]
    #[arg(long, value_name = "PORTS", num_args = 0..=1, value_parser = parse_ports, requires = "scan", help_heading = "Scanning")]
    pub udp: Option<Option<PortList>>,

    /// Skip the reverse DNS lookups.
    #[arg(long, requires = "scan", help_heading = "Scanning")]
    pub no_names: bool,

    /// Find hosts with ping only, even when ARP requests could be sent
    /// (with root or CAP_NET_RAW, the subnet is swept with ARP, which
    /// finds hosts that ignore ping).
    #[arg(long, requires = "scan", help_heading = "Scanning")]
    pub no_arp: bool,

    /// Skip tracing the route (the routers passed) to hosts beyond this
    /// machine's own subnet.
    #[arg(long, requires = "scan", help_heading = "Scanning")]
    pub no_trace: bool,

    /// Seconds to wait for each ping or connection.
    #[arg(long, value_name = "SECONDS", default_value_t = 1.0, value_parser = parse_seconds, help_heading = "Scanning")]
    pub timeout: f64,

    /// Pings or connections in flight at once (1 to 65535).
    #[arg(
        long,
        value_name = "N",
        default_value_t = 254,
        value_parser = clap::value_parser!(u16).range(1..),
        help_heading = "Scanning"
    )]
    pub concurrency: u16,

    /// Show this machine's networks and exit.
    #[arg(long, exclusive = true, help_heading = "Scanning")]
    pub list_interfaces: bool,

    /// Write the finished map as JSON (show it again with --json).
    #[arg(long, value_name = "FILE", requires = "scan", help_heading = "Output")]
    pub save: Option<PathBuf>,

    /// Scan without opening a window: print what is found (pairs with --save).
    #[arg(long, requires = "scan", help_heading = "Output")]
    pub no_window: bool,

    /// Show every step, Bevy's included, and which Python is embedded.
    #[arg(short, long, help_heading = "Output")]
    pub verbose: bool,

    /// Draw a map saved with --save instead of scanning.
    #[arg(long, value_name = "FILE", conflicts_with = "scan")]
    pub json: Option<PathBuf>,

    /// Render N frames, then quit with status 0 (smoke test).
    #[arg(long, value_name = "N", help_heading = "Debugging")]
    pub exit_after_frames: Option<u32>,

    /// Save a PNG of the window, 30 frames before exiting (or at frame 30).
    #[arg(long, value_name = "FILE", help_heading = "Debugging")]
    pub screenshot: Option<PathBuf>,

    /// Select this host (by id) as soon as the map is drawn.
    #[arg(long, value_name = "HOST_ID", help_heading = "Debugging")]
    pub select: Option<String>,

    /// With --select: also fly to it, as a double click would.
    #[arg(long, requires = "select", help_heading = "Debugging")]
    pub fly: bool,

    /// Open the help overlay, as `?` would.
    #[arg(long, help_heading = "Debugging")]
    pub help_overlay: bool,
}

impl Args {
    /// The scan the command line asks for, if it asks for one.
    pub fn scan_settings(&self) -> Option<ScanSettings> {
        let network = *self.scan.as_ref()?;
        let ports = match &self.ports {
            None => Vec::new(),
            Some(None) => scanner::ports::COMMON_TCP_PORTS.to_vec(),
            Some(Some(PortList(ports))) => ports.clone(),
        };
        let udp_ports = match &self.udp {
            None => Vec::new(),
            Some(None) => scanner::ports::COMMON_UDP_PORTS.to_vec(),
            Some(Some(PortList(ports))) => ports.clone(),
        };
        Some(ScanSettings {
            network,
            interface: self.interface.clone(),
            ports,
            udp_ports,
            names: !self.no_names,
            arp: !self.no_arp,
            trace: !self.no_trace,
            timeout: Duration::from_secs_f64(self.timeout),
            concurrency: usize::from(self.concurrency),
            save: self.save.clone(),
        })
    }
}

/// `--scan`'s value: a network like 192.168.1.0/24 (host bits are dropped,
/// so 192.168.1.143/24 means the same) or a single address. A /16 at most.
pub fn parse_network(text: &str) -> Result<Ipv4Net, String> {
    let text = text.trim();
    let invalid = || format!("not an IPv4 network or address: {text:?}");
    let network = if text.contains('/') {
        text.parse::<Ipv4Net>().map_err(|_| invalid())?
    } else {
        Ipv4Net::from(text.parse::<std::net::Ipv4Addr>().map_err(|_| invalid())?)
    }
    .trunc();
    let addresses = 1u64 << (32 - network.prefix_len());
    if addresses > MAX_SWEEP_ADDRESSES {
        return Err(format!(
            "{network} has {addresses} addresses; a /16 is the largest sweep allowed"
        ));
    }
    Ok(network)
}

/// `--ports`' value, read by [`parse_port_list`]. (A plain `Vec` would make
/// clap expect several values.)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortList(Vec<u16>);

fn parse_ports(text: &str) -> Result<PortList, String> {
    parse_port_list(text)
        .map(PortList)
        .map_err(|err| err.to_string())
}

/// `--timeout`'s value: seconds, more than 0, at most an hour.
pub fn parse_seconds(text: &str) -> Result<f64, String> {
    match text.trim().parse::<f64>() {
        Ok(seconds) if seconds > 0.0 && seconds <= 3600.0 => Ok(seconds),
        _ => Err(format!(
            "not a number of seconds between 0 and 3600: {text:?}"
        )),
    }
}

/// `--list-interfaces`: this machine's networks, one per line.
pub fn list_interfaces() -> ExitCode {
    match local::local_networks() {
        Ok(networks) if !networks.is_empty() => {
            for n in networks {
                let gateway = n.gateway.map_or("-".to_string(), |g| g.to_string());
                println!(
                    "{:<12} {:<16} {:<20} gateway {gateway}",
                    n.interface,
                    n.ip.to_string(),
                    n.network.to_string()
                );
            }
            ExitCode::SUCCESS
        }
        Ok(_) => {
            eprintln!("netvisualizer: no IPv4 networks found");
            ExitCode::FAILURE
        }
        Err(err) => {
            eprintln!("netvisualizer: could not list the network interfaces: {err}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Args, clap::Error> {
        Args::try_parse_from(std::iter::once("netvisualizer").chain(args.iter().copied()))
    }

    fn settings(args: &[&str]) -> ScanSettings {
        parse(args).unwrap().scan_settings().unwrap()
    }

    #[test]
    fn without_scan_nothing_is_scanned() {
        assert!(parse(&[]).unwrap().scan_settings().is_none());
        assert!(
            parse(&["--json", "x.json"])
                .unwrap()
                .scan_settings()
                .is_none()
        );
    }

    #[test]
    fn scan_takes_an_optional_network_and_the_scan_options() {
        assert_eq!(settings(&["--scan"]), ScanSettings::default());

        let s = settings(&["--scan", "192.168.1.143/24", "--ports", "--no-names"]);
        assert_eq!(s.network, Some("192.168.1.0/24".parse().unwrap()));
        assert_eq!(s.ports, scanner::ports::COMMON_TCP_PORTS);
        assert!(!s.names);

        let s = settings(&[
            "--scan",
            "--ports",
            "22,80",
            "-i",
            "wlan0",
            "--timeout",
            "0.5",
            "--concurrency",
            "64",
            "--save",
            "s.json",
        ]);
        assert_eq!(s.network, None);
        assert_eq!(s.ports, [22, 80]);
        assert_eq!(s.interface.as_deref(), Some("wlan0"));
        assert_eq!(s.timeout, Duration::from_millis(500));
        assert_eq!(s.concurrency, 64);
        assert_eq!(s.save, Some(PathBuf::from("s.json")));
    }

    #[test]
    fn udp_ports_are_off_unless_asked_for() {
        let off = settings(&["--scan"]);
        assert!(off.udp_ports.is_empty(), "{:?}", off.udp_ports);
        assert_eq!(
            settings(&["--scan", "--udp"]).udp_ports,
            scanner::ports::COMMON_UDP_PORTS
        );
        assert_eq!(
            settings(&["--scan", "--udp", "53,123"]).udp_ports,
            [53, 123]
        );
    }

    #[test]
    fn scan_options_need_scan_and_bad_values_are_refused() {
        for args in [
            &["--ports"][..],
            &["--no-names"],
            &["-i", "wlan0"],
            &["--save", "x.json"],
            &["--scan", "--concurrency", "0"],
            &["--scan", "--concurrency", "70000"],
            &["--no-window"],
            &["--scan", "--json", "x.json"],
            &["--list-interfaces", "--scan"],
            &["--scan", "--timeout", "0"],
            &["--scan", "--ports", "http"],
            &["--scan", "10.0.0.0/8"],
        ] {
            assert!(parse(args).is_err(), "{args:?} should be refused");
        }
    }

    #[test]
    fn networks_are_ipv4_and_at_most_a_slash_16() {
        assert_eq!(
            parse_network("10.0.0.7"),
            Ok("10.0.0.7/32".parse().unwrap())
        );
        assert_eq!(
            parse_network(" 10.1.2.3/16 "),
            Ok("10.1.0.0/16".parse().unwrap())
        );
        assert_eq!(
            parse_network("10.0.0.0/15"),
            Err("10.0.0.0/15 has 131072 addresses; a /16 is the largest sweep allowed".to_string())
        );
        assert!(parse_network("fe80::/64").is_err());
        assert!(parse_network("lan").is_err());
    }
}
