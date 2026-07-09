//! Typed configuration values and the parsers that produce them.
//!
//! Every value the user can write in TOML or pass to `--set` lands in one of
//! these types. Parsing happens once, at the edge, so the rest of the pipeline
//! never sees a `"1920x1080"` that might actually say `"1920X1O80"`.
//!
//! Each type implements [`FromStr`] and derives `Deserialize` through it, so a
//! TOML file, a `--set` override, and a CLI flag all reject the same garbage
//! with the same message.

use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer};

/// A configuration value could not be parsed.
///
/// Carries a message that names the offending input and what was expected.
/// Converts into [`crate::Error::Config`], which the CLI maps to exit code 2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError(String);

impl ParseError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ParseError {}

impl From<ParseError> for crate::Error {
    fn from(err: ParseError) -> Self {
        crate::Error::Config(err.0)
    }
}

type ParseResult<T> = Result<T, ParseError>;

/// Derive `TryFrom<String>` from `FromStr` so `#[serde(try_from = "String")]`
/// reuses the same parser — and therefore the same error message — as the CLI.
macro_rules! deserialize_via_from_str {
    ($($ty:ty),+ $(,)?) => {$(
        impl TryFrom<String> for $ty {
            type Error = ParseError;

            fn try_from(value: String) -> ParseResult<Self> {
                value.parse()
            }
        }
    )+};
}

deserialize_via_from_str!(Resolution, Codec, Fit, Position, Seconds, Color, FontChoice);

/// Output frame size in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct Resolution {
    /// Frame width in pixels. Always even.
    pub width: u32,
    /// Frame height in pixels. Always even.
    pub height: u32,
}

impl Resolution {
    /// The named sizes accepted in config files, in the order `--help` lists them.
    const NAMED: &'static [(&'static str, Resolution)] = &[
        ("720p", Resolution::new(1280, 720)),
        ("1080p", Resolution::new(1920, 1080)),
        ("4k", Resolution::new(3840, 2160)),
    ];

    const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }
}

impl fmt::Display for Resolution {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}x{}", self.width, self.height)
    }
}

impl FromStr for Resolution {
    type Err = ParseError;

    fn from_str(s: &str) -> ParseResult<Self> {
        let input = s.trim();
        let lowered = input.to_ascii_lowercase();

        if let Some((_, named)) = Self::NAMED.iter().find(|(name, _)| *name == lowered) {
            return Ok(*named);
        }

        let malformed = || {
            ParseError::new(format!(
                "invalid resolution `{input}`: expected `WIDTHxHEIGHT` (e.g. `1920x1080`) \
                 or a named size (`720p`, `1080p`, `4k`)"
            ))
        };

        let (width, height) = lowered.split_once('x').ok_or_else(malformed)?;
        let width: u32 = width.parse().map_err(|_| malformed())?;
        let height: u32 = height.parse().map_err(|_| malformed())?;

        if width == 0 || height == 0 {
            return Err(malformed());
        }

        if width % 2 != 0 || height % 2 != 0 {
            return Err(ParseError::new(format!(
                "invalid resolution `{input}`: width and height must be even \
                 (the yuv420p pixel format ffmpeg encodes to cannot subsample odd dimensions)"
            )));
        }

        Ok(Self::new(width, height))
    }
}

/// Video codec passed to ffmpeg.
///
/// Only [`Codec::X264`] is wired into the encoder for v0.1 (RFC-001 NG3); the
/// other variants parse so that a config written for a later release fails at
/// render time with a clear message instead of at parse time with a typo hint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub enum Codec {
    /// `libx264`, the v0.1 default.
    X264,
    /// `libx265`.
    X265,
    /// AV1.
    Av1,
}

impl Codec {
    /// The spelling used in config files and on the command line.
    pub fn as_str(self) -> &'static str {
        match self {
            Codec::X264 => "x264",
            Codec::X265 => "x265",
            Codec::Av1 => "av1",
        }
    }
}

impl FromStr for Codec {
    type Err = ParseError;

    fn from_str(s: &str) -> ParseResult<Self> {
        match s.trim() {
            "x264" => Ok(Codec::X264),
            "x265" => Ok(Codec::X265),
            "av1" => Ok(Codec::Av1),
            other => Err(one_of("codec", other, ["x264", "x265", "av1"])),
        }
    }
}

