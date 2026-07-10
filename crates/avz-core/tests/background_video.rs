//! The looped background video, read from its own ffmpeg subprocess.
//!
//! `VISION.md` §5.3 layer 1: a second ffmpeg decodes, loops, scales, and
//! frame-rate-converts the video, and avz reads exactly `width * height * 4`
//! bytes per frame. That division of labour is the whole design, so the tests
//! that matter are about what avz *asked for* and what came back — never about
//! the codec, which is ffmpeg's problem.
//!
//! The loop lives here rather than in a unit test because seamlessness is a
//! property of ffmpeg's `-stream_loop`, not of any function avz wrote: only a
//! real decode of a real file can say whether the frame after the last one is
//! the first one again.
//!
//! Needs the system ffmpeg. On Fedora: `sudo dnf install ffmpeg`.

use std::path::{Path, PathBuf};
use std::process::Command;

use avz_core::config::Fit;
use avz_core::encode::{DEFAULT_PROGRAM, Ffmpeg, preflight};
use avz_core::render::{BackgroundVideo, VideoSettings};

/// Small enough that a hundred frames of it cost nothing, and not square, so a
/// fit mode that transposes the axes is visible.
const WIDTH: u32 = 64;
const HEIGHT: u32 = 36;
const FPS: u32 = 30;

/// One second of video, which at [`FPS`] is exactly this many frames.
const LOOP_FRAMES: usize = 30;

fn system_ffmpeg() -> Ffmpeg {
    preflight(DEFAULT_PROGRAM)
        .expect("the background-video tests need ffmpeg: `sudo dnf install ffmpeg`")
}

fn settings() -> VideoSettings {
    VideoSettings {
        width: WIDTH,
        height: HEIGHT,
        fps: FPS,
        fit: Fit::Cover,
        blur: 0.0,
        darken: 0.0,
    }
}

/// Encode a one-second clip whose frame `n` is the flat colour `(8n, 128, 200)`.
///
/// A flat frame per index is what makes "the loop repeated exactly" an equality
/// on whole frames: a seam that dropped or duplicated one frame renumbers every
/// frame after it, and the picture says which. `testsrc2` would not do — at this
/// size its adjacent frames are the same bytes, and a stuttering seam would pass.
///
/// Lossless (`-qp 0`), so the decoder hands back the colours the generator wrote
/// and a red channel stepping by 8 cannot be quantized into its neighbour.
fn one_second_loop(dir: &Path, source_fps: u32) -> PathBuf {
    let path = dir.join("loop.mp4");
    let status = Command::new(DEFAULT_PROGRAM)
        .args(["-y", "-hide_banner", "-loglevel", "error", "-f", "lavfi"])
        .arg("-i")
        .arg(format!(
            "color=c=black:s={WIDTH}x{HEIGHT}:r={source_fps}:d=1"
        ))
        // `N * 8` never reaches 256 within a second at either rate, so the
        // expression needs no `mod` and therefore no escaped comma.
        .args(["-vf", "format=rgb24,geq=r='N*8':g=128:b=200"])
        .args(["-c:v", "libx264", "-qp", "0", "-pix_fmt", "yuv444p"])
        .arg(&path)
        .status()
        .expect("ffmpeg encodes the loop fixture");
    assert!(status.success(), "ffmpeg could not encode the loop fixture");
    path
}

/// Read `count` frames from a background video, failing loudly on a stall.
fn read_frames(video: &mut BackgroundVideo, count: usize) -> Vec<Vec<u8>> {
    (0..count)
        .map(|index| {
            video
                .next_frame()
                .unwrap_or_else(|err| panic!("frame {index} never arrived: {err}"))
        })
        .collect()
}

/// A one-second loop under a three-second render must repeat exactly, with no
/// frame duplicated or dropped at the seam.
///
/// `-stream_loop -1` re-decodes the same file, so loop *n* is byte-identical to
/// loop 0 — which makes "seamless" an equality rather than a tolerance. A seam
/// that stuttered by one frame would still be periodic with period 30 at some
/// offset, so the frames of one period are also required to differ from each
/// other: a frozen loop passes periodicity and nothing else.
#[test]
fn a_one_second_loop_repeats_frame_for_frame_under_a_longer_render() {
    let ffmpeg = system_ffmpeg();
    let dir = tempfile::tempdir().expect("tempdir");
    let source = one_second_loop(dir.path(), FPS);

    let mut video =
        BackgroundVideo::start(&ffmpeg, &source, &settings()).expect("the loop starts decoding");
    let frames = read_frames(&mut video, LOOP_FRAMES * 3);

    for index in 0..LOOP_FRAMES * 2 {
        assert_eq!(
            frames[index],
            frames[index + LOOP_FRAMES],
            "frame {index} and frame {} are one loop apart and must be the same picture",
            index + LOOP_FRAMES,
        );
    }
    assert_ne!(
        frames[0], frames[1],
        "a loop of one still frame proves nothing about the seam",
    );
    assert_ne!(
        frames[LOOP_FRAMES - 1],
        frames[LOOP_FRAMES],
        "the seam must advance the picture, not freeze on the last frame",
    );
}

