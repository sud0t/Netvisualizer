//! Scanning: the [`StartScan`] message, the scan stages on a background
//! thread, and the maps they produce coming back into the viewer.
//!
//! The stages run in this order (what `__main__.run_scan` did in Python),
//! and after each one the viewer gets a new map, so it redraws as the scan
//! goes:
//!
//! 1. the sweep: an ARP request to every address on our own link when a
//!    packet socket may be opened (`scanner::arp`); an ICMP echo request to
//!    every address of the network when the ARP sweep could not run or the
//!    network reaches beyond the link (`scanner::sweep`), the hosts ARP
//!    found keeping their entries;
//! 2. the gateway is kept even if it ignored the ping, and every host is
//!    marked as on our own subnet or not;
//! 3. MACs the ARP sweep did not bring, from the kernel's ARP table, which
//!    the ping sweep filled (`scanner::neighbours`); then their vendors
//!    through Python (vendors.rs);
//! 4. names, unless turned off: reverse DNS (`scanner::names`);
//! 5. ports, when asked for: the TCP connect scan (`scanner::ports`).
//!
//! The scanner is async (hundreds of probes in flight at once), so a scan
//! runs on its own thread with its own tokio runtime and reports over a
//! channel; Bevy systems only ever read what has already arrived.
//! `--no-window` runs the same thread and prints the reports instead.

use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use bevy::prelude::*;
use scanner::arp::PacketSocket;
use scanner::local::{self, LocalNetwork, LocalNetworkError};
use scanner::names;
use scanner::neighbours;
use scanner::ports::{self, PortScanOptions};
use scanner::sweep::{self, SweepOptions};
use scanner::trace::{self, Path, TraceOptions};
use scanner::{Ipv4Net, Proto};
use serde_json::Value;
use tokio::sync::Notify;

use crate::ViewerSet;
use crate::graph::MapDocument;
use crate::map::{MapDoc, Scan, Status as HostStatus};
use crate::ui::Status;
use crate::vendors;

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
    /// UDP ports to probe after the TCP ones; empty skips it.
    pub udp_ports: Vec<u16>,
    /// Look the hosts' names up with reverse DNS.
    pub names: bool,
    /// Sweep the part of the network on our own link with ARP when a packet
    /// socket can be opened (root or `CAP_NET_RAW`); off means ping only.
    pub arp: bool,
    /// Trace the route to the hosts beyond our own subnet: the routers on
    /// the way become hosts of the map, and the links follow the path.
    pub trace: bool,
    /// How long each ping or connection may take, and how long the ARP
    /// sweep listens after its last request.
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
            udp_ports: Vec::new(),
            names: true,
            arp: true,
            trace: true,
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

    /// The part of the swept network that is on our own link, where ARP
    /// reaches: all of it when it lies inside our subnet, our subnet when it
    /// is the bigger one, nothing when they do not overlap.
    pub fn link_network(&self) -> Option<Ipv4Net> {
        let (swept, link) = (self.network, self.local.network);
        if link.contains(&swept) {
            Some(swept)
        } else if swept.contains(&link) {
            Some(link)
        } else {
            None
        }
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
/// start-up and the Scan button sends the same. Ignored while a scan is
/// running.
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
    /// The scan is over; `Err` says why it stopped early. The last event,
    /// unless the scan was cancelled.
    Finished(Result<(), String>),
    /// The scan was cancelled; the maps sent so far stand. The last event.
    Cancelled,
}

/// Start a scan on its own thread. The thread ends after sending
/// [`ScanEvent::Finished`] or [`ScanEvent::Cancelled`], or as soon as
/// nobody listens any more. Notifying the handle cancels the scan.
pub fn spawn(plan: ScanPlan) -> (Receiver<ScanEvent>, Arc<Notify>) {
    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(Notify::new());
    let stop = Arc::clone(&cancel);
    thread::Builder::new()
        .name("netvisualizer-scan".into())
        .spawn(move || {
            // One thread is plenty: the probes all wait on the network, and
            // the blocking DNS lookups get tokio's own blocking threads.
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();
            let outcome = match runtime {
                // Cancelling drops the stages' future, and every probe in
                // flight, its socket included, goes with it.
                Ok(runtime) => runtime.block_on(async {
                    tokio::select! {
                        result = run(&plan, &tx) => Some(result),
                        () = stop.notified() => None,
                    }
                }),
                Err(err) => Some(Err(format!("could not start the scan's runtime: {err}"))),
            };
            let last = match outcome {
                Some(result) => ScanEvent::Finished(result),
                None => ScanEvent::Cancelled,
            };
            let _ = tx.send(last); // fails only if the viewer quit
        })
        .expect("could not start the scan thread");
    (rx, cancel)
}

