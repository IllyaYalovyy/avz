//! Unit tests for the config schema, its parsers, and the precedence chain.
//!
//! Two rows of the `docs/TESTING.md` risk matrix live here: "config precedence
//! wrong (`--set` loses to file)" and "unknown TOML key silently ignored". Both
//! break reproducible renders quietly, which is the worst way to break them.

use std::path::{Path, PathBuf};

use super::*;

/// `f32` comparisons in these tests only ever check that the right *source*
/// value survived the merge, never the result of arithmetic.
fn assert_close(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < 1e-6,
        "expected {expected}, got {actual}"
    );
}

fn layer(source: &str) -> ConfigLayer {
    ConfigLayer::from_toml_str(source).expect("layer parses")
}

// ---------------------------------------------------------------------------
// Precedence: CLI > --set > --config > preset defaults > built-in defaults
// ---------------------------------------------------------------------------

#[test]
fn set_override_beats_config_file_value() {
    let sources = Sources {
        file: layer("[visual]\nintensity = 1.0\n"),
        set: ConfigLayer::from_set_assignments(["visual.intensity=1.4"]).expect("--set parses"),
        ..Sources::default()
    };

    let config = sources.resolve().expect("resolves");
    assert_close(config.visual.intensity, 1.4);
}

#[test]
fn cli_flag_beats_set_override() {
    let cli = ConfigLayer {
        visual: VisualLayer {
            preset: Some("pulse".to_owned()),
            ..VisualLayer::default()
        },
        ..ConfigLayer::default()
    };

    let sources = Sources {
        file: layer("[visual]\npreset = \"ink\"\n"),
        set: ConfigLayer::from_set_assignments(["visual.preset=nebula"]).expect("--set parses"),
        cli,
        ..Sources::default()
    };

    let config = sources.resolve().expect("resolves");
    assert_eq!(config.visual.preset, "pulse");
}

#[test]
fn config_file_beats_preset_defaults() {
    let sources = Sources {
        preset_defaults: layer("[output]\nfps = 60\n"),
        file: layer("[output]\nfps = 24\n"),
        ..Sources::default()
    };

    assert_eq!(sources.resolve().expect("resolves").output.fps, 24);
}

#[test]
fn preset_defaults_beat_builtin_defaults() {
    let sources = Sources {
        preset_defaults: layer("[visual]\nsmoothing = 0.8\n"),
        ..Sources::default()
    };

    assert_close(sources.resolve().expect("resolves").visual.smoothing, 0.8);
}

/// `--sample` exists for fast iteration, so it trades pixels for turnaround
/// (`VISION.md` §3). Nothing else in the chain has to opt in.
#[test]
fn a_sample_render_defaults_to_a_reduced_resolution() {
    let sources = Sources {
        sample_defaults: ConfigLayer::for_sample(),
        ..Sources::default()
    };

    let config = sources.resolve().expect("resolves");
    assert_eq!(config.output.resolution.to_string(), "1280x720");
    assert_eq!(config.output.fps, 30, "only the resolution is reduced");
}

/// A default, not a decree: someone sampling to check the final look must be
/// able to ask for the final resolution.
#[test]
fn an_explicit_resolution_beats_the_sample_default() {
    let sources = Sources {
        sample_defaults: ConfigLayer::for_sample(),
        file: layer("[output]\nresolution = \"1080p\"\n"),
        ..Sources::default()
    };

    let config = sources.resolve().expect("resolves");
    assert_eq!(config.output.resolution.to_string(), "1920x1080");
}

/// The sample default is opt-in: a whole-song render must stay at 1080p.
#[test]
fn a_whole_song_render_keeps_the_builtin_resolution() {
    let config = Sources::default().resolve().expect("resolves");

    assert_eq!(config.output.resolution.to_string(), "1920x1080");
}

