//! Live mode: follow a running `python -m netvisualizer` map server.
//!
//! A plain thread asks `/api/health` for the revision every two seconds and
//! downloads `/api/map` only when it changed, so the viewer can open before a
//! scan has finished and redraws as each stage lands.
//! Results cross into Bevy over an mpsc channel: no system ever waits on the
//! network, and no async runtime is involved.

use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use bevy::prelude::*;
use serde::Deserialize;

use crate::ViewerSet;
use crate::graph::MapDocument;
use crate::map::{CONTRACT_VERSION, MapDoc};
use crate::ui::Status;

/// How often the server is asked for its revision.
pub const POLL_INTERVAL: Duration = Duration::from_secs(2);
/// A server that takes longer than this to answer counts as unreachable.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
/// ureq stops reading a body at 10 MB unless told otherwise; a big sweep's map can exceed that.
const MAX_BODY_BYTES: u64 = 256 * 1024 * 1024;

/// What the poll thread reports.
#[derive(Debug)]
pub enum FetchEvent {
    /// A map with a revision the viewer has not drawn yet.
    Map(Box<MapDoc>),
    /// The server answers again after a failure, still at the drawn revision.
    Reachable,
    /// A request failed; the thread keeps polling.
    Unreachable(String),
}

/// The receiving end of the poll thread (`Receiver` is not `Sync`, hence the mutex).
#[derive(Resource)]
pub struct Fetcher(Mutex<Receiver<FetchEvent>>);

impl Fetcher {
    /// Start polling `url` (`http://127.0.0.1:8765`, with or without the scheme
    /// or a trailing slash). The thread ends by itself once the viewer quits.
    pub fn spawn(url: &str) -> Fetcher {
        let base = base_url(url);
        let (tx, rx) = mpsc::channel();
        thread::Builder::new()
            .name("netvisualizer-fetch".into())
            .spawn(move || poll_loop(&base, &tx))
            .expect("could not start the fetch thread");
        Fetcher(Mutex::new(rx))
    }
}

pub struct FetchPlugin;

impl Plugin for FetchPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, receive.in_set(ViewerSet::Fetch));
    }
}

/// `http://host:port` with no trailing slash; a bare `host:port` gets `http://`.
pub fn base_url(url: &str) -> String {
    let url = url.trim().trim_end_matches('/');
    if url.contains("://") {
        url.to_string()
    } else {
        format!("http://{url}")
    }
}

/// Hand what the thread found to the rest of the viewer. A new document makes
/// graph.rs rebuild, the panel re-sort and scene.rs re-frame (while the user
/// has not moved); the selection survives because it is kept by host id.
fn receive(fetcher: Res<Fetcher>, mut doc: ResMut<MapDocument>, mut status: ResMut<Status>) {
    let Ok(rx) = fetcher.0.lock() else {
        return;
    };
    for event in rx.try_iter() {
        match event {
            FetchEvent::Map(map) => {
                if map.version > CONTRACT_VERSION {
                    warn!(
                        "map contract version {} is newer than this viewer ({CONTRACT_VERSION})",
                        map.version
                    );
                }
                status.show_map(map.summary());
                doc.0 = Some(*map);
            }
            FetchEvent::Reachable => status.restore(),
            FetchEvent::Unreachable(message) => {
                status.show_error(format!("server unreachable, retrying… ({message})"));
            }
        }
    }
}

fn poll_loop(base: &str, tx: &Sender<FetchEvent>) {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(REQUEST_TIMEOUT))
        .build()
        .into();
    let mut poller = Poller::default();
    loop {
        if let Some(event) = poller.poll(|path| get(&agent, base, path))
            && tx.send(event).is_err()
        {
            return; // the viewer has quit
        }
        thread::sleep(POLL_INTERVAL);
    }
}

/// One GET, body as text. Errors name the path: `/api/map: HTTP 500`.
fn get(agent: &ureq::Agent, base: &str, path: &str) -> Result<String, String> {
    let describe = |err: ureq::Error| match err {
        ureq::Error::StatusCode(code) => format!("{path}: HTTP {code}"),
        other => other.to_string(),
    };
    agent
        .get(&format!("{base}{path}"))
        .call()
        .map_err(describe)?
        .body_mut()
        .with_config()
        .limit(MAX_BODY_BYTES)
        .read_to_string()
        .map_err(describe)
}

