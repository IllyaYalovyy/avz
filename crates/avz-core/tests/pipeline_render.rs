//! The whole pipeline, end to end: decode → analyze → render → encode.
//!
//! Every test here renders on the software adapter, because that is what makes
//! pixel assertions stable across machines (`docs/TESTING.md`).
//!
//! Most drive a shell stand-in for ffmpeg that copies its stdin straight to the
//! output, which turns the mp4 into the raw RGBA avz actually piped — the only
//! way to assert *which* frames were rendered and *how bright* each one was. The
//! last test drives the real ffmpeg and compares audio bitstreams, because
//! `--sample` promises the muxed audio covers the same range as the picture.
//!
//! These need Mesa's software Vulkan driver and the system ffmpeg. On Fedora:
//! `sudo dnf install mesa-vulkan-drivers ffmpeg`.

#![cfg(unix)]

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use avz_core::analysis::{self, FeatureTimeline};
use avz_core::config::{BackgroundSource, Config, Fit, FontChoice, Palette, SampleRange};
use avz_core::encode::{DEFAULT_PROGRAM, Ffmpeg, preflight};
use avz_core::pipeline::{RenderRequest, RenderSummary, render};
use avz_core::render::{AdapterChoice, AdapterKind};
use avz_core::{Error, NoopProgress, Phase, Progress};

/// Small, even, and 256-byte aligned per row (320 × 4 B = 1280 B), so a padding
/// bug cannot hide here — `offscreen_readback.rs` owns that risk at 300 px wide.
const WIDTH: u32 = 320;
const HEIGHT: u32 = 180;
const FPS: u32 = 30;

/// Bytes in one tightly packed RGBA frame.
const FRAME_BYTES: usize = (WIDTH * HEIGHT * 4) as usize;

/// The CC0 fixture: 5 s of a 60 Hz kick decaying every 500 ms under a 1 kHz
/// tone, so loudness rises and falls several times a second.
fn fixture_mp3() -> PathBuf {
    fixture("tone-tagged.mp3")
}

/// A committed CC0 fixture. See `assets/fixtures/README.md`.
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/fixtures")
        .join(name)
        .canonicalize()
        .unwrap_or_else(|err| panic!("the CC0 fixture {name} is committed: {err}"))
}

fn config() -> Config {
    let mut config = Config::default();
    config.output.resolution = "320x180".parse().expect("a legal resolution");
    config.output.fps = FPS;
    // The fake ffmpeg ignores it; the real one encodes 60 frames faster for it.
    config.output.quality = 30;
    config
}

fn sample(range: &str) -> SampleRange {
    range.parse().expect("a sample range")
}

/// `out.mp4` and the `out.mp4.part` ffmpeg writes before the rename.
fn output_paths(dir: &Path) -> (PathBuf, PathBuf) {
    (dir.join("out.mp4"), dir.join("out.mp4.part"))
}

