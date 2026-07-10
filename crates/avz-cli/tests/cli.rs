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

/// A missing song is an input problem (exit 3), reached only once ffmpeg has
/// been found.
#[test]
fn render_of_a_missing_file_exits_3() {
    let path = path_with_fake_ffmpeg();

    avz()
        .env("PATH", path.path())
        .args(["render", "no-such-song.mp3"])
        .assert()
        .code(3)
        .stderr(contains("no such file"));
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

/// The preflight exists to fail *early*. If it ever ran after the input was
/// opened, this is the assertion that would notice: `x.mp3` does not exist, and
/// avz must complain about ffmpeg rather than about the file.
#[test]
fn render_checks_for_ffmpeg_before_doing_any_work() {
    let path = path_without_ffmpeg();

    avz()
        .env("PATH", path.path())
        .args(["render", "x.mp3"])
        .assert()
        .code(4)
        .stderr(contains("no such file").not());
}

/// A backwards `--sample` never reaches the decoder: exit 2, bad arguments.
#[test]
fn render_with_a_backwards_sample_range_exits_2() {
    avz()
        .args(["render", "song.mp3", "--sample", "3s..1s"])
        .assert()
        .code(2)
        .stderr(contains("the end must come after the start"));
}

/// The song is five seconds long, so there is nothing at six. That is the user's
/// argument, not a render failure: exit 2.
#[test]
fn render_of_a_sample_past_the_end_of_the_song_exits_2() {
    avz()
        .arg("render")
        .arg(fixture("tone-tagged.mp3"))
        .args(["--sample", "6s..8s"])
        .assert()
        .code(2)
        .stderr(contains("the song is only 5"));
}

/// The encoder renames its part file over the output path, so an `--out` aimed
/// at the input would destroy the song.
#[test]
fn render_refuses_to_write_over_its_own_input() {
    avz()
        .arg("render")
        .arg(fixture("tone-tagged.mp3"))
        .arg("--out")
        .arg(fixture("tone-tagged.mp3"))
        .assert()
        .code(2)
        .stderr(contains("the song avz is reading"));
}

/// UT-001 and UT-002 at the CLI: a sampled render of a real mp3 produces a
/// playable mp4 next to the input, with both streams and the sampled duration.
///
/// Software adapter, because a golden-ish assertion must not depend on the
/// developer's GPU. Needs the system ffmpeg and Mesa lavapipe.
#[test]
fn render_writes_a_sampled_mp4_next_to_the_input() {
    let dir = tempfile::tempdir().expect("tempdir");
    let song = dir.path().join("song.mp3");
    fs::copy(fixture("tone-tagged.mp3"), &song).expect("copy the fixture");

    avz()
        .arg("render")
        .arg(&song)
        .args(["--sample", "200ms", "--adapter", "software"])
        .assert()
        .success()
        .stdout(contains("song.mp4"))
        .stdout(contains("6 frames"))
        .stdout(contains("software rendering"));

    let output = dir.path().join("song.mp4");
    assert!(output.is_file(), "the render produced no mp4");
    assert!(
        !dir.path().join("song.mp4.part").exists(),
        "the part file is renamed, not left behind"
    );

    // `--sample` defaults to a reduced resolution (VISION.md §3).
    assert_eq!(probe(&output, "v", "width,height"), "1280,720");
    assert_eq!(probe(&output, "a", "codec_name"), "mp3");
}

/// `--quiet` suppresses everything but errors (`VISION.md` §3).
#[test]
fn a_quiet_render_prints_nothing_on_success() {
    let dir = tempfile::tempdir().expect("tempdir");
    let song = dir.path().join("song.mp3");
    fs::copy(fixture("tone-tagged.mp3"), &song).expect("copy the fixture");

    avz()
        .arg("render")
        .arg(&song)
        .args(["--sample", "100ms", "--adapter", "software", "--quiet"])
        .assert()
        .success()
        .stdout("");

    assert!(dir.path().join("song.mp4").is_file());
}

/// Ask `ffprobe` for one entry of every `kind` (`v` or `a`) stream.
fn probe(file: &Path, kind: &str, entries: &str) -> String {
    let output = std::process::Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", kind])
        .args(["-show_entries", &format!("stream={entries}")])
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

// ---------------------------------------------------------------------------
// `avz presets` and preset parameters (UT-004, UT-007)
// ---------------------------------------------------------------------------

/// UT-004, first half: every preset avz ships, with its one-line description.
#[test]
fn presets_command_lists_all_registered() {
    avz()
        .arg("presets")
        .assert()
        .success()
        .stdout(contains("pulse"))
        .stdout(contains("concentric rings driven by the kick"))
        .stdout(contains("nebula"))
        .stdout(contains("an fbm flow field over feedback trails"));
}

/// `nebula`'s schema reaches the terminal too, perf hint and all: a preset that
/// is registered but whose parameters nobody can discover is undiscoverable.
#[test]
fn presets_nebula_prints_its_schema_and_perf_hint() {
    avz()
        .args(["presets", "nebula"])
        .assert()
        .success()
        .stdout(contains("trail_decay"))
        .stdout(contains(
            "How much of the previous frame survives into this one.",
        ))
        .stdout(contains("octaves"))
        .stdout(contains("burst_strength"))
        .stdout(contains("performance:"));
}

/// UT-004, second half: name, type, default, range, and description, for every
/// parameter the preset declares.
#[test]
fn presets_name_prints_schema_fields() {
    avz()
        .args(["presets", "pulse"])
        .assert()
        .success()
        .stdout(contains("bass_drive"))
        .stdout(contains("float"))
        .stdout(contains("0..4"))
        .stdout(contains("How hard the kick swells the core disc."))
        .stdout(contains("ring_count"))
        .stdout(contains("int"))
        .stdout(contains("flash"))
        .stdout(contains("bool"))
        .stdout(contains("true|false"));
}

/// A typo'd preset name is the user's argument: exit 2, and say what does exist.
#[test]
fn presets_of_an_unknown_preset_exits_2_and_names_the_known_ones() {
    avz()
        .args(["presets", "pulze"])
        .assert()
        .code(2)
        .stderr(contains("pulse"));
}

/// A `--bg` that names nothing is an input problem, not a usage error: exit 3,
/// name the path, and never reach the decoder (`VISION.md` §8).
#[test]
fn render_with_a_missing_background_image_exits_3_and_names_the_path() {
    let path = path_with_fake_ffmpeg();
    let dir = tempfile::tempdir().expect("tempdir");
    let out = dir.path().join("out.mp4");

    avz()
        .env("PATH", path.path())
        .arg("render")
        .arg(fixture("tone-tagged.mp3"))
        .arg("--out")
        .arg(&out)
        .args(["--bg", "art/forest.png"])
        .assert()
        .code(3)
        .stderr(contains("forest.png"));

    assert!(
        !out.exists(),
        "a background image that is not there must not leave a render behind"
    );
}

/// A typo'd `--palette` is the user's argument: exit 2, name the typo, and list
/// every palette that does exist — before the song is decoded.
#[test]
fn render_with_an_unknown_palette_exits_2_and_names_the_known_ones() {
    let path = path_with_fake_ffmpeg();
    let dir = tempfile::tempdir().expect("tempdir");
    let out = dir.path().join("out.mp4");

    avz()
        .env("PATH", path.path())
        .arg("render")
        .arg(fixture("tone-tagged.mp3"))
        .arg("--out")
        .arg(&out)
        .args(["--palette", "embers"])
        .assert()
        .code(2)
        .stderr(contains("unknown palette `embers`"))
        .stderr(contains("glacier"))
        .stderr(contains("carpathian"));

    assert!(
        !out.exists(),
        "a rejected palette must not leave a render behind"
    );
}

/// The inline form is spelled for a shell, not for TOML: one comma-separated
/// argument. A malformed color in it is a usage error naming the entry.
#[test]
fn render_with_a_malformed_inline_palette_exits_2_and_names_the_entry() {
    let path = path_with_fake_ffmpeg();

    avz()
        .env("PATH", path.path())
        .arg("render")
        .arg(fixture("tone-tagged.mp3"))
        .args(["--palette", "#1a1a2e,#gg0000"])
        .assert()
        .code(2)
        .stderr(contains("palette entry 2"));
}

/// One inline color is a color, not a palette; nine is more than avz resamples.
#[test]
fn render_with_an_inline_palette_of_the_wrong_length_exits_2() {
    let path = path_with_fake_ffmpeg();

    avz()
        .env("PATH", path.path())
        .arg("render")
        .arg(fixture("tone-tagged.mp3"))
        .args(["--palette", "#1a1a2e"])
        .assert()
        .code(2)
        .stderr(contains("2 to 8 colors"));
}

/// UT-007: a `--set` outside the schema's range fails with exit code 2 before
/// any rendering starts, and leaves no output file behind.
#[test]
fn out_of_range_value_fails_exit_2_before_render() {
    let path = path_with_fake_ffmpeg();
    let dir = tempfile::tempdir().expect("tempdir");
    let song = dir.path().join("song.mp3");
    fs::copy(fixture("tone-tagged.mp3"), &song).expect("copy the fixture");

    avz()
        .env("PATH", path.path())
        .arg("render")
        .arg(&song)
        .args(["--set", "bass_drive=99"])
        .assert()
        .code(2)
        .stderr(contains("bass_drive"))
        .stderr(contains("0..4"));

    assert!(
        !song.with_extension("mp4").exists(),
        "a rejected config must not leave a render behind"
    );
}

/// The bare `--set name=value` shorthand resolves into the active preset's
/// parameters, so a typo is reported as a parameter and not as a config section.
#[test]
fn unknown_param_via_set_exits_2_with_a_suggestion() {
    let path = path_with_fake_ffmpeg();
    let dir = tempfile::tempdir().expect("tempdir");

    avz()
        .env("PATH", path.path())
        .arg("render")
        .arg(fixture("tone-tagged.mp3"))
        .arg("--out")
        .arg(dir.path().join("out.mp4"))
        .args(["--set", "pulse.bas_drive=2"])
        .assert()
        .code(2)
        .stderr(contains("unknown parameter `bas_drive`"))
        .stderr(contains("did you mean `bass_drive`"));
}

/// UT-007: `[visual.params]` in a `--config` file is validated against the same
/// schema. This is what proves the `--config` layer reaches the preset at all.
#[test]
fn a_config_files_preset_params_are_validated_against_the_schema() {
    let path = path_with_fake_ffmpeg();
    let dir = tempfile::tempdir().expect("tempdir");
    let config = dir.path().join("base.toml");
    fs::write(&config, "[visual.params]\nring_count = 99\n").expect("write");

    avz()
        .env("PATH", path.path())
        .arg("render")
        .arg(fixture("tone-tagged.mp3"))
        .arg("--config")
        .arg(&config)
        .arg("--out")
        .arg(dir.path().join("out.mp4"))
        .assert()
        .code(2)
        .stderr(contains("ring_count"))
        .stderr(contains("1..32"));
}

/// The precedence contract from `VISION.md` §5.5, at the CLI: `--set` wins over
/// the config file. A `--set` that is legal must rescue a file value that is not.
#[test]
fn a_set_override_beats_an_illegal_value_in_the_config_file() {
    let path = path_with_fake_ffmpeg();
    let dir = tempfile::tempdir().expect("tempdir");
    let config = dir.path().join("base.toml");
    fs::write(&config, "[visual.params]\nring_count = 99\n").expect("write");

    // The render still fails — the fake ffmpeg encodes nothing — but with exit
    // code 4, a render failure, and not the exit 2 a losing `--set` would give.
    avz()
        .env("PATH", path.path())
        .arg("render")
        .arg(fixture("tone-tagged.mp3"))
        .arg("--config")
        .arg(&config)
        .arg("--out")
        .arg(dir.path().join("out.mp4"))
        .args(["--set", "ring_count=8"])
        .assert()
        .code(4)
        .stderr(contains("ring_count").not());
}
