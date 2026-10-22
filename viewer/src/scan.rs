//! Scanning: the [`StartScan`] message, the scan stages on a background
//! thread, and the maps they produce coming back into the viewer.
//!
//! The stages run in this order (what `__main__.run_scan` did in Python),
//! and after each one the viewer gets a new map, so it redraws as the scan
//! goes:
//!
//! 1. the sweep: an ICMP echo request to every address (`scanner::sweep`);
//! 2. the gateway is kept even if it ignored the ping, and every host is
//!    marked as on our own subnet or not;
//! 3. names, unless turned off: reverse DNS (`scanner::names`);
//! 4. ports, when asked for: the TCP connect scan (`scanner::ports`).
//!
//! The scanner is async (hundreds of probes in flight at once), so a scan
//! runs on its own thread with its own tokio runtime and reports over a
//! channel; Bevy systems only ever read what has already arrived.
//! `--no-window` runs the same thread and prints the reports instead.

use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use bevy::prelude::*;
use scanner::Ipv4Net;
use scanner::local::{self, LocalNetwork, LocalNetworkError};
use scanner::names;
use scanner::ports::{self, PortScanOptions};
use scanner::sweep::{self, SweepOptions};
use serde_json::Value;

use crate::ViewerSet;
use crate::graph::MapDocument;
use crate::map::{MapDoc, Scan, Status as HostStatus};
use crate::ui::Status;

/// How often the scan thread reports progress at most.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);

/// What to scan and how. Filled in from the command line for now; the scan
/// options in the UI (Milestone 10) will edit it.
#[derive(Resource, Debug, Clone, PartialEq)]
pub struct ScanSettings {
    /// The network to sweep; `None` means the subnet of `interface`.
    pub network: Option<Ipv4Net>,
    /// Whose subnet to sweep and whose address is "ours"; `None` means the
    /// interface our internet traffic leaves from.
    pub interface: Option<String>,
    /// TCP ports to try on every host once the sweep is done; empty skips
    /// the port scan.
    pub ports: Vec<u16>,
    /// Look the hosts' names up with reverse DNS.
    pub names: bool,
    /// How long each ping or connection may take.
    pub timeout: Duration,
    /// How many pings or connections are in flight at once.
    pub concurrency: usize,
    /// Write the finished map here.
    pub save: Option<PathBuf>,
}

impl Default for ScanSettings {
    fn default() -> Self {
        ScanSettings {
            network: None,
            interface: None,
            ports: Vec::new(),
            names: true,
            timeout: Duration::from_secs(1),
            concurrency: 254,
            save: None,
        }
    }
}

/// A scan worked out: the settings plus where this machine is.
#[derive(Debug, Clone)]
pub struct ScanPlan {
    pub settings: ScanSettings,
    /// The interface the scan runs from: our address, its subnet, its gateway.
    pub local: LocalNetwork,
    /// What the sweep covers: `settings.network`, or else `local.network`.
    pub network: Ipv4Net,
}

impl ScanPlan {
    /// Find out where we are. Only reads local state, so it is instant; it
    /// fails when the interface does not exist or there is no network.
    pub fn new(settings: &ScanSettings) -> Result<ScanPlan, LocalNetworkError> {
        let local = local::default_network(settings.interface.as_deref())?;
        Ok(ScanPlan {
            network: settings.network.unwrap_or(local.network),
            settings: settings.clone(),
            local,
        })
    }

    /// "sweeping 192.168.1.0/24 from 192.168.1.143 on wlan0 (gateway 192.168.1.254)"
    pub fn describe(&self) -> String {
        let gateway = match self.local.gateway {
            Some(gateway) => format!(" (gateway {gateway})"),
            None => String::new(),
        };
        format!(
            "sweeping {} from {} on {}{gateway}",
            self.network, self.local.ip, self.local.interface
        )
    }
}

