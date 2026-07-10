//! TOML configuration: schema, validation, and merging.
//!
//! Precedence is fixed: CLI flags > `--set` overrides > `--config` file > preset
//! defaults > built-in defaults. Unknown keys are rejected with "did you mean"
//! suggestions rather than silently ignored (`VISION.md` §5.5).
//!
//! One layer sits inside that chain without appearing in it: the defaults
//! `--sample` implies ([`ConfigLayer::for_sample`]). They rank above preset
//! defaults and below the config file, so the reduced sample resolution
//! `VISION.md` §3 asks for is a default like any other — overridable, and never
//! able to displace something the user actually wrote.
//!
//! Two types carry the weight. [`ConfigLayer`] is one source of settings, with
//! every field optional — that is what makes "this layer has no opinion about
//! `fps`" expressible. [`Config`] is the fully-resolved, validated result the
//! pipeline consumes. [`Sources`] is the only place the precedence order is
//! written down.

mod set;
mod value;

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::{Error, Result};

pub use value::{
    Codec, Color, Fit, FontChoice, MAX_PALETTE_COLORS, MIN_PALETTE_COLORS, Palette, ParseError,
    Position, Resolution, SampleRange, Seconds, Seed,
};

/// A fully-resolved, validated configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// Frame size, rate, codec, and quality.
    pub output: Output,
    /// Preset selection and its parameters.
    pub visual: Visual,
    /// The layer beneath the visualizer.
    pub background: Background,
    /// The title/artist card.
    pub text: Text,
}

/// Resolved `[output]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    /// Frame size in pixels.
    pub resolution: Resolution,
    /// Frames per second.
    pub fps: u32,
    /// Video codec.
    pub codec: Codec,
    /// CRF quality; lower is better.
    pub quality: u8,
}

/// Resolved `[visual]`.
#[derive(Debug, Clone, PartialEq)]
pub struct Visual {
    /// Preset name.
    pub preset: String,
    /// Color scheme.
    pub palette: Palette,
    /// Global motion scale.
    pub intensity: f32,
    /// Global envelope decay scale, 0..=1.
    pub smoothing: f32,
    /// RNG seed.
    pub seed: Seed,
    /// Preset-specific parameters. Validated against the preset schema in
    /// RFC-001 Step 15, not here.
    pub params: toml::Table,
}

/// Resolved `[background]`.
#[derive(Debug, Clone, PartialEq)]
pub struct Background {
    /// The image or video beneath the visualizer, if any.
    pub source: Option<BackgroundSource>,
    /// How the source is fitted to the frame.
    pub fit: Fit,
    /// Gaussian blur, as a standard deviation in pixels of the output frame.
    pub blur: f32,
    /// How much of the background's light to take away, 0..=1.
    pub darken: f32,
}

/// What the background layer draws.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackgroundSource {
    /// A static image.
    Image(PathBuf),
    /// A looped, muted video. Rejected by the renderer until RFC-001 NG2 lands.
    Video(PathBuf),
}

/// Resolved `[text]`.
#[derive(Debug, Clone, PartialEq)]
pub struct Text {
    /// Whether to draw the card at all.
    pub enabled: bool,
    /// Where the card sits.
    pub position: Position,
    /// When the card fades in.
    pub in_at: Seconds,
    /// How long the card stays up.
    pub hold: Seconds,
    /// Which font renders it.
    pub font: FontChoice,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            output: Output {
                resolution: "1080p".parse().expect("built-in resolution parses"),
                fps: 30,
                codec: Codec::X264,
                quality: 18,
            },
            visual: Visual {
                preset: "pulse".to_owned(),
                palette: Palette::Named("ember".to_owned()),
                intensity: 1.0,
                smoothing: 0.35,
                seed: Seed::Auto,
                params: toml::Table::new(),
            },
            background: Background {
                source: None,
                fit: Fit::Cover,
                blur: 0.0,
                darken: 0.0,
            },
            text: Text {
                enabled: true,
                position: Position::BottomLeft,
                in_at: "1.0s".parse().expect("built-in duration parses"),
                hold: "6.0s".parse().expect("built-in duration parses"),
                font: FontChoice::Auto,
            },
        }
    }
}

/// One source of configuration, with every setting optional.
///
/// Absent means "this source has no opinion", which is what lets a higher-
/// precedence layer stay silent about the keys it does not touch.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ConfigLayer {
    /// Optional `[output]` settings.
    pub output: OutputLayer,
    /// Optional `[visual]` settings.
    pub visual: VisualLayer,
    /// Optional `[background]` settings.
    pub background: BackgroundLayer,
    /// Optional `[text]` settings.
    pub text: TextLayer,
}