/// An ffmpeg stand-in that answers avz's two preflight probes — `-version`, and
/// the `-encoders` listing that says it can encode the default codec — and
/// otherwise runs `body` with `$part` bound to its last argument.
///
/// `-encoders` is recognized by its *second* argument, because the
/// background-video reader's argv also opens with `-hide_banner`. See
/// `encode_ffmpeg.rs`.
fn fake_ffmpeg(dir: &Path, body: &str) -> Ffmpeg {
    let path = dir.join("ffmpeg");
    let script = format!(
        "#!/bin/sh
if [ \"$1\" = '-version' ]; then
    echo 'ffmpeg version 7.1.5 Copyright (c) 2000-2026'
    exit 0
fi
if [ \"$2\" = '-encoders' ]; then
    echo ' V....D libx264              libx264 H.264 (codec h264)'
    exit 0
fi
for part; do :; done
{body}
"
    );
    fs::write(&path, script).expect("write fake ffmpeg");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod");
    wait_until_executable(&path);

    preflight(&path).expect("the fake ffmpeg identifies itself as ffmpeg")
}

/// An ffmpeg stand-in that keeps every byte avz wrote to its stdin.
fn recording_ffmpeg(dir: &Path) -> Ffmpeg {
    fake_ffmpeg(dir, "cat > \"$part\"")
}

/// Wait out `ETXTBSY` on the script we just wrote: a sibling test thread that
/// forked inside `fs::write`'s open window handed its child an inherited
/// descriptor. See the same helper in `encode/preflight.rs`.
fn wait_until_executable(path: &Path) {
    for _ in 0..1_000 {
        match Command::new(path).arg("-version").output() {
            Err(err) if err.kind() == io::ErrorKind::ExecutableFileBusy => {
                std::thread::sleep(Duration::from_millis(1));
            }
            _ => return,
        }
    }
    panic!("{}: still busy after a second", path.display());
}

fn system_ffmpeg() -> Ffmpeg {
    preflight(DEFAULT_PROGRAM).expect("the pipeline tests need ffmpeg: `sudo dnf install ffmpeg`")
}

/// Only one Vulkan device may be open in this process at a time.
///
/// wgpu names every command encoder through `VK_EXT_debug_utils`, and the Vulkan
/// loader's `SetDebugUtilsObjectNameEXT` terminator walks its device list without
/// holding a lock. A test that is submitting frames while a sibling test opens or
/// closes a device therefore segfaults inside `loader_get_icd_and_device`
/// (`/lib64/libvulkan.so.1`) — which crashed this test binary on about one run in
/// three, and `cargo test --all-targets` with it.
///
/// `avz` opens exactly one device per process, so this is a property of a test
/// harness that runs its tests in parallel threads, not of the pipeline. Holding
/// this lock for the lifetime of each `Gpu` gives the tests what production
/// already has.
static ONE_DEVICE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// Claim the process's one Vulkan device for the rest of the current scope.
///
/// A poisoned lock means some sibling test already panicked. Its failure is the
/// interesting one, so this test carries on rather than adding a second.
fn one_device_at_a_time() -> MutexGuard<'static, ()> {
    ONE_DEVICE_AT_A_TIME
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// Render the fixture on lavapipe, into whatever `ffmpeg` does with the frames.
///
/// Takes [`one_device_at_a_time`] for the whole render, so callers must not hold
/// it already: the lock is not reentrant.
fn render_fixture(
    ffmpeg: &Ffmpeg,
    output: &Path,
    sample: Option<SampleRange>,
    progress: &dyn Progress,
) -> Result<RenderSummary, Error> {
    let _device = one_device_at_a_time();
    let config = config();
    render(
        &RenderRequest {
            input: &fixture_mp3(),
            output,
            config: &config,
            // Software, never auto: a golden pixel assertion must not silently
            // run on the developer's GPU (`AGENTS.md`, determinism).
            adapter: AdapterChoice::Software,
            sample,
            ffmpeg,
        },
        progress,
    )
}

/// The feature timeline the pipeline builds internally, rebuilt here so the
/// expected brightness of every frame can be derived independently.
fn fixture_timeline() -> FeatureTimeline {
    let audio = analysis::decode(fixture_mp3()).expect("the fixture decodes");
    analysis::analyze(&audio, FPS).expect("the fixture analyzes")
}

/// The RGBA frames the encoder received, in order.
fn recorded_frames(path: &Path) -> Vec<Vec<u8>> {
    let raw = fs::read(path).expect("the recording ffmpeg wrote what it was sent");
    assert_eq!(
        raw.len() % FRAME_BYTES,
        0,
        "{} bytes is not a whole number of {WIDTH}x{HEIGHT} frames",
        raw.len()
    );
    raw.chunks_exact(FRAME_BYTES).map(<[u8]>::to_vec).collect()
}

/// How bright one frame is, on average: Rec. 709 luma over every pixel.
///
/// A preset draws geometry, not a flat field, so no single pixel stands for the
/// frame. The mean does, and `pulse` scales its whole picture by the loudness
/// envelope — which is what the brightness test below observes.
fn mean_luma(frame: &[u8]) -> f64 {
    let total: f64 = frame
        .chunks_exact(4)
        .map(|pixel| {
            0.2126 * f64::from(pixel[0])
                + 0.7152 * f64::from(pixel[1])
                + 0.0722 * f64::from(pixel[2])
        })
        .sum();
    total / f64::from(WIDTH * HEIGHT)
}

/// Pearson correlation of two equally long tracks.
///
/// "Brightness follows loudness" is a claim about the shape of two curves, not
/// about any one frame's value — a preset is free to draw rings and sparkle on
/// top. Correlation states it without hard-coding what `pulse` draws.
fn correlation(xs: &[f64], ys: &[f64]) -> f64 {
    assert_eq!(xs.len(), ys.len());
    let mean = |values: &[f64]| values.iter().sum::<f64>() / values.len() as f64;
    let (mx, my) = (mean(xs), mean(ys));

    let covariance: f64 = xs
        .iter()
        .zip(ys)
        .map(|(x, y)| (x - mx) * (y - my))
        .sum::<f64>();
    let spread =
        |values: &[f64], m: f64| values.iter().map(|v| (v - m).powi(2)).sum::<f64>().sqrt();

    covariance / (spread(xs, mx) * spread(ys, my))
}

/// A [`Progress`] that remembers everything it was told.
#[derive(Debug, Default)]
struct Recorder {
    events: Mutex<Vec<Event>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Event {
    Started(Phase, Option<u64>),
    Advanced(Phase, u64),
    Finished(Phase),
    Warned(String),
    AdapterSelected(AdapterKind, String),
}

impl Recorder {
    fn events(&self) -> Vec<Event> {
        self.events.lock().expect("no test panicked here").clone()
    }

    fn phases(&self) -> Vec<Phase> {
        self.events()
            .into_iter()
            .filter_map(|event| match event {
                Event::Started(phase, _) => Some(phase),
                _ => None,
            })
            .collect()
    }

    fn warnings(&self) -> Vec<String> {
        self.events()
            .into_iter()
            .filter_map(|event| match event {
                Event::Warned(message) => Some(message),
                _ => None,
            })
            .collect()
    }

    fn adapters(&self) -> Vec<(AdapterKind, String)> {
        self.events()
            .into_iter()
            .filter_map(|event| match event {
                Event::AdapterSelected(kind, name) => Some((kind, name)),
                _ => None,
            })
            .collect()
    }

    fn push(&self, event: Event) {
        self.events
            .lock()
            .expect("no test panicked here")
            .push(event);
    }
}

impl Progress for Recorder {
    fn phase_started(&self, phase: Phase, total: Option<u64>) {
        self.push(Event::Started(phase, total));
    }

    fn advance(&self, phase: Phase, units: u64) {
        self.push(Event::Advanced(phase, units));
    }

    fn phase_finished(&self, phase: Phase) {
        self.push(Event::Finished(phase));
    }

    fn warn(&self, message: &str) {
        self.push(Event::Warned(message.to_owned()));
    }

    fn adapter_selected(&self, kind: AdapterKind, name: &str) {
        self.push(Event::AdapterSelected(kind, name.to_owned()));
    }
}

/// The heart of `--sample` (UT-002): only the requested excerpt is rendered, and
/// the frames that come out are the frames of *that* excerpt, not of the start of
/// the song. An off-by-one in the frame range would leave the picture a frame
/// away from the music for the whole render.
///
/// The reference is the full render's own frames. That pins the sharper promise
/// too: a `--sample` preview draws exactly the pixels the final render will draw
/// at those timestamps, so the preset's clock is the *song's* frame index and
/// not the excerpt's (`VISION.md` §3, "fast iteration").
#[test]
fn a_sampled_render_writes_exactly_the_frames_of_the_requested_range() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ffmpeg = recording_ffmpeg(dir.path());
    let (excerpt, part) = output_paths(dir.path());
    let whole = dir.path().join("whole.mp4");

    let summary = render_fixture(&ffmpeg, &excerpt, Some(sample("1s..2s")), &NoopProgress)
        .expect("the fixture renders");
    render_fixture(&ffmpeg, &whole, None, &NoopProgress).expect("the whole fixture renders");

    assert_eq!(summary.frames, 30, "one second at 30 fps");
    assert_eq!(summary.duration(), Duration::from_secs(1));
    assert_eq!(summary.adapter, AdapterKind::Software);
    assert!(!part.exists(), "the part file is renamed on success");

    let sampled = recorded_frames(&excerpt);
    let complete = recorded_frames(&whole);
    assert_eq!(sampled.len(), 30);

    for (offset, frame) in sampled.iter().enumerate() {
        assert_eq!(
            frame,
            &complete[30 + offset],
            "sample frame {offset} is not song frame {}",
            30 + offset,
        );
    }

    let opaque = sampled[0].chunks_exact(4).all(|pixel| pixel[3] == 255);
    assert!(opaque, "the visualizer layer renders a translucent frame");
}

/// The M1 acceptance criterion, observed on real pixels: brightness visibly
/// follows loudness. A shader that ignored the timeline would pass every
/// assertion above about frame *counts* and none of this one.
///
/// `pulse` scales its whole picture by `rms_env`, the loudness envelope, so the
/// two curves must rise and fall together over the second the fixture's kick
/// decays through four times. Correlation rather than a per-frame value: rings,
/// sparkle, and the onset flash are all free to draw on top.
#[test]
fn the_rendered_brightness_visibly_follows_the_loudness_of_the_song() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ffmpeg = recording_ffmpeg(dir.path());
    let (output, _) = output_paths(dir.path());

    render_fixture(&ffmpeg, &output, Some(sample("0s..1s")), &NoopProgress).expect("renders");

    let brightness: Vec<f64> = recorded_frames(&output)
        .iter()
        .map(|frame| mean_luma(frame))
        .collect();
    let timeline = fixture_timeline();
    let loudness: Vec<f64> = (0..brightness.len())
        .map(|index| f64::from(timeline.frame(index).rms_env))
        .collect();

    let darkest = brightness.iter().copied().fold(f64::MAX, f64::min);
    let brightest = brightness.iter().copied().fold(f64::MIN, f64::max);
    assert!(
        brightest - darkest > 10.0,
        "the kick decays four times in this second; brightness barely moved: \
         {darkest:.1}..{brightest:.1}"
    );

    let together = correlation(&loudness, &brightness);
    assert!(
        together > 0.9,
        "brightness and loudness only correlate at {together:.3}: the visuals are \
         not following the song"
    );
}

#[test]
fn a_render_without_a_sample_covers_every_frame_of_the_song() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ffmpeg = recording_ffmpeg(dir.path());
    let (output, _) = output_paths(dir.path());

    let summary = render_fixture(&ffmpeg, &output, None, &NoopProgress).expect("renders");

    let timeline = fixture_timeline();
    assert_eq!(summary.frames as usize, timeline.len());
    assert_eq!(recorded_frames(&output).len(), timeline.len());
}