/// The shared shape of "you wrote X, it has to be one of these" errors.
///
/// Deliberately phrased like serde's own `unknown variant` message: the "did you
/// mean" pass in [`config_error`](super::config_error) reads the backticked list
/// straight out of it, so a hand-written parser and a derived one both get a
/// suggestion.
fn one_of<const N: usize>(what: &str, got: &str, expected: [&str; N]) -> ParseError {
    let expected = expected
        .iter()
        .map(|value| format!("`{value}`"))
        .collect::<Vec<_>>()
        .join(", ");
    ParseError::new(format!(
        "{what}: unknown variant `{got}`, expected one of {expected}"
    ))
}

/// How a background image is fitted to the output frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub enum Fit {
    /// Fill the frame, cropping overflow. Preserves aspect ratio.
    Cover,
    /// Fit inside the frame, letterboxing. Preserves aspect ratio.
    Contain,
    /// Fill the frame, distorting aspect ratio.
    Stretch,
}

impl Fit {
    /// The spelling used in config files.
    pub fn as_str(self) -> &'static str {
        match self {
            Fit::Cover => "cover",
            Fit::Contain => "contain",
            Fit::Stretch => "stretch",
        }
    }
}

impl FromStr for Fit {
    type Err = ParseError;

    fn from_str(s: &str) -> ParseResult<Self> {
        match s.trim() {
            "cover" => Ok(Fit::Cover),
            "contain" => Ok(Fit::Contain),
            "stretch" => Ok(Fit::Stretch),
            other => Err(one_of("fit", other, ["cover", "contain", "stretch"])),
        }
    }
}

/// Where the text card sits: the nine-grid from `VISION.md` §5.3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub enum Position {
    /// Top-left corner.
    TopLeft,
    /// Top edge, horizontally centered.
    TopCenter,
    /// Top-right corner.
    TopRight,
    /// Left edge, vertically centered.
    CenterLeft,
    /// Frame center.
    Center,
    /// Right edge, vertically centered.
    CenterRight,
    /// Bottom-left corner.
    BottomLeft,
    /// Bottom edge, horizontally centered.
    BottomCenter,
    /// Bottom-right corner.
    BottomRight,
}

impl Position {
    /// Every position, in the order error messages list them.
    pub const ALL: &'static [Position] = &[
        Position::TopLeft,
        Position::TopCenter,
        Position::TopRight,
        Position::CenterLeft,
        Position::Center,
        Position::CenterRight,
        Position::BottomLeft,
        Position::BottomCenter,
        Position::BottomRight,
    ];

    /// The spelling used in config files.
    pub fn as_str(self) -> &'static str {
        match self {
            Position::TopLeft => "top-left",
            Position::TopCenter => "top-center",
            Position::TopRight => "top-right",
            Position::CenterLeft => "center-left",
            Position::Center => "center",
            Position::CenterRight => "center-right",
            Position::BottomLeft => "bottom-left",
            Position::BottomCenter => "bottom-center",
            Position::BottomRight => "bottom-right",
        }
    }
}

impl FromStr for Position {
    type Err = ParseError;

    fn from_str(s: &str) -> ParseResult<Self> {
        let input = s.trim();
        Position::ALL
            .iter()
            .find(|position| position.as_str() == input)
            .copied()
            .ok_or_else(|| {
                one_of(
                    "position",
                    input,
                    [
                        "top-left",
                        "top-center",
                        "top-right",
                        "center-left",
                        "center",
                        "center-right",
                        "bottom-left",
                        "bottom-center",
                        "bottom-right",
                    ],
                )
            })
    }
}

/// A point in time, or a span of it, in seconds.
///
/// Accepts `1.0s`, `250ms`, `0:45`, and `1:02:03.5`. A bare number is rejected:
/// its unit would be a guess.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Deserialize)]
#[serde(try_from = "String")]
pub struct Seconds(f64);

impl Seconds {
    /// Wrap a non-negative, finite count of seconds.
    pub fn new(seconds: f64) -> ParseResult<Self> {
        if !seconds.is_finite() || seconds < 0.0 {
            return Err(ParseError::new(format!(
                "invalid duration `{seconds}`: expected a finite, non-negative number of seconds"
            )));
        }
        Ok(Self(seconds))
    }

    /// The value in seconds.
    pub fn as_secs_f64(self) -> f64 {
        self.0
    }
}

