//! `avz render song.mp3` — the pipeline, wired to a terminal.
//!
//! Everything a render needs from the user is resolved here — the output path,
//! the config layers, the adapter — and everything the render has to say comes
//! back through the [`Progress`](avz_core::Progress) trait, because `avz-core`
//! never prints (`AGENTS.md`, core/cli split). [`Ui`] is what draws it.

use std::path::{Path, PathBuf};

use avz_core::Error;
use avz_core::Progress as _;
use avz_core::config::{
    BackgroundLayer, ConfigLayer, EffectsLayer, OutputLayer, Resolution, Sources, TextLayer,
    VisualLayer,
};
use avz_core::encode::{self, DEFAULT_PROGRAM};
use avz_core::pipeline::{self, RenderRequest, RenderSummary};

use crate::cli::RenderArgs;
use crate::progress::Ui;

/// The extension every render produces.
const OUTPUT_EXTENSION: &str = "mp4";

/// Render `args.input`, reporting progress to the terminal.
pub fn run(args: &RenderArgs, ui: &Ui) -> anyhow::Result<()> {
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

    // `VISION.md` §5.5: CLI flags > `--set` > `--config` > preset defaults >
    // built-in defaults. `Sources` is where that order is written down.
    let sources = Sources {
        sample_defaults: match args.sample {
            Some(_) => ConfigLayer::for_sample(),
            None => ConfigLayer::default(),
        },
        file: match &args.config {
            Some(path) => ConfigLayer::from_file(path)?,
            None => ConfigLayer::default(),
        },
        set: ConfigLayer::from_set_assignments(&args.set)?,
        cli: cli_layer(args),
        ..Sources::default()
    };
    let reduced_for_sample = sample_resolution_is_a_default(args, &sources);
    let config = sources.resolve()?;

    if reduced_for_sample {
        ui.warn(&sample_resolution_warning(config.output.resolution));
    }

    let summary = pipeline::render(
        &RenderRequest {
            input: &args.input,
            output: &output,
            config: &config,
            adapter: args.adapter,
            sample: args.sample,
            ffmpeg: &ffmpeg,
        },
        ui,
    )?;

    ui.report(&describe(&summary));

    Ok(())
}

/// Whether the frame size about to be rendered is the one `--sample` picked
/// rather than one the user asked for.
///
/// No CLI flag reaches `output.resolution`, so the only layers that can outrank
/// the sample default are the config file and `--set`.
fn sample_resolution_is_a_default(args: &RenderArgs, sources: &Sources) -> bool {
    args.sample.is_some()
        && sources.file.output.resolution.is_none()
        && sources.set.output.resolution.is_none()
}

/// What to say when `--sample` quietly halves the frame size.
///
/// "Fast iteration" (`VISION.md` §3) is the whole point of the reduced default,
/// and a preview the user believes is full-size is a preview they will judge the
/// wrong picture by.
fn sample_resolution_warning(resolution: Resolution) -> String {
    format!(
        "`--sample` renders at {resolution} rather than full size, so an excerpt comes \
         back in seconds — pass `--set output.resolution=1080p`, or set \
         `output.resolution` in a config file, to preview at the size you will ship",
    )
}

