//! What the scan dock edits: the [`ScanForm`], the scan's options as the
//! user has them set, and the [`ScanSettings`] they become when Scan is
//! pressed. The dock (ui/dock.rs) only shows and flips these fields; the
//! same [`StartScan`](crate::scan::StartScan) message `--scan` sends does
//! the rest in scan.rs.
//!
//! Every feature of a scan is a toggle here: ARP, traceroute, names, TCP
//! ports and UDP ports are each on or off, and the port lists behind the
//! TCP and UDP toggles are remembered while they are off. Timeout and
//! concurrency step through presets, so there is nothing to mistype.

use std::path::PathBuf;
use std::time::Duration;

use bevy::prelude::*;
use scanner::local;
use scanner::ports::{COMMON_TCP_PORTS, COMMON_UDP_PORTS, parse_port_list};

use crate::cli::{parse_network, parse_seconds};
use crate::scan::ScanSettings;

/// The timeouts the stepper runs through, in seconds.
pub const TIMEOUT_STEPS: [f64; 7] = [0.25, 0.5, 1.0, 2.0, 3.0, 5.0, 10.0];
/// The probe counts the stepper runs through.
pub const CONCURRENCY_STEPS: [u16; 7] = [16, 32, 64, 128, 254, 512, 1024];

/// The scan's toggles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Feature {
    Arp,
    Trace,
    Names,
    Tcp,
    Udp,
}

impl Feature {
    pub const ALL: [Feature; 5] = [
        Feature::Arp,
        Feature::Trace,
        Feature::Names,
        Feature::Tcp,
        Feature::Udp,
    ];

    /// The chip's label.
    pub fn label(self) -> &'static str {
        match self {
            Feature::Arp => "ARP",
            Feature::Trace => "trace",
            Feature::Names => "names",
            Feature::Tcp => "TCP",
            Feature::Udp => "UDP",
        }
    }

    /// The chip's tooltip.
    pub fn describe(self) -> &'static str {
        match self {
            Feature::Arp => "sweep the subnet with ARP (needs root or CAP_NET_RAW; ping otherwise)",
            Feature::Trace => {
                "trace the route to hosts beyond the subnet; the routers join the map"
            }
            Feature::Names => "look the hosts' names up (reverse DNS)",
            Feature::Tcp => "try TCP ports on every host (the list is in the options)",
            Feature::Udp => "probe UDP ports on every host (the list is in the options)",
        }
    }
}

/// The dock's fields, as set; [`ScanForm::settings`] reads them.
#[derive(Resource, Debug, Clone, PartialEq, Eq)]
pub struct ScanForm {
    /// A network or address; empty means the interface's own subnet.
    pub network: String,
    /// An interface name; empty means the one with the default route.
    pub interface: String,
    /// This machine's interfaces, for the cycler.
    pub interfaces: Vec<String>,
    pub arp: bool,
    pub trace: bool,
    pub names: bool,
    /// Port lists as `--ports` and `--udp` take them; empty means the
    /// feature is off.
    pub tcp_ports: String,
    pub udp_ports: String,
    /// What the lists were before their toggle went off, for when it comes
    /// back on.
    tcp_memory: String,
    udp_memory: String,
    pub timeout: String,
    pub concurrency: String,
    /// Why the last press of Scan did nothing.
    pub error: Option<String>,
}

impl Default for ScanForm {
    fn default() -> Self {
        ScanForm::from_settings(&ScanSettings::default())
    }
}

impl ScanForm {
    /// The fields filled in from `settings` (the command line's, at start-up).
    pub fn from_settings(settings: &ScanSettings) -> ScanForm {
        ScanForm {
            network: settings.network.map(|n| n.to_string()).unwrap_or_default(),
            interface: settings.interface.clone().unwrap_or_default(),
            interfaces: Vec::new(),
            arp: settings.arp,
            trace: settings.trace,
            names: settings.names,
            tcp_ports: port_text(&settings.ports, &COMMON_TCP_PORTS),
            udp_ports: port_text(&settings.udp_ports, &COMMON_UDP_PORTS),
            tcp_memory: String::new(),
            udp_memory: String::new(),
            timeout: seconds_text(settings.timeout),
            concurrency: settings.concurrency.to_string(),
            error: None,
        }
    }

