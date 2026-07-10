//! The `VISION.md` §8 exit-code contract, in one place.
//!
//! ```text
//! 0  ok
//! 2  bad arguments or configuration
//! 3  the input file is missing, unreadable, or the wrong format
//! 4  rendering or encoding failed
//! ```
//!
//! Scripts depend on these numbers — `for f in album/*.mp3; do avz render "$f" ||
//! break; done` is the batch story `VISION.md` §3 ships instead of a `batch`
//! subcommand, and it can only tell "this song has no tags" from "the disk is
//! full" by the code. So every class of failure gets a row here, driven through
//! the assembled binary the way a shell drives it.
//!
//! Individual failures are asserted in detail elsewhere (`cli.rs` for the
//! messages, `avz-core`'s tests for the seams). What this file adds is
//! *coverage of the classification*: a new `Error` variant that lands in the
//! wrong bucket fails here, and `crates/avz-cli/src/exit.rs` is the one function
//! it fails against.
//!
//! Alongside the code, each row asserts the error text is a sentence and not an
//! `io::Error` that escaped: the message must name the thing the user gave avz.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt as _;
use predicates::str::contains;
use tempfile::TempDir;

fn avz() -> Command {
    Command::cargo_bin("avz").expect("avz binary builds")
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/fixtures")
        .join(name)
}

/// Write `body` as an executable `ffmpeg` on a `PATH` of its own.
fn fake_ffmpeg(body: &str) -> TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let ffmpeg = dir.path().join("ffmpeg");
    fs::write(&ffmpeg, body).expect("write the ffmpeg stand-in");
    fs::set_permissions(&ffmpeg, fs::Permissions::from_mode(0o755)).expect("chmod");
    dir
}

/// An ffmpeg that passes the preflight and encodes nothing.
fn ffmpeg_that_passes_preflight() -> TempDir {
    fake_ffmpeg("#!/bin/sh\necho 'ffmpeg version 7.1.5 Copyright (c) 2000-2026'\n")
}

/// An ffmpeg that passes the preflight and then dies on the first frame, the way
/// a real one does when the disk fills or the codec is missing.
fn ffmpeg_that_dies_midrender() -> TempDir {
    fake_ffmpeg(
        "#!/bin/sh\n\
         case \"$1\" in\n\
         -version) echo 'ffmpeg version 7.1.5 Copyright (c) 2000-2026'; exit 0;;\n\
         esac\n\
         echo 'Conversion failed: no space left on device' >&2\n\
         exit 1\n",
    )
}

/// An empty `PATH`, so `ffmpeg` cannot be found on it.
fn no_ffmpeg() -> TempDir {
    tempfile::tempdir().expect("tempdir")
}

// ---------------------------------------------------------------------------
// 0 — ok
// ---------------------------------------------------------------------------

/// The zero-config happy path exits 0. Nothing else in this file means anything
/// if success is not distinguishable from failure.
#[test]
fn a_render_that_succeeds_exits_0() {
    let dir = tempfile::tempdir().expect("tempdir");
    let song = dir.path().join("song.mp3");
    fs::copy(fixture("tone-tagged.mp3"), &song).expect("copy the fixture");

    avz()
        .arg("render")
        .arg(&song)
        .args(["--sample", "100ms", "--adapter", "software"])
        .assert()
        .code(0);
}

/// `--help` is a request, not a failure, however clap chooses to signal it.
#[test]
fn help_and_version_exit_0() {
    avz().arg("--help").assert().code(0);
    avz().arg("--version").assert().code(0);
    avz().args(["render", "--help"]).assert().code(0);
}

// ---------------------------------------------------------------------------
// 2 — bad arguments or configuration
// ---------------------------------------------------------------------------

/// A flag that does not exist never reaches avz's own code: clap rejects it, and
/// `main` must not mistake that for a render failure.
#[test]
fn an_unknown_flag_exits_2() {
    avz()
        .args(["render", "song.mp3", "--nope"])
        .assert()
        .code(2)
        .stderr(contains("--nope"));
}

#[test]
fn a_missing_required_argument_exits_2() {
    avz().arg("render").assert().code(2);
}

#[test]
fn an_unknown_subcommand_exits_2() {
    avz().arg("renderr").assert().code(2);
}

/// An unknown key in a `--config` file is the user's configuration: exit 2, and
/// name the key they probably meant.
#[test]
fn an_unknown_config_key_exits_2_and_suggests_a_real_one() {
    let ffmpeg = ffmpeg_that_passes_preflight();
    let dir = tempfile::tempdir().expect("tempdir");
    let config = dir.path().join("base.toml");
    fs::write(&config, "[output]\nfpss = 30\n").expect("write");

    avz()
        .env("PATH", ffmpeg.path())
        .arg("render")
        .arg(fixture("tone-tagged.mp3"))
        .arg("--config")
        .arg(&config)
        .arg("--out")
        .arg(dir.path().join("out.mp4"))
        .assert()
        .code(2)
        .stderr(contains("fpss"))
        .stderr(contains("fps"));
}

