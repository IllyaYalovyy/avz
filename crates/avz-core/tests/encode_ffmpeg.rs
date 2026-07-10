//! The ffmpeg encoder subprocess, against real processes.
//!
//! Three kinds of test live here. The atomic-output and mid-render-death tests
//! drive a shell stand-in for ffmpeg, because a real encoder cannot be made to
//! die on cue. The mux test drives the real system ffmpeg and compares the audio
//! bitstream it wrote against the source mp3, which is what proves `-c:a copy`
//! was not quietly swapped for a re-encode (`docs/TESTING.md`). The codec-matrix
//! tests drive the real encoders, and skip the ones this ffmpeg was not built
//! with — a Fedora `ffmpeg-free` has none of x264 or x265.

#![cfg(unix)]

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use avz_core::config::Codec;
use avz_core::encode::{
    DEFAULT_PROGRAM, EncodeSettings, Encoder, Ffmpeg, encoders, ensure_encoder, preflight,
    video_encoder,
};

/// Small, even, and cheap to encode. 320×180×4 B = 230 400 B per frame, which
/// is comfortably larger than a pipe buffer — so a write to a dead ffmpeg is a
/// broken pipe rather than a write that vanishes into the kernel.
const WIDTH: u32 = 320;
const HEIGHT: u32 = 180;
const FPS: u32 = 30;

fn settings() -> EncodeSettings {
    EncodeSettings {
        width: WIDTH,
        height: HEIGHT,
        fps: FPS,
        codec: Codec::X264,
        quality: 30,
        audio_start: Duration::ZERO,
    }
}

/// A frame of one repeated byte, so a round trip through a pipe is checkable.
fn frame(fill: u8) -> Vec<u8> {
    vec![fill; (WIDTH * HEIGHT * 4) as usize]
}

/// The mp3 every encode test muxes.
fn fixture_mp3() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/fixtures/tone-tagged.mp3")
        .canonicalize()
        .expect("the CC0 fixture is committed at assets/fixtures/tone-tagged.mp3")
}

fn system_ffmpeg() -> Ffmpeg {
    preflight(DEFAULT_PROGRAM)
        .expect("the encode tests need the system ffmpeg: `sudo dnf install ffmpeg`")
}