#[test]
fn a_silent_layer_does_not_erase_a_lower_one() {
    let sources = Sources {
        file: layer("[output]\nfps = 24\nquality = 20\n"),
        set: ConfigLayer::from_set_assignments(["output.quality=30"]).expect("--set parses"),
        ..Sources::default()
    };

    let config = sources.resolve().expect("resolves");
    assert_eq!(config.output.fps, 24, "the file's fps must survive --set");
    assert_eq!(config.output.quality, 30);
}

#[test]
fn builtin_defaults_are_a_complete_1080p30_render() {
    let config = Sources::default().resolve().expect("resolves");

    assert_eq!(config.output.resolution.to_string(), "1920x1080");
    assert_eq!(config.output.fps, 30);
    assert_eq!(config.output.codec, Codec::X264);
    assert_eq!(config.output.quality, 18);
    assert_eq!(config.visual.seed, Seed::Auto);
    assert_eq!(config.background.source, None);
    assert!(config.text.enabled);
    assert_eq!(config.text.title, None, "the card's words come from ID3");
    assert_eq!(config.text.artist, None);
}

// ---------------------------------------------------------------------------
// Strict keys and "did you mean"
// ---------------------------------------------------------------------------

#[test]
fn unknown_toml_key_rejected_with_suggestion() {
    let err = ConfigLayer::from_toml_str("[visual]\nintencity = 1.0\n").expect_err("rejected");
    let message = err.to_string();

    assert!(message.contains("intencity"), "{message}");
    assert!(message.contains("did you mean `intensity`"), "{message}");
    assert!(message.contains("line 2"), "{message}");
}

#[test]
fn unknown_top_level_table_rejected_with_suggestion() {
    let err = ConfigLayer::from_toml_str("[backgrund]\nblur = 1.0\n").expect_err("rejected");
    let message = err.to_string();

    assert!(message.contains("did you mean `background`"), "{message}");
}

#[test]
fn unknown_enum_value_rejected_with_suggestion() {
    let err = ConfigLayer::from_toml_str("[background]\nfit = \"covr\"\n").expect_err("rejected");
    let message = err.to_string();

    assert!(message.contains("did you mean `cover`"), "{message}");
}

#[test]
fn unrelated_unknown_key_is_rejected_without_a_misleading_hint() {
    let err = ConfigLayer::from_toml_str("[visual]\nzzzzzzzz = 1.0\n").expect_err("rejected");
    let message = err.to_string();

    assert!(message.contains("zzzzzzzz"), "{message}");
    assert!(!message.contains("did you mean"), "{message}");
}

#[test]
fn unknown_key_in_a_config_file_names_the_file_and_the_line() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("album.toml");
    std::fs::write(&path, "[output]\nfps = 30\nfpss = 60\n").expect("write");

    let err = ConfigLayer::from_file(&path).expect_err("rejected");
    let message = err.to_string();

    assert!(message.contains("album.toml"), "{message}");
    assert!(message.contains("line 3"), "{message}");
    assert!(message.contains("did you mean `fps`"), "{message}");
}

#[test]
fn a_missing_config_file_is_an_input_problem_not_a_config_problem() {
    let err = ConfigLayer::from_file(Path::new("/nonexistent/avz-does-not-exist.toml"))
        .expect_err("rejected");

    assert!(
        matches!(err, Error::Input(_)),
        "expected an input error, got {err:?}"
    );
}

#[test]
fn preset_params_accept_arbitrary_keys() {
    let config = layer("[visual.params]\nparticle_count = 4000\nbass_drive = 1.2\n")
        .resolve()
        .expect("resolves");

    assert_eq!(
        config.visual.params["particle_count"].as_integer(),
        Some(4000)
    );
}

// ---------------------------------------------------------------------------
// --set
// ---------------------------------------------------------------------------

#[test]
fn set_assignments_apply_in_order_with_the_last_one_winning() {
    let layer = ConfigLayer::from_set_assignments(["output.fps=24", "output.fps=60"])
        .expect("--set parses");

    assert_eq!(layer.output.fps, Some(60));
}

