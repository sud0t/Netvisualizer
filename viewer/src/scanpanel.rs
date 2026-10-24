//! The scan card: a Scan button and the scan's options, so a scan can be
//! started from the window the way `--scan` starts one; while a scan runs,
//! its stage, a progress bar and a Cancel button.
//!
//! The card only edits the [`ScanSettings`] resource and sends the same
//! [`StartScan`] message `--scan` does; scan.rs does the rest. Cancel pokes
//! the scan thread's `Notify`, which scan.rs selects on. Clear empties the
//! map: a scan adds to the map that is there, and nothing else takes a
//! host off it.

use std::path::PathBuf;
use std::time::Duration;

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use scanner::local;
use scanner::ports::{COMMON_TCP_PORTS, COMMON_UDP_PORTS, parse_port_list};

use crate::cli::{parse_network, parse_seconds};
use crate::graph::MapDocument;
use crate::panel::card;
use crate::picking::SelectHost;
use crate::scan::{ScanJob, ScanSettings, StartScan};
use crate::theme;
use crate::ui::{EDGE, Status, mono, text};

/// The card's fields, as typed; [`ScanForm::settings`] reads them.
#[derive(Resource, Debug, Clone, PartialEq, Eq)]
pub struct ScanForm {
    /// The options below the Scan row are shown.
    pub open: bool,
    /// A network or address; empty means the interface's own subnet.
    pub network: String,
    /// An interface name; empty means the one with the default route.
    pub interface: String,
    /// This machine's interfaces, for the drop-down.
    pub interfaces: Vec<String>,
    pub arp: bool,
    pub trace: bool,
    pub names: bool,
    /// Port lists as `--ports` and `--udp` take them; empty means none.
    pub tcp_ports: String,
    pub udp_ports: String,
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
            open: false,
            network: settings.network.map(|n| n.to_string()).unwrap_or_default(),
            interface: settings.interface.clone().unwrap_or_default(),
            interfaces: Vec::new(),
            arp: settings.arp,
            trace: settings.trace,
            names: settings.names,
            tcp_ports: port_text(&settings.ports, &COMMON_TCP_PORTS),
            udp_ports: port_text(&settings.udp_ports, &COMMON_UDP_PORTS),
            timeout: seconds_text(settings.timeout),
            concurrency: settings.concurrency.to_string(),
            error: None,
        }
    }

    /// The settings the fields describe, or what is wrong with them. `save`
    /// is not on the card: it stays what the command line said.
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

pub struct ScanPanelPlugin;

impl Plugin for ScanPanelPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ScanForm>()
            .add_systems(Startup, seed_form);
    }
}

/// Start with the command line's settings in the fields, and this machine's
/// interfaces in the drop-down.
fn seed_form(settings: Res<ScanSettings>, mut form: ResMut<ScanForm>) {
    *form = ScanForm::from_settings(&settings);
    form.interfaces = local::local_networks()
        .unwrap_or_default()
        .into_iter()
        .map(|network| network.interface)
        .collect();
    form.interfaces.dedup(); // an interface with two subnets is listed once
}

/// Draw the card, right under the HUD (an egui pass system; ui.rs orders it).
#[allow(clippy::too_many_arguments)] // a Bevy system lists what it reads
pub fn draw_scan_card(
    mut contexts: EguiContexts,
    mut form: ResMut<ScanForm>,
    mut settings: ResMut<ScanSettings>,
    job: Option<Res<ScanJob>>,
    mut start: MessageWriter<StartScan>,
    mut doc: ResMut<MapDocument>,
    mut status: ResMut<Status>,
    mut select: MessageWriter<SelectHost>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    // egui remembers the HUD's rectangle from the last frame.
    let top = ctx
        .memory(|memory| memory.area_rect(egui::Id::new("hud")))
        .map_or(EDGE + 48.0, |rect| rect.bottom() + 8.0);
    egui::Area::new(egui::Id::new("scan"))
        .anchor(egui::Align2::LEFT_TOP, egui::vec2(EDGE, top))
        .show(ctx, |ui| {
            card(8)
                .inner_margin(egui::Margin::symmetric(10, 8))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(8.0, 6.0);
                    match &job {
                        Some(job) => draw_running(ui, job),
                        None => draw_idle(
                            ui,
                            &mut form,
                            &mut settings,
                            &mut start,
                            &mut doc,
                            &mut status,
                            &mut select,
                        ),
                    }
                });
        });
    Ok(())
}

