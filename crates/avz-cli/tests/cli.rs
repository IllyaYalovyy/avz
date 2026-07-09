//! CLI surface contract: subcommand discovery, usage errors, and exit codes.
//!
//! Exit codes are fixed by VISION.md §8: 0 ok, 2 bad args/config, 3 input file
//! problems, 4 render/encode failure.

use assert_cmd::Command;
use predicates::str::contains;

fn avz() -> Command {
    Command::cargo_bin("avz").expect("avz binary builds")
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
    avz()
        .args(["render", "x.mp3"])
        .assert()
        .code(4)
        .stderr(contains("not implemented"));
}

#[test]
fn quiet_and_verbose_conflict_is_rejected() {
    avz()
        .args(["--quiet", "--verbose", "probe", "x.mp3"])
        .assert()
        .code(2);
}