#[test]
fn set_infers_toml_types_and_falls_back_to_strings() {
    let layer = ConfigLayer::from_set_assignments([
        "output.fps=60",
        "output.resolution=720p",
        "visual.intensity=1.4",
        "visual.seed=7",
        "text.enabled=false",
        "background.image=art/forest.png",
    ])
    .expect("--set parses");

    assert_eq!(layer.output.fps, Some(60));
    assert_eq!(
        layer.output.resolution.map(|r| r.to_string()).as_deref(),
        Some("1280x720")
    );
    assert_eq!(layer.visual.seed, Some(Seed::Fixed(7)));
    assert_eq!(layer.text.enabled, Some(false));
    assert_eq!(
        layer.background.image,
        Some(PathBuf::from("art/forest.png"))
    );
}

#[test]
fn set_merges_preset_params_key_wise_instead_of_replacing_them() {
    let sources = Sources {
        file: layer("[visual.params]\nparticle_count = 4000\nbass_drive = 1.2\n"),
        set: ConfigLayer::from_set_assignments(["visual.params.bass_drive=2.0"])
            .expect("--set parses"),
        ..Sources::default()
    };

    let params = sources.resolve().expect("resolves").visual.params;
    assert_eq!(params["particle_count"].as_integer(), Some(4000));
    assert_eq!(params["bass_drive"].as_float(), Some(2.0));
}

/// `VISION.md` §3 shows `--set visual.intensity=1.4`, but a preset parameter
/// lives two tables down. A bare name is the spelling worth typing, and it can
/// only mean one thing: a parameter of the preset being rendered.
#[test]
fn a_bare_set_key_is_a_parameter_of_the_active_preset() {
    let layer = ConfigLayer::from_set_assignments(["bass_drive=2.0"]).expect("--set parses");

    let params = layer.visual.params.expect("a params table");
    assert_eq!(params["bass_drive"].as_float(), Some(2.0));
}

/// `--set pulse.bass_drive=2` names the preset the parameter belongs to, which
/// is how a config file or a shell history reads years later.
#[test]
fn a_preset_prefixed_set_key_is_a_parameter_of_that_preset() {
    let layer = ConfigLayer::from_set_assignments(["pulse.bass_drive=2.0"]).expect("--set parses");

    let params = layer.visual.params.expect("a params table");
    assert_eq!(params["bass_drive"].as_float(), Some(2.0));
}

/// The shorthand must not swallow a typo'd section name and turn it into a
/// parameter nobody declared — the error would then name the wrong thing.
#[test]
fn a_set_key_under_an_unknown_section_names_the_sections_and_the_presets() {
    let err = ConfigLayer::from_set_assignments(["outputt.fps=30"]).expect_err("rejected");
    let message = err.to_string();

    assert!(message.contains("outputt.fps=30"), "{message}");
    assert!(message.contains("did you mean `output`"), "{message}");

    // A preset avz does not ship, spelled close enough to one it does that the
    // shorthand would happily have taken it for a section.
    let err = ConfigLayer::from_set_assignments(["nebulaa.turbulence=2"]).expect_err("rejected");
    assert!(
        err.to_string().contains("nebula"),
        "an unshipped preset must name the ones that do exist: {err}"
    );
}

/// Shorthand and long form reach the same table, so a config file and a `--set`
/// can be mixed without surprise.
#[test]
fn the_shorthand_and_the_long_form_set_the_same_parameter() {
    let long = ConfigLayer::from_set_assignments(["visual.params.bass_drive=2.0"]);
    let short = ConfigLayer::from_set_assignments(["bass_drive=2.0"]);

    assert_eq!(long.expect("parses"), short.expect("parses"));
}

#[test]
fn unknown_set_key_is_rejected_with_a_suggestion_and_the_assignment() {
    let err = ConfigLayer::from_set_assignments(["visual.intencity=1.4"]).expect_err("rejected");
    let message = err.to_string();

    assert!(message.contains("visual.intencity=1.4"), "{message}");
    assert!(message.contains("did you mean `intensity`"), "{message}");
}