/// `VISION.md` §8: analyzing, then rendering, then finalizing. The two-pass
/// design means analysis is *finished* before the first frame is rendered.
#[test]
fn progress_reports_the_three_phases_in_order_with_a_frame_total() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ffmpeg = recording_ffmpeg(dir.path());
    let (output, _) = output_paths(dir.path());
    let recorder = Recorder::default();

    render_fixture(&ffmpeg, &output, Some(sample("1s..1.5s")), &recorder).expect("renders");

    assert_eq!(
        recorder.phases(),
        [Phase::Analyzing, Phase::Rendering, Phase::Finalizing]
    );

    let events = recorder.events();
    let analysis_done = events
        .iter()
        .position(|event| *event == Event::Finished(Phase::Analyzing))
        .expect("analysis finished");
    let first_frame = events
        .iter()
        .position(|event| *event == Event::Started(Phase::Rendering, Some(15)))
        .expect("rendering announced its 15 frames");
    assert!(
        analysis_done < first_frame,
        "analysis must complete before rendering starts"
    );

    let advances = events
        .iter()
        .filter(|event| **event == Event::Advanced(Phase::Rendering, 1))
        .count();
    assert_eq!(advances, 15, "every frame advances the bar exactly once");
}

/// Every render says which adapter is doing the work — kind and the driver's
/// own name for it — exactly once, so the user knows whether a GPU took the
/// job or lavapipe is emulating one before the first frame lands, not after
/// the render has quietly taken the whole evening (`VISION.md` §7).
#[test]
fn every_render_announces_the_one_adapter_doing_the_work() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ffmpeg = recording_ffmpeg(dir.path());
    let (output, _) = output_paths(dir.path());
    let recorder = Recorder::default();

    render_fixture(&ffmpeg, &output, Some(sample("0s..0.2s")), &recorder).expect("renders");

    let adapters = recorder.adapters();
    assert_eq!(
        adapters.len(),
        1,
        "announced once, not per frame: {adapters:?}"
    );
    let (kind, name) = &adapters[0];
    assert_eq!(
        *kind,
        AdapterKind::Software,
        "this render asked for lavapipe by name"
    );
    assert!(
        !name.is_empty(),
        "the driver's own adapter name, not a blank"
    );
}

/// `--adapter software` was asked for by name, so there is nothing to warn about
/// (`VISION.md` §3). The fallback warning belongs to `--adapter auto` alone.
#[test]
fn an_explicit_software_render_warns_about_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ffmpeg = recording_ffmpeg(dir.path());
    let (output, _) = output_paths(dir.path());
    let recorder = Recorder::default();

    render_fixture(&ffmpeg, &output, Some(sample("0s..0.2s")), &recorder).expect("renders");

    assert!(recorder.warnings().is_empty(), "{:?}", recorder.warnings());
}

/// UT-003, on a host where the only Vulkan adapter is lavapipe. Guarded by
/// `scripts/quality.d/70-gpu-less-host-falls-back-to-lavapipe.sh`, which points
/// Vulkan at the lavapipe ICD and sets this variable.
#[test]
fn a_gpu_less_auto_render_warns_once_and_says_how_to_silence_it() {
    if std::env::var_os("AVZ_TEST_EXPECT_NO_GPU").is_none() {
        return;
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let ffmpeg = recording_ffmpeg(dir.path());
    let (output, _) = output_paths(dir.path());
    let recorder = Recorder::default();

    let _device = one_device_at_a_time();
    let config = config();
    render(
        &RenderRequest {
            input: &fixture_mp3(),
            output: &output,
            config: &config,
            adapter: AdapterChoice::Auto,
            sample: Some(sample("0s..0.2s")),
            ffmpeg: &ffmpeg,
        },
        &recorder,
    )
    .expect("auto renders even with no GPU");

    let warnings = recorder.warnings();
    assert_eq!(
        warnings.len(),
        1,
        "warn once, not once per frame: {warnings:?}"
    );
    let warning = &warnings[0];
    assert!(warning.contains("software rendering"), "{warning}");
    assert!(warning.contains("fps"), "say what it costs: {warning}");
    assert!(
        warning.contains("--adapter software"),
        "say how to silence it: {warning}"
    );
}

/// A typo'd `visual.preset` is the user's argument too, and they should hear
/// about it before a five-minute song is decoded, not after.
#[test]
fn an_unknown_preset_fails_before_the_song_is_even_decoded() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ffmpeg = recording_ffmpeg(dir.path());
    let (output, part) = output_paths(dir.path());

    let mut config = config();
    config.visual.preset = "pulze".to_owned();

    let _device = one_device_at_a_time();
    let err = render(
        &RenderRequest {
            input: &fixture_mp3(),
            output: &output,
            config: &config,
            adapter: AdapterChoice::Software,
            sample: None,
            ffmpeg: &ffmpeg,
        },
        &NoopProgress,
    )
    .expect_err("there is no preset called `pulze`");

    assert!(matches!(err, Error::Config(_)), "got {err:?}");
    assert!(
        err.to_string().contains("pulse"),
        "name what does exist: {err}"
    );
    assert!(!output.exists() && !part.exists(), "nothing was written");
}

