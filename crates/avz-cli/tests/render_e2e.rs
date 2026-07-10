//! The whole tool, end to end: `avz render song.mp3` in, a playable mp4 out.
//!
//! Everything below this test is exercised somewhere else, one seam at a time —
//! the decoder against synthetic signals, the readback against odd row widths,
//! the encoder against a shell stand-in that can be made to die on cue. What none
//! of those can answer is whether the assembled binary, run the way a user runs
//! it, produces a file a player will open. That is what this asserts, and it is
//! the test `docs/TESTING.md` names as the one CI runs on every push.
//!
//! `ffprobe` is the oracle rather than the pixels: a container with two streams,
//! the frame count and frame rate avz reported, and a duration matching the
//! `--sample` that was asked for. Pixel-level and bitstream-level truth belong to
//! `avz-core`'s `pipeline_render.rs` and `encode_ffmpeg.rs`, which can see them.
//!
//! Software adapter, always. A render whose output CI compares must not depend on
//! the GPU of whoever runs it (`AGENTS.md`, determinism). Needs Mesa's software
//! Vulkan driver and the system ffmpeg. On Fedora:
//! `sudo dnf install mesa-vulkan-drivers ffmpeg`.

#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as Subprocess;

use assert_cmd::Command;
use predicates::str::contains;

/// The excerpt CI renders. Long enough to hold several kick decays of the
/// fixture, short enough that every push pays about a second for it.
const SAMPLE: &str = "2s";

/// How long the mp4 must play for: `SAMPLE`, to the frame.
const SAMPLE_SECS: f64 = 2.0;

/// The default frame rate, and the rate the sampled excerpt is rendered at.
const FPS: u32 = 30;

/// `SAMPLE_SECS * FPS`, which is also what the CLI prints.
const FRAMES: u64 = 60;

/// Slack on the container duration.
///
/// The video is exactly [`SAMPLE_SECS`] long; the muxed mp3 runs to the end of
/// the mp3 frame that covers it, and `-shortest` cuts the container back to the
/// shorter stream. A tenth of a second is far below a rendered frame's worth of
/// error and far above the container's rounding.
const DURATION_TOLERANCE_SECS: f64 = 0.1;

/// A committed CC0 fixture. See `assets/fixtures/README.md`.
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/fixtures")
        .join(name)
}

/// UT-001 (`VISION.md` §7), through the binary: a real mp3 in, one video stream
/// and one audio stream out, playing for exactly as long as was sampled.
///
/// Every assertion here is one a broken pipeline could fail while still writing
/// *some* mp4: a dropped frame shortens the duration, a lost `-map 1:a` removes
/// the audio, a second mapped stream would make the file ambiguous to a player.
#[test]
fn a_two_second_software_render_is_a_playable_mp4_with_one_video_and_one_audio_stream() {
    let dir = tempfile::tempdir().expect("tempdir");
    let song = dir.path().join("song.mp3");
    fs::copy(fixture("tone-tagged.mp3"), &song).expect("copy the fixture");

    Command::cargo_bin("avz")
        .expect("avz binary builds")
        .arg("render")
        .arg(&song)
        .args(["--sample", SAMPLE, "--adapter", "software"])
        .assert()
        .success()
        .stdout(contains("rendering on"))
        .stdout(contains("software rasterizer"))
        .stdout(contains(format!("{FRAMES} frames")))
        .stdout(contains(format!("{SAMPLE_SECS:.2}s")));

    let output = dir.path().join("song.mp4");
    assert!(output.is_file(), "the render produced no mp4");
    assert!(
        !dir.path().join("song.mp4.part").exists(),
        "the part file is renamed on success, never left behind"
    );

    // One of each: a player asked to open this must not have to choose.
    assert_eq!(
        stream(&output, "v", "index").lines().count(),
        1,
        "exactly one video stream"
    );
    assert_eq!(
        stream(&output, "a", "index").lines().count(),
        1,
        "exactly one audio stream"
    );

    assert_eq!(stream(&output, "v", "codec_name"), "h264");
    assert_eq!(stream(&output, "a", "codec_name"), "mp3");
    assert_eq!(
        stream(&output, "v", "r_frame_rate"),
        format!("{FPS}/1"),
        "the video is timestamped at the configured frame rate"
    );
    assert_eq!(
        stream(&output, "v", "nb_frames"),
        FRAMES.to_string(),
        "every rendered frame reached the container"
    );

    let duration: f64 = format_entry(&output, "duration")
        .parse()
        .expect("ffprobe reports the container duration in seconds");
    assert!(
        (duration - SAMPLE_SECS).abs() <= DURATION_TOLERANCE_SECS,
        "`--sample {SAMPLE}` produced a {duration}s video"
    );
}

/// UT-008 (`designs/USER-TASKS.md`), end to end and through real ffmpeg:
///
/// ```bash
/// avz config --example > avz.toml
/// avz render song.mp3 --config avz.toml --sample 2s
/// ```
///
/// The template pins `resolution = "1920x1080"`, and a 1080p lavapipe render is
/// minutes of CI time to learn nothing this test is about. `--set` outranks the
/// config file (`VISION.md` §5.5), so the frame size is the one thing overruled;
/// every other key in the template is the one that reaches the render.
#[test]
fn the_example_config_renders_a_playable_mp4() {
    let dir = tempfile::tempdir().expect("tempdir");
    let song = dir.path().join("song.mp3");
    fs::copy(fixture("tone-tagged.mp3"), &song).expect("copy the fixture");

    let template = Command::cargo_bin("avz")
        .expect("avz binary builds")
        .args(["config", "--example"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let config = dir.path().join("avz.toml");
    fs::write(&config, template).expect("write the template avz just printed");

    Command::cargo_bin("avz")
        .expect("avz binary builds")
        .arg("render")
        .arg(&song)
        .arg("--config")
        .arg(&config)
        .args(["--sample", SAMPLE, "--adapter", "software"])
        .args(["--set", "output.resolution=320x180"])
        .assert()
        .success()
        .stdout(contains(format!("{FRAMES} frames")));

    let output = dir.path().join("song.mp4");
    assert!(output.is_file(), "the example config produced no mp4");
    assert_eq!(stream(&output, "v", "codec_name"), "h264");
    assert_eq!(
        stream(&output, "a", "codec_name"),
        "mp3",
        "the template must not have talked avz into re-encoding the audio",
    );
}

/// Ask `ffprobe` for one entry of every `kind` (`v` or `a`) stream.
fn stream(file: &Path, kind: &str, entry: &str) -> String {
    ffprobe(file, &["-select_streams", kind], &format!("stream={entry}"))
}

/// Ask `ffprobe` for one entry of the container itself.
fn format_entry(file: &Path, entry: &str) -> String {
    ffprobe(file, &[], &format!("format={entry}"))
}

fn ffprobe(file: &Path, select: &[&str], entries: &str) -> String {
    let output = Subprocess::new("ffprobe")
        .args(["-v", "error"])
        .args(select)
        .args(["-show_entries", entries])
        .args(["-of", "csv=p=0"])
        .arg(file)
        .output()
        .expect("the render tests need ffprobe: `sudo dnf install ffmpeg`");

    assert!(
        output.status.success(),
        "ffprobe failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}
