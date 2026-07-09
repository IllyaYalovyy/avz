//! Preflight: prove a usable `ffmpeg` exists before anything expensive starts.
//!
//! A render costs minutes of analysis and GPU work. Discovering a missing
//! encoder at the end of that is the worst possible time, so `avz` runs
//! `ffmpeg -version` up front and refuses to start otherwise (`VISION.md` §5.4).
//!
//! The failure message is the whole point: it names the binary it looked for and
//! the command that installs it.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{Error, Result};

/// The program name looked up on `PATH` when the caller has no better idea.
pub const DEFAULT_PROGRAM: &str = "ffmpeg";

/// Fedora is the primary target (`VISION.md`), so its install line leads.
const INSTALL_HINT: &str =
    "install it with `sudo dnf install ffmpeg` on Fedora, or your distribution's equivalent";

/// An ffmpeg binary that has been verified to exist and identify itself.
///
/// Holding the resolved program path means the encoder cannot drift onto a
/// different binary than the one preflight approved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ffmpeg {
    program: PathBuf,
    version: String,
}

impl Ffmpeg {
    /// The program preflight verified, as given to [`preflight`].
    pub fn program(&self) -> &Path {
        &self.program
    }

    /// The version ffmpeg reported, e.g. `7.1.5` or `N-109212-g1a2b3c4`.
    pub fn version(&self) -> &str {
        &self.version
    }
}