/// A `--config` file that does not parse is configuration, not input: the song
/// is fine, the TOML is not.
#[test]
fn a_malformed_config_file_exits_2() {
    let ffmpeg = ffmpeg_that_passes_preflight();
    let dir = tempfile::tempdir().expect("tempdir");
    let config = dir.path().join("broken.toml");
    fs::write(&config, "[output\nfps = 30\n").expect("write");

    avz()
        .env("PATH", ffmpeg.path())
        .arg("render")
        .arg(fixture("tone-tagged.mp3"))
        .arg("--config")
        .arg(&config)
        .assert()
        .code(2);
}

/// A `--config` file that is not there is a *configuration* problem, not an
/// input-file one: exit 3 is reserved for the song, so a batch loop can tell
/// "skip this broken song" from "every song will fail". The reason is a sentence,
/// never the errno the operating system handed back.
#[test]
fn a_missing_config_file_exits_2_and_names_the_path() {
    let ffmpeg = ffmpeg_that_passes_preflight();

    avz()
        .env("PATH", ffmpeg.path())
        .arg("render")
        .arg(fixture("tone-tagged.mp3"))
        .args(["--config", "no-such-config.toml"])
        .assert()
        .code(2)
        .stderr(contains("no-such-config.toml"))
        .stderr(contains("no such file"))
        .stderr(contains("os error").not());
}

/// A sample the song cannot satisfy is an argument, not a broken file.
#[test]
fn a_sample_past_the_end_of_the_song_exits_2() {
    avz()
        .arg("render")
        .arg(fixture("tone-tagged.mp3"))
        .args(["--sample", "6s..8s"])
        .assert()
        .code(2)
        .stderr(contains("the song is only 5"));
}

/// `background.video` parses, validates, and cannot be drawn (RFC-001 NG2). It
/// is refused as configuration rather than silently ignored.
#[test]
fn a_background_video_exits_2_and_says_it_is_not_built_yet() {
    let ffmpeg = ffmpeg_that_passes_preflight();
    let dir = tempfile::tempdir().expect("tempdir");
    let config = dir.path().join("video.toml");
    fs::write(&config, "[background]\nvideo = \"loops/smoke.mp4\"\n").expect("write");

    avz()
        .env("PATH", ffmpeg.path())
        .arg("render")
        .arg(fixture("tone-tagged.mp3"))
        .arg("--config")
        .arg(&config)
        .arg("--out")
        .arg(dir.path().join("out.mp4"))
        .assert()
        .code(2)
        .stderr(contains("not supported yet"));
}

// ---------------------------------------------------------------------------
// 3 — the input file
// ---------------------------------------------------------------------------

#[test]
fn a_missing_song_exits_3_and_names_it() {
    let ffmpeg = ffmpeg_that_passes_preflight();

    avz()
        .env("PATH", ffmpeg.path())
        .args(["render", "no-such-song.mp3"])
        .assert()
        .code(3)
        .stderr(contains("no-such-song.mp3"))
        .stderr(contains("no such file"));
}

/// Bytes that are not an mp3 are an input problem, and the message says which
/// file — never a bare `io::Error` or a symphonia enum.
#[test]
fn a_song_that_is_not_an_mp3_exits_3() {
    let ffmpeg = ffmpeg_that_passes_preflight();
    let dir = tempfile::tempdir().expect("tempdir");
    let song = dir.path().join("song.mp3");
    fs::write(&song, b"this is not an mp3").expect("write");

    avz()
        .env("PATH", ffmpeg.path())
        .arg("render")
        .arg(&song)
        .assert()
        .code(3)
        .stderr(contains("song.mp3"));
}

/// An mp3 that ends mid-stream is the file's problem, not avz's: exit 3, naming
/// the file, in a sentence. A thousand bytes is the ID3 tag and a hair of audio
/// — the same slice `truncated_mp3_yields_input_error_not_panic` uses in
/// `avz-core`.
///
/// Which sentence depends on who notices first. `render` reads the tags for the
/// text card before it decodes, so lofty usually speaks before symphonia can say
/// "truncated"; that exact wording is pinned in `analysis/decode.rs`, where the
/// decoder is reached directly. What this row owns is the classification, and
/// that the errno never surfaces.
#[test]
fn a_truncated_mp3_exits_3() {
    let ffmpeg = ffmpeg_that_passes_preflight();
    let dir = tempfile::tempdir().expect("tempdir");
    // Not named "truncated": the message assertion must not pass on the file
    // name alone.
    let song = dir.path().join("half-a-song.mp3");
    let whole = fs::read(fixture("tone-tagged.mp3")).expect("read the fixture");
    fs::write(&song, &whole[..1000]).expect("write");

    avz()
        .env("PATH", ffmpeg.path())
        .arg("render")
        .arg(&song)
        .assert()
        .code(3)
        .stderr(contains("half-a-song.mp3"))
        .stderr(contains("os error").not());
}

