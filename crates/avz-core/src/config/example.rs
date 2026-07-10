//! The documented TOML template `avz config --example` prints (`VISION.md` §5.5).
//!
//! Every live key is written from [`Config::default()`], never from a literal, so
//! a default that moves moves the template with it. What cannot be derived that
//! way — which keys exist, and what each one means — is pinned by the meta-tests
//! below: `every_declared_key_is_documented` reads the field list out of
//! serde's own `unknown field` message, so a key added to [`ConfigLayer`] and
//! forgotten here fails the build rather than quietly going undocumented.
//!
//! Keys with no default are commented out. `background.image`, `text.title`, and
//! the preset's own `[visual.params]` cannot be given a value that is both
//! illustrative and correct: uncommenting `image = "art/forest.png"` should be
//! the user's decision, and a `params` key that names no parameter of the active
//! preset is a hard error at render time.

use std::fmt::Write as _;

use super::{Config, Seed};

/// A documented config file holding every setting at its built-in default.
///
/// Round-trips: parsing this text under the strict schema and resolving it
/// yields exactly [`Config::default()`], which is what makes it a safe starting
/// point rather than a second, drifting set of defaults.
pub fn example() -> String {
    let config = Config::default();
    let mut out = String::new();

    out.push_str(HEADER);

    out.push_str("\n[output]\n");
    key(
        &mut out,
        "Frame size: `720p`, `1080p`, `4k`, or `WIDTHxHEIGHT` with even sides.",
        "resolution",
        &quoted(config.output.resolution),
    );
    key(
        &mut out,
        "Frames per second.",
        "fps",
        &config.output.fps.to_string(),
    );
    key(
        &mut out,
        "Video codec. avz v0.1 encodes `x264` only.",
        "codec",
        &quoted(config.output.codec.as_str()),
    );
    key(
        &mut out,
        &format!(
            "x264 CRF quality, 0 (visually lossless, huge) to {} (worst).",
            super::MAX_CRF
        ),
        "quality",
        &config.output.quality.to_string(),
    );

    out.push_str("\n[visual]\n");
    key(
        &mut out,
        "Which preset draws the video. `avz presets` lists them.",
        "preset",
        &quoted(&config.visual.preset),
    );
    key(
        &mut out,
        "A built-in palette name, or inline hex: [\"#1a1a2e\", \"#e94560\"].",
        "palette",
        &palette(&config.visual.palette),
    );
    key(
        &mut out,
        "Global motion scale. Greater than 0.",
        "intensity",
        &float(config.visual.intensity),
    );
    key(
        &mut out,
        "Global envelope decay scale, 0 to 1. Higher is slower and smoother.",
        "smoothing",
        &float(config.visual.smoothing),
    );
    key(
        &mut out,
        "`\"auto\"` hashes the input file's name, so re-rendering the same song \
         anywhere gives the same video. Any non-negative integer fixes it instead.",
        "seed",
        &seed(config.visual.seed),
    );

    out.push('\n');
    out.push_str(&commented(
        "Preset parameters, validated against the active preset's schema. Run\n\
         `avz presets <name>` to see the parameters it takes and their defaults.",
        "[visual.params]\nbass_drive = 1.2",
    ));

    out.push_str("\n[background]\n");
    out.push_str(&commented(
        "A still image beneath the visuals. Mutually exclusive with `video`.",
        "image = \"art/forest.png\"",
    ));
    out.push_str(&commented(
        "A looped, muted video beneath the visuals. Mutually exclusive with `image`.\n\
         ffmpeg loops, scales, and frame-rate-converts it; it always starts at its\n\
         first frame, so `--sample` moves the song and never the loop.",
        "video = \"loops/smoke.mp4\"",
    ));
    key(
        &mut out,
        "How the source is fitted to the frame: `cover`, `contain`, or `stretch`.",
        "fit",
        &quoted(config.background.fit.as_str()),
    );
    key(
        &mut out,
        "Gaussian blur of the background, as a standard deviation in output pixels.",
        "blur",
        &float(config.background.blur),
    );
    key(
        &mut out,
        "How much of the background's light to take away, 0 to 1.",
        "darken",
        &float(config.background.darken),
    );

    out.push_str("\n[text]\n");
    key(
        &mut out,
        "Draw the title/artist card at all.",
        "enabled",
        &config.text.enabled.to_string(),
    );
    key(
        &mut out,
        "Where the card sits: `top-left` through `bottom-right`, or `center`.",
        "position",
        &quoted(config.text.position.as_str()),
    );
    key(
        &mut out,
        "When the card starts fading in.",
        "in_at",
        &quoted(config.text.in_at),
    );
    key(
        &mut out,
        "How long the card holds at full opacity, between the two fades.",
        "hold",
        &quoted(config.text.hold),
    );
    key(
        &mut out,
        "How long each fade lasts, in and out alike.",
        "fade",
        &quoted(config.text.fade),
    );
    key(
        &mut out,
        "`auto` uses the bundled font; anything else is a path to a font file.",
        "font",
        &quoted("auto"),
    );
    key(
        &mut out,
        "Title type size, as a fraction of the frame height.",
        "size",
        &float(config.text.size),
    );
    key(
        &mut out,
        "Distance from the frame edge, as a fraction of the frame height.",
        "margin",
        &float(config.text.margin),
    );
    out.push('\n');
    out.push_str(&commented(
        "Title and artist for the card, overriding the mp3's ID3 tags.",
        "title = \"Cold Design\"\nartist = \"avz\"",
    ));

    out
}