/// Start a scan with the current [`ScanSettings`]. `--scan` sends one at
/// start-up; the Scan button (Milestone 10) will send the same. Ignored
/// while a scan is running.
#[derive(Message, Debug, Clone, Copy, Default)]
pub struct StartScan;

/// What the scan thread reports, in order.
#[derive(Debug)]
pub enum ScanEvent {
    /// The map as it stands; its `scan.stage` says what runs next
    /// ("resolving names…"), or "done".
    Map(Box<MapDoc>),
    /// `done` of `total` probes of the running stage are settled.
    Progress { done: usize, total: usize },
    /// A line worth telling the user: who answered, a fallback, a saved file.
    Note(String),
    /// The scan is over; `Err` says why it stopped early. Always the last event.
    Finished(Result<(), String>),
}

/// Start a scan on its own thread. The thread ends after sending
/// [`ScanEvent::Finished`], or as soon as nobody listens any more.
pub fn spawn(plan: ScanPlan) -> Receiver<ScanEvent> {
    let (tx, rx) = mpsc::channel();
    thread::Builder::new()
        .name("netvisualizer-scan".into())
        .spawn(move || {
            // One thread is plenty: the probes all wait on the network, and
            // the blocking DNS lookups get tokio's own blocking threads.
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();
            let result = match runtime {
                Ok(runtime) => runtime.block_on(run(&plan, &tx)),
                Err(err) => Err(format!("could not start the scan's runtime: {err}")),
            };
            let _ = tx.send(ScanEvent::Finished(result)); // fails only if the viewer quit
        })
        .expect("could not start the scan thread");
    rx
}

/// The stages, in order (see the module comment).
async fn run(plan: &ScanPlan, tx: &Sender<ScanEvent>) -> Result<(), String> {
    let settings = &plan.settings;
    let mut report = Reporter::new(plan, tx);
    report.map(&[], &format!("sweeping {}…", plan.network));

    let options = SweepOptions {
        timeout: settings.timeout,
        concurrency: settings.concurrency,
    };
    let sweep = sweep::ping_sweep(&plan.network, &options, progress(tx))
        .await
        .map_err(|err| format!("the sweep failed: {err}"))?;
    for note in &sweep.notes {
        report.note(note.clone());
    }
    report.note(format!(
        "{}: {} of {} addresses answered ({})",
        plan.network,
        sweep.hosts.len(),
        sweep.probed,
        sweep.method.describe()
    ));
    let mut hosts = sweep.hosts;
    keep_gateway(&mut hosts, plan.local.gateway);
    local::mark_local(&mut hosts, &plan.local.network);

    if settings.names {
        report.map(&hosts, "resolving names…");
        names::resolve_hostnames(&mut hosts, names::DEFAULT_CONCURRENCY, false).await;
        // Reverse DNS rarely knows the machine asking.
        let me = IpAddr::V4(plan.local.ip);
        if let Some(host) = hosts.iter_mut().find(|h| h.ip == me)
            && host.hostname.is_none()
        {
            host.hostname = names::own_hostname();
        }
        let named = hosts.iter().filter(|h| h.hostname.is_some()).count();
        report.note(format!("{named} of {} hosts have a name", hosts.len()));
    }

    if !settings.ports.is_empty() {
        let count = settings.ports.len();
        report.map(&hosts, &format!("trying {count} TCP ports…"));
        let options = PortScanOptions {
            timeout: settings.timeout,
            concurrency: settings.concurrency,
        };
        ports::tcp_port_scan(&mut hosts, &settings.ports, &options, progress(tx)).await;
        let open: usize = hosts.iter().map(|h| h.open_ports.len()).sum();
        report.note(format!(
            "{open} open ports on {} hosts ({count} ports tried each)",
            hosts.len()
        ));
    }

    let doc = report.map(&hosts, "done");
    if let Some(path) = &settings.save {
        doc.save(path)
            .map_err(|err| format!("could not save {}: {err}", path.display()))?;
        report.note(format!(
            "saved {} hosts to {}",
            doc.hosts.len(),
            path.display()
        ));
    }
    Ok(())
}

