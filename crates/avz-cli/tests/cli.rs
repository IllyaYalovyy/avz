//! CLI surface contract: subcommand discovery, usage errors, and exit codes.
//!
//! Exit codes are fixed by VISION.md §8: 0 ok, 2 bad args/config, 3 input file
//! problems, 4 render/encode failure.

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

/// A committed CC0 fixture. See `assets/fixtures/README.md`.
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/fixtures")
        .join(name)
}

/// A `PATH` holding nothing at all, so `ffmpeg` cannot be found on it.
///
/// Tests of the missing-ffmpeg path must not depend on whether the developer's
/// machine happens to have ffmpeg installed.
fn path_without_ffmpeg() -> TempDir {
    tempfile::tempdir().expect("tempdir")
}

/// A `PATH` holding a stand-in that answers `-version` the way ffmpeg does.
///
/// Lets tests reach the code *behind* the preflight gate without a real encoder.
fn path_with_fake_ffmpeg() -> TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let ffmpeg = dir.path().join("ffmpeg");
    fs::write(
        &ffmpeg,
        "#!/bin/sh\necho 'ffmpeg version 7.1.5 Copyright (c) 2000-2026'\n",
    )
    .expect("write fake ffmpeg");
    fs::set_permissions(&ffmpeg, fs::Permissions::from_mode(0o755)).expect("chmod");
    dir
}

#[test]
fn help_lists_all_subcommands() {
    avz()
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("render"))
        .stdout(contains("probe"))
        .stdout(contains("presets"))
        .stdout(contains("config"));
}

#[test]
fn render_without_args_exits_2() {
    avz().arg("render").assert().code(2);
}

#[test]
fn render_stub_exits_4_with_polite_message() {
    let path = path_with_fake_ffmpeg();

    avz()
        .env("PATH", path.path())
        .args(["render", "x.mp3"])
        .assert()
        .code(4)
        .stderr(contains("not implemented"));
}

#[test]
fn render_without_ffmpeg_fails_with_the_fedora_install_hint() {
    let path = path_without_ffmpeg();

    avz()
        .env("PATH", path.path())
        .args(["render", "x.mp3"])
        .assert()
        .code(4)
        .stderr(contains("ffmpeg not found"))
        .stderr(contains("sudo dnf install ffmpeg"));
}

/// The preflight exists to fail *early*. If it ever ran after analysis or
/// rendering, this is the assertion that would notice: the stub render never
/// gets to say "not implemented".
#[test]
fn render_checks_for_ffmpeg_before_doing_any_work() {
    let path = path_without_ffmpeg();

    avz()
        .env("PATH", path.path())
        .args(["render", "x.mp3"])
        .assert()
        .code(4)
        .stderr(contains("not implemented").not());
}

/// `probe` reads tags; it never encodes. Gating it on ffmpeg would be a lie.
#[test]
fn probe_does_not_require_ffmpeg() {
    let path = path_without_ffmpeg();

    avz()
        .env("PATH", path.path())
        .arg("probe")
        .arg(fixture("tone-tagged.mp3"))
        .assert()
        .success();
}

/// UT-005: title, artist, album, duration, sample rate, and whether cover art is
/// embedded — with its mime type and dimensions.
#[test]
fn probe_prints_tags_duration_and_cover_art() {
    avz()
        .arg("probe")
        .arg(fixture("tone-tagged.mp3"))
        .assert()
        .success()
        .stdout(contains("Sine Tones"))
        .stdout(contains("avz test fixture"))
        .stdout(contains("Public Domain Tones"))
        .stdout(contains("0:05.04"))
        .stdout(contains("44100 Hz"))
        .stdout(contains("2 (stereo)"))
        .stdout(contains("image/png"))
        .stdout(contains("256x256"));
}

/// UT-005: "Missing tags are reported as missing, not as an error."
#[test]
fn probe_reports_missing_tags_as_missing_rather_than_failing() {
    avz()
        .arg("probe")
        .arg(fixture("tone-untagged.mp3"))
        .assert()
        .success()
        .stdout(contains("title").and(contains("(missing)")))
        .stdout(contains("cover art").and(contains("(none)")))
        // The audio is still described even with no tag to be found.
        .stdout(contains("44100 Hz"));
}

/// An input-file problem is exit code 3, not a panic and not a render failure.
#[test]
fn probe_of_a_missing_file_exits_3() {
    avz()
        .args(["probe", "no-such-song.mp3"])
        .assert()
        .code(3)
        .stderr(contains("no such file"));
}

#[test]
fn probe_of_a_file_that_is_not_audio_exits_3() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("not-audio.mp3");
    fs::write(&path, b"this is not an mp3").expect("write");

    avz()
        .arg("probe")
        .arg(&path)
        .assert()
        .code(3)
        .stderr(contains("not a recognized audio file"));
}

#[test]
fn quiet_and_verbose_conflict_is_rejected() {
    avz()
        .args(["--quiet", "--verbose", "probe", "x.mp3"])
        .assert()
        .code(2);
}