impl fmt::Display for Seconds {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}s", self.0)
    }
}

impl FromStr for Seconds {
    type Err = ParseError;

    fn from_str(s: &str) -> ParseResult<Self> {
        let input = s.trim();
        let malformed = || {
            ParseError::new(format!(
                "invalid duration `{input}`: expected a unit (`1.0s`, `250ms`) \
                 or clock notation (`0:45`, `1:02:03`)"
            ))
        };

        if input.contains(':') {
            return Seconds::new(parse_clock(input).ok_or_else(malformed)?);
        }

        let seconds = if let Some(millis) = input.strip_suffix("ms") {
            millis.trim().parse::<f64>().map_err(|_| malformed())? / 1000.0
        } else if let Some(seconds) = input.strip_suffix('s') {
            seconds.trim().parse::<f64>().map_err(|_| malformed())?
        } else {
            return Err(malformed());
        };

        Seconds::new(seconds)
    }
}

/// Parse `m:ss`, `m:ss.fff`, or `h:mm:ss`, returning `None` for anything else.
fn parse_clock(input: &str) -> Option<f64> {
    let parts: Vec<&str> = input.split(':').collect();
    if !(2..=3).contains(&parts.len()) {
        return None;
    }

    let (seconds, leading) = parts.split_last()?;
    let seconds: f64 = seconds.parse().ok()?;
    if !(0.0..60.0).contains(&seconds) {
        return None;
    }

    let mut total = seconds;
    let mut scale = 60.0;
    for part in leading.iter().rev() {
        let value: u32 = part.parse().ok()?;
        total += f64::from(value) * scale;
        scale *= 60.0;
    }

    Some(total)
}

/// The slice of a song to render, from `--sample`.
///
/// `60s` means the first 60 seconds; `0:45..1:45` is an explicit range.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SampleRange {
    /// Inclusive start offset into the song.
    pub start: Seconds,
    /// Exclusive end offset into the song. Always greater than `start`.
    pub end: Seconds,
}

impl SampleRange {
    /// How long the sample runs, in seconds.
    pub fn duration_secs(self) -> f64 {
        self.end.as_secs_f64() - self.start.as_secs_f64()
    }
}

impl FromStr for SampleRange {
    type Err = ParseError;

    fn from_str(s: &str) -> ParseResult<Self> {
        let input = s.trim();

        let (start, end) = match input.split_once("..") {
            Some((start, end)) => (start.parse()?, end.parse()?),
            None => (Seconds(0.0), input.parse()?),
        };

        if end <= start {
            return Err(ParseError::new(format!(
                "invalid sample range `{input}`: the end must come after the start"
            )));
        }

        Ok(Self { start, end })
    }
}

/// An sRGB color with alpha, written as `#rgb`, `#rrggbb`, or `#rrggbbaa`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct Color {
    /// Red channel.
    pub r: u8,
    /// Green channel.
    pub g: u8,
    /// Blue channel.
    pub b: u8,
    /// Alpha channel. `255` unless an eight-digit hex form was used.
    pub a: u8,
}

impl FromStr for Color {
    type Err = ParseError;

    fn from_str(s: &str) -> ParseResult<Self> {
        let input = s.trim();
        let malformed = || {
            ParseError::new(format!(
                "invalid color `{input}`: expected `#rgb`, `#rrggbb`, or `#rrggbbaa` \
                 (e.g. `#1a1a2e`)"
            ))
        };

        let hex = input.strip_prefix('#').ok_or_else(malformed)?;
        if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(malformed());
        }

        let byte = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).map_err(|_| malformed());
        // `#rgb` is shorthand for `#rrggbb`: each nibble doubles, so `f` -> `ff`.
        let nibble = |at: usize| {
            u8::from_str_radix(&hex[at..at + 1], 16)
                .map(|value| value * 0x11)
                .map_err(|_| malformed())
        };

        match hex.len() {
            3 => Ok(Color {
                r: nibble(0)?,
                g: nibble(1)?,
                b: nibble(2)?,
                a: 0xff,
            }),
            6 => Ok(Color {
                r: byte(0)?,
                g: byte(2)?,
                b: byte(4)?,
                a: 0xff,
            }),
            8 => Ok(Color {
                r: byte(0)?,
                g: byte(2)?,
                b: byte(4)?,
                a: byte(6)?,
            }),
            _ => Err(malformed()),
        }
    }
}