/// Keep the gateway on the map even when it ignored the ping (many routers
/// do); it is drawn greyed out.
fn keep_gateway(hosts: &mut Vec<scanner::Host>, gateway: Option<Ipv4Addr>) {
    let Some(gateway) = gateway.map(IpAddr::V4) else {
        return;
    };
    if hosts.iter().any(|h| h.ip == gateway) {
        return;
    }
    let mut host = scanner::Host::new(gateway);
    host.extra.insert("status".into(), "down".into());
    host.extra
        .insert("note".into(), "did not answer ping".into());
    hosts.push(host);
}

/// Sends the scan's maps (numbering them) and notes.
struct Reporter<'a> {
    plan: &'a ScanPlan,
    tx: &'a Sender<ScanEvent>,
    revision: u64,
}

impl<'a> Reporter<'a> {
    fn new(plan: &'a ScanPlan, tx: &'a Sender<ScanEvent>) -> Self {
        Reporter {
            plan,
            tx,
            revision: 0,
        }
    }

    /// Send the map of `hosts`, with `stage` for the HUD; returns it too.
    fn map(&mut self, hosts: &[scanner::Host], stage: &str) -> MapDoc {
        self.revision += 1;
        let scan = Scan {
            kind: Some("ping_sweep".to_string()),
            network: Some(self.plan.network.to_string()),
            stage: Some(stage.to_string()),
            ..Scan::default()
        };
        let mut doc = MapDoc::from_hosts(
            hosts,
            Some(IpAddr::V4(self.plan.local.ip)),
            self.plan.local.gateway.map(IpAddr::V4),
            scan,
        );
        doc.revision = self.revision;
        let _ = self.tx.send(ScanEvent::Map(Box::new(doc.clone())));
        doc
    }

    fn note(&self, text: String) {
        let _ = self.tx.send(ScanEvent::Note(text));
    }
}

/// A progress callback for the scanner that passes `(done, total)` on at
/// most every [`PROGRESS_INTERVAL`], and always the last step.
fn progress(tx: &Sender<ScanEvent>) -> impl FnMut(usize, usize) + '_ {
    let mut last_sent: Option<Instant> = None;
    move |done, total| {
        let now = Instant::now();
        let due = last_sent.is_none_or(|sent| now.duration_since(sent) >= PROGRESS_INTERVAL);
        if done == total || due {
            last_sent = Some(now);
            let _ = tx.send(ScanEvent::Progress { done, total });
        }
    }
}

// ---------------------------------------------------------------------------
// In the viewer.
// ---------------------------------------------------------------------------

pub struct ScanPlugin;

impl Plugin for ScanPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ScanSettings>()
            .add_message::<StartScan>()
            .add_systems(
                Update,
                (start_scan, receive).chain().in_set(ViewerSet::Receive),
            );
    }
}

/// The running scan's reports. Exists only while a scan runs (`Receiver` is
/// not `Sync`, hence the mutex).
#[derive(Resource)]
pub struct ScanJob(Mutex<Receiver<ScanEvent>>);

impl ScanJob {
    pub fn new(events: Receiver<ScanEvent>) -> ScanJob {
        ScanJob(Mutex::new(events))
    }
}

fn start_scan(
    mut requests: MessageReader<StartScan>,
    settings: Res<ScanSettings>,
    job: Option<Res<ScanJob>>,
    mut status: ResMut<Status>,
    mut commands: Commands,
) {
    if requests.read().count() == 0 {
        return;
    }
    if job.is_some() {
        warn!("a scan is already running");
        return;
    }
    match ScanPlan::new(&settings) {
        Ok(plan) => {
            info!("{}", plan.describe());
            commands.insert_resource(ScanJob::new(spawn(plan)));
        }
        Err(err) => {
            error!("cannot scan: {err}");
            status.show_error(format!("cannot scan: {err}"));
        }
    }
}

