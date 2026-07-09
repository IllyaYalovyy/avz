//! The `avz` command-line surface (`VISION.md` §3).

use std::path::PathBuf;

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
}