/// What the file says about itself before it says anything about a render.
const HEADER: &str = "\
# avz configuration, written by `avz config --example`.
#
# Every key below carries its built-in default, so this file renders what
# `avz render song.mp3` renders. Delete what you do not want to pin.
#
# Precedence: CLI flags > `--set key=value` > this file > preset defaults >
# built-in defaults. Unknown keys are rejected, with a suggestion.
#
# Commented-out keys have no default. Uncomment one to turn it on.
";

/// One documented `key = value` line, preceded by its comment.
fn key(out: &mut String, comment: &str, name: &str, value: &str) {
    out.push_str(&wrap(comment));
    let _ = writeln!(out, "{name} = {value}");
}

/// A documented block that is commented out because it has no default.
fn commented(comment: &str, body: &str) -> String {
    let mut out = wrap(comment);
    for line in body.lines() {
        let _ = writeln!(out, "# {line}");
    }
    out
}

/// The width a comment wraps at, chosen so the file reads in an 80-column
/// terminal beside the `--help` that points at it.
const COMMENT_WIDTH: usize = 78;

/// Wrap `comment` into `# `-prefixed lines. Newlines in the input are kept.
fn wrap(comment: &str) -> String {
    let mut out = String::new();

    for paragraph in comment.lines() {
        let mut line = String::from("#");
        for word in paragraph.split_whitespace() {
            if line.chars().count() + 1 + word.chars().count() > COMMENT_WIDTH && line != "#" {
                out.push_str(&line);
                out.push('\n');
                line = String::from("#");
            }
            line.push(' ');
            line.push_str(word);
        }
        out.push_str(&line);
        out.push('\n');
    }

    out
}

/// A TOML string, from anything that knows how to display itself.
///
/// Every value written this way round-trips through its own [`FromStr`], because
/// that is what `example_parses_under_strict_validation_into_the_built_in_defaults`
/// checks — and none of
/// them can contain a quote to escape.
///
/// [`FromStr`]: std::str::FromStr
fn quoted(value: impl std::fmt::Display) -> String {
    format!("\"{value}\"")
}

/// A float with its fractional part intact.
///
/// `Debug`, not `Display`: TOML reads `1` as an integer and refuses to
/// deserialize it into an `Option<f64>`, so `intensity = 1.0` has to keep the
/// `.0` that `Display` drops.
fn float(value: f32) -> String {
    format!("{value:?}")
}

/// The palette, as the config file spells it.
fn palette(palette: &super::Palette) -> String {
    match palette {
        super::Palette::Named(name) => quoted(name),
        super::Palette::Inline(colors) => {
            let colors: Vec<String> = colors.iter().map(quoted).collect();
            format!("[{}]", colors.join(", "))
        }
    }
}