#[test]
fn set_without_an_equals_sign_is_rejected() {
    let err = ConfigLayer::from_set_assignments(["visual.intensity"]).expect_err("rejected");
    assert!(err.to_string().contains("key.path=value"), "{err}");
}

#[test]
fn set_with_an_empty_path_segment_is_rejected() {
    let err = ConfigLayer::from_set_assignments(["visual..intensity=1.0"]).expect_err("rejected");
    assert!(err.to_string().contains("empty"), "{err}");
}

// ---------------------------------------------------------------------------
// Cross-field validation
// ---------------------------------------------------------------------------

#[test]
fn background_image_and_video_together_rejected() {
    let err = layer("[background]\nimage = \"a.png\"\nvideo = \"b.mp4\"\n")
        .resolve()
        .expect_err("rejected");

    let message = err.to_string();
    assert!(message.contains("mutually exclusive"), "{message}");
}

#[test]
fn a_higher_layer_background_source_replaces_the_lower_one() {
    let sources = Sources {
        file: layer("[background]\nimage = \"art/forest.png\"\n"),
        set: ConfigLayer::from_set_assignments(["background.video=loops/smoke.mp4"])
            .expect("--set parses"),
        ..Sources::default()
    };

    let config = sources.resolve().expect("resolves");
    assert_eq!(
        config.background.source,
        Some(BackgroundSource::Video(PathBuf::from("loops/smoke.mp4")))
    );
}

#[test]
fn out_of_range_values_are_rejected_by_name() {
    let cases = [
        ("[output]\nfps = 0\n", "fps"),
        ("[output]\nquality = 99\n", "quality"),
        ("[visual]\nintensity = 0.0\n", "intensity"),
        ("[visual]\nsmoothing = 1.5\n", "smoothing"),
        ("[background]\ndarken = -0.1\n", "darken"),
        ("[background]\nblur = -1.0\n", "blur"),
        ("[visual]\npreset = \"\"\n", "preset"),
    ];

    for (source, key) in cases {
        let err = layer(source).resolve().expect_err("rejected");
        assert!(err.to_string().contains(key), "{source:?} -> {err}");
    }
}

#[test]
fn blank_string_values_are_rejected_by_name() {
    // A blank value is a typo or a shell variable that expanded to nothing. Taking
    // it literally defers the failure to the renderer, which can only report that
    // it cannot open `""`.
    let cases = [
        ("[visual]\npreset = \"   \"\n", "preset"),
        ("[background]\nimage = \"\"\n", "background.image"),
        ("[background]\nvideo = \"   \"\n", "background.video"),
    ];

    for (source, key) in cases {
        let err = layer(source).resolve().expect_err("rejected");
        assert!(err.to_string().contains(key), "{source:?} -> {err}");
    }
}

/// The card's timing and geometry are numbers with meanings, and every one of
/// them has a range outside which the card is not a card.
#[test]
fn out_of_range_text_geometry_is_rejected_by_name() {
    let cases = [
        ("[text]\nsize = 0.0\n", "text.size"),
        ("[text]\nsize = 1.5\n", "text.size"),
        ("[text]\nmargin = -0.1\n", "text.margin"),
        ("[text]\nmargin = 0.5\n", "text.margin"),
    ];

    for (source, key) in cases {
        let err = layer(source).resolve().expect_err("rejected");
        assert!(err.to_string().contains(key), "{source:?} -> {err}");
    }
}

/// `--title` and `--artist` override the ID3 tags, so they are settings like any
/// other and reach the renderer through the same chain.
#[test]
fn text_overrides_and_fade_survive_the_precedence_chain() {
    let sources = Sources {
        file: layer("[text]\ntitle = \"From the file\"\nfade = \"0.25s\"\n"),
        cli: layer("[text]\ntitle = \"From the flag\"\n"),
        ..Sources::default()
    };

    let config = sources.resolve().expect("resolves");

    assert_eq!(config.text.title.as_deref(), Some("From the flag"));
    assert_close(config.text.fade.as_secs_f64() as f32, 0.25);
    assert_eq!(
        config.text.artist, None,
        "an override nobody wrote must not invent an artist"
    );
}