    /// The settings the fields describe, or what is wrong with them. `save`
    /// is not on the dock: it stays what the command line said.
    pub fn settings(&self, save: Option<PathBuf>) -> Result<ScanSettings, String> {
        let network = match self.network.trim() {
            "" => None,
            text => Some(parse_network(text)?),
        };
        let interface = match self.interface.trim() {
            "" => None,
            name => Some(name.to_string()),
        };
        let timeout = Duration::from_secs_f64(parse_seconds(&self.timeout)?);
        let concurrency = match self.concurrency.trim().parse::<u16>() {
            Ok(n) if n > 0 => usize::from(n),
            _ => {
                return Err(format!(
                    "not a number of probes at once between 1 and 65535: {:?}",
                    self.concurrency
                ));
            }
        };
        Ok(ScanSettings {
            network,
            interface,
            ports: ports(&self.tcp_ports, &COMMON_TCP_PORTS)?,
            udp_ports: ports(&self.udp_ports, &COMMON_UDP_PORTS)?,
            names: self.names,
            arp: self.arp,
            trace: self.trace,
            timeout,
            concurrency,
            save,
        })
    }

    /// Whether a feature is on.
    pub fn enabled(&self, feature: Feature) -> bool {
        match feature {
            Feature::Arp => self.arp,
            Feature::Trace => self.trace,
            Feature::Names => self.names,
            Feature::Tcp => !self.tcp_ports.trim().is_empty(),
            Feature::Udp => !self.udp_ports.trim().is_empty(),
        }
    }

    /// Flip a feature. A port scan that comes back on gets the list it had,
    /// or the common ports.
    pub fn toggle(&mut self, feature: Feature) {
        match feature {
            Feature::Arp => self.arp = !self.arp,
            Feature::Trace => self.trace = !self.trace,
            Feature::Names => self.names = !self.names,
            Feature::Tcp => toggle_ports(&mut self.tcp_ports, &mut self.tcp_memory),
            Feature::Udp => toggle_ports(&mut self.udp_ports, &mut self.udp_memory),
        }
    }

    /// The interface as the dock shows it.
    pub fn interface_label(&self) -> String {
        if self.interface.is_empty() {
            "auto".to_string()
        } else {
            self.interface.clone()
        }
    }

    /// Go round: the default route, then each interface, then the default again.
    pub fn cycle_interface(&mut self) {
        let next = if self.interface.is_empty() {
            self.interfaces.first().cloned().unwrap_or_default()
        } else {
            self.interfaces
                .iter()
                .position(|name| *name == self.interface)
                .and_then(|i| self.interfaces.get(i + 1))
                .cloned()
                .unwrap_or_default()
        };
        self.interface = next;
    }

    /// The timeout as the dock shows it: "1 s", "0.5 s".
    pub fn timeout_label(&self) -> String {
        format!("{} s", self.timeout.trim())
    }

    /// To the next preset up (`+1`) or down (`-1`) from the current value.
    pub fn step_timeout(&mut self, direction: i8) {
        let current = self.timeout.trim().parse::<f64>().unwrap_or(1.0);
        let next = step(&TIMEOUT_STEPS, current, direction);
        self.timeout = seconds_text(Duration::from_secs_f64(next));
    }

    pub fn concurrency_label(&self) -> String {
        self.concurrency.trim().to_string()
    }

    pub fn step_concurrency(&mut self, direction: i8) {
        let current = self.concurrency.trim().parse::<u16>().unwrap_or(254);
        let next = step(&CONCURRENCY_STEPS, f64::from(current), direction);
        self.concurrency = (next as u16).to_string();
    }
}