/// Write an executable stand-in for ffmpeg.
///
/// It answers `-version` like the real thing so [`preflight`] accepts it, and
/// otherwise runs `body` with `$part` bound to its last argument — the
/// `out.mp4.part` path avz told it to write.
fn fake_ffmpeg(dir: &Path, body: &str) -> Ffmpeg {
    let path = dir.join("ffmpeg");
    let script = format!(
        "#!/bin/sh
if [ \"$1\" = '-version' ]; then
    echo 'ffmpeg version 7.1.5 Copyright (c) 2000-2026'
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

/// Wait out `ETXTBSY` on the script we just wrote: a sibling test thread that
/// forked inside `fs::write`'s open window handed its child an inherited
/// descriptor, and Linux refuses to `exec` a file still open for writing. The
/// child drops it on its own `exec` microseconds later. See the same helper in
/// `encode/preflight.rs`.
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

/// Block until `path` exists, so a test never races the subprocess that makes it.
fn wait_for(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if path.exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("{} never appeared", path.display());
}

/// `out.mp4` and the `out.mp4.part` ffmpeg writes before the rename.
fn output_paths(dir: &Path) -> (PathBuf, PathBuf) {
    let output = dir.join("out.mp4");
    let part = dir.join("out.mp4.part");
    (output, part)
}

#[test]
fn a_frame_of_the_wrong_size_is_an_encode_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ffmpeg = fake_ffmpeg(dir.path(), "cat > \"$part\"");
    let (output, _) = output_paths(dir.path());

    let mut encoder =
        Encoder::start(&ffmpeg, &settings(), &fixture_mp3(), &output).expect("ffmpeg starts");

    assert_eq!(encoder.frame_bytes(), (WIDTH * HEIGHT * 4) as usize);

    let err = encoder
        .write_frame(&[0; 16])
        .expect_err("a 16-byte frame is not a 320x180 frame");

    let msg = err.to_string();
    assert!(msg.contains("230400"), "message must name the size: {msg}");
}

/// The output must not exist until ffmpeg has flushed and exited cleanly. Until
/// then the bytes live in `out.mp4.part`, which is what makes a killed render
/// leave nothing behind that looks playable.
#[test]
fn the_output_appears_only_after_a_successful_finish() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ffmpeg = fake_ffmpeg(dir.path(), "cat > \"$part\"");
    let (output, part) = output_paths(dir.path());

    let mut encoder =
        Encoder::start(&ffmpeg, &settings(), &fixture_mp3(), &output).expect("ffmpeg starts");
    encoder.write_frame(&frame(0x11)).expect("first frame");
    wait_for(&part);

    assert!(
        !output.exists(),
        "the final path must stay absent while frames are still arriving"
    );

    encoder.write_frame(&frame(0x22)).expect("second frame");
    encoder
        .finish()
        .expect("a clean ffmpeg exit renames the part file");

    assert!(!part.exists(), "the part file is renamed, not left behind");
    let written = fs::read(&output).expect("the output exists after finish");
    let mut expected = frame(0x11);
    expected.extend(frame(0x22));
    assert_eq!(
        written, expected,
        "ffmpeg received exactly the frames avz wrote"
    );
}

/// The invariant from `AGENTS.md`: never leave a half-written file. A `.part`
/// that ffmpeg already began writing must be removed, not renamed and not kept.
#[test]
fn ffmpeg_death_midrender_leaves_no_output_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ffmpeg = fake_ffmpeg(
        dir.path(),
        "printf 'half a container' > \"$part\"
echo 'x264: encoder is on fire' >&2
exit 1",
    );
    let (output, part) = output_paths(dir.path());

    let mut encoder =
        Encoder::start(&ffmpeg, &settings(), &fixture_mp3(), &output).expect("ffmpeg starts");

    // Whichever call notices first — the write that hits a closed pipe, or the
    // wait inside `finish` — must report the death rather than swallow it.
    let mut err = None;
    for fill in 0..8 {
        if let Err(failure) = encoder.write_frame(&frame(fill)) {
            err = Some(failure);
            break;
        }
    }
    let err = match err {
        Some(err) => err,
        None => encoder
            .finish()
            .expect_err("an ffmpeg that exited 1 did not produce a video"),
    };

    let msg = err.to_string();
    assert!(
        msg.contains("encoder is on fire"),
        "ffmpeg's own complaint must reach the user: {msg}"
    );
    assert!(!output.exists(), "no half-written output may survive");
    assert!(!part.exists(), "no half-written part file may survive");
}

/// ffmpeg can succeed and the rename still fail — a directory in the way, a
/// read-only parent. The part file is not a consolation prize.
#[test]
fn a_render_that_cannot_be_moved_into_place_leaves_no_part_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ffmpeg = fake_ffmpeg(dir.path(), "cat > \"$part\"");
    let (output, part) = output_paths(dir.path());
    fs::create_dir(&output).expect("something else already owns the output path");

    let mut encoder =
        Encoder::start(&ffmpeg, &settings(), &fixture_mp3(), &output).expect("ffmpeg starts");
    encoder.write_frame(&frame(0x33)).expect("a frame");

    let err = encoder
        .finish()
        .expect_err("a file cannot be renamed over a directory");

    let msg = err.to_string();
    assert!(
        msg.contains("into place"),
        "the message must say what failed: {msg}"
    );
    assert!(
        !part.exists(),
        "a part file that cannot be moved is removed"
    );
}