/// The color scheme driving a preset: a built-in name or inline hex colors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Palette {
    /// A built-in palette, e.g. `ember`. Resolved against the palette registry
    /// in RFC-001 Step 16.
    Named(String),
    /// One to five colors given inline.
    Inline(Vec<Color>),
}

/// The most colors a palette can define; the shader uniform holds five.
pub const MAX_PALETTE_COLORS: usize = 5;

impl FromStr for Palette {
    type Err = ParseError;

    fn from_str(s: &str) -> ParseResult<Self> {
        let input = s.trim();

        // A `#` anywhere means the user is spelling colors out, not naming a
        // palette. This is what lets `--palette '#1a1a2e,#e94560'` work without
        // TOML array syntax on the command line.
        if input.contains('#') {
            let colors = input
                .split(',')
                .map(|color| color.trim().parse())
                .collect::<ParseResult<Vec<Color>>>()?;
            return Palette::inline(colors);
        }

        let named = !input.is_empty()
            && input.starts_with(|c: char| c.is_ascii_lowercase())
            && input
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');

        if !named {
            return Err(ParseError::new(format!(
                "invalid palette `{input}`: expected a built-in name such as `ember`, \
                 or hex colors such as `#1a1a2e,#e94560`"
            )));
        }

        Ok(Palette::Named(input.to_owned()))
    }
}

impl<'de> Deserialize<'de> for Palette {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(PaletteVisitor)
    }
}

struct PaletteVisitor;

impl<'de> Visitor<'de> for PaletteVisitor {
    type Value = Palette;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a palette name such as `ember`, or an array of hex colors")
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Palette, E> {
        value.parse().map_err(de::Error::custom)
    }

    fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Palette, A::Error> {
        let mut colors = Vec::new();
        while let Some(color) = seq.next_element::<Color>()? {
            colors.push(color);
        }
        Palette::inline(colors).map_err(de::Error::custom)
    }
}

impl Palette {
    /// Build an inline palette, checking the color count against the shader
    /// uniform's capacity.
    pub fn inline(colors: Vec<Color>) -> ParseResult<Self> {
        if colors.is_empty() {
            return Err(ParseError::new(
                "an inline palette needs at least one color",
            ));
        }

        if colors.len() > MAX_PALETTE_COLORS {
            return Err(ParseError::new(format!(
                "an inline palette takes at most {MAX_PALETTE_COLORS} colors, got {}",
                colors.len()
            )));
        }

        Ok(Palette::Inline(colors))
    }
}

/// The RNG seed. `auto` derives it from the input file name so re-renders match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Seed {
    /// Derive the seed from the input file name.
    Auto,
    /// Use exactly this seed.
    Fixed(u64),
}

impl FromStr for Seed {
    type Err = ParseError;

    fn from_str(s: &str) -> ParseResult<Self> {
        let input = s.trim();
        if input == "auto" {
            return Ok(Seed::Auto);
        }

        input.parse().map(Seed::Fixed).map_err(|_| {
            ParseError::new(format!(
                "invalid seed `{input}`: expected `auto` or a non-negative integer"
            ))
        })
    }
}

impl<'de> Deserialize<'de> for Seed {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(SeedVisitor)
    }
}

struct SeedVisitor;

impl Visitor<'_> for SeedVisitor {
    type Value = Seed;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("`\"auto\"` or a non-negative integer")
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Seed, E> {
        value.parse().map_err(de::Error::custom)
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Seed, E> {
        Ok(Seed::Fixed(value))
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Seed, E> {
        u64::try_from(value)
            .map(Seed::Fixed)
            .map_err(|_| de::Error::custom(format!("seed `{value}` must not be negative")))
    }
}

/// Which font renders the text card.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub enum FontChoice {
    /// Use the bundled OFL font.
    Auto,
    /// Load a font from this path.
    Path(PathBuf),
}

impl FromStr for FontChoice {
    type Err = ParseError;

    fn from_str(s: &str) -> ParseResult<Self> {
        Ok(match s.trim() {
            "auto" => FontChoice::Auto,
            "" => {
                return Err(ParseError::new(
                    "invalid font ``: expected `auto` or a path to a font file",
                ));
            }
            path => FontChoice::Path(PathBuf::from(path)),
        })
    }
}