/// A typo'd `visual.palette` costs a millisecond, not a decode — the same
/// promise `visual.preset` makes, and for the same reason.
#[test]
fn an_unknown_palette_fails_before_the_song_is_even_decoded() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ffmpeg = recording_ffmpeg(dir.path());
    let (output, part) = output_paths(dir.path());

    let mut config = config();
    config.visual.palette = Palette::Named("embers".to_owned());

    let _device = one_device_at_a_time();
    let err = render(
        &RenderRequest {
            input: &fixture_mp3(),
            output: &output,
            config: &config,
            adapter: AdapterChoice::Software,
            sample: None,
            ffmpeg: &ffmpeg,
        },
        &NoopProgress,
    )
    .expect_err("there is no palette called `embers`");

    assert!(matches!(err, Error::Config(_)), "got {err:?}");
    assert!(
        err.to_string().contains("carpathian"),
        "name what does exist: {err}"
    );
    assert!(!output.exists() && !part.exists(), "nothing was written");
}

/// The M3 acceptance criterion, on pixels: a `[visual.params]` value from the
/// config reaches the shader and changes the picture.
///
/// `param_reaches_declared_uniform_slot` proves the schema packs into the slot
/// the shader reads. This proves the *pipeline* carries a user's table that far
/// — that `config.visual.params` is resolved rather than dropped on the way from
/// the config file to the uniform.
#[test]
fn a_preset_parameter_from_the_config_reaches_the_rendered_pixels() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ffmpeg = recording_ffmpeg(dir.path());
    let (plain, _) = output_paths(dir.path());
    let tuned = dir.path().join("tuned.mp4");

    render_fixture(&ffmpeg, &plain, Some(sample("0s..0.2s")), &NoopProgress).expect("renders");

    let mut config = config();
    // `vignette` at zero: the corners stop falling off, so every frame changes.
    config.visual.params = "vignette = 0.0".parse().expect("a params table");

    let _device = one_device_at_a_time();
    render(
        &RenderRequest {
            input: &fixture_mp3(),
            output: &tuned,
            config: &config,
            adapter: AdapterChoice::Software,
            sample: Some(sample("0s..0.2s")),
            ffmpeg: &ffmpeg,
        },
        &NoopProgress,
    )
    .expect("the tuned render succeeds");
    drop(_device);

    assert_ne!(
        recorded_frames(&plain),
        recorded_frames(&tuned),
        "`visual.params.vignette` never reached the shader",
    );
}

/// A parameter outside its declared range is the user's argument. It must fail
/// before the song is decoded — the whole point of validating against a schema
/// rather than letting the shader clamp it.
#[test]
fn an_out_of_range_parameter_fails_before_the_song_is_decoded() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ffmpeg = recording_ffmpeg(dir.path());
    let (output, part) = output_paths(dir.path());

    let mut config = config();
    config.visual.params = "bass_drive = 99.0".parse().expect("a params table");

    let progress = Recorder::default();
    let _device = one_device_at_a_time();
    let err = render(
        &RenderRequest {
            input: &fixture_mp3(),
            output: &output,
            config: &config,
            adapter: AdapterChoice::Software,
            sample: None,
            ffmpeg: &ffmpeg,
        },
        &progress,
    )
    .expect_err("`bass_drive` tops out well below 99");

    assert!(matches!(err, Error::Config(_)), "exit 2, got {err:?}");
    let message = err.to_string();
    assert!(message.contains("bass_drive"), "name the parameter: {err}");
    assert!(message.contains("0..4"), "name the range: {err}");
    assert!(
        progress.phases().is_empty(),
        "the song was decoded before the config was validated",
    );
    assert!(!output.exists() && !part.exists(), "nothing was written");
}

/// A parameter the preset does not have, with the name it probably meant.
#[test]
fn an_unknown_parameter_fails_before_the_song_is_decoded_and_suggests_a_name() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ffmpeg = recording_ffmpeg(dir.path());
    let (output, part) = output_paths(dir.path());

    let mut config = config();
    config.visual.params = "bas_drive = 2.0".parse().expect("a params table");

    let progress = Recorder::default();
    let _device = one_device_at_a_time();
    let err = render(
        &RenderRequest {
            input: &fixture_mp3(),
            output: &output,
            config: &config,
            adapter: AdapterChoice::Software,
            sample: None,
            ffmpeg: &ffmpeg,
        },
        &progress,
    )
    .expect_err("there is no `bas_drive`");

    assert!(matches!(err, Error::Config(_)), "exit 2, got {err:?}");
    assert!(
        err.to_string().contains("did you mean `bass_drive`"),
        "{err}"
    );
    assert!(progress.phases().is_empty(), "nothing was analyzed");
    assert!(!output.exists() && !part.exists(), "nothing was written");
}

/// A sample the song cannot satisfy is the user's argument, not a render
/// failure — exit code 2, and nothing is spawned, rendered, or written.
#[test]
fn a_sample_that_starts_after_the_song_fails_before_anything_is_written() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ffmpeg = recording_ffmpeg(dir.path());
    let (output, part) = output_paths(dir.path());

    let err = render_fixture(&ffmpeg, &output, Some(sample("6s..8s")), &NoopProgress)
        .expect_err("the fixture is 5 seconds long");

    assert!(matches!(err, Error::Config(_)), "got {err:?}");
    assert!(!output.exists(), "nothing was rendered");
    assert!(!part.exists(), "nothing was started");
}