/// The stages, in order (see the module comment).
async fn run(plan: &ScanPlan, tx: &Sender<ScanEvent>) -> Result<(), String> {
    let settings = &plan.settings;
    let mut report = Reporter::new(plan, tx);

    // ARP for the part of the network on our own link, when it is allowed
    // and possible; ping for the rest, or for everything otherwise.
    let arp = match plan.link_network().filter(|_| settings.arp) {
        Some(link) => match PacketSocket::open(&plan.local) {
            Ok(socket) => Some((link, socket)),
            Err(err) => {
                report.note(format!("ARP sweep skipped: {err}"));
                None
            }
        },
        None => None,
    };
    let mut ping = match &arp {
        Some((link, _)) if *link == plan.network => None,
        _ => Some(plan.network),
    };
    report.kind = match (&arp, &ping) {
        (Some(_), None) => "arp_sweep",
        (Some(_), Some(_)) => "arp_ping_sweep",
        (None, _) => "ping_sweep",
    };
    report.map(&[], &format!("sweeping {}…", plan.network));

    let mut hosts = Vec::new();
    if let Some((link, socket)) = &arp {
        match socket.sweep(link, settings.timeout, progress(tx)).await {
            Ok(sweep) => {
                report.note(format!(
                    "{link}: {} of {} addresses answered ARP",
                    sweep.hosts.len(),
                    sweep.probed
                ));
                hosts = sweep.hosts;
            }
            // The interface went bad under us (a queue that never drains,
            // the link gone): the ping sweep may still get through.
            Err(err) => {
                report.note(format!("the ARP sweep failed ({err}); pinging instead"));
                report.kind = "ping_sweep";
                ping = Some(plan.network);
            }
        }
    }
    if let Some(network) = ping {
        let options = SweepOptions {
            timeout: settings.timeout,
            concurrency: settings.concurrency,
        };
        let sweep = sweep::ping_sweep(&network, &options, progress(tx))
            .await
            .map_err(|err| format!("the sweep failed: {err}"))?;
        for note in &sweep.notes {
            report.note(note.clone());
        }
        report.note(format!(
            "{network}: {} of {} addresses answered ({})",
            sweep.hosts.len(),
            sweep.probed,
            sweep.method.describe()
        ));
        // A host both sweeps found keeps its ARP entry, which has the MAC.
        let known: HashSet<IpAddr> = hosts.iter().map(|h| h.ip).collect();
        hosts.extend(sweep.hosts.into_iter().filter(|h| !known.contains(&h.ip)));
        hosts.sort_by_key(|h| h.ip);
    }
    // The ARP sweep never finds this machine (nobody answers their own
    // request), nor does a ping sweep of some other network: put it first,
    // so the later stages fill it in like any other host.
    let me = IpAddr::V4(plan.local.ip);
    if !hosts.iter().any(|h| h.ip == me) {
        hosts.insert(0, scanner::Host::new(me));
    }
    let probe = match report.kind {
        "arp_sweep" => "ARP",
        "ping_sweep" => "ping",
        _ => "ARP or ping",
    };
    keep_gateway(&mut hosts, plan.local.gateway, &plan.network, probe);
    local::mark_local(&mut hosts, &plan.local.network);

    // Hosts beyond our link: which routers do the packets pass on the way?
    // Those routers become hosts too (so they get names like anyone), and
    // the map draws each path instead of guessing "behind the gateway".
    let remote: Vec<Ipv4Addr> = hosts
        .iter()
        .filter(|h| h.is_local == Some(false))
        .filter_map(|h| match h.ip {
            IpAddr::V4(ip) => Some(ip),
            IpAddr::V6(_) => None,
        })
        .collect();
    if settings.trace && !remote.is_empty() {
        report.map(
            &hosts,
            &format!("tracing the way to {} hosts…", remote.len()),
        );
        let options = TraceOptions {
            max_hops: trace::DEFAULT_MAX_HOPS,
            timeout: settings.timeout,
        };
        match trace::trace_paths(&remote, &options, settings.concurrency, progress(tx)).await {
            Ok(paths) => {
                let mut routers = 0;
                for path in &paths {
                    for router in path.routers() {
                        if !hosts.iter().any(|h| h.ip == IpAddr::V4(router)) {
                            hosts.push(scanner::Host::new(router));
                            routers += 1;
                        }
                    }
                    if let Some(host) = hosts.iter_mut().find(|h| h.ip == IpAddr::V4(path.target)) {
                        host.extra.insert("path".into(), path.describe().into());
                        if let Some(rtt) = path.rtt {
                            let millis = (rtt.as_secs_f64() * 10_000.0).round() / 10.0;
                            host.extra.insert("latency_ms".into(), millis.into());
                        }
                    }
                }
                local::mark_local(&mut hosts, &plan.local.network); // the routers too
                let reached = paths.iter().filter(|p| p.reached).count();
                report.note(format!(
                    "{reached} of {} ways traced, {routers} routers found",
                    remote.len()
                ));
                report.paths = paths;
            }
            Err(err) => report.note(format!("traceroute skipped: {err}")),
        }
    }

    // The ARP sweep's hosts carry their MACs; for the ping sweep's, the pings
    // made the kernel ARP for every address on our link that answered, so
    // its table has them now. A loopback or remote-only scan has none, and
    // then there is nothing to look up either.
    neighbours::attach_macs(&mut hosts, &neighbours::known_macs(&plan.local));
    let with_mac = hosts.iter().filter(|h| h.mac.is_some()).count();
    if with_mac > 0 {
        report.map(&hosts, "looking up vendors…");
        match vendors::look_up(&mut hosts) {
            Ok(named) => report.note(format!(
                "{with_mac} of {} hosts have a MAC, {named} a vendor",
                hosts.len()
            )),
            Err(err) => report.note(format!("vendor names skipped: {err}")),
        }
    }

    if settings.names {
        report.map(&hosts, "resolving names…");
        names::resolve_hostnames(
            &mut hosts,
            names::DEFAULT_CONCURRENCY,
            names::DEFAULT_TIMEOUT,
            false,
        )
        .await;
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

    if !settings.udp_ports.is_empty() {
        let count = settings.udp_ports.len();
        report.map(&hosts, &format!("trying {count} UDP ports…"));
        let options = PortScanOptions {
            timeout: settings.timeout,
            concurrency: settings.concurrency,
        };
        ports::udp_port_scan(&mut hosts, &settings.udp_ports, &options, progress(tx)).await;
        let udp = hosts
            .iter()
            .flat_map(|h| &h.open_ports)
            .filter(|p| p.proto == Proto::Udp);
        let answered = udp
            .clone()
            .filter(|p| p.state.as_deref() == Some("open"))
            .count();
        let silent = udp.count() - answered;
        report.note(format!(
            "{answered} UDP ports answered and {silent} stayed silent (open or filtered) on {} hosts ({count} ports tried each)",
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

/// Keep the gateway on the map even when the sweep did not find it. Inside
/// the swept network that means it ignored the `probe` ("ping", which many
/// routers drop), and it is drawn greyed out. Outside it, it was never
/// probed, but the swept hosts are reached through it, so it is drawn like
/// any other host.
fn keep_gateway(
    hosts: &mut Vec<scanner::Host>,
    gateway: Option<Ipv4Addr>,
    swept: &Ipv4Net,
    probe: &str,
) {
    let Some(gateway) = gateway else {
        return;
    };
    if hosts.iter().any(|h| h.ip == IpAddr::V4(gateway)) {
        return;
    }
    let mut host = scanner::Host::new(gateway);
    if swept.contains(&gateway) {
        host.extra.insert("status".into(), "down".into());
        host.extra
            .insert("note".into(), format!("did not answer {probe}").into());
    } else {
        host.extra.insert(
            "note".into(),
            "not swept: outside the scanned network".into(),
        );
    }
    hosts.push(host);
}

/// Sends the scan's maps (numbering them) and notes.
struct Reporter<'a> {
    plan: &'a ScanPlan,
    tx: &'a Sender<ScanEvent>,
    revision: u64,
    /// The maps' `scan.type`: which sweep finds the hosts.
    kind: &'static str,
    /// The ways to the hosts beyond our link, once traced.
    paths: Vec<Path>,
}

impl<'a> Reporter<'a> {
    fn new(plan: &'a ScanPlan, tx: &'a Sender<ScanEvent>) -> Self {
        Reporter {
            plan,
            tx,
            revision: 0,
            kind: "ping_sweep",
            paths: Vec::new(),
        }
    }

    /// Send the map of `hosts`, with `stage` for the HUD; returns it too.
    fn map(&mut self, hosts: &[scanner::Host], stage: &str) -> MapDoc {
        self.revision += 1;
        let scan = Scan {
            kind: Some(self.kind.to_string()),
            network: Some(self.plan.network.to_string()),
            stage: Some(stage.to_string()),
            ..Scan::default()
        };
        let mut doc = MapDoc::from_hosts(
            hosts,
            Some(IpAddr::V4(self.plan.local.ip)),
            self.plan.local.gateway.map(IpAddr::V4),
            &self.paths,
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

/// The running scan: its reports, how to stop it, and where it stands for
/// the scan card. Exists only while a scan runs. (`Receiver` is not `Sync`,
/// hence the mutex; nothing ever waits on it.)
#[derive(Resource)]
pub struct ScanJob {
    events: Mutex<Receiver<ScanEvent>>,
    cancel: Arc<Notify>,
    /// What the scan is doing now, from its latest map.
    pub stage: String,
    /// `done` of `total` steps of the running stage, when it counts any.
    pub progress: Option<(usize, usize)>,
}

impl ScanJob {
    pub fn new(events: Receiver<ScanEvent>, cancel: Arc<Notify>) -> ScanJob {
        ScanJob {
            events: Mutex::new(events),
            cancel,
            stage: "starting…".to_string(),
            progress: None,
        }
    }

    /// Stop the scan: the thread gives up whatever stage it is in and
    /// reports [`ScanEvent::Cancelled`].
    pub fn cancel(&self) {
        self.cancel.notify_one();
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
            let (events, cancel) = spawn(plan);
            commands.insert_resource(ScanJob::new(events, cancel));
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
    job: Option<ResMut<ScanJob>>,
    mut doc: ResMut<MapDocument>,
    mut status: ResMut<Status>,
    mut commands: Commands,
) {
    let Some(mut job) = job else {
        return;
    };
    let job = &mut *job;
    let Ok(events) = job.events.get_mut() else {
        return;
    };
    loop {
        match events.try_recv() {
            Ok(ScanEvent::Map(map)) => {
                if let Some(stage) = &map.scan.stage {
                    info!("{stage}");
                    job.stage.clone_from(stage);
                }
                job.progress = None;
                status.show_map(map.summary());
                doc.0 = Some(*map);
            }
            Ok(ScanEvent::Progress { done, total }) => job.progress = Some((done, total)),
            Ok(ScanEvent::Note(note)) => info!("{note}"),
            Ok(ScanEvent::Finished(result)) => {
                if let Err(err) = result {
                    error!("scan failed: {err}");
                    status.show_error(format!("scan failed: {err}"));
                }
                commands.remove_resource::<ScanJob>();
                return; // the last event: the thread is gone or going
            }
            Ok(ScanEvent::Cancelled) => {
                info!("scan cancelled");
                status.show_message("scan cancelled");
                commands.remove_resource::<ScanJob>();
                return;
            }
            Err(TryRecvError::Empty) => return,
            // The thread died without a last word (a panic, which the log
            // shows): the job would otherwise be "running" for ever.
            Err(TryRecvError::Disconnected) => {
                error!("the scan stopped without finishing");
                status.show_error("the scan stopped without finishing".to_string());
                commands.remove_resource::<ScanJob>();
                return;
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
    let mut finished = false;
    let (events, _cancel) = spawn(plan); // nothing cancels a headless scan but Ctrl-C
    for event in events {
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
            ScanEvent::Finished(Ok(())) => finished = true,
            ScanEvent::Cancelled => {
                eprintln!("netvisualizer: the scan was cancelled");
                return ExitCode::FAILURE;
            }
        }
    }
    if !finished {
        // The thread died without a last word: a panic, printed above.
        eprintln!("netvisualizer: the scan stopped without finishing");
        return ExitCode::FAILURE;
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

/// One line per host: role, address, name, MAC, vendor, open ports.
///
/// ```text
/// scanner  192.168.1.143    kylr-laptop              78:2b:46:07:06:cf Intel Corporate
/// gateway  192.168.1.254    router.lan               08:c7:f5:ed:02:cb -                        53/tcp domain, 80/tcp http
/// ```
fn host_table(map: &MapDoc) -> Vec<String> {
    map.hosts
        .iter()
        .map(|host| {
            let text = |key: &str| host.fields.get(key).and_then(Value::as_str).unwrap_or("-");
            let ports: Vec<String> = host
                .fields
                .get("open_ports")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|port| {
                    let number = &port["number"];
                    let proto = port["proto"].as_str().unwrap_or("?");
                    // "53/udp?": the port stayed silent, so open or filtered.
                    let unsure = if port["state"].as_str() == Some("open|filtered") {
                        "?"
                    } else {
                        ""
                    };
                    match port["service"].as_str() {
                        Some(service) => format!("{number}/{proto}{unsure} {service}"),
                        None => format!("{number}/{proto}{unsure}"),
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
                "{:<8} {:<16} {:<24} {:<17} {:<24} {tail}",
                host.role().name(),
                host.id,
                text("hostname"),
                text("mac"),
                text("vendor"),
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

    /// A scan of 127.0.0.0/30 "from" loopback, which has no gateway: both
    /// addresses answer.
    fn loopback_plan(ports: Vec<u16>) -> ScanPlan {
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
                gateway: None,
            },
        }
    }

    fn all_events(plan: ScanPlan) -> Vec<ScanEvent> {
        spawn(plan).0.into_iter().collect()
    }

    /// A scan that would wait 5 s for silence is cut short by a cancel.
    #[test]
    fn a_cancelled_scan_stops_early_and_says_so() {
        let mut plan = documentation_plan("192.0.2.0/28");
        plan.settings.timeout = Duration::from_secs(5);
        let started = Instant::now();
        let (events, cancel) = spawn(plan);
        thread::sleep(Duration::from_millis(200));
        cancel.notify_one();
        let events: Vec<ScanEvent> = events.into_iter().collect();
        assert!(
            matches!(events.last(), Some(ScanEvent::Cancelled)),
            "{events:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(3));
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
        let events = all_events(loopback_plan(vec![port]));

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
        assert_eq!(
            done.scan.kind.as_deref(),
            Some("ping_sweep"),
            "loopback has no MAC to send ARP from"
        );
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
                .any(|n| n.starts_with("ARP sweep skipped: interface \"lo\" has no MAC")),
            "{notes:?}"
        );
        assert!(
            notes
                .iter()
                .any(|n| n.starts_with("2 open ports") || n.starts_with("1 open ports")),
            "{notes:?}"
        );
    }

    /// A scan "from" 192.0.2.10 on 192.0.2.0/24, a network reserved for
    /// documentation where nothing ever answers; ping only, no traceroute.
    fn documentation_plan(network: &str) -> ScanPlan {
        let settings = ScanSettings {
            network: Some(network.parse().unwrap()),
            names: false,
            arp: false,
            trace: false,
            timeout: Duration::from_millis(300),
            ..ScanSettings::default()
        };
        ScanPlan {
            network: settings.network.unwrap(),
            settings,
            local: LocalNetwork {
                interface: "test0".into(),
                ip: Ipv4Addr::new(192, 0, 2, 10),
                network: "192.0.2.0/24".parse().unwrap(),
                gateway: Some(Ipv4Addr::new(192, 0, 2, 1)),
            },
        }
    }

    #[test]
    fn a_silent_gateway_is_kept_greyed_out_and_the_map_is_saved() {
        let path =
            std::env::temp_dir().join(format!("netvisualizer-scan-{}.json", std::process::id()));
        let mut plan = documentation_plan("192.0.2.0/30"); // the gateway's own network
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
        assert_eq!(stages, ["sweeping 192.0.2.0/30…", "done"]);

        assert!(
            !events
                .iter()
                .any(|e| matches!(e, ScanEvent::Note(n) if n.starts_with("ARP sweep"))),
            "ARP was turned off: {events:?}"
        );

        let saved = MapDoc::load(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        let ids: Vec<&str> = saved.hosts.iter().map(|h| h.id.as_str()).collect();
        assert_eq!(
            ids,
            ["192.0.2.10", "192.0.2.1"],
            "the scanner and its gateway"
        );
        let gateway = &saved.hosts[1];
        assert_eq!(gateway.role(), crate::map::Role::Gateway);
        assert_eq!(gateway.status(), HostStatus::Down);
        assert_eq!(gateway.fields["extra"]["note"], "did not answer ping");
        assert_eq!(saved.revision, 2);
    }

    /// Scanning 127.0.0.0/30 from 192.0.2.0/24: both loopback addresses are
    /// outside that subnet, so they hang off the gateway, which is kept
    /// although it was not swept, and not greyed out.
    #[test]
    fn hosts_on_another_network_are_linked_through_the_gateway() {
        let events = all_events(documentation_plan("127.0.0.0/30"));
        let done = *maps(&events).last().unwrap();
        let links: Vec<(&str, &str, Option<&str>)> = done
            .links
            .iter()
            .map(|l| (l.source.as_str(), l.target.as_str(), l.kind.as_deref()))
            .collect();
        assert_eq!(
            links,
            [
                ("192.0.2.1", "127.0.0.1", Some("route")),
                ("192.0.2.1", "127.0.0.2", Some("route")),
                ("192.0.2.10", "192.0.2.1", Some("subnet")),
            ]
        );
        let gateway = done.hosts.iter().find(|h| h.id == "192.0.2.1").unwrap();
        assert_eq!(
            gateway.status(),
            HostStatus::Up,
            "never pinged, so not down"
        );
        assert_eq!(gateway.fields["is_local"], true);
    }

    /// A UDP port bound on 127.0.0.1 that never answers: listed for that
    /// host as open|filtered, and not at all for 127.0.0.2, which refuses.
    #[test]
    fn udp_ports_are_tried_after_tcp() {
        let quiet = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = quiet.local_addr().unwrap().port();
        let mut plan = loopback_plan(Vec::new());
        plan.settings.udp_ports = vec![port];
        let events = all_events(plan);
        let maps = maps(&events);
        let stages: Vec<&str> = maps
            .iter()
            .filter_map(|m| m.scan.stage.as_deref())
            .collect();
        assert!(stages.contains(&"trying 1 UDP ports…"), "{stages:?}");
        let done = *maps.last().unwrap();
        let ports = &done.hosts[0].fields["open_ports"];
        assert_eq!(ports[0]["number"], port);
        assert_eq!(ports[0]["proto"], "udp");
        assert_eq!(ports[0]["state"], "open|filtered");
        assert_eq!(done.hosts[1].fields["open_ports"], serde_json::json!([]));
        let table = host_table(done);
        assert!(table[0].ends_with(&format!("{port}/udp?")), "{}", table[0]);
        drop(quiet);
    }

    /// With traceroute on, the loopback hosts turn out to be reached with
    /// no router in between, so they hang off the scanner, not the gateway.
    #[test]
    fn a_traced_way_replaces_the_guess_through_the_gateway() {
        let mut plan = documentation_plan("127.0.0.0/30");
        plan.settings.trace = true;
        let events = all_events(plan);
        let maps = maps(&events);
        let stages: Vec<&str> = maps
            .iter()
            .filter_map(|m| m.scan.stage.as_deref())
            .collect();
        assert_eq!(
            stages,
            [
                "sweeping 127.0.0.0/30…",
                "tracing the way to 2 hosts…",
                "done"
            ]
        );
        let done = *maps.last().unwrap();
        let links: Vec<(&str, &str, Option<&str>)> = done
            .links
            .iter()
            .map(|l| (l.source.as_str(), l.target.as_str(), l.kind.as_deref()))
            .collect();
        assert_eq!(
            links,
            [
                ("192.0.2.10", "192.0.2.1", Some("subnet")),
                ("192.0.2.10", "127.0.0.1", Some("route")),
                ("192.0.2.10", "127.0.0.2", Some("route")),
            ]
        );
        let host = done.hosts.iter().find(|h| h.id == "127.0.0.1").unwrap();
        assert_eq!(host.fields["extra"]["path"], serde_json::json!([]));
        assert!(host.fields["extra"]["latency_ms"].is_number(), "{host:?}");
        let notes: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                ScanEvent::Note(n) => Some(n.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            notes.contains(&"2 of 2 ways traced, 0 routers found"),
            "{notes:?}"
        );
    }

    #[test]
    fn arp_covers_the_part_of_the_network_on_our_link() {
        let link = |network: &str| documentation_plan(network).link_network();
        assert_eq!(link("192.0.2.0/30"), Some("192.0.2.0/30".parse().unwrap()));
        assert_eq!(link("192.0.2.0/24"), Some("192.0.2.0/24".parse().unwrap()));
        assert_eq!(
            link("192.0.0.0/16"),
            Some("192.0.2.0/24".parse().unwrap()),
            "the rest is pinged"
        );
        assert_eq!(link("10.0.0.0/24"), None);
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
            "scanner  192.168.1.50     netvisualizer-host.lan   -                 -                        22/tcp"
        );
        assert!(
            table[1].starts_with(
                "gateway  192.168.1.1      gateway.lan              00:0a:eb:11:01:25 TP-Link                  53/udp, 80/tcp"
            ),
            "{}",
            table[1]
        );
        assert_eq!(
            table[27],
            "host     192.168.1.254    -                        -                 -                        (down)"
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
        app.insert_resource(ScanJob::new(rx, Arc::new(Notify::new())));
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
        tx.send(ScanEvent::Progress { done: 3, total: 27 }).unwrap();
        app.update();

        let doc = app.world().resource::<MapDocument>();
        assert_eq!(doc.0.as_ref().map(|d| d.hosts.len()), Some(27));
        let status = app.world().resource::<Status>();
        assert_eq!(
            status.text,
            "ping_sweep · resolving names… · 27 hosts · 0 links · rev 2"
        );
        let job = app.world().resource::<ScanJob>();
        assert_eq!(job.stage, "resolving names…");
        assert_eq!(job.progress, Some((3, 27)));

        tx.send(ScanEvent::Finished(Err("the sweep failed: boom".into())))
            .unwrap();
        app.update();
        let status = app.world().resource::<Status>();
        assert!(status.error);
        assert_eq!(status.text, "scan failed: the sweep failed: boom");
        assert!(!app.world().contains_resource::<ScanJob>());
    }

    /// The scan thread died without sending `Finished` (a panic): the job
    /// must not stay "running" for ever.
    #[test]
    fn a_cancelled_job_ends_without_an_error() {
        let mut app = app();
        let (tx, rx) = mpsc::channel();
        app.insert_resource(ScanJob::new(rx, Arc::new(Notify::new())));
        tx.send(ScanEvent::Cancelled).unwrap();
        app.update();
        let status = app.world().resource::<Status>();
        assert!(!status.error);
        assert_eq!(status.text, "scan cancelled");
        assert!(!app.world().contains_resource::<ScanJob>());
    }

    #[test]
    fn a_scan_thread_that_dies_ends_the_job_with_an_error() {
        let mut app = app();
        let (tx, rx) = mpsc::channel();
        app.insert_resource(ScanJob::new(rx, Arc::new(Notify::new())));
        tx.send(ScanEvent::Note("sweeping".into())).unwrap();
        app.update();
        assert!(app.world().contains_resource::<ScanJob>(), "still running");

        drop(tx);
        app.update();
        let status = app.world().resource::<Status>();
        assert!(status.error);
        assert_eq!(status.text, "the scan stopped without finishing");
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
