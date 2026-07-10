//! Preflight: prove a usable `ffmpeg` exists before anything expensive starts.
//!
//! A render costs minutes of analysis and GPU work. Discovering a missing
//! encoder at the end of that is the worst possible time, so `avz` runs
//! `ffmpeg -version` up front and refuses to start otherwise (`VISION.md` §5.4).
//!
//! A binary that runs is not yet a binary that encodes: Fedora's stock
//! `ffmpeg-free` builds without `libx264` and `libx265`, and there are builds
//! without `libsvtav1`. [`encoders`] asks the binary itself which encoders it
//! has, so `--codec x265` on a build that cannot is refused up front too.
//!
//! The failure message is the whole point: it names the binary it looked for and
//! the command that installs it.

use std::collections::BTreeSet;
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

/// Every encoder `ffmpeg` was built with, by name.
///
/// The names are ffmpeg's own — `libx264`, not `x264` — because they are what
/// `-c:v` takes. Asking the binary beats a table of distribution packages: a
/// self-built ffmpeg is as legitimate as a packaged one, and only it knows.
///
/// # Errors
///
/// [`Error::Encode`] if `ffmpeg -encoders` will not run or exits non-zero. The
/// binary already answered `-version`, so this is a broken build, not a missing
/// one, and there is nothing the user can be told to install.
pub fn encoders(ffmpeg: &Ffmpeg) -> Result<BTreeSet<String>> {
    let program = ffmpeg.program();

    let output = Command::new(program)
        .args(["-hide_banner", "-encoders"])
        .output()
        .map_err(|err| {
            Error::Encode(format!(
                "cannot run `{} -encoders`: {err}",
                program.display(),
            ))
        })?;

    if !output.status.success() {
        return Err(Error::Encode(format!(
            "`{} -encoders` failed ({}){}",
            program.display(),
            output.status,
            complaint(&output.stderr),
        )));
    }

    let listed = parse_encoders(&String::from_utf8_lossy(&output.stdout));

    if listed.is_empty() {
        return Err(Error::Encode(format!(
            "`{} -encoders` listed no encoders; point avz at the real ffmpeg binary, or {INSTALL_HINT}",
            program.display(),
        )));
    }

    tracing::debug!(program = %program.display(), encoders = listed.len(), "ffmpeg encoders listed");

    Ok(listed)
}