/// `AGENTS.md`: never leave a half-written file. An ffmpeg that dies partway
/// through must take its `.part` with it, and the failure must reach the caller.
#[test]
fn an_ffmpeg_that_dies_midrender_leaves_no_output_behind() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ffmpeg = fake_ffmpeg(
        dir.path(),
        "printf 'half a container' > \"$part\"
echo 'x264: encoder is on fire' >&2
exit 1",
    );
    let (output, part) = output_paths(dir.path());

    let err = render_fixture(&ffmpeg, &output, Some(sample("0s..2s")), &NoopProgress)
        .expect_err("an ffmpeg that exits 1 renders no video");

    assert!(matches!(err, Error::Encode(_)), "got {err:?}");
    assert!(
        err.to_string().contains("encoder is on fire"),
        "ffmpeg's own complaint must survive the pipeline: {err}"
    );
    assert!(!output.exists(), "no half-written output may survive");
    assert!(!part.exists(), "no half-written part file may survive");
}

/// A file that is not an mp3 is an input problem, reported before the GPU is
/// touched or ffmpeg is spawned.
#[test]
fn an_input_that_will_not_decode_is_an_input_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ffmpeg = recording_ffmpeg(dir.path());
    let (output, _) = output_paths(dir.path());
    let not_audio = dir.path().join("not-audio.mp3");
    fs::write(&not_audio, b"this is not an mp3").expect("write");

    // Decode fails before an adapter is ever requested, but the lock costs
    // nothing and survives a pipeline that one day opens the device earlier.
    let _device = one_device_at_a_time();
    let config = config();
    let err = render(
        &RenderRequest {
            input: &not_audio,
            output: &output,
            config: &config,
            adapter: AdapterChoice::Software,
            sample: None,
            ffmpeg: &ffmpeg,
        },
        &NoopProgress,
    )
    .expect_err("a text file is not a song");

    assert!(matches!(err, Error::Input(_)), "got {err:?}");
    assert!(!output.exists());
}

/// The `--sample` audio promise (UT-002): "audio in the output covers the same
/// range." Against the real encoder, with the real mux.
///
/// An `ffprobe` codec assertion cannot see this — re-encoding an mp3 still
/// reports `codec_name=mp3`, and re-encoding *the right second* of it would look
/// identical. The bitstream is what tells the truth: a copied slice of the
/// original appears verbatim inside it, and not at its beginning.
#[test]
fn a_sampled_render_muxes_the_matching_slice_of_the_original_audio() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (output, part) = output_paths(dir.path());
    let song = fixture_mp3();

    let summary = render_fixture(
        &system_ffmpeg(),
        &output,
        Some(sample("1s..3s")),
        &NoopProgress,
    )
    .expect("the system ffmpeg encodes two seconds of video");

    assert_eq!(summary.frames, 60);
    assert!(!part.exists(), "the part file is renamed on success");
    assert!(output.exists(), "the render produced an mp4");

    assert_eq!(probe(&output, "v", "codec_name"), "h264");
    assert_eq!(probe(&output, "a", "codec_name"), "mp3");

    let muxed = audio_bitstream(&output);
    let original = audio_bitstream(&song);
    assert!(!muxed.is_empty(), "the mp4 carries audio at all");

    let at = find(&original, &muxed).unwrap_or_else(|| {
        panic!(
            "the muxed audio is not a verbatim slice of the original: \
             {} of {} bytes were re-encoded",
            muxed.len(),
            original.len(),
        )
    });
    assert!(
        at > 0,
        "the sample starts at 1s, but the muxed audio starts at the top of the song"
    );
}

/// The first index at which `needle` occurs in `haystack`.
///
/// A naive scan over every window would compare tens of millions of bytes in a
/// debug build; anchoring on the first eight narrows it to a handful.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    let anchor: &[u8] = needle.get(..8)?;

    haystack
        .windows(anchor.len())
        .enumerate()
        .filter(|(_, window)| *window == anchor)
        .map(|(at, _)| at)
        .find(|&at| haystack[at..].starts_with(needle))
}