/// While a scan runs: Cancel, the stage, and how far the stage is.
fn draw_running(ui: &mut egui::Ui, job: &ScanJob) {
    ui.horizontal(|ui| {
        if ui
            .add(egui::Button::new(text("Cancel", theme::TEXT)))
            .on_hover_text("stop the scan; the map keeps what it has")
            .clicked()
        {
            job.cancel();
        }
        ui.label(mono(&job.stage, theme::MUTED));
    });
    let (fraction, label) = match job.progress {
        Some((done, total)) if total > 0 => {
            (done as f32 / total as f32, format!("{done} / {total}"))
        }
        _ => (0.0, String::new()), // a stage that reports no steps
    };
    ui.add(
        egui::ProgressBar::new(fraction)
            .desired_width(260.0)
            .text(mono(&label, theme::TEXT))
            .animate(job.progress.is_none()),
    );
}

/// Idle: the Scan button, the network box, the options when opened, and
/// Clear while there is a map to clear.
fn draw_idle(
    ui: &mut egui::Ui,
    form: &mut ScanForm,
    settings: &mut ScanSettings,
    start: &mut MessageWriter<StartScan>,
    doc: &mut MapDocument,
    status: &mut Status,
    select: &mut MessageWriter<SelectHost>,
) {
    ui.horizontal(|ui| {
        let scan = egui::Button::new(text("Scan", theme::TEXT).strong());
        if ui
            .add(scan)
            .on_hover_text("find the hosts; nothing touches the network before this")
            .clicked()
        {
            match form.settings(settings.save.clone()) {
                Ok(wanted) => {
                    form.error = None;
                    *settings = wanted;
                    start.write(StartScan);
                }
                Err(err) => form.error = Some(err),
            }
        }
        ui.add(
            egui::TextEdit::singleline(&mut form.network)
                .hint_text("network (default: the subnet)")
                .desired_width(190.0),
        );
        // Plain characters: egui's bundled font has no triangles.
        let arrow = if form.open { "- options" } else { "+ options" };
        if ui
            .add(egui::Button::new(text(arrow, theme::MUTED)).frame(false))
            .clicked()
        {
            form.open = !form.open;
        }
        let has_map = doc.0.as_ref().is_some_and(|map| !map.hosts.is_empty());
        let clear = egui::Button::new(text("Clear", theme::MUTED)).frame(false);
        if ui
            .add_enabled(has_map, clear)
            .on_hover_text("forget every host; the next scan starts a map afresh")
            .clicked()
        {
            doc.0 = None;
            status.show_message("map cleared");
            select.write(SelectHost {
                id: None,
                fly: false,
            });
        }
    });
    if let Some(err) = &form.error {
        ui.label(mono(err, theme::ERROR));
    }
    if !form.open {
        return;
    }
    egui::Grid::new("scan-options")
        .num_columns(2)
        .spacing([10.0, 4.0])
        .show(ui, |ui| {
            ui.label(mono("interface", theme::MUTED));
            let shown = if form.interface.is_empty() {
                "default route".to_string()
            } else {
                form.interface.clone()
            };
            egui::ComboBox::from_id_salt("scan-interface")
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut form.interface, String::new(), "default route");
                    for name in form.interfaces.clone() {
                        ui.selectable_value(&mut form.interface, name.clone(), name);
                    }
                });
            ui.end_row();

            ui.label(mono("find hosts", theme::MUTED));
            ui.horizontal(|ui| {
                ui.checkbox(&mut form.arp, "ARP")
                    .on_hover_text("needs root or CAP_NET_RAW; ping otherwise");
                ui.checkbox(&mut form.trace, "traceroute")
                    .on_hover_text("the routers on the way to hosts beyond the subnet");
                ui.checkbox(&mut form.names, "names")
                    .on_hover_text("reverse DNS");
            });
            ui.end_row();

            field(
                ui,
                "TCP ports",
                &mut form.tcp_ports,
                "none; \"common\" or 22,80,8000-8010",
            );
            field(
                ui,
                "UDP ports",
                &mut form.udp_ports,
                "none; \"common\" or 53,123",
            );
            field(ui, "timeout (s)", &mut form.timeout, "per probe");
            field(ui, "at once", &mut form.concurrency, "probes in flight");
        });
}

fn field(ui: &mut egui::Ui, label: &str, value: &mut String, hint: &str) {
    ui.label(mono(label, theme::MUTED));
    ui.add(
        egui::TextEdit::singleline(value)
            .hint_text(hint)
            .desired_width(230.0),
    );
    ui.end_row();
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
}