#[test]
fn a_blank_font_path_is_rejected() {
    for input in ["", "   "] {
        let err = input.parse::<FontChoice>().expect_err("rejected");
        assert!(err.to_string().contains("font"), "{input:?} -> {err}");
    }

    // The same parser backs the TOML key, so the file rejects it too.
    ConfigLayer::from_toml_str("[text]\nfont = \"\"\n").expect_err("rejected");
}

// ---------------------------------------------------------------------------
// Value parsers
// ---------------------------------------------------------------------------

#[test]
fn parses_named_and_wxh_resolutions() {
    let cases = [
        ("1920x1080", 1920, 1080),
        ("720p", 1280, 720),
        ("1080p", 1920, 1080),
        ("4k", 3840, 2160),
        ("4K", 3840, 2160),
        (" 1280X720 ", 1280, 720),
    ];

    for (input, width, height) in cases {
        let resolution: Resolution = input.parse().unwrap_or_else(|e| panic!("{input}: {e}"));
        assert_eq!(
            (resolution.width, resolution.height),
            (width, height),
            "{input}"
        );
    }
}

#[test]
fn rejects_garbage_resolution() {
    for input in [
        "1080",
        "abc",
        "1920x",
        "x1080",
        "0x0",
        "1920x1080x30",
        "-8x8",
    ] {
        let err = input.parse::<Resolution>().expect_err(input);
        assert!(err.to_string().contains("1920x1080"), "{input}: {err}");
    }
}

#[test]
fn rejects_odd_resolution_dimensions() {
    // yuv420p chroma subsampling needs even dimensions; ffmpeg would fail late.
    let err = "1921x1080".parse::<Resolution>().expect_err("rejected");
    assert!(err.to_string().contains("even"), "{err}");
}

#[test]
fn parses_sample_range_and_bare_duration() {
    let bare: SampleRange = "60s".parse().expect("parses");
    assert_close(bare.start.as_secs_f64() as f32, 0.0);
    assert_close(bare.end.as_secs_f64() as f32, 60.0);

    let range: SampleRange = "0:45..1:45".parse().expect("parses");
    assert_close(range.start.as_secs_f64() as f32, 45.0);
    assert_close(range.end.as_secs_f64() as f32, 105.0);
    assert_close(range.duration_secs() as f32, 60.0);
}

#[test]
fn rejects_a_sample_range_that_ends_before_it_starts() {
    let err = "1:45..0:45".parse::<SampleRange>().expect_err("rejected");
    assert!(err.to_string().contains("after"), "{err}");
}

#[test]
fn parses_durations_with_units_and_clock_notation() {
    let cases = [
        ("1.0s", 1.0),
        ("0s", 0.0),
        ("250ms", 0.25),
        ("0:45", 45.0),
        ("1:45.5", 105.5),
        ("1:02:03", 3723.0),
    ];

    for (input, expected) in cases {
        let seconds: Seconds = input.parse().unwrap_or_else(|e| panic!("{input}: {e}"));
        assert!(
            (seconds.as_secs_f64() - expected).abs() < 1e-9,
            "{input}: expected {expected}, got {}",
            seconds.as_secs_f64()
        );
    }
}

#[test]
fn rejects_a_duration_without_a_unit() {
    let err = "1.0".parse::<Seconds>().expect_err("rejected");
    assert!(err.to_string().contains("1.0s"), "{err}");

    for input in ["-1s", "abc", "1:75", "s"] {
        input.parse::<Seconds>().expect_err(input);
    }
}