/// Every frame is exactly the frame avz will upload: `width * height * 4` bytes,
/// tightly packed, opaque where the video covers the frame.
#[test]
fn a_frame_is_the_tightly_packed_rgba_the_layer_expects() {
    let ffmpeg = system_ffmpeg();
    let dir = tempfile::tempdir().expect("tempdir");
    let source = one_second_loop(dir.path(), FPS);

    let mut video =
        BackgroundVideo::start(&ffmpeg, &source, &settings()).expect("the loop starts decoding");
    let frame = video.next_frame().expect("the first frame arrives");

    assert_eq!(frame.len(), (WIDTH * HEIGHT * 4) as usize);
    assert!(
        frame.chunks_exact(4).all(|pixel| pixel[3] == 255),
        "`cover` fills the frame, so no pixel of it is transparent",
    );
}

/// ffmpeg converts the frame rate, so a 15 fps loop under a 30 fps render still
/// hands avz one frame per rendered frame — each source frame shown twice.
///
/// Without `-r`, a 15 fps source would starve the render: avz would ask for
/// thirty frames a second from a subprocess producing fifteen, and the whole
/// pipeline would run at half speed behind a decoder that had nothing to say.
#[test]
fn a_slower_source_is_frame_rate_converted_rather_than_starving_the_render() {
    let ffmpeg = system_ffmpeg();
    let dir = tempfile::tempdir().expect("tempdir");
    let source = one_second_loop(dir.path(), 15);

    let mut video =
        BackgroundVideo::start(&ffmpeg, &source, &settings()).expect("the loop starts decoding");
    let frames = read_frames(&mut video, LOOP_FRAMES);

    assert_eq!(frames.len(), LOOP_FRAMES);
    assert_eq!(
        frames[0], frames[1],
        "a 15 fps source shows each of its frames for two frames of a 30 fps render",
    );
    assert_ne!(frames[0], frames[2], "and then it advances");
}

/// `contain` letterboxes, and the bars must be transparent so the palette
/// backdrop shows through them exactly as it does under a `contain` image.
///
/// A source wider than the frame is letterboxed top and bottom, so the first
/// row is a bar and the middle row is video.
#[test]
fn a_contained_video_letterboxes_with_transparent_bars() {
    let ffmpeg = system_ffmpeg();
    let dir = tempfile::tempdir().expect("tempdir");
    // 64x16 into a 64x36 frame: bars above and below.
    let source = dir.path().join("wide.mp4");
    let status = Command::new(DEFAULT_PROGRAM)
        .args(["-y", "-hide_banner", "-loglevel", "error", "-f", "lavfi"])
        .args(["-i", "testsrc2=size=64x16:rate=30:duration=1"])
        .args(["-c:v", "libx264", "-pix_fmt", "yuv420p"])
        .arg(&source)
        .status()
        .expect("ffmpeg encodes the wide fixture");
    assert!(status.success());

    let mut video = BackgroundVideo::start(
        &ffmpeg,
        &source,
        &VideoSettings {
            fit: Fit::Contain,
            ..settings()
        },
    )
    .expect("the loop starts decoding");
    let frame = video.next_frame().expect("the first frame arrives");

    let alpha_at = |x: u32, y: u32| frame[((y * WIDTH + x) * 4 + 3) as usize];
    assert_eq!(alpha_at(WIDTH / 2, 0), 0, "the top bar is transparent");
    assert_eq!(
        alpha_at(WIDTH / 2, HEIGHT - 1),
        0,
        "the bottom bar is transparent"
    );
    assert_eq!(
        alpha_at(WIDTH / 2, HEIGHT / 2),
        255,
        "the video itself is opaque"
    );
}

/// A path that names no video is the user's argument, and ffmpeg's failure to
/// open it must reach them as an input problem rather than a hang.
#[test]
fn a_source_ffmpeg_cannot_open_fails_with_its_own_complaint() {
    let ffmpeg = system_ffmpeg();
    let dir = tempfile::tempdir().expect("tempdir");
    let absent = dir.path().join("nothing.mp4");

    let mut video =
        BackgroundVideo::start(&ffmpeg, &absent, &settings()).expect("spawning ffmpeg succeeds");
    let err = video
        .next_frame()
        .expect_err("there is no video to decode a frame from");

    let msg = err.to_string();
    assert!(msg.contains("nothing.mp4"), "name the video: {msg}");
}