/// Hand what the scan reported to the rest of the viewer. A new map makes
/// graph.rs rebuild, the panel re-sort and scene.rs re-frame (while the user
/// has not moved the camera); the selection survives, being kept by host id.
fn receive(
    job: Option<Res<ScanJob>>,
    mut doc: ResMut<MapDocument>,
    mut status: ResMut<Status>,
    mut commands: Commands,
) {
    let Some(job) = job else {
        return;
    };
    let Ok(events) = job.0.lock() else {
        return;
    };
    for event in events.try_iter() {
        match event {
            ScanEvent::Map(map) => {
                if let Some(stage) = &map.scan.stage {
                    info!("{stage}");
                }
                status.show_map(map.summary());
                doc.0 = Some(*map);
            }
            ScanEvent::Progress { .. } => {} // the HUD shows no progress yet (Milestone 10)
            ScanEvent::Note(note) => info!("{note}"),
            ScanEvent::Finished(result) => {
                if let Err(err) = result {
                    error!("scan failed: {err}");
                    status.show_error(format!("scan failed: {err}"));
                }
                commands.remove_resource::<ScanJob>();
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Without a window.
// ---------------------------------------------------------------------------

/// `--no-window`: run the scan here and print what it reports, then the hosts.
pub fn run_headless(plan: ScanPlan) -> ExitCode {
    println!("{}", plan.describe());
    let started = Instant::now();
    let mut stage = String::new();
    let mut last_progress = Instant::now();
    let mut last_map = None;
    for event in spawn(plan) {
        match event {
            ScanEvent::Map(map) => {
                let new_stage = map.scan.stage.clone().unwrap_or_default();
                if new_stage != stage && new_stage != "done" && map.revision > 1 {
                    println!("{new_stage}");
                }
                stage = new_stage;
                last_progress = Instant::now(); // a stage starts: no progress line for a second
                last_map = Some(map);
            }
            // Only for stages slow enough to need it: at most once a second.
            ScanEvent::Progress { done, total } => {
                if done < total && last_progress.elapsed() >= Duration::from_secs(1) {
                    println!("  {done} / {total}");
                    last_progress = Instant::now();
                }
            }
            ScanEvent::Note(note) => println!("{note}"),
            ScanEvent::Finished(Err(err)) => {
                eprintln!("netvisualizer: {err}");
                return ExitCode::FAILURE;
            }
            ScanEvent::Finished(Ok(())) => {}
        }
    }
    if let Some(map) = last_map {
        println!();
        for line in host_table(&map) {
            println!("{line}");
        }
    }
    println!("done in {:.1} s", started.elapsed().as_secs_f64());
    ExitCode::SUCCESS
}

/// One line per host: role, address, name, open ports.
///
/// ```text
/// scanner  192.168.1.143    kylr-laptop
/// gateway  192.168.1.254    router.lan               53/tcp domain, 80/tcp http
/// ```
fn host_table(map: &MapDoc) -> Vec<String> {
    map.hosts
        .iter()
        .map(|host| {
            let name = host
                .fields
                .get("hostname")
                .and_then(Value::as_str)
                .unwrap_or("-");
            let ports: Vec<String> = host
                .fields
                .get("open_ports")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|port| {
                    let number = &port["number"];
                    let proto = port["proto"].as_str().unwrap_or("?");
                    match port["service"].as_str() {
                        Some(service) => format!("{number}/{proto} {service}"),
                        None => format!("{number}/{proto}"),
                    }
                })
                .collect();
            let mut tail = ports.join(", ");
            if host.status() == HostStatus::Down {
                if !tail.is_empty() {
                    tail.push(' ');
                }
                tail.push_str("(down)");
            }
            format!(
                "{:<8} {:<16} {:<24} {tail}",
                host.role().name(),
                host.id,
                name
            )
            .trim_end()
            .to_string()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;

    use super::*;

    /// A scan of 127.0.0.0/30 "from" loopback: both addresses answer, and
    /// one has a port open.
    fn loopback_plan(ports: Vec<u16>, gateway: Option<Ipv4Addr>) -> ScanPlan {
        let settings = ScanSettings {
            network: Some("127.0.0.0/30".parse().unwrap()),
            ports,
            timeout: Duration::from_millis(500),
            ..ScanSettings::default()
        };
        ScanPlan {
            network: settings.network.unwrap(),
            settings,
            local: LocalNetwork {
                interface: "lo".into(),
                ip: Ipv4Addr::LOCALHOST,
                network: "127.0.0.0/8".parse().unwrap(),
                gateway,
            },
        }
    }

    fn all_events(plan: ScanPlan) -> Vec<ScanEvent> {
        spawn(plan).into_iter().collect()
    }

    fn maps(events: &[ScanEvent]) -> Vec<&MapDoc> {
        events
            .iter()
            .filter_map(|e| match e {
                ScanEvent::Map(map) => Some(map.as_ref()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_scan_sends_a_map_per_stage_and_finishes() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let events = all_events(loopback_plan(vec![port], None));

        assert!(
            matches!(events.last(), Some(ScanEvent::Finished(Ok(())))),
            "{events:?}"
        );
        let maps = maps(&events);
        let stages: Vec<&str> = maps
            .iter()
            .filter_map(|m| m.scan.stage.as_deref())
            .collect();
        assert_eq!(
            stages,
            [
                "sweeping 127.0.0.0/30…",
                "resolving names…",
                "trying 1 TCP ports…",
                "done"
            ]
        );
        let revisions: Vec<u64> = maps.iter().map(|m| m.revision).collect();
        assert_eq!(revisions, [1, 2, 3, 4]);
        assert_eq!(maps[0].hosts.len(), 1, "before the sweep: only the scanner");

        let done = maps[3];
        let ids: Vec<&str> = done.hosts.iter().map(|h| h.id.as_str()).collect();
        assert_eq!(ids, ["127.0.0.1", "127.0.0.2"]);
        let scanner = &done.hosts[0];
        assert_eq!(scanner.role(), crate::map::Role::Scanner);
        assert!(scanner.fields["hostname"].is_string(), "named: {scanner:?}");
        assert_eq!(scanner.fields["is_local"], true);
        assert_eq!(scanner.fields["open_ports"][0]["number"], port);
        assert_eq!(done.links.len(), 1);

        let notes: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                ScanEvent::Note(n) => Some(n.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            notes
                .iter()
                .any(|n| n.starts_with("127.0.0.0/30: 2 of 2 addresses answered")),
            "{notes:?}"
        );
        assert!(
            notes
                .iter()
                .any(|n| n.starts_with("2 open ports") || n.starts_with("1 open ports")),
            "{notes:?}"
        );
    }

    #[test]
    fn a_silent_gateway_is_kept_greyed_out_and_the_map_is_saved() {
        let path =
            std::env::temp_dir().join(format!("netvisualizer-scan-{}.json", std::process::id()));
        let mut plan = loopback_plan(Vec::new(), Some(Ipv4Addr::new(192, 0, 2, 1)));
        plan.settings.names = false;
        plan.settings.save = Some(path.clone());
        let events = all_events(plan);
        assert!(
            matches!(events.last(), Some(ScanEvent::Finished(Ok(())))),
            "{events:?}"
        );

        let stages: Vec<String> = maps(&events)
            .iter()
            .filter_map(|m| m.scan.stage.clone())
            .collect();
        assert_eq!(stages, ["sweeping 127.0.0.0/30…", "done"]);

        let saved = MapDoc::load(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        let gateway = saved.hosts.iter().find(|h| h.id == "192.0.2.1").unwrap();
        assert_eq!(gateway.role(), crate::map::Role::Gateway);
        assert_eq!(gateway.status(), HostStatus::Down);
        assert_eq!(saved.hosts.len(), 3);
        assert_eq!(saved.revision, 2);
    }

    #[test]
    fn progress_is_throttled_but_the_last_step_always_arrives() {
        let (tx, rx) = mpsc::channel();
        let mut report = progress(&tx);
        for done in 1..=1000 {
            report(done, 1000);
        }
        drop(report);
        let steps: Vec<(usize, usize)> = rx
            .try_iter()
            .map(|e| match e {
                ScanEvent::Progress { done, total } => (done, total),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(steps.first(), Some(&(1, 1000)));
        assert_eq!(steps.last(), Some(&(1000, 1000)));
        assert!(steps.len() < 10, "{steps:?}");
    }

    #[test]
    fn the_host_table_lists_role_address_name_and_ports() {
        let mut doc = MapDoc::from_json(include_str!("../tests/fixtures/demo.json")).unwrap();
        let silent = r#"{"id": "192.168.1.254", "role": "host", "extra": {"status": "down"}}"#;
        doc.hosts.push(serde_json::from_str(silent).unwrap());
        let table = host_table(&doc);
        assert_eq!(table.len(), 28);
        assert_eq!(
            table[0],
            "scanner  192.168.1.50     netvisualizer-host.lan   22/tcp"
        );
        assert!(
            table[1]
                .starts_with("gateway  192.168.1.1      gateway.lan              53/udp, 80/tcp"),
            "{}",
            table[1]
        );
        assert_eq!(
            table[27],
            "host     192.168.1.254    -                        (down)"
        );
    }

    // ----- the Bevy side -----

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .configure_sets(Update, ViewerSet::Receive)
            .init_resource::<MapDocument>()
            .init_resource::<Status>()
            .add_plugins(ScanPlugin);
        app
    }

    #[test]
    fn maps_from_the_scan_replace_the_document_and_the_job_ends_with_the_scan() {
        let mut app = app();
        let (tx, rx) = mpsc::channel();
        app.insert_resource(ScanJob::new(rx));
        let mut map = MapDoc::from_json(
            r#"{"revision": 2, "scan": {"type": "ping_sweep", "stage": "resolving names…"}}"#,
        )
        .unwrap();
        map.hosts = MapDoc::from_json(include_str!("../tests/fixtures/demo.json"))
            .unwrap()
            .hosts;
        tx.send(ScanEvent::Map(Box::new(map))).unwrap();
        tx.send(ScanEvent::Note("27 of 27 addresses answered".into()))
            .unwrap();
        app.update();

        let doc = app.world().resource::<MapDocument>();
        assert_eq!(doc.0.as_ref().map(|d| d.hosts.len()), Some(27));
        let status = app.world().resource::<Status>();
        assert_eq!(
            status.text,
            "ping_sweep · resolving names… · 27 hosts · 0 links · rev 2"
        );
        assert!(app.world().contains_resource::<ScanJob>(), "still running");

        tx.send(ScanEvent::Finished(Err("the sweep failed: boom".into())))
            .unwrap();
        app.update();
        let status = app.world().resource::<Status>();
        assert!(status.error);
        assert_eq!(status.text, "scan failed: the sweep failed: boom");
        assert!(!app.world().contains_resource::<ScanJob>());
    }

    #[test]
    fn a_scan_that_cannot_start_says_why() {
        let mut app = app();
        app.insert_resource(ScanSettings {
            interface: Some("no-such-interface0".into()),
            ..ScanSettings::default()
        });
        app.world_mut().write_message(StartScan);
        app.update();
        let status = app.world().resource::<Status>();
        assert!(status.error);
        assert!(
            status
                .text
                .starts_with("cannot scan: no IPv4 network on interface"),
            "{}",
            status.text
        );
        assert!(!app.world().contains_resource::<ScanJob>());
    }
}