/// A render abandoned by panic or `?` must not litter either. `Encoder` cleans
/// up when it is dropped without a `finish`.
#[test]
fn a_dropped_encoder_kills_ffmpeg_and_removes_the_part_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    // Creates the part file, then never reads stdin and never exits.
    let ffmpeg = fake_ffmpeg(dir.path(), ": > \"$part\"\nexec sleep 300");
    let (output, part) = output_paths(dir.path());

    let encoder =
        Encoder::start(&ffmpeg, &settings(), &fixture_mp3(), &output).expect("ffmpeg starts");
    wait_for(&part);
    drop(encoder);

    assert!(!part.exists(), "a dropped encoder removes its part file");
    assert!(!output.exists(), "a dropped encoder produces no output");
}

/// The one test that proves the audio promise, against the real encoder.
///
/// `ffprobe` alone is not enough: re-encoding an mp3 with `libmp3lame` still
/// reports `codec_name=mp3`, so a codec assertion would pass through a
/// generation of quality loss. The bitstream is what tells the truth — a copied
/// stream is byte-for-byte the head of the original, and `-shortest` is why it
/// is a prefix rather than the whole thing (`VISION.md` §5.4).
#[test]
fn muxed_audio_stream_is_copied_not_reencoded() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (output, part) = output_paths(dir.path());
    let song = fixture_mp3();

    let mut encoder = Encoder::start(&system_ffmpeg(), &settings(), &song, &output)
        .expect("the system ffmpeg starts");
    for index in 0..FPS as u8 {
        encoder
            .write_frame(&frame(index.wrapping_mul(8)))
            .expect("a frame reaches ffmpeg");
    }
    encoder
        .finish()
        .expect("the system ffmpeg encodes a second of video");

    assert!(!part.exists(), "the part file is renamed on success");
    assert!(output.exists(), "the render produced an mp4");

    assert_eq!(
        probe(&output, "a", "codec_name"),
        "mp3",
        "the codec changed"
    );
    assert_eq!(probe(&output, "v", "codec_name"), "h264");
    assert_eq!(
        probe(&output, "a", "index").lines().count(),
        1,
        "exactly one audio stream"
    );
    assert_eq!(
        probe(&output, "v", "index").lines().count(),
        1,
        "exactly one video stream"
    );

    let muxed = audio_bitstream(&output);
    let original = audio_bitstream(&song);
    assert!(!muxed.is_empty(), "the mp4 carries audio at all");
    assert!(
        original.starts_with(&muxed),
        "the muxed audio is not the original bitstream: {} of {} bytes were re-encoded",
        muxed.len(),
        original.len(),
    );
}

// ---------------------------------------------------------------------------
// The codec matrix
// ---------------------------------------------------------------------------

/// The mp4 stream name `ffprobe` reports for what each codec produces.
///
/// avz's spelling is the ffmpeg *encoder*'s; ffprobe reports the *codec* the
/// bitstream is. `libx264` writes h264, and only a decode can tell.
fn probed_codec_name(codec: Codec) -> &'static str {
    match codec {
        Codec::X264 => "h264",
        Codec::X265 => "hevc",
        Codec::Av1 => "av1",
    }
}