/// Check that `program` is a runnable ffmpeg.
///
/// Pass [`DEFAULT_PROGRAM`] to resolve `ffmpeg` through `PATH`.
///
/// # Errors
///
/// [`Error::Encode`] if the program is absent, unrunnable, exits non-zero, or
/// does not identify itself as ffmpeg. Every message says what to do next.
pub fn preflight(program: impl AsRef<Path>) -> Result<Ffmpeg> {
    let program = program.as_ref();

    let output = Command::new(program)
        .arg("-version")
        .output()
        .map_err(|err| spawn_failed(program, &err))?;

    if !output.status.success() {
        return Err(Error::Encode(format!(
            "`{} -version` failed ({}){}",
            program.display(),
            output.status,
            complaint(&output.stderr),
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let Some(version) = parse_version(&stdout) else {
        return Err(Error::Encode(format!(
            "`{}` did not identify itself as ffmpeg; point avz at the real ffmpeg binary, or {INSTALL_HINT}",
            program.display(),
        )));
    };

    tracing::debug!(program = %program.display(), version, "ffmpeg preflight passed");

    Ok(Ffmpeg {
        program: program.to_path_buf(),
        version: version.to_owned(),
    })
}

/// Explain why the process never started.
///
/// `NotFound` is the case worth a paragraph: it is what every user without
/// ffmpeg installed will hit, and the fix is one command.
fn spawn_failed(program: &Path, err: &io::Error) -> Error {
    if err.kind() == io::ErrorKind::NotFound {
        return Error::Encode(format!(
            "ffmpeg not found: `{}` is not on PATH. avz encodes video with the system ffmpeg binary — {INSTALL_HINT}",
            program.display(),
        ));
    }

    Error::Encode(format!(
        "cannot run `{} -version`: {err}",
        program.display(),
    ))
}

/// ffmpeg's own words about why it exited non-zero, if it said anything.
fn complaint(stderr: &[u8]) -> String {
    let stderr = String::from_utf8_lossy(stderr);
    match stderr.lines().find(|line| !line.trim().is_empty()) {
        Some(line) => format!(": {}", line.trim()),
        None => String::new(),
    }
}

/// Pull the version out of ffmpeg's `-version` banner.
///
/// The first line is `ffmpeg version <version> Copyright (c) ...` for release
/// and git builds alike. Anything else is not ffmpeg.
fn parse_version(stdout: &str) -> Option<&str> {
    stdout
        .lines()
        .next()?
        .strip_prefix("ffmpeg version ")?
        .split_whitespace()
        .next()
}

#[cfg(test)]
mod tests {
    use super::*;

    const RELEASE_BANNER: &str = "\
ffmpeg version 7.1.5 Copyright (c) 2000-2026 the FFmpeg developers
built with gcc 15 (GCC)
";

    const GIT_BANNER: &str = "\
ffmpeg version N-109212-g1a2b3c4de Copyright (c) 2000-2026 the FFmpeg developers
";

    /// A path that cannot exist, so the spawn fails with `NotFound`.
    const ABSENT: &str = "/nonexistent/avz-preflight-no-such-ffmpeg";

    #[test]
    fn release_version_banner_is_parsed() {
        assert_eq!(parse_version(RELEASE_BANNER), Some("7.1.5"));
    }

    #[test]
    fn git_build_version_banner_is_parsed() {
        assert_eq!(parse_version(GIT_BANNER), Some("N-109212-g1a2b3c4de"));
    }

    #[test]
    fn a_banner_from_another_program_has_no_version() {
        assert_eq!(parse_version("GNU coreutils echo 9.5\n"), None);
        assert_eq!(parse_version(""), None);
        assert_eq!(parse_version("ffmpeg version\n"), None);
    }

    #[test]
    fn missing_ffmpeg_fails_with_the_fedora_install_hint() {
        let err = preflight(ABSENT).expect_err("an absent binary cannot preflight");

        assert!(matches!(err, Error::Encode(_)), "got {err:?}");
        let msg = err.to_string();
        assert!(msg.contains(ABSENT), "message must name the binary: {msg}");
        assert!(
            msg.contains("sudo dnf install ffmpeg"),
            "message must say how to install ffmpeg: {msg}"
        );
    }

    #[cfg(unix)]
    mod unix {
        use std::fs;
        use std::os::unix::fs::PermissionsExt as _;

        use super::*;

        /// Write an executable stand-in for ffmpeg with the given shell body.
        fn fake_ffmpeg(dir: &Path, body: &str) -> PathBuf {
            let path = dir.join("ffmpeg");
            fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write fake ffmpeg");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod");
            path
        }

        #[test]
        fn a_working_ffmpeg_reports_its_version() {
            let dir = tempfile::tempdir().expect("tempdir");
            let path = fake_ffmpeg(
                dir.path(),
                "echo 'ffmpeg version 7.1.5 Copyright (c) 2000-2026'",
            );

            let ffmpeg = preflight(&path).expect("a real-looking ffmpeg preflights");

            assert_eq!(ffmpeg.program(), path);
            assert_eq!(ffmpeg.version(), "7.1.5");
        }

        #[test]
        fn a_binary_that_is_not_ffmpeg_is_rejected() {
            let dir = tempfile::tempdir().expect("tempdir");
            let path = fake_ffmpeg(dir.path(), "echo 'GNU coreutils echo 9.5'");

            let err = preflight(&path).expect_err("a non-ffmpeg binary cannot preflight");

            let msg = err.to_string();
            assert!(
                msg.contains("did not identify itself as ffmpeg"),
                "message must explain what was wrong: {msg}"
            );
        }

        #[test]
        fn an_ffmpeg_that_exits_nonzero_is_rejected() {
            let dir = tempfile::tempdir().expect("tempdir");
            let path = fake_ffmpeg(dir.path(), "echo 'broken build' >&2\nexit 3");

            let err = preflight(&path).expect_err("a failing binary cannot preflight");

            let msg = err.to_string();
            assert!(
                msg.contains("-version"),
                "message must name the probe: {msg}"
            );
            assert!(
                msg.contains("broken build"),
                "message must surface ffmpeg's own complaint: {msg}"
            );
        }

        #[test]
        fn a_program_that_cannot_be_executed_is_rejected() {
            let dir = tempfile::tempdir().expect("tempdir");
            let path = fake_ffmpeg(dir.path(), "echo 'ffmpeg version 7.1.5'");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("chmod");

            let err = preflight(&path).expect_err("a non-executable file cannot preflight");

            assert!(matches!(err, Error::Encode(_)), "got {err:?}");
            let msg = err.to_string();
            assert!(
                msg.contains(&path.display().to_string()),
                "message must name the binary: {msg}"
            );
        }
    }
}