/// On ↔ off for a port list: off clears it (remembering it), on brings the
/// memory back, or the common ports when there is none.
fn toggle_ports(list: &mut String, memory: &mut String) {
    if list.trim().is_empty() {
        *list = if memory.trim().is_empty() {
            "common".to_string()
        } else {
            memory.clone()
        };
    } else {
        memory.clone_from(list);
        list.clear();
    }
}

/// The preset after `current` going up, or before it going down; a value
/// between presets moves to the nearest one in that direction. Stops at
/// the ends.
fn step<T: Copy + Into<f64>>(steps: &[T], current: f64, direction: i8) -> f64 {
    let values: Vec<f64> = steps.iter().map(|&v| v.into()).collect();
    if direction > 0 {
        values
            .iter()
            .copied()
            .find(|v| *v > current + 1e-9)
            .unwrap_or(*values.last().unwrap_or(&current))
    } else {
        values
            .iter()
            .rev()
            .copied()
            .find(|v| *v < current - 1e-9)
            .unwrap_or(*values.first().unwrap_or(&current))
    }
}

/// A port list as typed: nothing for none, "common" for the usual ones,
/// else what [`parse_port_list`] reads.
fn ports(text: &str, common: &[u16]) -> Result<Vec<u16>, String> {
    let text = text.trim();
    if text.is_empty() {
        Ok(Vec::new())
    } else if text.eq_ignore_ascii_case("common") {
        Ok(common.to_vec())
    } else {
        parse_port_list(text).map_err(|err| err.to_string())
    }
}

/// A port list as text: "common" when it is the usual list, else the numbers.
fn port_text(ports: &[u16], common: &[u16]) -> String {
    if ports.is_empty() {
        String::new()
    } else if ports == common {
        "common".to_string()
    } else {
        ports
            .iter()
            .map(u16::to_string)
            .collect::<Vec<_>>()
            .join(",")
    }
}

/// Seconds as typed: "1", "0.5".
fn seconds_text(timeout: Duration) -> String {
    let seconds = timeout.as_secs_f64();
    if seconds.fract() == 0.0 {
        format!("{seconds:.0}")
    } else {
        format!("{seconds}")
    }
}

pub struct ScanFormPlugin;

impl Plugin for ScanFormPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ScanForm>()
            .add_systems(PreStartup, seed_form);
    }
}

