//! The `avz` command-line surface (`VISION.md` §3).

use std::path::PathBuf;

use avz_core::config::SampleRange;
use avz_core::render::AdapterChoice;
use clap::{Args, Parser, Subcommand};

/// Abstract music video generator. mp3 in, music-reactive video out.
#[derive(Debug, Parser)]
#[command(name = "avz", version, long_about = None)]
#[command(propagate_version = true)]
pub struct Cli {
    /// Print debug diagnostics: adapter chosen, ffmpeg command line, phase timings.
    #[arg(long, short, global = true, conflicts_with = "quiet")]
    pub verbose: bool,

    /// Suppress everything but errors.
    #[arg(long, short, global = true)]
    pub quiet: bool,

    #[command(subcommand)]
    pub command: Command,
}

/// What the user asked `avz` to do.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Render an abstract music video from an mp3.
    Render(RenderArgs),

    /// Inspect an mp3: tags, duration, embedded cover art.
    Probe(ProbeArgs),

    /// List presets, or show one preset's parameter schema.
    Presets(PresetsArgs),

    /// Work with TOML configuration files.
    Config(ConfigArgs),
}

impl Command {
    /// The user-facing name of this subcommand.
    pub fn name(&self) -> &'static str {
        match self {
            Command::Render(_) => "render",
            Command::Probe(_) => "probe",
            Command::Presets(_) => "presets",
            Command::Config(_) => "config",
        }
    }
}

#[derive(Debug, Args)]
pub struct RenderArgs {
    /// The mp3 to render.
    pub input: PathBuf,

    /// Output path. Defaults to `<song-stem>.mp4` next to the input.
    #[arg(long, short)]
    pub out: Option<PathBuf>,

    /// Render an excerpt: `60s` for the first minute, `0:45..1:45` for a range.
    ///
    /// Sample renders drop to 720p unless a resolution is configured, so a
    /// chorus comes back in seconds instead of minutes.
    #[arg(long, value_name = "RANGE")]
    pub sample: Option<SampleRange>,

    /// A TOML config file. See `avz config --example` for a documented template.
    #[arg(long, value_name = "FILE")]
    pub config: Option<PathBuf>,

    /// Override one setting: `--set visual.intensity=1.4`. Repeatable.
    ///
    /// A key that names no config section is a parameter of the preset being
    /// rendered, so `--set bass_drive=1.5` and `--set pulse.bass_drive=1.5` are
    /// both shorthand for `--set visual.params.bass_drive=1.5`. Run
    /// `avz presets <name>` to see what a preset accepts.
    #[arg(long, value_name = "KEY=VALUE")]
    pub set: Vec<String>,

    /// Which Vulkan adapter to render on.
    ///
    /// `auto` prefers a GPU and falls back to software rendering with a warning.
    /// `gpu` fails if there is no GPU. `software` always uses Mesa's lavapipe.
    #[arg(long, default_value_t = AdapterChoice::Auto, value_name = "auto|gpu|software")]
    pub adapter: AdapterChoice,
}

#[derive(Debug, Args)]
pub struct ProbeArgs {
    /// The mp3 to inspect.
    pub input: PathBuf,
}

#[derive(Debug, Args)]
pub struct PresetsArgs {
    /// Show the full parameter schema for one preset instead of listing all.
    pub name: Option<String>,
}

#[derive(Debug, Args)]
pub struct ConfigArgs {
    /// Print a documented example config to stdout.
    #[arg(long)]
    pub example: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory as _;

    #[test]
    fn cli_definition_is_internally_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn subcommand_names_match_the_ux_contract() {
        let cli = Cli::try_parse_from(["avz", "render", "song.mp3"]).expect("parses");
        assert_eq!(cli.command.name(), "render");

        let cli = Cli::try_parse_from(["avz", "probe", "song.mp3"]).expect("parses");
        assert_eq!(cli.command.name(), "probe");

        let cli = Cli::try_parse_from(["avz", "presets"]).expect("parses");
        assert_eq!(cli.command.name(), "presets");

        let cli = Cli::try_parse_from(["avz", "config"]).expect("parses");
        assert_eq!(cli.command.name(), "config");
    }

    #[test]
    fn verbose_and_quiet_cannot_be_combined() {
        let err = Cli::try_parse_from(["avz", "--quiet", "--verbose", "probe", "song.mp3"])
            .expect_err("conflicting flags are rejected");
        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn global_flags_are_accepted_after_the_subcommand() {
        let cli = Cli::try_parse_from(["avz", "probe", "song.mp3", "--verbose"]).expect("parses");
        assert!(cli.verbose);
    }

    fn render_args(args: &[&str]) -> RenderArgs {
        let argv = ["avz", "render", "song.mp3"].iter().chain(args);
        match Cli::try_parse_from(argv).expect("parses").command {
            Command::Render(args) => args,
            other => panic!("expected a render command, got {other:?}"),
        }
    }

    /// A render with no flags renders the whole song, on whatever adapter avz
    /// finds, to a path derived from the input (`VISION.md` §3).
    #[test]
    fn a_bare_render_samples_nothing_and_chooses_its_own_adapter() {
        let args = render_args(&[]);

        assert!(args.sample.is_none());
        assert!(args.out.is_none());
        assert_eq!(args.adapter, AdapterChoice::Auto);
    }

    /// Both `--sample` spellings from `VISION.md` §3.
    #[test]
    fn sample_accepts_a_bare_duration_and_a_clock_range() {
        let first_minute = render_args(&["--sample", "60s"]).sample.expect("a sample");
        assert_eq!(first_minute.start.as_secs_f64(), 0.0);
        assert_eq!(first_minute.end.as_secs_f64(), 60.0);

        let chorus = render_args(&["--sample", "0:45..1:45"])
            .sample
            .expect("a sample");
        assert_eq!(chorus.start.as_secs_f64(), 45.0);
        assert_eq!(chorus.duration_secs(), 60.0);
    }

    /// A backwards range is a usage error, caught before anything is decoded.
    #[test]
    fn a_sample_that_ends_before_it_starts_is_rejected() {
        let err = Cli::try_parse_from(["avz", "render", "song.mp3", "--sample", "3s..1s"])
            .expect_err("a range must run forwards");

        assert_eq!(err.kind(), clap::error::ErrorKind::ValueValidation);
    }

    #[test]
    fn every_adapter_choice_is_spelled_the_way_core_parses_it() {
        for (flag, expected) in [
            ("auto", AdapterChoice::Auto),
            ("gpu", AdapterChoice::Gpu),
            ("software", AdapterChoice::Software),
        ] {
            assert_eq!(render_args(&["--adapter", flag]).adapter, expected);
        }
    }

    #[test]
    fn an_unknown_adapter_is_a_usage_error() {
        let err = Cli::try_parse_from(["avz", "render", "song.mp3", "--adapter", "lavapipe"])
            .expect_err("`lavapipe` is the driver, not the flag value");

        assert_eq!(err.kind(), clap::error::ErrorKind::ValueValidation);
    }
}