/// The raw audio packet payloads of `file`, with no container around them.
fn audio_bitstream(file: &Path) -> Vec<u8> {
    let output = Command::new(DEFAULT_PROGRAM)
        .args(["-v", "error", "-i"])
        .arg(file)
        .args(["-map", "0:a", "-c", "copy", "-f", "data", "-"])
        .output()
        .expect("the pipeline tests need the system ffmpeg");

    assert!(
        output.status.success(),
        "ffmpeg could not read the audio of {}: {}",
        file.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

/// Ask `ffprobe` for one entry of every `kind` (`v` or `a`) stream.
fn probe(file: &Path, kind: &str, entry: &str) -> String {
    let output = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", kind])
        .args(["-show_entries", &format!("stream={entry}")])
        .args(["-of", "csv=p=0"])
        .arg(file)
        .output()
        .expect("the pipeline tests need ffprobe: `sudo dnf install ffmpeg`");

    assert!(
        output.status.success(),
        "ffprobe failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// A solid-color PNG, for the background-image layer.
fn solid_png(dir: &Path, name: &str, color: [u8; 4]) -> PathBuf {
    let path = dir.join(name);
    let mut png = image::RgbaImage::new(8, 8);
    for pixel in png.pixels_mut() {
        *pixel = image::Rgba(color);
    }
    png.save(&path).expect("write the background png");
    path
}

/// Mean value of one channel over a frame, 0..=255.
fn mean_channel(frame: &[u8], channel: usize) -> f64 {
    let total: f64 = frame
        .chunks_exact(4)
        .map(|pixel| f64::from(pixel[channel]))
        .sum();
    total / f64::from(WIDTH * HEIGHT)
}

/// `--bg` is a layer, not a suggestion: the image the user named must be under
/// every rendered frame. Compared against the same render without it, because
/// `pulse` draws over the background and no single pixel is guaranteed to be
/// the image alone.
#[test]
fn a_background_image_reaches_the_rendered_frames() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ffmpeg = recording_ffmpeg(dir.path());
    let with_image = dir.path().join("with.mp4");
    let without_image = dir.path().join("without.mp4");

    let mut config = config();
    config.background.source = Some(BackgroundSource::Image(solid_png(
        dir.path(),
        "green.png",
        [0x00, 0xff, 0x00, 0xff],
    )));
    config.background.fit = Fit::Stretch;

    let _device = one_device_at_a_time();
    render(
        &RenderRequest {
            input: &fixture_mp3(),
            output: &with_image,
            config: &config,
            adapter: AdapterChoice::Software,
            sample: Some(sample("0s..0.2s")),
            ffmpeg: &ffmpeg,
        },
        &NoopProgress,
    )
    .expect("the fixture renders over a background image");

    config.background.source = None;
    render(
        &RenderRequest {
            input: &fixture_mp3(),
            output: &without_image,
            config: &config,
            adapter: AdapterChoice::Software,
            sample: Some(sample("0s..0.2s")),
            ffmpeg: &ffmpeg,
        },
        &NoopProgress,
    )
    .expect("the fixture renders over the palette backdrop");

    let green = recorded_frames(&with_image);
    let backdrop = recorded_frames(&without_image);
    assert_eq!(green.len(), 6, "0.2 s at 30 fps");

    for (index, frame) in green.iter().enumerate() {
        assert!(
            mean_channel(frame, 1) > 200.0,
            "frame {index} should be mostly the green background, got {}",
            mean_channel(frame, 1),
        );
        assert!(
            mean_channel(frame, 1) > mean_channel(&backdrop[index], 1) + 100.0,
            "frame {index} is no greener than the render with no image at all",
        );
    }
}

/// A background smaller than the frame renders perfectly and looks soft. The
/// warning is the only thing standing between the user and a blurry video they
/// will not understand, so it must fire — once, before the frames, and naming
/// the way out.
#[test]
fn a_background_smaller_than_the_frame_warns_once_before_any_frame_is_drawn() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ffmpeg = recording_ffmpeg(dir.path());
    let (output, _) = output_paths(dir.path());
    let recorder = Recorder::default();

    let mut config = config();
    // `solid_png` is 8×8; the frame is 320×180.
    config.background.source = Some(BackgroundSource::Image(solid_png(
        dir.path(),
        "small.png",
        [0x00, 0xff, 0x00, 0xff],
    )));

    let _device = one_device_at_a_time();
    render(
        &RenderRequest {
            input: &fixture_mp3(),
            output: &output,
            config: &config,
            adapter: AdapterChoice::Software,
            sample: Some(sample("0s..0.2s")),
            ffmpeg: &ffmpeg,
        },
        &recorder,
    )
    .expect("a small background is a soft render, not a failed one");

    let warnings = recorder.warnings();
    assert_eq!(warnings.len(), 1, "warn once, not per frame: {warnings:?}");
    let warning = &warnings[0];
    assert!(warning.contains("small.png"), "name the image: {warning}");
    assert!(warning.contains("8x8"), "name its size: {warning}");
    assert!(warning.contains("320x180"), "name the frame: {warning}");
    assert!(
        warning.contains("background.blur"),
        "say what to do: {warning}"
    );
}

/// An image at least as large as the frame is only ever downsampled, and the
/// user has nothing to act on. A warning here would fire on every well-made
/// render there is.
#[test]
fn a_background_as_large_as_the_frame_warns_about_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ffmpeg = recording_ffmpeg(dir.path());
    let (output, _) = output_paths(dir.path());
    let recorder = Recorder::default();

    let path = dir.path().join("large.png");
    image::RgbaImage::from_pixel(WIDTH, HEIGHT, image::Rgba([0x00, 0xff, 0x00, 0xff]))
        .save(&path)
        .expect("write the background png");

    let mut config = config();
    config.background.source = Some(BackgroundSource::Image(path));

    let _device = one_device_at_a_time();
    render(
        &RenderRequest {
            input: &fixture_mp3(),
            output: &output,
            config: &config,
            adapter: AdapterChoice::Software,
            sample: Some(sample("0s..0.2s")),
            ffmpeg: &ffmpeg,
        },
        &recorder,
    )
    .expect("renders");

    assert!(
        recorder.warnings().is_empty(),
        "an exactly-sized background is not upscaled: {:?}",
        recorder.warnings(),
    );
}

/// `darken` is what makes the visuals read on top of a bright photograph
/// (`VISION.md` §5.3). It reaches the pixels, and it takes light away.
#[test]
fn darkening_the_background_dims_the_rendered_frames() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ffmpeg = recording_ffmpeg(dir.path());
    let bright = dir.path().join("bright.mp4");
    let dim = dir.path().join("dim.mp4");

    let mut config = config();
    config.background.source = Some(BackgroundSource::Image(solid_png(
        dir.path(),
        "white.png",
        [0xff, 0xff, 0xff, 0xff],
    )));
    config.background.fit = Fit::Stretch;

    let _device = one_device_at_a_time();
    let render_to = |output: &Path, config: &Config| {
        render(
            &RenderRequest {
                input: &fixture_mp3(),
                output,
                config,
                adapter: AdapterChoice::Software,
                sample: Some(sample("0s..0.1s")),
                ffmpeg: &ffmpeg,
            },
            &NoopProgress,
        )
        .expect("the fixture renders");
    };

    render_to(&bright, &config);
    config.background.darken = 0.75;
    render_to(&dim, &config);

    let bright = recorded_frames(&bright);
    let dim = recorded_frames(&dim);

    for (index, frame) in dim.iter().enumerate() {
        assert!(
            mean_luma(frame) < mean_luma(&bright[index]),
            "frame {index} was not dimmed by `background.darken`",
        );
    }
}

/// A background image that does not exist costs a millisecond, not a decode —
/// the same promise `visual.preset` and `visual.palette` already keep.
#[test]
fn a_missing_background_image_fails_before_the_song_is_even_decoded() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ffmpeg = recording_ffmpeg(dir.path());
    let (output, part) = output_paths(dir.path());

    let mut config = config();
    config.background.source = Some(BackgroundSource::Image(dir.path().join("forest.png")));

    let _device = one_device_at_a_time();
    let err = render(
        &RenderRequest {
            input: &fixture_mp3(),
            output: &output,
            config: &config,
            adapter: AdapterChoice::Software,
            sample: None,
            ffmpeg: &ffmpeg,
        },
        &NoopProgress,
    )
    .expect_err("there is no such background image");

    assert!(matches!(err, Error::Input(_)), "got {err:?}");
    assert!(err.to_string().contains("forest.png"), "{err}");
    assert!(!output.exists() && !part.exists(), "nothing was written");
}

/// Render `input` with `config`, on lavapipe, into `ffmpeg`.
fn render_song(
    ffmpeg: &Ffmpeg,
    input: &Path,
    output: &Path,
    config: &Config,
    sample: &str,
    progress: &dyn Progress,
) -> Result<RenderSummary, Error> {
    let _device = one_device_at_a_time();
    render(
        &RenderRequest {
            input,
            output,
            config,
            adapter: AdapterChoice::Software,
            sample: Some(self::sample(sample)),
            ffmpeg,
        },
        progress,
    )
}

