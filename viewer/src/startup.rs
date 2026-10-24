//! What the window is started with: the log filter, the GPU probe behind
//! `WGPU_BACKEND`, and the smoke test, `--exit-after-frames` with
//! `--screenshot` and `--select`, which renders a few frames and quits so a
//! build can be checked without clicking through it.

use std::path::PathBuf;

use bevy::log::{DEFAULT_FILTER, Level, LogPlugin};
use bevy::prelude::*;
use bevy::render::settings::{Backends, WgpuSettings};
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use bevy::tasks::block_on;

use crate::graph::Graph;
use crate::picking::SelectHost;
use crate::python;
use crate::scan::ScanJob;

/// The screenshot is taken this many frames before the end, and never
/// before this frame: the first frames are still black, and the GPU
/// readback has to finish before the program quits.
const SCREENSHOT_LEAD_FRAMES: u32 = 30;

/// Bevy logs its GPU set-up at "info", which is noise unless asked for with
/// `-v`; the program's own messages (what the scan finds) stay at "info".
/// A `RUST_LOG` variable still overrides both.
pub fn log_settings(verbose: bool) -> LogPlugin {
    let (level, ours) = if verbose {
        (Level::INFO, "debug")
    } else {
        (Level::WARN, "info")
    };
    LogPlugin {
        level,
        filter: format!("{DEFAULT_FILTER}netvisualizer={ours}"),
        ..default()
    }
}

/// `-v`: which Python the program embeds, and what it can import.
pub fn log_python() {
    info!("{}", python::describe());
}

/// Bevy picks the GPU backend from `WGPU_BACKEND` when that variable is set,
/// and panics with "Unable to find a GPU" if the backend it names has no
/// usable adapter. The dev machine's Hyprland config exports `WGPU_BACKEND=gl`
/// and wgpu's OpenGL path finds nothing there, so check first: when the
/// requested backend yields no adapter, say so and let wgpu choose from all
/// of them (Vulkan on that machine).
pub fn render_settings() -> WgpuSettings {
    let mut settings = WgpuSettings::default(); // this is where WGPU_BACKEND is read
    let Ok(requested) = std::env::var("WGPU_BACKEND") else {
        return settings;
    };
    let backends = settings.backends.unwrap_or(Backends::all());
    // Built the way Bevy builds its own (no display handle), so the probe
    // sees the same adapters Bevy will.
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends,
        flags: settings.instance_flags,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    if block_on(instance.enumerate_adapters(backends)).is_empty() {
        eprintln!(
            "netvisualizer: WGPU_BACKEND={requested} is set but no GPU here supports that backend; \
             letting wgpu choose one instead"
        );
        settings.backends = Some(Backends::all());
    }
    settings
}

/// Frame counting for `--exit-after-frames` and `--screenshot`.
#[derive(Resource, Debug)]
pub struct SmokeTest {
    frames: u32,
    exit_after: Option<u32>,
    screenshot: Option<PathBuf>,
    screenshot_frame: u32,
}

impl SmokeTest {
    /// Quit after `exit_after_frames` frames, if given; save `screenshot`
    /// [`SCREENSHOT_LEAD_FRAMES`] before the end, if given, and stretch a
    /// run too short for that rather than leave it without its picture.
    pub fn new(exit_after_frames: Option<u32>, screenshot: Option<PathBuf>) -> SmokeTest {
        let screenshot_frame = exit_after_frames
            .map_or(SCREENSHOT_LEAD_FRAMES, |n| {
                n.saturating_sub(SCREENSHOT_LEAD_FRAMES)
            })
            .max(SCREENSHOT_LEAD_FRAMES);
        let exit_after = exit_after_frames.map(|n| {
            if screenshot.is_some() {
                n.max(screenshot_frame + SCREENSHOT_LEAD_FRAMES)
            } else {
                n
            }
        });
        SmokeTest {
            frames: 0,
            exit_after,
            screenshot,
            screenshot_frame,
        }
    }
}

/// Count the frames; take the screenshot and quit when their frames come.
pub fn smoke_test(
    mut commands: Commands,
    mut smoke: ResMut<SmokeTest>,
    mut exit: MessageWriter<AppExit>,
) {
    smoke.frames += 1;
    if smoke.frames == smoke.screenshot_frame
        && let Some(path) = smoke.screenshot.take()
    {
        info!("saving screenshot to {}", path.display());
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path));
    }
    if smoke.exit_after.is_some_and(|n| smoke.frames >= n) {
        exit.write(AppExit::Success);
    }
}

/// `--select` / `--fly`, consumed once the first map is drawn.
#[derive(Resource, Debug)]
pub struct DebugSelect {
    pub id: Option<String>,
    pub fly: bool,
}

/// Apply `--select` / `--fly` as soon as a drawn map has the host. With
/// `--scan` the first map holds only the scanner, so the host may take a
/// few stages to appear; it is only missing for good once the scan is over.
pub fn debug_select(
    graph: Res<Graph>,
    job: Option<Res<ScanJob>>,
    mut debug: ResMut<DebugSelect>,
    mut select: MessageWriter<SelectHost>,
) {
    if !graph.is_changed() || !graph.drawn {
        return;
    }
    let Some(id) = debug.id.as_deref() else {
        return;
    };
    if !graph.nodes.contains_key(id) {
        if job.is_none() {
            warn!("--select {id}: no such host in the map");
            debug.id = None;
        }
        return;
    }
    select.write(SelectHost {
        id: debug.id.take(),
        fly: debug.fly,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_screenshot_comes_thirty_frames_before_the_end_but_never_too_early() {
        let plain = SmokeTest::new(Some(240), None);
        assert_eq!((plain.exit_after, plain.screenshot_frame), (Some(240), 210));
        assert!(plain.screenshot.is_none());

        let short = SmokeTest::new(Some(10), Some("x.png".into()));
        assert_eq!(short.screenshot_frame, 30, "the first frames are black");
        assert_eq!(short.exit_after, Some(60), "stretched to fit the readback");

        let endless = SmokeTest::new(None, Some("x.png".into()));
        assert_eq!((endless.exit_after, endless.screenshot_frame), (None, 30));
    }
}