#[test]
fn parses_hex_colors_in_every_documented_form() {
    assert_eq!(
        "#1a1a2e".parse::<Color>().expect("parses"),
        Color {
            r: 0x1a,
            g: 0x1a,
            b: 0x2e,
            a: 0xff
        }
    );
    assert_eq!(
        "#fff".parse::<Color>().expect("parses"),
        Color {
            r: 0xff,
            g: 0xff,
            b: 0xff,
            a: 0xff
        }
    );
    assert_eq!(
        "#1a1a2e80".parse::<Color>().expect("parses"),
        Color {
            r: 0x1a,
            g: 0x1a,
            b: 0x2e,
            a: 0x80
        }
    );
}

#[test]
fn rejects_malformed_hex_colors() {
    for input in ["1a1a2e", "#12345", "#gggggg", "#"] {
        let err = input.parse::<Color>().expect_err(input);
        assert!(err.to_string().contains('#'), "{input}: {err}");
    }
}

#[test]
fn parses_named_and_inline_palettes() {
    let named = layer("[visual]\npalette = \"ember\"\n");
    assert_eq!(
        named.visual.palette,
        Some(Palette::Named("ember".to_owned()))
    );

    let inline = layer("[visual]\npalette = [\"#1a1a2e\", \"#e94560\"]\n");
    let Some(Palette::Inline(colors)) = inline.visual.palette else {
        panic!("expected an inline palette");
    };
    assert_eq!(colors.len(), 2);
}

/// An inline palette is resampled onto the uniform's five slots, so it may hold
/// more colors than the uniform does — but not so many that the palette the user
/// gets stops resembling the one they wrote.
#[test]
fn an_inline_palette_takes_between_two_and_eight_colors() {
    let black: Color = "#000000".parse().expect("parses");

    for count in MIN_PALETTE_COLORS..=MAX_PALETTE_COLORS {
        Palette::inline(vec![black; count]).unwrap_or_else(|err| panic!("{count} colors: {err}"));
    }

    for count in [0, MIN_PALETTE_COLORS - 1, MAX_PALETTE_COLORS + 1] {
        let err = Palette::inline(vec![black; count]).expect_err("out of range");
        let message = err.to_string();
        assert!(
            message.contains(&MIN_PALETTE_COLORS.to_string())
                && message.contains(&MAX_PALETTE_COLORS.to_string()),
            "{count} colors must be refused with the range that is allowed: {message}",
        );
    }
}

/// A single color is not a palette: every slot of the uniform would hold it, and
/// there is nothing to interpolate between.
#[test]
fn rejects_an_inline_palette_of_one_color() {
    let err = "#e94560".parse::<Palette>().expect_err("one color");
    assert!(err.to_string().contains("got 1"), "{err}");
}

/// `--palette` outranks `--set visual.palette` and the config file, the way
/// every other CLI flag does (`VISION.md` §5.5).
#[test]
fn a_palette_flag_beats_a_set_override_and_the_config_file() {
    let cli = ConfigLayer {
        visual: VisualLayer {
            palette: Some(Palette::Named("mono".to_owned())),
            ..VisualLayer::default()
        },
        ..ConfigLayer::default()
    };

    let sources = Sources {
        file: layer("[visual]\npalette = \"glacier\"\n"),
        set: ConfigLayer::from_set_assignments(["visual.palette=verdant"]).expect("--set parses"),
        cli,
        ..Sources::default()
    };

    let config = sources.resolve().expect("resolves");
    assert_eq!(config.visual.palette, Palette::Named("mono".to_owned()));
}

/// A bad hex color is reported by its position in the array, so nobody counts
/// commas to find it. Both spellings — the TOML array and the `--palette` string.
#[test]
fn bad_hex_rejected_with_position() {
    let err = ConfigLayer::from_toml_str("[visual]\npalette = [\"#1a1a2e\", \"#gg0000\"]\n")
        .expect_err("`#gg0000` is not a color");
    assert!(err.to_string().contains("palette entry 2"), "{err}");

    let err = "#1a1a2e,#e94560,e94560"
        .parse::<Palette>()
        .expect_err("the third color is missing its `#`");
    assert!(err.to_string().contains("palette entry 3"), "{err}");

    let err = ConfigLayer::from_toml_str("[visual]\npalette = []\n")
        .expect_err("an empty list is not a palette");
    assert!(err.to_string().contains("got 0"), "{err}");
}