/// Encode a second of video with every codec the system ffmpeg has, and skip the
/// ones it does not: `libx264` and `libx265` are absent from Fedora's stock
/// `ffmpeg-free`, and a CI box without them must still be able to run the suite.
///
/// The video codec is what this asserts; the audio stays a copied mp3 whichever
/// encoder drew the pictures, which is the invariant the matrix must not break.
#[test]
fn every_available_codec_encodes_a_playable_stream_and_still_copies_the_audio() {
    let ffmpeg = system_ffmpeg();
    let available = encoders(&ffmpeg).expect("the system ffmpeg lists its encoders");
    let mut encoded = 0;

    for codec in [Codec::X264, Codec::X265, Codec::Av1] {
        if !available.contains(video_encoder(codec)) {
            eprintln!(
                "skipping {}: this ffmpeg has no `{}` encoder",
                codec.as_str(),
                video_encoder(codec),
            );
            continue;
        }

        let dir = tempfile::tempdir().expect("tempdir");
        let (output, part) = output_paths(dir.path());

        let settings = EncodeSettings {
            codec,
            ..settings()
        };
        let mut encoder = Encoder::start(&ffmpeg, &settings, &fixture_mp3(), &output)
            .unwrap_or_else(|err| panic!("{} starts: {err}", codec.as_str()));
        for index in 0..FPS as u8 {
            encoder
                .write_frame(&frame(index.wrapping_mul(8)))
                .expect("a frame reaches ffmpeg");
        }
        encoder
            .finish()
            .unwrap_or_else(|err| panic!("{} encodes a second of video: {err}", codec.as_str()));

        assert!(!part.exists(), "the part file is renamed on success");
        assert_eq!(
            probe(&output, "v", "codec_name"),
            probed_codec_name(codec),
            "{} wrote the wrong video codec",
            codec.as_str(),
        );
        assert_eq!(
            probe(&output, "a", "codec_name"),
            "mp3",
            "{} re-encoded the audio",
            codec.as_str(),
        );
        encoded += 1;
    }

    assert!(
        encoded > 0,
        "the system ffmpeg has none of avz's encoders; the matrix is untested"
    );
}

/// A codec this ffmpeg cannot encode is the user's configuration, not a render
/// failure: exit 2, before the song is decoded (`VISION.md` §8). The message has
/// to name the encoder, because `libx265` is the string the user must go and
/// install — `x265` will find them nothing.
#[test]
fn a_codec_the_ffmpeg_was_not_built_with_is_refused_by_name() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ffmpeg = fake_ffmpeg(
        dir.path(),
        "if [ \"$2\" = '-encoders' ]; then\n\
             echo ' V....D libx264              libx264 H.264 (codec h264)'\n\
             exit 0\n\
         fi\n",
    );

    ensure_encoder(&ffmpeg, Codec::X264).expect("an ffmpeg with libx264 encodes x264");

    for codec in [Codec::X265, Codec::Av1] {
        let err = ensure_encoder(&ffmpeg, codec).expect_err("this ffmpeg has only libx264");

        assert!(
            matches!(err, avz_core::Error::Config(_)),
            "a codec the user typed is configuration: {err:?}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains(codec.as_str()),
            "name the codec asked for: {msg}"
        );
        assert!(
            msg.contains(video_encoder(codec)),
            "name the encoder it needs: {msg}"
        );
        assert!(msg.contains("RPM Fusion"), "say where to get it: {msg}");
        assert!(
            msg.contains("--codec x264"),
            "name a codec that works: {msg}"
        );
    }
}

/// The system ffmpeg is the one avz will encode with, so the codec it claims and
/// the codec it has must agree — otherwise `ensure_encoder` is a check that
/// passes for a codec `Encoder::start` then fails on.
#[test]
fn the_system_ffmpeg_agrees_with_the_encoder_check() {
    let ffmpeg = system_ffmpeg();
    let available = encoders(&ffmpeg).expect("the system ffmpeg lists its encoders");

    for codec in [Codec::X264, Codec::X265, Codec::Av1] {
        assert_eq!(
            ensure_encoder(&ffmpeg, codec).is_ok(),
            available.contains(video_encoder(codec)),
            "{} disagrees with `ffmpeg -encoders`",
            codec.as_str(),
        );
    }
}

/// The raw audio packet payloads of `file`, with no container around them.
///
/// `-c copy -f data` writes exactly the bytes the demuxer read, which is what
/// makes two streams comparable across an mp3 file and an mp4 container.
fn audio_bitstream(file: &Path) -> Vec<u8> {
    let output = Command::new(DEFAULT_PROGRAM)
        .args(["-v", "error", "-i"])
        .arg(file)
        .args(["-map", "0:a", "-c", "copy", "-f", "data", "-"])
        .output()
        .expect("the encode tests need the system ffmpeg");

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
        .expect("the encode tests need ffprobe: `sudo dnf install ffmpeg`");

    assert!(
        output.status.success(),
        "ffprobe failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}