/// Pull the encoder names out of `ffmpeg -encoders`.
///
/// Every listed encoder is a six-character capability field, the name, and a
/// description:
///
/// ```text
///  V....D libx264              libx264 H.264 / AVC (codec h264)
/// ```
///
/// The legend above the list wears the same field (` V..... = Video`), so a name
/// of `=` is the one thing that has to be dropped. Reading the field rather than
/// counting header lines survives an ffmpeg that reorders its banner.
fn parse_encoders(stdout: &str) -> BTreeSet<String> {
    stdout
        .lines()
        .filter_map(|line| {
            let mut tokens = line.split_whitespace();
            let flags = tokens.next()?;
            let name = tokens.next()?;

            let listed = flags.len() == 6
                && matches!(flags.as_bytes()[0], b'V' | b'A' | b'S')
                && flags.bytes().all(|flag| b"VAS.FXBD".contains(&flag));

            (listed && name != "=").then(|| name.to_owned())
        })
        .collect()
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

    /// `ffmpeg -encoders`, trimmed to one of each shape it prints.
    const ENCODERS: &str = "\
Encoders:
 V..... = Video
 A..... = Audio
 S..... = Subtitle
 .F.... = Frame-level multithreading
 ..S... = Slice-level multithreading
 ...X.. = Codec is experimental
 ....B. = Supports draw_horiz_band
 .....D = Supports direct rendering method 1
 ------
 V....D libx264              libx264 H.264 / AVC / MPEG-4 AVC (codec h264)
 V....D libx265              libx265 H.265 / HEVC (codec hevc)
 V..... libsvtav1            SVT-AV1 encoder (codec av1)
 A....D libmp3lame           libmp3lame MP3 (codec mp3)
 S..... srt                  SubRip subtitle
";

    #[test]
    fn every_listed_encoder_is_named_and_the_legend_is_not() {
        let listed = parse_encoders(ENCODERS);

        for encoder in ["libx264", "libx265", "libsvtav1", "libmp3lame", "srt"] {
            assert!(listed.contains(encoder), "{encoder} is listed: {listed:?}");
        }
        assert_eq!(listed.len(), 5, "the legend is not an encoder: {listed:?}");
    }

    #[test]
    fn an_ffmpeg_without_an_encoder_does_not_claim_it() {
        let without_x265 = ENCODERS.replace(" V....D libx265", " V....D libnope");

        assert!(!parse_encoders(&without_x265).contains("libx265"));
    }

    #[test]
    fn output_from_another_program_lists_no_encoders() {
        assert!(parse_encoders("GNU coreutils echo 9.5\n").is_empty());
        assert!(parse_encoders("").is_empty());
    }

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
            wait_until_executable(&path);
            path
        }

        /// Wait out `ETXTBSY` on the script we just wrote.
        ///
        /// `fs::write` has closed its descriptor by the time it returns, but any
        /// sibling test thread that forked inside that window handed its child an
        /// inherited copy, and Linux refuses to `exec` a file that any process
        /// still holds open for writing. The child drops it on its own `exec` a
        /// few microseconds later, and the condition cannot recur afterwards
        /// because no descriptor to this file remains anywhere.
        ///
        /// This is an artifact of spawning processes from a threaded test binary,
        /// not anything `preflight` does — so the tests wait it out rather than
        /// teaching production code to retry.
        fn wait_until_executable(path: &Path) {
            for _ in 0..1_000 {
                match Command::new(path).arg("-version").output() {
                    Err(err) if err.kind() == io::ErrorKind::ExecutableFileBusy => {
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                    _ => return,
                }
            }
            panic!("{}: still busy after a second", path.display());
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

        /// An ffmpeg that answers both probes: `-version` for [`preflight`],
        /// `-encoders` for [`encoders`].
        fn fake_ffmpeg_with_encoders(dir: &Path, listing: &str) -> PathBuf {
            fake_ffmpeg(
                dir,
                &format!(
                    "case \"$1\" in\n\
                     -version) echo 'ffmpeg version 7.1.5 Copyright (c) 2000-2026'; exit 0;;\n\
                     esac\n\
                     printf '%s' '{listing}'\n"
                ),
            )
        }

        #[test]
        fn the_encoders_ffmpeg_reports_are_the_encoders_avz_sees() {
            let dir = tempfile::tempdir().expect("tempdir");
            let path = fake_ffmpeg_with_encoders(dir.path(), ENCODERS);
            let ffmpeg = preflight(&path).expect("preflight");

            let listed = encoders(&ffmpeg).expect("a listing is parsed");

            assert!(listed.contains("libx264"), "{listed:?}");
            assert!(listed.contains("libsvtav1"), "{listed:?}");
            assert!(!listed.contains("libaom-av1"), "{listed:?}");
        }

        /// A binary that answers `-version` and then lists nothing is not an
        /// ffmpeg avz can encode with, and saying "no `libx264` encoder" would
        /// send the user after a package they already have.
        #[test]
        fn an_ffmpeg_that_lists_no_encoders_is_rejected() {
            let dir = tempfile::tempdir().expect("tempdir");
            let path = fake_ffmpeg_with_encoders(dir.path(), "Encoders:\n");
            let ffmpeg = preflight(&path).expect("preflight");

            let err = encoders(&ffmpeg).expect_err("an empty listing is not a listing");

            assert!(matches!(err, Error::Encode(_)), "got {err:?}");
            assert!(err.to_string().contains("no encoders"), "{err}");
        }

        #[test]
        fn an_encoder_probe_that_exits_nonzero_is_rejected() {
            let dir = tempfile::tempdir().expect("tempdir");
            let path = fake_ffmpeg(
                dir.path(),
                "case \"$1\" in\n\
                 -version) echo 'ffmpeg version 7.1.5'; exit 0;;\n\
                 esac\n\
                 echo 'Unrecognized option' >&2\n\
                 exit 1\n",
            );
            let ffmpeg = preflight(&path).expect("preflight");

            let err = encoders(&ffmpeg).expect_err("a failing probe cannot be believed");

            assert!(matches!(err, Error::Encode(_)), "got {err:?}");
            assert!(err.to_string().contains("-encoders"), "{err}");
            assert!(err.to_string().contains("Unrecognized option"), "{err}");
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