/// Optional `[output]` settings.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct OutputLayer {
    /// See [`Output::resolution`].
    pub resolution: Option<Resolution>,
    /// See [`Output::fps`]. Range-checked during [`ConfigLayer::resolve`].
    pub fps: Option<i64>,
    /// See [`Output::codec`].
    pub codec: Option<Codec>,
    /// See [`Output::quality`]. Range-checked during [`ConfigLayer::resolve`].
    pub quality: Option<i64>,
}

/// Optional `[visual]` settings.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct VisualLayer {
    /// See [`Visual::preset`].
    pub preset: Option<String>,
    /// See [`Visual::palette`].
    pub palette: Option<Palette>,
    /// See [`Visual::intensity`].
    pub intensity: Option<f64>,
    /// See [`Visual::smoothing`].
    pub smoothing: Option<f64>,
    /// See [`Visual::seed`].
    pub seed: Option<Seed>,
    /// See [`Visual::params`]. Merged key-wise, not replaced wholesale.
    pub params: Option<toml::Table>,
}

/// Optional `[background]` settings.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct BackgroundLayer {
    /// A static image. Mutually exclusive with `video`.
    pub image: Option<PathBuf>,
    /// A looped video. Mutually exclusive with `image`.
    pub video: Option<PathBuf>,
    /// See [`Background::fit`].
    pub fit: Option<Fit>,
    /// See [`Background::blur`].
    pub blur: Option<f64>,
    /// See [`Background::darken`].
    pub darken: Option<f64>,
}

/// Optional `[text]` settings.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct TextLayer {
    /// See [`Text::enabled`].
    pub enabled: Option<bool>,
    /// See [`Text::position`].
    pub position: Option<Position>,
    /// See [`Text::in_at`].
    pub in_at: Option<Seconds>,
    /// See [`Text::hold`].
    pub hold: Option<Seconds>,
    /// See [`Text::font`].
    pub font: Option<FontChoice>,
}

/// The configuration sources, ordered lowest precedence first.
///
/// This struct *is* the precedence contract from `VISION.md` §5.5. Changing the
/// order in [`Sources::resolve`] changes it everywhere.
#[derive(Debug, Clone, Default)]
pub struct Sources {
    /// Defaults declared by the selected preset's schema (RFC-001 Step 15).
    pub preset_defaults: ConfigLayer,
    /// Defaults implied by `--sample`. See [`ConfigLayer::for_sample`].
    pub sample_defaults: ConfigLayer,
    /// The `--config` file.
    pub file: ConfigLayer,
    /// `--set key.path=value` overrides.
    pub set: ConfigLayer,
    /// Individual CLI flags such as `--preset`.
    pub cli: ConfigLayer,
}

impl Sources {
    /// Merge every source onto the built-in defaults and validate the result.
    pub fn resolve(self) -> Result<Config> {
        let Sources {
            mut preset_defaults,
            sample_defaults,
            file,
            set,
            cli,
        } = self;

        preset_defaults.overlay(sample_defaults);
        preset_defaults.overlay(file);
        preset_defaults.overlay(set);
        preset_defaults.overlay(cli);
        preset_defaults.resolve()
    }
}

/// Take the higher-precedence value when it has one.
fn overlay<T>(lower: &mut Option<T>, higher: Option<T>) {
    if higher.is_some() {
        *lower = higher;
    }
}

impl ConfigLayer {
    /// Parse a layer from TOML text, rejecting unknown keys.
    pub fn from_toml_str(source: &str) -> Result<Self> {
        toml::from_str(source).map_err(config_error)
    }

    /// Read and parse a `--config` file.
    ///
    /// An unreadable file is an [`Error::Input`] (exit 3); a readable file with
    /// bad contents is an [`Error::Config`] (exit 2).
    pub fn from_file(path: &Path) -> Result<Self> {
        let source = std::fs::read_to_string(path)
            .map_err(|err| Error::Input(format!("{}: {err}", path.display())))?;

        Self::from_toml_str(&source).map_err(|err| match err {
            Error::Config(message) => Error::Config(format!("{}:\n{message}", path.display())),
            other => other,
        })
    }

    /// The settings `--sample` implies, and nothing else.
    ///
    /// A sample render exists so the user can look at a chorus in seconds rather
    /// than minutes (`VISION.md` §3), so it drops to [`SAMPLE_RESOLUTION`]. It
    /// sits below the config file in the precedence chain, so anyone who wants
    /// to preview the final resolution can still ask for it.
    pub fn for_sample() -> Self {
        Self {
            output: OutputLayer {
                resolution: Some(
                    SAMPLE_RESOLUTION
                        .parse()
                        .expect("the built-in sample resolution parses"),
                ),
                ..OutputLayer::default()
            },
            ..Self::default()
        }
    }