/// Start with the command line's settings in the fields, and this machine's
/// interfaces in the cycler.
fn seed_form(settings: Res<ScanSettings>, mut form: ResMut<ScanForm>) {
    *form = ScanForm::from_settings(&settings);
    form.interfaces = local::local_networks()
        .unwrap_or_default()
        .into_iter()
        .map(|network| network.interface)
        .collect();
    form.interfaces.dedup(); // an interface with two subnets is listed once
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_form_round_trips_the_settings() {
        let settings = ScanSettings {
            network: Some("10.0.0.0/24".parse().unwrap()),
            interface: Some("eth0".into()),
            ports: COMMON_TCP_PORTS.to_vec(),
            udp_ports: vec![53, 123],
            names: false,
            arp: true,
            trace: false,
            timeout: Duration::from_millis(500),
            concurrency: 64,
            save: None,
        };
        let form = ScanForm::from_settings(&settings);
        assert_eq!(form.network, "10.0.0.0/24");
        assert_eq!(form.tcp_ports, "common");
        assert_eq!(form.udp_ports, "53,123");
        assert_eq!(form.timeout, "0.5");
        assert_eq!(form.concurrency, "64");
        assert_eq!(
            form.settings(Some("x.json".into())),
            Ok(ScanSettings {
                save: Some("x.json".into()),
                ..settings
            })
        );
        assert_eq!(
            ScanForm::default().settings(None),
            Ok(ScanSettings::default())
        );
    }

    #[test]
    fn bad_fields_are_refused_with_a_reason() {
        let mut form = ScanForm {
            network: "lan".into(),
            ..ScanForm::default()
        };
        let err = form.settings(None).unwrap_err();
        assert!(err.contains("not an IPv4 network"), "{err}");
        form.network = " 10.0.0.0/8 ".into();
        assert!(form.settings(None).unwrap_err().contains("largest sweep"));
        form.network.clear();
        form.tcp_ports = "http".into();
        assert!(form.settings(None).unwrap_err().contains("not a port"));
        form.tcp_ports = "Common".into();
        form.udp_ports = "53, 161".into();
        let settings = form.settings(None).unwrap();
        assert_eq!(settings.ports, COMMON_TCP_PORTS);
        assert_eq!(settings.udp_ports, [53, 161]);
        form.timeout = "0".into();
        assert!(form.settings(None).unwrap_err().contains("seconds"));
        form.timeout = "2".into();
        form.concurrency = "0".into();
        assert!(form.settings(None).unwrap_err().contains("probes"));
    }

    #[test]
    fn every_feature_is_a_toggle_and_port_lists_are_remembered() {
        let mut form = ScanForm::default();
        assert!(form.enabled(Feature::Arp) && form.enabled(Feature::Trace));
        assert!(form.enabled(Feature::Names));
        assert!(!form.enabled(Feature::Tcp) && !form.enabled(Feature::Udp));
        form.toggle(Feature::Arp);
        assert!(!form.arp);
        form.toggle(Feature::Tcp);
        assert_eq!(
            form.tcp_ports, "common",
            "on with no memory: the common ports"
        );
        form.tcp_ports = "22,80".into();
        form.toggle(Feature::Tcp);
        assert_eq!(form.tcp_ports, "", "off clears the list");
        assert!(!form.enabled(Feature::Tcp));
        form.toggle(Feature::Tcp);
        assert_eq!(form.tcp_ports, "22,80", "on again brings the list back");
        form.toggle(Feature::Udp);
        assert_eq!(form.udp_ports, "common");
    }

    #[test]
    fn timeout_and_concurrency_step_through_presets() {
        let mut form = ScanForm::default();
        assert_eq!(form.timeout_label(), "1 s");
        form.step_timeout(1);
        assert_eq!(form.timeout, "2");
        form.step_timeout(-1);
        form.step_timeout(-1);
        assert_eq!(form.timeout, "0.5");
        form.timeout = "0.7".into(); // from the command line, between presets
        form.step_timeout(1);
        assert_eq!(form.timeout, "1");
        form.timeout = "0.7".into();
        form.step_timeout(-1);
        assert_eq!(form.timeout, "0.5");
        for _ in 0..20 {
            form.step_timeout(1);
        }
        assert_eq!(form.timeout, "10", "stops at the top");

        assert_eq!(form.concurrency_label(), "254");
        form.step_concurrency(1);
        assert_eq!(form.concurrency, "512");
        form.concurrency = "100".into();
        form.step_concurrency(-1);
        assert_eq!(form.concurrency, "64");
        for _ in 0..20 {
            form.step_concurrency(-1);
        }
        assert_eq!(form.concurrency, "16", "stops at the bottom");
    }

    #[test]
    fn the_interface_cycles_through_the_list_and_back_to_auto() {
        let mut form = ScanForm {
            interfaces: vec!["wlan0".into(), "eth0".into()],
            ..ScanForm::default()
        };
        assert_eq!(form.interface_label(), "auto");
        form.cycle_interface();
        assert_eq!(form.interface_label(), "wlan0");
        form.cycle_interface();
        assert_eq!(form.interface_label(), "eth0");
        form.cycle_interface();
        assert_eq!(form.interface_label(), "auto");
        form.interface = "gone0".into(); // from the command line, not in the list
        form.cycle_interface();
        assert_eq!(form.interface, "");
    }
}
