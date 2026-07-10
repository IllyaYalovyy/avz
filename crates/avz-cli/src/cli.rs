//! The `avz` command-line surface (`VISION.md` §3).

use std::path::PathBuf;

use avz_core::config::{Codec, MAX_CRF, Palette, SampleRange, Seed};
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
///
/// `Render` is much the largest variant, and it stays unboxed: exactly one of
/// these exists per process, it lives on the stack of `main` for the whole run,
/// and clap's `Subcommand` derive needs a variant whose field implements `Args`
/// — which `Box<RenderArgs>` does not.
#[allow(clippy::large_enum_variant)]
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

    /// Which visualizer to render: `avz presets` lists the names.
    ///
    /// Not validated by clap — the preset registry is what knows which names
    /// exist, and an unknown one fails with the list of those that do, before
    /// the song is decoded.
    #[arg(long, value_name = "NAME")]
    pub preset: Option<String>,

    /// The color scheme: a built-in name, or two to eight inline hex colors.
    ///
    /// `--palette glacier` names one avz ships; `--palette '#1a1a2e,#e94560'`
    /// spells one out and lets avz resample it onto the five slots a shader
    /// reads. An unknown name fails with the list of names that exist.
    #[arg(long, value_name = "NAME|#HEX,#HEX")]
    pub palette: Option<Palette>,

    /// A static image to composite beneath the visuals.
    ///
    /// Fitted to the frame by `background.fit` (`cover` by default), and
    /// optionally blurred and darkened so the visuals read on top:
    /// `--set background.blur=6 --set background.darken=0.35`.
    #[arg(long, value_name = "FILE")]
    pub bg: Option<PathBuf>,

    /// Title for the text card, overriding the ID3 tag.
    #[arg(long, value_name = "TEXT", conflicts_with = "no_text")]
    pub title: Option<String>,

    /// Artist for the text card, overriding the ID3 tag.
    #[arg(long, value_name = "TEXT", conflicts_with = "no_text")]
    pub artist: Option<String>,

    /// Draw no title/artist card.
    ///
    /// A song with neither tag draws none anyway, and says so. This silences it.
    #[arg(long)]
    pub no_text: bool,

    /// The seed the shader's noise is hashed from: `auto`, or an integer.
    ///
    /// `auto` derives it from the song's file name, so re-rendering the same
    /// song — from another directory, or on another machine — gives the same
    /// video. Two seeds a number apart give unrelated ones.
    #[arg(long, value_name = "auto|N")]
    pub seed: Option<Seed>,

    /// Video codec. avz v0.1 encodes `x264` only.
    #[arg(long, value_name = "x264")]
    pub codec: Option<Codec>,

    /// x264 CRF quality, 0 (visually lossless, huge) to 51 (worst).
    ///
    /// The default, 18, is safe to upload. Every step of about 6 halves or
    /// doubles the file size.
    #[arg(long, value_name = "CRF", value_parser = clap::value_parser!(u8).range(0..=MAX_CRF as i64))]
    pub quality: Option<u8>,

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
    ///
    /// Every key carries its built-in default, so the file it writes renders
    /// what a bare `avz render` renders: `avz config --example > avz.toml`.
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

    /// The four subcommands `VISION.md` §3 names, spelled the way it spells them.
    #[test]
    fn subcommand_names_match_the_ux_contract() {
        let command = |argv: [&str; 2]| Cli::try_parse_from(argv).expect("parses").command;
        let command_with_input =
            |argv: [&str; 3]| Cli::try_parse_from(argv).expect("parses").command;

        assert!(matches!(
            command_with_input(["avz", "render", "song.mp3"]),
            Command::Render(_)
        ));
        assert!(matches!(
            command_with_input(["avz", "probe", "song.mp3"]),
            Command::Probe(_)
        ));
        assert!(matches!(command(["avz", "presets"]), Command::Presets(_)));
        assert!(matches!(command(["avz", "config"]), Command::Config(_)));
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

    /// `--preset nebula` is the flag `VISION.md` §3 opens its typical invocation
    /// with. The name is not checked here: the registry is what knows which
    /// presets exist, and `render` is where it says so with the list.
    #[test]
    fn preset_names_the_visualizer_to_render() {
        assert_eq!(
            render_args(&["--preset", "nebula"]).preset.as_deref(),
            Some("nebula"),
        );
    }

    /// A bare render leaves the preset to the config file and the built-in
    /// default, the way `--palette` leaves the colors.
    #[test]
    fn a_bare_render_has_no_opinion_about_the_preset() {
        assert!(render_args(&[]).preset.is_none());
    }

    /// Both spellings of `--palette` from `VISION.md` §3: a built-in name, and
    /// hex colors the shell can pass without TOML array syntax.
    #[test]
    fn palette_accepts_a_built_in_name_and_an_inline_hex_list() {
        assert_eq!(
            render_args(&["--palette", "glacier"]).palette,
            Some(Palette::Named("glacier".to_owned())),
        );

        let Some(Palette::Inline(colors)) = render_args(&["--palette", "#1a1a2e,#e94560"]).palette
        else {
            panic!("expected an inline palette");
        };
        assert_eq!(colors.len(), 2);
    }

    /// A bare render leaves the palette to the config file and the built-in
    /// default. A flag that defaulted to `ember` would outrank both.
    #[test]
    fn a_bare_render_has_no_opinion_about_the_palette() {
        assert!(render_args(&[]).palette.is_none());
    }

    /// A malformed palette is caught by clap, before anything else runs. An
    /// unknown *name* is not malformed — the registry, not the parser, decides
    /// that, so it fails later with the list of names.
    #[test]
    fn a_malformed_palette_is_a_usage_error() {
        let err =
            Cli::try_parse_from(["avz", "render", "song.mp3", "--palette", "#gg0000,#000000"])
                .expect_err("`#gg0000` is not a color");
        assert_eq!(err.kind(), clap::error::ErrorKind::ValueValidation);
        assert!(
            err.to_string().contains("palette entry 1"),
            "the bad entry is named: {err}",
        );
    }

    /// The three flags `VISION.md` §5.2 names for the text card.
    #[test]
    fn the_text_card_can_be_titled_credited_and_turned_off() {
        let args = render_args(&["--title", "Cold Design", "--artist", "avz"]);

        assert_eq!(args.title.as_deref(), Some("Cold Design"));
        assert_eq!(args.artist.as_deref(), Some("avz"));
        assert!(!args.no_text);

        assert!(render_args(&["--no-text"]).no_text);
    }

    /// `--no-text` says there is no card; `--title` says what is on it. Asking
    /// for both is a contradiction, and clap catches it before anything runs.
    #[test]
    fn a_titled_card_that_is_also_disabled_is_a_usage_error() {
        let err = Cli::try_parse_from(["avz", "render", "song.mp3", "--no-text", "--title", "x"])
            .expect_err("a card cannot be both named and absent");

        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn a_bare_render_has_no_opinion_about_the_text_card() {
        let args = render_args(&[]);

        assert!(args.title.is_none() && args.artist.is_none() && !args.no_text);
    }

    /// `--seed` takes both spellings `visual.seed` takes, and nothing else.
    #[test]
    fn seed_accepts_auto_and_an_integer() {
        assert_eq!(render_args(&["--seed", "auto"]).seed, Some(Seed::Auto));
        assert_eq!(render_args(&["--seed", "7"]).seed, Some(Seed::Fixed(7)));

        // `=`, because a bare `-1` is an argument to clap, not a value.
        let err = Cli::try_parse_from(["avz", "render", "song.mp3", "--seed=-1"])
            .expect_err("a seed is not negative");
        assert_eq!(err.kind(), clap::error::ErrorKind::ValueValidation);
    }

    /// A CRF outside x264's scale is a usage error, caught by clap before
    /// ffmpeg is even looked for (`VISION.md` §5.4, §8: exit 2).
    #[test]
    fn quality_outside_the_crf_scale_is_a_usage_error() {
        assert_eq!(render_args(&["--quality", "0"]).quality, Some(0));
        assert_eq!(render_args(&["--quality", "51"]).quality, Some(51));

        // `--quality=N`, because a bare `-1` is an argument to clap, not a value.
        for out_of_range in ["52", "-1", "300"] {
            let flag = format!("--quality={out_of_range}");
            let err = Cli::try_parse_from(["avz", "render", "song.mp3", &flag])
                .expect_err("a CRF outside 0..=51 is not a quality");

            assert_eq!(
                err.kind(),
                clap::error::ErrorKind::ValueValidation,
                "`--quality {out_of_range}`",
            );
        }
    }

    /// The deferred codecs still *parse* (RFC-001 NG3): the render, not the
    /// argument parser, is what says they are not supported yet.
    #[test]
    fn codec_parses_every_name_and_rejects_the_rest() {
        assert_eq!(render_args(&["--codec", "x264"]).codec, Some(Codec::X264));
        assert_eq!(render_args(&["--codec", "av1"]).codec, Some(Codec::Av1));

        let err = Cli::try_parse_from(["avz", "render", "song.mp3", "--codec", "h264"])
            .expect_err("`h264` is the standard, `x264` is the encoder");
        assert_eq!(err.kind(), clap::error::ErrorKind::ValueValidation);
    }

    /// A bare render leaves all three to the config file and the built-in
    /// defaults, the way `--palette` does.
    #[test]
    fn a_bare_render_has_no_opinion_about_the_seed_codec_or_quality() {
        let args = render_args(&[]);

        assert!(args.seed.is_none() && args.codec.is_none() && args.quality.is_none());
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