    /// Parse `--set key.path=value` assignments, later ones winning.
    pub fn from_set_assignments<I, S>(assignments: I) -> Result<Self>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut merged = ConfigLayer::default();
        for assignment in assignments {
            merged.overlay(set::layer_from_assignment(assignment.as_ref())?);
        }
        Ok(merged)
    }

    /// Overlay a higher-precedence layer onto this one.
    pub fn overlay(&mut self, higher: ConfigLayer) {
        overlay(&mut self.output.resolution, higher.output.resolution);
        overlay(&mut self.output.fps, higher.output.fps);
        overlay(&mut self.output.codec, higher.output.codec);
        overlay(&mut self.output.quality, higher.output.quality);

        overlay(&mut self.visual.preset, higher.visual.preset);
        overlay(&mut self.visual.palette, higher.visual.palette);
        overlay(&mut self.visual.intensity, higher.visual.intensity);
        overlay(&mut self.visual.smoothing, higher.visual.smoothing);
        overlay(&mut self.visual.seed, higher.visual.seed);

        // Preset params merge key-wise: `--set visual.params.bass_drive=2.0`
        // must not wipe out the other params the config file set.
        if let Some(higher_params) = higher.visual.params {
            let params = self.visual.params.get_or_insert_default();
            params.extend(higher_params);
        }

        // `image` and `video` are mutually exclusive, so a higher layer naming
        // one of them replaces whichever the lower layer named.
        if higher.background.image.is_some() || higher.background.video.is_some() {
            self.background.image = higher.background.image;
            self.background.video = higher.background.video;
        }
        overlay(&mut self.background.fit, higher.background.fit);
        overlay(&mut self.background.blur, higher.background.blur);
        overlay(&mut self.background.darken, higher.background.darken);

        overlay(&mut self.text.enabled, higher.text.enabled);
        overlay(&mut self.text.position, higher.text.position);
        overlay(&mut self.text.in_at, higher.text.in_at);
        overlay(&mut self.text.hold, higher.text.hold);
        overlay(&mut self.text.font, higher.text.font);
    }

    /// Apply this layer to the built-in defaults and validate.
    pub fn resolve(self) -> Result<Config> {
        let mut config = Config::default();

        if let Some(resolution) = self.output.resolution {
            config.output.resolution = resolution;
        }
        if let Some(fps) = self.output.fps {
            config.output.fps = u32::try_from(fps)
                .ok()
                .filter(|fps| (1..=MAX_FPS).contains(fps))
                .ok_or_else(|| {
                    Error::Config(format!(
                        "`output.fps` must be between 1 and {MAX_FPS}, got {fps}"
                    ))
                })?;
        }
        if let Some(codec) = self.output.codec {
            config.output.codec = codec;
        }
        if let Some(quality) = self.output.quality {
            config.output.quality = u8::try_from(quality)
                .ok()
                .filter(|quality| *quality <= MAX_CRF)
                .ok_or_else(|| {
                    Error::Config(format!(
                        "`output.quality` is a CRF value between 0 and {MAX_CRF}, got {quality}"
                    ))
                })?;
        }

        if let Some(preset) = self.visual.preset {
            if preset.trim().is_empty() {
                return Err(Error::Config(
                    "`visual.preset` must not be blank".to_owned(),
                ));
            }
            config.visual.preset = preset;
        }
        if let Some(palette) = self.visual.palette {
            // A named palette is checked against the registry by the renderer,
            // the way `visual.preset` is: both happen before the song is
            // decoded, and neither makes this module depend on `render`.
            config.visual.palette = palette;
        }
        if let Some(intensity) = self.visual.intensity {
            config.visual.intensity = positive("visual.intensity", intensity)?;
        }
        if let Some(smoothing) = self.visual.smoothing {
            config.visual.smoothing = unit_interval("visual.smoothing", smoothing)?;
        }
        if let Some(seed) = self.visual.seed {
            config.visual.seed = seed;
        }
        if let Some(params) = self.visual.params {
            config.visual.params = params;
        }

        config.background.source = match (self.background.image, self.background.video) {
            (Some(_), Some(_)) => {
                return Err(Error::Config(
                    "`background.image` and `background.video` are mutually exclusive; \
                     the background layer draws one or the other"
                        .to_owned(),
                ));
            }
            (Some(image), None) => Some(BackgroundSource::Image(non_blank_path(
                "background.image",
                image,
            )?)),
            (None, Some(video)) => Some(BackgroundSource::Video(non_blank_path(
                "background.video",
                video,
            )?)),
            (None, None) => None,
        };
        if let Some(fit) = self.background.fit {
            config.background.fit = fit;
        }
        if let Some(blur) = self.background.blur {
            config.background.blur = non_negative("background.blur", blur)?;
        }
        if let Some(darken) = self.background.darken {
            config.background.darken = unit_interval("background.darken", darken)?;
        }

        if let Some(enabled) = self.text.enabled {
            config.text.enabled = enabled;
        }
        if let Some(position) = self.text.position {
            config.text.position = position;
        }
        if let Some(in_at) = self.text.in_at {
            config.text.in_at = in_at;
        }
        if let Some(hold) = self.text.hold {
            config.text.hold = hold;
        }
        if let Some(font) = self.text.font {
            config.text.font = font;
        }

        Ok(config)
    }
}