/// A `--bg` that names nothing is the *background* input file, and it is still
/// an input problem.
#[test]
fn a_missing_background_image_exits_3_and_names_it() {
    let ffmpeg = ffmpeg_that_passes_preflight();
    let dir = tempfile::tempdir().expect("tempdir");

    avz()
        .env("PATH", ffmpeg.path())
        .arg("render")
        .arg(fixture("tone-tagged.mp3"))
        .arg("--out")
        .arg(dir.path().join("out.mp4"))
        .args(["--bg", "art/forest.png"])
        .assert()
        .code(3)
        .stderr(contains("forest.png"));
}

#[test]
fn a_probe_of_a_missing_song_exits_3() {
    avz()
        .args(["probe", "no-such-song.mp3"])
        .assert()
        .code(3)
        .stderr(contains("no such file"));
}

// ---------------------------------------------------------------------------
// 4 — rendering or encoding failed
// ---------------------------------------------------------------------------

/// No ffmpeg at all: not the user's arguments, not their song. Exit 4, with the
/// Fedora install hint (`VISION.md` §5.4).
#[test]
fn a_missing_ffmpeg_exits_4_with_the_install_hint() {
    let path = no_ffmpeg();

    avz()
        .env("PATH", path.path())
        .args(["render", "song.mp3"])
        .assert()
        .code(4)
        .stderr(contains("ffmpeg not found"))
        .stderr(contains("sudo dnf install ffmpeg"));
}

/// ffmpeg dies with the frames half-written. Exit 4, ffmpeg's own last words
/// reach the user through the context chain, and no `.part` file survives.
#[test]
fn an_ffmpeg_that_dies_midrender_exits_4_and_reports_its_last_words() {
    let ffmpeg = ffmpeg_that_dies_midrender();
    let dir = tempfile::tempdir().expect("tempdir");
    let out = dir.path().join("out.mp4");

    avz()
        .env("PATH", ffmpeg.path())
        .arg("render")
        .arg(fixture("tone-tagged.mp3"))
        .arg("--out")
        .arg(&out)
        .args(["--sample", "100ms", "--adapter", "software"])
        .assert()
        .code(4)
        .stderr(contains("encode failed"))
        .stderr(contains("no space left on device"));

    assert!(!out.exists(), "a dead encoder left an mp4 behind");
    assert!(
        !dir.path().join("out.mp4.part").exists(),
        "a dead encoder left a part file behind",
    );
}

/// A binary on `PATH` called `ffmpeg` that is not ffmpeg fails the preflight
/// rather than the render, but it is still a pipeline problem: exit 4.
#[test]
fn an_ffmpeg_that_is_not_ffmpeg_exits_4() {
    let ffmpeg = fake_ffmpeg("#!/bin/sh\necho 'this is not an encoder'\n");

    avz()
        .env("PATH", ffmpeg.path())
        .args(["render", "song.mp3"])
        .assert()
        .code(4);
}

/// A command avz accepts and has not built yet is a pipeline failure, not a
/// usage error: the arguments were fine (`VISION.md` §9, M0).
#[test]
fn an_unimplemented_command_exits_4() {
    avz()
        .args(["config", "--example"])
        .assert()
        .code(4)
        .stderr(contains("not implemented"));
}

// ---------------------------------------------------------------------------
// The shape of every error message
// ---------------------------------------------------------------------------

/// Whatever went wrong, the first line starts with `error:` and names the thing
/// the user handed avz. A bare `io::Error` — "No such file or directory (os
/// error 2)" with nothing around it — tells them nothing about which file.
#[test]
fn every_failure_prefixes_its_message_and_names_what_the_user_gave_avz() {
    let ffmpeg = ffmpeg_that_passes_preflight();

    let cases: [(&[&str], &str); 3] = [
        (&["render", "no-such-song.mp3"], "no-such-song.mp3"),
        (&["probe", "no-such-song.mp3"], "no-such-song.mp3"),
        (&["presets", "pulze"], "pulze"),
    ];

    for (args, needle) in cases {
        avz()
            .env("PATH", ffmpeg.path())
            .args(args)
            .assert()
            .failure()
            .stderr(contains("error:"))
            .stderr(contains(needle));
    }
}