/// The settings named by individual CLI flags — the top of the precedence chain.
///
/// A flag the user did not pass stays `None`, so it cannot displace the config
/// file. That is why `--palette` has no clap default: `ember` is the *built-in*
/// default, and a flag defaulting to it would outrank every config file.
fn cli_layer(args: &RenderArgs) -> ConfigLayer {
    ConfigLayer {
        output: OutputLayer {
            codec: args.codec,
            // Widened, never range-checked here: clap already rejected a CRF
            // outside x264's scale, and `resolve` is what a config file's
            // `output.quality` answers to.
            quality: args.quality.map(i64::from),
            ..OutputLayer::default()
        },
        visual: VisualLayer {
            preset: args.preset.clone(),
            palette: args.palette.clone(),
            seed: args.seed,
            ..VisualLayer::default()
        },
        background: BackgroundLayer {
            image: args.bg.clone(),
            ..BackgroundLayer::default()
        },
        text: TextLayer {
            // `--no-text` disables the card; its absence says nothing, or every
            // render would overrule a config file that enabled one.
            enabled: args.no_text.then_some(false),
            title: args.title.clone(),
            artist: args.artist.clone(),
            ..TextLayer::default()
        },
        // No flag reaches `[effects]`: `--set effects.zoom=1.2` is the spelling.
        effects: EffectsLayer::default(),
    }
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

#[cfg(test)]
mod tests {
    use std::fs;

    use avz_core::render::AdapterKind;

    use super::*;

    fn render_args(args: &[&str]) -> RenderArgs {
        use clap::Parser as _;

        let argv = ["avz", "render", "song.mp3"].iter().chain(args);
        match crate::cli::Cli::try_parse_from(argv)
            .expect("parses")
            .command
        {
            crate::cli::Command::Render(args) => args,
            other => panic!("expected a render command, got {other:?}"),
        }
    }

    /// `--preset nebula` is the first flag of `VISION.md` §3's typical
    /// invocation. Without this it parses, names a real preset, and is silently
    /// dropped — every render would draw `pulse`.
    #[test]
    fn the_preset_flag_reaches_the_cli_config_layer() {
        let layer = cli_layer(&render_args(&["--preset", "nebula"]));

        assert_eq!(layer.visual.preset.as_deref(), Some("nebula"));
    }

    /// The flag reaches the layer that outranks `--set` and `--config`. Without
    /// this, `--palette` parses, validates, and is silently dropped.
    #[test]
    fn the_palette_flag_reaches_the_cli_config_layer() {
        let layer = cli_layer(&render_args(&["--palette", "glacier"]));

        assert_eq!(
            layer.visual.palette,
            Some(avz_core::config::Palette::Named("glacier".to_owned())),
        );
    }

    /// `--bg art/forest.png` is the invocation `VISION.md` §3 promises. Without
    /// this the flag parses, names a real file, and is silently dropped.
    #[test]
    fn the_bg_flag_reaches_the_cli_config_layer() {
        let layer = cli_layer(&render_args(&["--bg", "art/forest.png"]));

        assert_eq!(
            layer.background.image,
            Some(PathBuf::from("art/forest.png"))
        );
        assert_eq!(
            layer.background.video, None,
            "`--bg` names an image; a loop is `--set background.video=PATH`",
        );
    }

    /// `--title`, `--artist`, and `--no-text` are the `VISION.md` §5.2 overrides,
    /// and they outrank the ID3 tags because they outrank the config file that
    /// would otherwise have named them.
    #[test]
    fn the_text_flags_reach_the_cli_config_layer() {
        let layer = cli_layer(&render_args(&["--title", "Cold Design", "--artist", "avz"]));

        assert_eq!(layer.text.title.as_deref(), Some("Cold Design"));
        assert_eq!(layer.text.artist.as_deref(), Some("avz"));
        assert_eq!(
            layer.text.enabled, None,
            "naming a card must not also decide whether cards are drawn"
        );

        assert_eq!(
            cli_layer(&render_args(&["--no-text"])).text.enabled,
            Some(false)
        );
    }

    /// `--seed`, `--codec`, and `--quality` reach the layer that outranks the
    /// config file. Without this they parse, validate, and are silently dropped:
    /// `--quality 30` would write a CRF-18 file and say nothing.
    #[test]
    fn the_seed_codec_and_quality_flags_reach_the_cli_config_layer() {
        let layer = cli_layer(&render_args(&[
            "--seed",
            "7",
            "--codec",
            "x264",
            "--quality",
            "30",
        ]));

        assert_eq!(layer.visual.seed, Some(avz_core::config::Seed::Fixed(7)));
        assert_eq!(layer.output.codec, Some(avz_core::config::Codec::X264));
        assert_eq!(layer.output.quality, Some(30));
    }

    /// The CRF the flag names is the CRF ffmpeg is given: `--quality` is the
    /// only thing between a user and `-crf` (`VISION.md` §5.4).
    #[test]
    fn the_quality_flag_becomes_the_resolved_crf() {
        let sources = Sources {
            cli: cli_layer(&render_args(&["--quality", "30"])),
            ..Sources::default()
        };

        assert_eq!(sources.resolve().expect("resolves").output.quality, 30);
    }

    /// An unpassed flag must have no opinion, or it would displace the config
    /// file from the top of the precedence chain.
    #[test]
    fn a_render_without_flags_contributes_an_empty_cli_layer() {
        assert_eq!(cli_layer(&render_args(&[])), ConfigLayer::default());
    }

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

    /// A render with no `--sample` draws at whatever resolution was configured,
    /// and there is nothing to warn about.
    #[test]
    fn a_full_render_never_warns_about_the_sample_resolution() {
        assert!(!sample_resolution_is_a_default(
            &render_args(&[]),
            &Sources::default()
        ));
    }

    /// `--sample` alone drops to 720p, and the user did not ask for that.
    #[test]
    fn a_sample_render_that_names_no_resolution_warns_that_it_was_reduced() {
        assert!(sample_resolution_is_a_default(
            &render_args(&["--sample", "30s"]),
            &Sources::default()
        ));
    }

    /// A resolution the user wrote down outranks the sample default, so nothing
    /// was reduced and nothing needs saying — from either layer that can say it.
    #[test]
    fn a_configured_resolution_silences_the_sample_resolution_warning() {
        let args = render_args(&["--sample", "30s"]);
        let resolution: Resolution = "1080p".parse().expect("a legal resolution");

        let from_file = Sources {
            file: ConfigLayer {
                output: avz_core::config::OutputLayer {
                    resolution: Some(resolution),
                    ..Default::default()
                },
                ..ConfigLayer::default()
            },
            ..Sources::default()
        };
        assert!(!sample_resolution_is_a_default(&args, &from_file));

        let from_set = Sources {
            set: ConfigLayer::from_set_assignments(&["output.resolution=1080p".to_owned()])
                .expect("a legal `--set`"),
            ..Sources::default()
        };
        assert!(!sample_resolution_is_a_default(&args, &from_set));
    }

    /// The warning names the size it dropped to and the key that undoes it.
    #[test]
    fn the_sample_resolution_warning_names_the_size_and_the_way_out() {
        let warning = sample_resolution_warning("1280x720".parse().expect("a legal resolution"));

        assert!(warning.contains("1280x720"), "{warning}");
        assert!(warning.contains("output.resolution"), "{warning}");
        assert!(
            warning.contains('—'),
            "a warning names the consequence and the action: {warning}",
        );
    }
}