/// The resolution a `--sample` render falls back to.
///
/// Reduced, not tiny: 720p is a quarter of the pixels of 1080p and still shows
/// what a preset is doing.
pub const SAMPLE_RESOLUTION: &str = "720p";

/// The highest frame rate worth encoding; anything above is a typo.
const MAX_FPS: u32 = 240;

/// x264's CRF scale tops out at 51.
const MAX_CRF: u8 = 51;

fn positive(key: &str, value: f64) -> Result<f32> {
    if !value.is_finite() || value <= 0.0 {
        return Err(Error::Config(format!(
            "`{key}` must be greater than 0, got {value}"
        )));
    }
    Ok(value as f32)
}

fn non_negative(key: &str, value: f64) -> Result<f32> {
    if !value.is_finite() || value < 0.0 {
        return Err(Error::Config(format!(
            "`{key}` must not be negative, got {value}"
        )));
    }
    Ok(value as f32)
}

/// Reject a path that is empty or only whitespace.
///
/// A blank path is a typo or an environment variable that expanded to nothing.
/// Accepting it defers the failure to the layer that tries to open the file,
/// which can only report that it cannot read `""`.
fn non_blank_path(key: &str, path: PathBuf) -> Result<PathBuf> {
    if path.as_os_str().to_string_lossy().trim().is_empty() {
        return Err(Error::Config(format!("`{key}` must not be blank")));
    }
    Ok(path)
}

fn unit_interval(key: &str, value: f64) -> Result<f32> {
    if !(0.0..=1.0).contains(&value) {
        return Err(Error::Config(format!(
            "`{key}` must be between 0 and 1, got {value}"
        )));
    }
    Ok(value as f32)
}

/// Turn a `toml` deserialization failure into a config error, appending a
/// "did you mean" hint when the message names a near-miss key.
fn config_error(err: toml::de::Error) -> Error {
    let hint = suggestion(err.message());
    let rendered = err.to_string();
    let rendered = rendered.trim_end();

    match hint {
        Some(near) => Error::Config(format!("{rendered}\nhint: did you mean `{near}`?")),
        None => Error::Config(rendered.to_owned()),
    }
}

/// How alike two keys must be before we are willing to guess.
///
/// A single typo in a short key (`fpss` for `fps`) scores 0.75; unrelated words
/// score far lower. Suggesting the wrong key is worse than suggesting none.
const SUGGESTION_THRESHOLD: f64 = 0.6;

/// Extract a "did you mean" suggestion from serde's `unknown field` or
/// `unknown variant` message, which lists the offending name first and the
/// accepted ones after it, all in backticks.
fn suggestion(message: &str) -> Option<String> {
    let offset = ["unknown field ", "unknown variant "]
        .iter()
        .find_map(|prefix| message.find(prefix).map(|at| at + prefix.len()))?;

    let mut quoted = message[offset..].split('`').skip(1).step_by(2);

    let unknown = quoted.next()?;
    let candidates: Vec<&str> = quoted.collect();

    closest(unknown, candidates.into_iter()).map(str::to_owned)
}

/// The candidate closest to `unknown`, if any is close enough to be worth saying.
///
/// The one place a "did you mean" is decided, so a preset parameter and a TOML
/// key are held to the same standard of resemblance.
pub(crate) fn closest<'a>(
    unknown: &str,
    candidates: impl Iterator<Item = &'a str>,
) -> Option<&'a str> {
    candidates
        .map(|candidate| {
            let score = strsim::normalized_damerau_levenshtein(unknown, candidate);
            (candidate, score)
        })
        .filter(|(_, score)| *score >= SUGGESTION_THRESHOLD)
        .max_by(|(_, a), (_, b)| a.total_cmp(b))
        .map(|(candidate, _)| candidate)
}

#[cfg(test)]
mod tests;