/// The seed, as the config file spells it: `"auto"` or a bare integer.
fn seed(seed: Seed) -> String {
    match seed {
        Seed::Auto => quoted("auto"),
        Seed::Fixed(value) => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConfigLayer;

    /// UT-008, the round trip: what `avz config --example` prints is a config
    /// file avz accepts, and one that changes nothing.
    ///
    /// Strict validation is the point. A template with a stale key would be the
    /// worst possible first experience: the tool telling the user that the file
    /// the tool wrote is wrong.
    #[test]
    fn example_parses_under_strict_validation_into_the_built_in_defaults() {
        let parsed = ConfigLayer::from_toml_str(&example())
            .expect("the example avz prints is a config avz accepts");

        assert_eq!(
            parsed.resolve().expect("the example resolves"),
            Config::default(),
            "the example must document the defaults, not a second set of them",
        );
    }

    /// The example is a *template*: it must not enable anything a bare render
    /// leaves off, or `--config avz.toml` would silently ask for a background.
    #[test]
    fn example_turns_nothing_on_that_a_bare_render_leaves_off() {
        let parsed = ConfigLayer::from_toml_str(&example()).expect("parses");

        assert_eq!(parsed.background.image, None);
        assert_eq!(parsed.background.video, None);
        assert_eq!(parsed.text.title, None);
        assert_eq!(parsed.text.artist, None);
        assert_eq!(
            parsed.visual.params, None,
            "a `[visual.params]` key names a parameter of one preset, and would \
             fail against every other preset's schema",
        );
    }

    /// The meta-test: every key the schema declares is documented.
    ///
    /// The field list is read out of serde's own `unknown field` message rather
    /// than written down here, so it cannot fall behind [`ConfigLayer`]. A new
    /// key reaches a user's config file only through this template.
    #[test]
    fn every_declared_key_is_documented() {
        let example = example();

        for section in declared_keys("") {
            let body = section_body(&example, &section);
            assert!(
                !body.is_empty(),
                "the example documents no `[{section}]` section",
            );

            for key in declared_keys(&section) {
                assert!(
                    mentions(&body, &section, &key),
                    "`{section}.{key}` is a config key the example never mentions; \
                     document it in `config::example`",
                );
            }
        }
    }

    /// The names of the values that comment claims are legal really are.
    #[test]
    fn the_example_documents_the_units_the_parsers_accept() {
        let example = example();

        for expected in [
            "`720p`",
            "`1080p`",
            "`4k`",
            "`x264`",
            "`cover`",
            "`contain`",
            "`stretch`",
            "`top-left`",
            "`bottom-right`",
            "`center`",
            "`auto`",
        ] {
            assert!(
                example.contains(expected),
                "the example never names {expected}: {example}",
            );
        }
    }

    /// Nothing but comments, section headers, and `key = value` lines: a config
    /// file a user edits should have no surprises in it.
    #[test]
    fn the_example_wraps_its_comments_to_a_readable_width() {
        for line in example().lines() {
            assert!(
                line.chars().count() <= COMMENT_WIDTH + 2,
                "an over-long line will wrap in a terminal: {line}",
            );
        }
    }

    /// The keys `ConfigLayer` (or one of its sections) declares, straight from
    /// serde's `unknown field` message.
    ///
    /// `section` is `""` for the top level. The message names the unknown field
    /// first and the accepted ones after it, all backticked.
    fn declared_keys(section: &str) -> Vec<String> {
        let source = match section {
            "" => "avz_no_such_key = 1\n".to_owned(),
            section => format!("[{section}]\navz_no_such_key = 1\n"),
        };

        let err = toml::from_str::<ConfigLayer>(&source)
            .expect_err("an unknown key is rejected")
            .to_string();

        let mut quoted = err.split('`').skip(1).step_by(2);
        assert_eq!(
            quoted.next(),
            Some("avz_no_such_key"),
            "serde names the unknown field first: {err}",
        );

        let keys: Vec<String> = quoted.map(str::to_owned).collect();
        assert!(!keys.is_empty(), "serde lists the accepted fields: {err}");
        keys
    }

    /// The lines of `example` under `[section]`, up to the next section header.
    fn section_body(example: &str, section: &str) -> Vec<String> {
        let header = format!("[{section}]");

        example
            .lines()
            .skip_while(|line| line.trim() != header)
            .skip(1)
            .take_while(|line| !is_header(line))
            .map(str::to_owned)
            .collect()
    }

    /// Whether a line opens a new top-level section. A commented-out subsection
    /// header (`# [visual.params]`) belongs to the section it sits in.
    fn is_header(line: &str) -> bool {
        line.starts_with('[') && line.ends_with(']')
    }

    /// Whether `body` documents `key`, live or commented out, as an assignment
    /// or as a subsection header.
    fn mentions(body: &[String], section: &str, key: &str) -> bool {
        let assignment = format!("{key} =");
        let subsection = format!("[{section}.{key}]");

        body.iter().any(|line| {
            let line = line.trim_start_matches('#').trim();
            line.starts_with(&assignment) || line == subsection
        })
    }
}