/// The config a text-card test renders with: type large enough that a 320x180
/// frame carries readable ink. The card is fully up by `in_at + fade` = 1.6 s.
fn text_config() -> Config {
    let mut config = config();
    config.text.size = 0.16;
    config
}

/// How many pixels of `frame` differ from `other`.
fn different_pixels(frame: &[u8], other: &[u8]) -> usize {
    frame
        .chunks_exact(4)
        .zip(other.chunks_exact(4))
        .filter(|(a, b)| a != b)
        .count()
}

/// The M4 promise on pixels: the card the ID3 tags name is drawn over the
/// visuals, and it is drawn *because* text is enabled.
///
/// Frames at 1.6 s are past `in_at + fade`, so the card is at full opacity.
/// Nothing else in the suite would notice a card that resolved, rasterized, and
/// never reached the compositor.
#[test]
fn the_text_card_from_id3_reaches_the_rendered_frames() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ffmpeg = recording_ffmpeg(dir.path());
    let (with_card, without_card) = (dir.path().join("with.mp4"), dir.path().join("without.mp4"));

    let mut config = text_config();
    render_song(
        &ffmpeg,
        &fixture_mp3(),
        &with_card,
        &config,
        "1.6s..1.7s",
        &NoopProgress,
    )
    .expect("the tagged fixture renders with its card");

    config.text.enabled = false;
    render_song(
        &ffmpeg,
        &fixture_mp3(),
        &without_card,
        &config,
        "1.6s..1.7s",
        &NoopProgress,
    )
    .expect("and renders without it");

    let carded = recorded_frames(&with_card);
    let bare = recorded_frames(&without_card);
    assert_eq!(carded.len(), 3, "0.1 s at 30 fps");

    for (index, frame) in carded.iter().enumerate() {
        let inked = different_pixels(frame, &bare[index]);
        assert!(
            inked > 100,
            "frame {index} carries only {inked} pixels of text card; the card \
             never reached the compositor",
        );
    }
}

/// `--no-text` and `[text] enabled = false` draw no card, and the frames are the
/// frames of a render that never had one.
#[test]
fn a_disabled_card_leaves_the_frame_exactly_as_it_found_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ffmpeg = recording_ffmpeg(dir.path());
    let (disabled, untagged) = (dir.path().join("off.mp4"), dir.path().join("bare.mp4"));

    let mut config = text_config();
    config.text.enabled = false;
    render_song(
        &ffmpeg,
        &fixture_mp3(),
        &disabled,
        &config,
        "1.6s..1.7s",
        &NoopProgress,
    )
    .expect("renders");

    // The same seconds of the same tones with no tags to draw: also no card, and
    // so the same frames, down to the byte.
    config.text.enabled = true;
    render_song(
        &ffmpeg,
        &fixture("tone-untagged.mp3"),
        &untagged,
        &config,
        "1.6s..1.7s",
        &NoopProgress,
    )
    .expect("renders");

    assert_eq!(
        recorded_frames(&disabled),
        recorded_frames(&untagged),
        "a skipped card is not the same as no card at all",
    );
}

/// The risk-matrix row: both tags missing and no overrides. The render succeeds,
/// the card is skipped, and the user is told what to do about it.
#[test]
fn missing_tags_warns_and_skips_card() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ffmpeg = recording_ffmpeg(dir.path());
    let (output, _) = output_paths(dir.path());
    let recorder = Recorder::default();

    let summary = render_song(
        &ffmpeg,
        &fixture("tone-untagged.mp3"),
        &output,
        &text_config(),
        "1.6s..1.7s",
        &recorder,
    )
    .expect("an untagged song still renders");

    assert_eq!(summary.frames, 3, "the render was not abandoned");

    let warnings = recorder.warnings();
    assert_eq!(warnings.len(), 1, "warn once, not per frame: {warnings:?}");
    let warning = &warnings[0];
    assert!(
        warning.contains("tone-untagged.mp3"),
        "name the song: {warning}"
    );
    assert!(warning.contains("--title"), "say what to do: {warning}");
    assert!(
        warning.contains("--no-text"),
        "say how to silence it: {warning}"
    );
}

/// `--title` and `--artist` put a card on a song that has no tags to draw one
/// from, and there is then nothing to warn about (UT-009).
#[test]
fn overrides_put_a_card_on_an_untagged_song_without_a_warning() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ffmpeg = recording_ffmpeg(dir.path());
    let (overridden, bare) = (dir.path().join("named.mp4"), dir.path().join("bare.mp4"));
    let recorder = Recorder::default();

    let mut config = text_config();
    config.text.title = Some("Cold Design".to_owned());
    render_song(
        &ffmpeg,
        &fixture("tone-untagged.mp3"),
        &overridden,
        &config,
        "1.6s..1.7s",
        &recorder,
    )
    .expect("renders");

    assert!(
        recorder.warnings().is_empty(),
        "an overridden title is a title: {:?}",
        recorder.warnings(),
    );

    config.text.title = None;
    render_song(
        &ffmpeg,
        &fixture("tone-untagged.mp3"),
        &bare,
        &config,
        "1.6s..1.7s",
        &NoopProgress,
    )
    .expect("renders");

    let named = recorded_frames(&overridden);
    let bare = recorded_frames(&bare);
    assert!(
        different_pixels(&named[0], &bare[0]) > 100,
        "`--title` resolved but never reached a pixel",
    );
}

/// A `[text] font` that does not exist costs a millisecond, not a decode — the
/// same promise `visual.preset`, `visual.palette`, and `background.image` keep.
#[test]
fn a_missing_font_fails_before_the_song_is_even_decoded() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ffmpeg = recording_ffmpeg(dir.path());
    let (output, part) = output_paths(dir.path());

    let mut config = config();
    config.text.font = FontChoice::Path(dir.path().join("Inter.ttf"));

    let progress = Recorder::default();
    let err = render_song(
        &ffmpeg,
        &fixture_mp3(),
        &output,
        &config,
        "0s..0.2s",
        &progress,
    )
    .expect_err("there is no such font");

    assert!(matches!(err, Error::Input(_)), "got {err:?}");
    assert!(err.to_string().contains("Inter.ttf"), "{err}");
    assert!(progress.phases().is_empty(), "nothing was analyzed");
    assert!(!output.exists() && !part.exists(), "nothing was written");
}