#[derive(Debug, Deserialize)]
struct Health {
    revision: u64,
}

/// The polling protocol, apart from the HTTP itself so tests can drive it.
#[derive(Debug, Default)]
struct Poller {
    /// Revision of the map last handed over.
    drawn: Option<u64>,
    /// The previous poll failed, so the next success must clear the error.
    failing: bool,
}

impl Poller {
    /// One round of `poll()`: health, then the map if its revision changed.
    fn poll(&mut self, mut get: impl FnMut(&str) -> Result<String, String>) -> Option<FetchEvent> {
        match self.check(&mut get) {
            Ok(Some(doc)) => {
                self.failing = false;
                self.drawn = Some(doc.revision);
                Some(FetchEvent::Map(Box::new(doc)))
            }
            Ok(None) => std::mem::take(&mut self.failing).then_some(FetchEvent::Reachable),
            Err(message) => {
                self.failing = true;
                Some(FetchEvent::Unreachable(message))
            }
        }
    }

    fn check(
        &self,
        get: &mut impl FnMut(&str) -> Result<String, String>,
    ) -> Result<Option<MapDoc>, String> {
        let health: Health = serde_json::from_str(&get("/api/health")?)
            .map_err(|err| format!("/api/health: {err}"))?;
        if self.drawn == Some(health.revision) {
            return Ok(None);
        }
        let doc = MapDoc::from_json(&get("/api/map")?).map_err(|err| format!("/api/map: {err}"))?;
        Ok(Some(doc))
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    use super::*;

    fn map_json(revision: u64, hosts: u64) -> String {
        let hosts: Vec<String> = (0..hosts)
            .map(|i| format!(r#"{{"id": "10.0.0.{i}", "role": "host"}}"#))
            .collect();
        format!(
            r#"{{"version": 1, "revision": {revision}, "hosts": [{}], "links": []}}"#,
            hosts.join(",")
        )
    }

    /// What a test server answers: its revision (the map then has that many
    /// hosts) and whether it is down. Shared with the server thread.
    #[derive(Clone, Default)]
    struct FakeServer {
        revision: Arc<AtomicU64>,
        down: Arc<AtomicBool>,
    }

    impl FakeServer {
        fn set_revision(&self, value: u64) {
            self.revision.store(value, Ordering::SeqCst);
        }

        fn set_down(&self, down: bool) {
            self.down.store(down, Ordering::SeqCst);
        }

        /// `(status, body)` for a path, like the Python handler.
        fn answer(&self, path: &str) -> (u16, String) {
            let revision = self.revision.load(Ordering::SeqCst);
            match path {
                "/api/health" => (200, format!(r#"{{"ok": true, "revision": {revision}}}"#)),
                "/api/map" => (200, map_json(revision, revision)),
                _ => (404, "no such endpoint".to_string()),
            }
        }

        /// The same answers without HTTP, for driving [`Poller`] directly.
        fn get(&self, path: &str) -> Result<String, String> {
            if self.down.load(Ordering::SeqCst) {
                return Err("io: Connection refused (os error 111)".to_string());
            }
            match self.answer(path) {
                (200, body) => Ok(body),
                (code, _) => Err(format!("{path}: HTTP {code}")),
            }
        }
    }

    #[test]
    fn the_map_is_downloaded_only_when_the_revision_changes() {
        let server = FakeServer::default();
        let mut requests: Vec<String> = Vec::new();
        let mut get = |path: &str| {
            requests.push(path.to_string());
            server.get(path)
        };
        let mut poller = Poller::default();

        assert!(matches!(poller.poll(&mut get), Some(FetchEvent::Map(doc)) if doc.revision == 0));
        assert!(
            poller.poll(&mut get).is_none(),
            "same revision: nothing to report"
        );
        server.set_revision(3);
        assert!(matches!(
            poller.poll(&mut get),
            Some(FetchEvent::Map(doc)) if doc.revision == 3 && doc.hosts.len() == 3
        ));
        assert_eq!(
            requests,
            [
                "/api/health",
                "/api/map",
                "/api/health",
                "/api/health",
                "/api/map"
            ]
        );
    }

    #[test]
    fn an_outage_is_reported_every_poll_and_cleared_once_when_the_server_returns() {
        let server = FakeServer::default();
        let mut poller = Poller::default();
        assert!(matches!(
            poller.poll(|p| server.get(p)),
            Some(FetchEvent::Map(_))
        ));

        server.set_down(true);
        for _ in 0..2 {
            match poller.poll(|p| server.get(p)) {
                Some(FetchEvent::Unreachable(message)) => assert!(message.contains("refused")),
                other => panic!("expected Unreachable, got {other:?}"),
            }
        }
        server.set_down(false);
        assert!(matches!(
            poller.poll(|p| server.get(p)),
            Some(FetchEvent::Reachable)
        ));
        assert!(
            poller.poll(|p| server.get(p)).is_none(),
            "cleared once, not every poll"
        );

        // Back up with a new revision: the map itself clears the error.
        server.set_down(true);
        assert!(matches!(
            poller.poll(|p| server.get(p)),
            Some(FetchEvent::Unreachable(_))
        ));
        server.set_down(false);
        server.set_revision(2);
        assert!(matches!(
            poller.poll(|p| server.get(p)),
            Some(FetchEvent::Map(_))
        ));
        assert!(poller.poll(|p| server.get(p)).is_none());
    }

    #[test]
    fn bad_json_is_an_error_not_a_crash() {
        let mut poller = Poller::default();
        let event = poller.poll(|path| match path {
            "/api/health" => Ok(r#"{"ok": true, "revision": 1}"#.to_string()),
            _ => Ok("<html>not json</html>".to_string()),
        });
        assert!(
            matches!(&event, Some(FetchEvent::Unreachable(m)) if m.starts_with("/api/map: ")),
            "{event:?}"
        );
        let event = poller.poll(|_| Ok("{}".to_string()));
        assert!(
            matches!(&event, Some(FetchEvent::Unreachable(m)) if m.starts_with("/api/health: ")),
            "{event:?}"
        );
    }

    #[test]
    fn base_urls_are_normalised() {
        assert_eq!(base_url("http://127.0.0.1:8765/"), "http://127.0.0.1:8765");
        assert_eq!(base_url("http://127.0.0.1:8765"), "http://127.0.0.1:8765");
        assert_eq!(base_url(" 10.0.0.5:8765 "), "http://10.0.0.5:8765");
    }

    /// Serves a [`FakeServer`] over real HTTP on a free port the way Python's
    /// http.server answers (HTTP/1.0, Content-Length, connection closed after
    /// each response). Returns the server's URL with a trailing slash, as
    /// `MapServer.url` has.
    fn serve_http(server: FakeServer) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                reader.read_line(&mut request_line).unwrap();
                let mut header = String::new();
                while reader.read_line(&mut header).unwrap() > 2 {
                    header.clear(); // drain the headers up to the blank line
                }
                let path = request_line.split_whitespace().nth(1).unwrap_or("/");
                let (code, body) = server.answer(path);
                let reason = if code == 200 { "OK" } else { "Not Found" };
                let _ = write!(
                    stream,
                    "HTTP/1.0 {code} {reason}\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        format!("http://{address}/")
    }

    fn agent() -> ureq::Agent {
        ureq::Agent::config_builder()
            .timeout_global(Some(REQUEST_TIMEOUT))
            .build()
            .into()
    }

    #[test]
    fn real_http_round_trip_with_ureq() {
        let server = FakeServer::default();
        server.set_revision(2);
        let base = base_url(&serve_http(server.clone()));
        let agent = agent();
        let mut poller = Poller::default();
        match poller.poll(|path| get(&agent, &base, path)) {
            Some(FetchEvent::Map(doc)) => {
                assert_eq!(doc.revision, 2);
                assert_eq!(doc.hosts.len(), 2);
            }
            other => panic!("expected a map, got {other:?}"),
        }
        assert!(poller.poll(|path| get(&agent, &base, path)).is_none());
        server.set_revision(5);
        assert!(matches!(
            poller.poll(|path| get(&agent, &base, path)),
            Some(FetchEvent::Map(doc)) if doc.hosts.len() == 5
        ));
        assert_eq!(
            get(&agent, &base, "/api/nope").unwrap_err(),
            "/api/nope: HTTP 404"
        );
    }

    #[test]
    fn a_closed_port_is_unreachable() {
        // Bind and drop to find a port nobody listens on.
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let base = format!("http://127.0.0.1:{port}");
        let agent = agent();
        let mut poller = Poller::default();
        assert!(matches!(
            poller.poll(|path| get(&agent, &base, path)),
            Some(FetchEvent::Unreachable(_))
        ));
    }
}