#[test]
fn parses_seed_auto_and_fixed() {
    assert_eq!(
        layer("[visual]\nseed = \"auto\"\n").visual.seed,
        Some(Seed::Auto)
    );
    assert_eq!(
        layer("[visual]\nseed = 42\n").visual.seed,
        Some(Seed::Fixed(42))
    );

    let err = ConfigLayer::from_toml_str("[visual]\nseed = -1\n").expect_err("rejected");
    assert!(err.to_string().contains("negative"), "{err}");
}

#[test]
fn parses_the_nine_grid_positions() {
    for position in Position::ALL {
        assert_eq!(
            position.as_str().parse::<Position>().expect("parses"),
            *position
        );
    }

    let err = "middle".parse::<Position>().expect_err("rejected");
    assert!(err.to_string().contains("bottom-left"), "{err}");
}

#[test]
fn parses_the_font_choice() {
    assert_eq!(
        "auto".parse::<FontChoice>().expect("parses"),
        FontChoice::Auto
    );
    assert_eq!(
        "/usr/share/fonts/x.ttf"
            .parse::<FontChoice>()
            .expect("parses"),
        FontChoice::Path(PathBuf::from("/usr/share/fonts/x.ttf"))
    );
}

#[test]
fn parses_every_codec_name() {
    for codec in [Codec::X264, Codec::X265, Codec::Av1] {
        assert_eq!(codec.as_str().parse::<Codec>().expect("parses"), codec);
    }
}

// ---------------------------------------------------------------------------
// The documented example must stay true
// ---------------------------------------------------------------------------

#[test]
fn example_from_vision_5_5_parses() {
    let config = ConfigLayer::from_toml_str(&vision_toml_example())
        .expect("the VISION §5.5 example parses")
        .resolve()
        .expect("the VISION §5.5 example resolves");

    assert_eq!(config.output.resolution.to_string(), "1920x1080");
    assert_eq!(config.output.fps, 30);
    assert_eq!(config.output.codec, Codec::X264);
    assert_eq!(config.visual.preset, "nebula");
    assert_eq!(config.visual.palette, Palette::Named("ember".to_owned()));
    assert_eq!(config.visual.seed, Seed::Auto);
    assert_eq!(
        config.visual.params["particle_count"].as_integer(),
        Some(4000)
    );
    assert_eq!(
        config.background.source,
        Some(BackgroundSource::Image(PathBuf::from("art/forest.png")))
    );
    assert_eq!(config.background.fit, Fit::Cover);
    assert_eq!(config.text.position, Position::BottomLeft);
    assert_close(config.text.in_at.as_secs_f64() as f32, 1.0);
    assert_eq!(config.text.font, FontChoice::Auto);
}

/// Pull the one ```toml block out of `VISION.md`, so the schema and the
/// documented example cannot drift apart without a test failing.
fn vision_toml_example() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../VISION.md");
    let markdown = std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("read {}: {err}", path.display()));

    let mut blocks = Vec::new();
    let mut current: Option<Vec<&str>> = None;

    for line in markdown.lines() {
        match current.as_mut() {
            None if line.trim() == "```toml" => current = Some(Vec::new()),
            None => {}
            Some(_) if line.trim() == "```" => {
                blocks.push(current.take().expect("inside a block").join("\n"));
            }
            Some(buffer) => buffer.push(line),
        }
    }

    assert_eq!(
        blocks.len(),
        1,
        "VISION.md should hold exactly one ```toml block: the §5.5 example"
    );
    blocks.remove(0)
}