/// A background video that is not there is the user's argument, and it must be
/// caught before the song is decoded — not by the ffmpeg that would have been
/// spawned after the analysis pass.
#[test]
fn a_missing_background_video_fails_before_the_song_is_even_decoded() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ffmpeg = recording_ffmpeg(dir.path());
    let (output, part) = output_paths(dir.path());
    let progress = Recorder::default();

    let mut config = config();
    config.background.source = Some(BackgroundSource::Video("loops/smoke.mp4".into()));

    let _device = one_device_at_a_time();
    let err = render(
        &RenderRequest {
            input: &fixture_mp3(),
            output: &output,
            config: &config,
            adapter: AdapterChoice::Software,
            sample: None,
            ffmpeg: &ffmpeg,
        },
        &progress,
    )
    .expect_err("there is no such video");

    assert!(matches!(err, Error::Input(_)), "got {err:?}");
    assert!(err.to_string().contains("smoke.mp4"), "{err}");
    assert!(progress.phases().is_empty(), "nothing was analyzed");
    assert!(!output.exists() && !part.exists(), "nothing was written");
}

/// An ffmpeg stand-in that decodes background video with the real thing and
/// records the frames avz encodes.
///
/// One preflighted binary serves both subprocesses, so a test that wants a real
/// decoder *and* the raw RGBA avz piped has to dispatch on the argv.
/// `-stream_loop` appears in the reader's invocation and in no other.
///
/// `exec`, so the process avz kills on drop is the one holding the pipe.
fn recording_ffmpeg_with_a_real_decoder(dir: &Path) -> Ffmpeg {
    fake_ffmpeg(
        dir,
        "case \"$*\" in *-stream_loop*) exec ffmpeg \"$@\";; esac\ncat > \"$part\"",
    )
}

/// A one-second, flat-magenta loop: bright in red and blue, black in green.
fn magenta_loop(dir: &Path) -> PathBuf {
    let path = dir.join("loop.mp4");
    let status = Command::new("ffmpeg")
        .args(["-y", "-hide_banner", "-loglevel", "error", "-f", "lavfi"])
        .args(["-i", "color=c=magenta:s=64x36:r=30:d=1"])
        .args(["-c:v", "libx264", "-qp", "0", "-pix_fmt", "yuv444p"])
        .arg(&path)
        .status()
        .expect("ffmpeg encodes the background loop");
    assert!(status.success(), "ffmpeg could not encode the loop");
    path
}

/// `background.video` is a layer, not a suggestion. Compared against the same
/// render without it, because `pulse` draws over the background and no single
/// pixel is guaranteed to be the video alone.
///
/// The loop is 64×36 and the frame is 320×180: ffmpeg does the scaling, and
/// `cover` fills the frame with it. What reaches the pixels is therefore the
/// video's colour and not the palette's — `ember`'s backdrop has no blue in it.
#[test]
fn a_background_video_reaches_the_rendered_frames() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ffmpeg = recording_ffmpeg_with_a_real_decoder(dir.path());
    let with_video = dir.path().join("with.mp4");
    let without_video = dir.path().join("without.mp4");

    let mut config = config();
    config.background.source = Some(BackgroundSource::Video(magenta_loop(dir.path())));

    let _device = one_device_at_a_time();
    render(
        &RenderRequest {
            input: &fixture_mp3(),
            output: &with_video,
            config: &config,
            adapter: AdapterChoice::Software,
            sample: Some(sample("0s..0.2s")),
            ffmpeg: &ffmpeg,
        },
        &NoopProgress,
    )
    .expect("the fixture renders over a background video");

    config.background.source = None;
    render(
        &RenderRequest {
            input: &fixture_mp3(),
            output: &without_video,
            config: &config,
            adapter: AdapterChoice::Software,
            sample: Some(sample("0s..0.2s")),
            ffmpeg: &ffmpeg,
        },
        &NoopProgress,
    )
    .expect("the fixture renders over the palette backdrop");

    let magenta = recorded_frames(&with_video);
    let backdrop = recorded_frames(&without_video);
    assert_eq!(magenta.len(), 6, "0.2 s at 30 fps");

    for (index, frame) in magenta.iter().enumerate() {
        assert!(
            mean_channel(frame, 2) > 200.0,
            "frame {index} should be mostly the magenta background, got {}",
            mean_channel(frame, 2),
        );
        assert!(
            mean_channel(frame, 2) > mean_channel(&backdrop[index], 2) + 100.0,
            "frame {index} is no bluer than the render with no video at all",
        );
    }
}

/// A background video hands the renderer one frame per rendered frame, in order.
///
/// A reader that re-uploaded the frame it had, or that ran ahead of the render,
/// would pass every static-picture assertion above: a magenta loop is magenta
/// whichever of its frames you draw. So the loop here counts in red, one step of
/// eight per frame, and the rendered frames must count with it.
#[test]
fn each_rendered_frame_draws_the_next_frame_of_the_loop() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ffmpeg = recording_ffmpeg_with_a_real_decoder(dir.path());
    let (output, _) = output_paths(dir.path());

    let source = dir.path().join("counter.mp4");
    let status = Command::new("ffmpeg")
        .args(["-y", "-hide_banner", "-loglevel", "error", "-f", "lavfi"])
        .args(["-i", "color=c=black:s=64x36:r=30:d=0.2"])
        .args(["-vf", "format=rgb24,geq=r='N*40':g=0:b=0"])
        .args(["-c:v", "libx264", "-qp", "0", "-pix_fmt", "yuv444p"])
        .arg(&source)
        .status()
        .expect("ffmpeg encodes the counting loop");
    assert!(status.success());

    let mut config = config();
    config.background.source = Some(BackgroundSource::Video(source));
    // The card would cover part of the frame with ink of its own.
    config.text.enabled = false;

    let _device = one_device_at_a_time();
    render(
        &RenderRequest {
            input: &fixture_mp3(),
            output: &output,
            config: &config,
            adapter: AdapterChoice::Software,
            sample: Some(sample("0s..0.2s")),
            ffmpeg: &ffmpeg,
        },
        &NoopProgress,
    )
    .expect("the fixture renders over the counting loop");

    let reds: Vec<f64> = recorded_frames(&output)
        .iter()
        .map(|frame| mean_channel(frame, 0))
        .collect();

    for (index, pair) in reds.windows(2).enumerate() {
        assert!(
            pair[1] > pair[0],
            "frame {} is not later in the loop than frame {index}: {reds:?}",
            index + 1,
        );
    }
}
