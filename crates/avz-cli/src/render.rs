//! `avz render song.mp3` — the pipeline, wired to a terminal.
//!
//! Everything a render needs from the user is resolved here — the output path,
//! the config layers, the adapter — and everything the render has to say comes
//! back through the [`Progress`] trait, because `avz-core` never prints
//! (`AGENTS.md`, core/cli split).
//!
//! The progress bar with frame count, fps, and ETA arrives in RFC-001 Step 21.
//! Until then phases are logged and the one actionable warning is printed.

use std::path::{Path, PathBuf};

use avz_core::config::{ConfigLayer, Sources};
use avz_core::encode::{self, DEFAULT_PROGRAM};
use avz_core::pipeline::{self, RenderRequest, RenderSummary};
use avz_core::{Error, Phase, Progress};

use crate::cli::RenderArgs;

/// The extension every render produces.
const OUTPUT_EXTENSION: &str = "mp4";

/// Render `args.input`, reporting progress to the terminal.
pub fn run(args: &RenderArgs, quiet: bool) -> anyhow::Result<()> {
    // Before analysis, before the GPU, before a single frame: a render that
    // cannot be encoded should fail in the first second, not the last one
    // (`VISION.md` §5.4).
    let ffmpeg = encode::preflight(DEFAULT_PROGRAM)?;

    let output = match &args.out {
        Some(path) => path.clone(),
        None => default_output(&args.input),
    };
    if overwrites_input(&args.input, &output) {
        return Err(Error::Config(format!(
            "`{}` is the song avz is reading; pass `--out` a different path",
            output.display(),
        ))
        .into());
    }

    let config = Sources {
        sample_defaults: match args.sample {
            Some(_) => ConfigLayer::for_sample(),
            None => ConfigLayer::default(),
        },
        ..Sources::default()
    }
    .resolve()?;

    let progress = Terminal { quiet };
    let summary = pipeline::render(
        &RenderRequest {
            input: &args.input,
            output: &output,
            config: &config,
            adapter: args.adapter,
            sample: args.sample,
            ffmpeg: &ffmpeg,
        },
        &progress,
    )?;

    if !quiet {
        println!("{}", describe(&summary));
    }

    Ok(())
}

/// `<song-stem>.mp4` next to the input (`VISION.md` §3).
fn default_output(input: &Path) -> PathBuf {
    input.with_extension(OUTPUT_EXTENSION)
}

/// Whether `output` names the very file avz is about to read.
///
/// The encoder renames its part file over the output path, so an `--out` aimed
/// back at the input would destroy the song. Both paths are resolved before they
/// are compared, so `./song.mp3` and `song.mp3` are recognized as the same file.
fn overwrites_input(input: &Path, output: &Path) -> bool {
    let resolve = |path: &Path| path.canonicalize().or_else(|_| std::path::absolute(path));

    match (resolve(input), resolve(output)) {
        (Ok(input), Ok(output)) => input == output,
        // Neither path can be resolved against the filesystem, so all that is
        // left is what the user typed. A false negative here fails later, in the
        // decoder, on a file that does not exist.
        _ => input == output,
    }
}

/// The one line a finished render prints.
fn describe(summary: &RenderSummary) -> String {
    format!(
        "wrote {} — {} frames, {:.2}s, {} rendering",
        summary.output.display(),
        summary.frames,
        summary.duration().as_secs_f64(),
        summary.adapter,
    )
}

/// [`Progress`] rendered as terminal output.
///
/// Phases go to `tracing`, which already writes to stderr and already honours
/// `--verbose` and `--quiet`. Warnings are printed directly, because a warning
/// the user must act on should not depend on a log level.
#[derive(Debug)]
struct Terminal {
    quiet: bool,
}

impl Terminal {
    /// Whether a warning reaches the user.
    fn shows_warnings(&self) -> bool {
        !self.quiet
    }
}

impl Progress for Terminal {
    fn phase_started(&self, phase: Phase, total: Option<u64>) {
        match total {
            Some(total) => tracing::info!(units = total, "{}", phase.label()),
            None => tracing::info!("{}", phase.label()),
        }
    }

    /// Deliberately silent: a per-frame log line would outnumber the frames it
    /// describes. The progress bar that consumes this lands in RFC-001 Step 21.
    fn advance(&self, _phase: Phase, _units: u64) {}

    fn phase_finished(&self, _phase: Phase) {}

    fn warn(&self, message: &str) {
        if self.shows_warnings() {
            eprintln!("warning: {message}");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use avz_core::render::AdapterKind;

    use super::*;

    #[test]
    fn the_default_output_sits_next_to_the_input_with_an_mp4_extension() {
        assert_eq!(
            default_output(Path::new("/music/song.mp3")),
            Path::new("/music/song.mp4")
        );
        assert_eq!(
            default_output(Path::new("song.mp3")),
            Path::new("song.mp4"),
            "a bare name stays a bare name"
        );
        assert_eq!(
            default_output(Path::new("no-extension")),
            Path::new("no-extension.mp4")
        );
    }

    /// The encoder renames its part file over the output. Aimed at the input,
    /// that would delete the song mid-render — after ffmpeg had already read it.
    #[test]
    fn an_output_that_is_the_input_is_refused_however_it_is_spelled() {
        let dir = tempfile::tempdir().expect("tempdir");
        let song = dir.path().join("song.mp3");
        fs::write(&song, b"pretend mp3").expect("write");

        assert!(overwrites_input(&song, &song));
        assert!(
            overwrites_input(&song, &dir.path().join("./song.mp3")),
            "the same file by another spelling is still the same file"
        );
        assert!(!overwrites_input(&song, &dir.path().join("song.mp4")));
    }

    /// Nothing exists yet, so only the paths themselves can be compared.
    #[test]
    fn an_output_matching_a_nonexistent_input_is_still_refused() {
        assert!(overwrites_input(
            Path::new("/no/such/song.mp3"),
            Path::new("/no/such/song.mp3")
        ));
        assert!(!overwrites_input(
            Path::new("/no/such/song.mp3"),
            Path::new("/no/such/song.mp4")
        ));
    }

    #[test]
    fn a_finished_render_reports_where_it_went_and_what_it_cost() {
        let line = describe(&RenderSummary {
            frames: 60,
            fps: 30,
            adapter: AdapterKind::Software,
            output: PathBuf::from("song.mp4"),
        });

        assert_eq!(
            line,
            "wrote song.mp4 — 60 frames, 2.00s, software rendering"
        );
    }

    /// `--quiet` suppresses everything but errors (`VISION.md` §3), and a
    /// warning is not an error.
    #[test]
    fn only_a_loud_render_shows_warnings() {
        assert!(Terminal { quiet: false }.shows_warnings());
        assert!(!Terminal { quiet: true }.shows_warnings());
    }
}
