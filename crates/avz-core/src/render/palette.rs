//! Palettes: the named built-ins, the inline hex form, and the color space they
//! meet in (`VISION.md` §6).
//!
//! A palette reaches a shader as `pal: array<vec4<f32>, 5>` — five colors, no
//! more and no fewer. The user may write fewer or more than five, so this module
//! is where any legal `visual.palette` becomes exactly [`PALETTE_SLOTS`] colors,
//! in **linear** space, because the render target is `Rgba8UnormSrgb` and encodes
//! back to sRGB on write.
//!
//! Slot 0 is the background a preset sits on; slots 1..4 are the accent ramp it
//! walks (`presets/pulse.wgsl`, `fn accent`). Every built-in is therefore ordered
//! darkest-first and chosen to read against a dark frame.
//!
//! **Resampling happens in Oklab.** An inline palette of two colors has to fill
//! five slots, and the midpoint of `#1a1a2e` and `#ffd93d` in sRGB is a muddy
//! olive: the sRGB axes are neither perceptually uniform nor linear in light.
//! Linear sRGB fixes the light but not the perception — it drags every blend
//! toward the brighter endpoint. Oklab is uniform enough that the middle slot of
//! a two-color palette looks like the middle color, which is the only thing a
//! resampled palette is asked to do. A slot that lands exactly on a given color
//! skips the trip entirely, so a five-color palette — every built-in — reaches
//! the uniform bit-for-bit as written.
//!
//! Oklab can leave the sRGB gamut between two in-gamut colors, so the result is
//! clamped back into `0.0..=1.0` on the way out.

use crate::config::{Color, MAX_PALETTE_COLORS, MIN_PALETTE_COLORS, Palette};
use crate::render::globals::PALETTE_SLOTS;
use crate::{Error, Result};

/// A palette as the uniform carries it: five linear-space RGBA colors.
pub type LinearPalette = [[f32; 4]; PALETTE_SLOTS];

/// One built-in palette: what `--palette` names it, what it looks like, and the
/// five colors themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltIn {
    /// The name `--palette` and `visual.palette` accept.
    pub name: &'static str,
    /// One line on what the palette is for.
    pub character: &'static str,
    /// Darkest first: slot 0 is the background, slots 1..4 the accent ramp.
    pub colors: [Color; PALETTE_SLOTS],
}

/// The palettes avz ships (`VISION.md` §6).
pub const BUILT_INS: [BuiltIn; 5] = [
    BuiltIn {
        name: "ember",
        character: "deep indigo → violet → coral → amber; embers over a night sky",
        colors: [
            Color::rgb(0x1a, 0x1a, 0x2e),
            Color::rgb(0x53, 0x34, 0x83),
            Color::rgb(0xe9, 0x45, 0x60),
            Color::rgb(0xf9, 0xa0, 0x3f),
            Color::rgb(0xff, 0xd9, 0x3d),
        ],
    },
    BuiltIn {
        name: "glacier",
        character: "midnight water → glacier ice → frost; cold, clean, and still",
        colors: [
            Color::rgb(0x0b, 0x16, 0x22),
            Color::rgb(0x1d, 0x4e, 0x6b),
            Color::rgb(0x3f, 0xa9, 0xc9),
            Color::rgb(0x8f, 0xd8, 0xe8),
            Color::rgb(0xe8, 0xf7, 0xfb),
        ],
    },
    BuiltIn {
        name: "verdant",
        character: "forest floor → moss → new leaf → lichen; green and growing",
        colors: [
            Color::rgb(0x0c, 0x1a, 0x12),
            Color::rgb(0x1e, 0x4d, 0x34),
            Color::rgb(0x3f, 0x8f, 0x5a),
            Color::rgb(0x8f, 0xc9, 0x6b),
            Color::rgb(0xe6, 0xf2, 0xa8),
        ],
    },
    BuiltIn {
        name: "mono",
        character: "neutral greys; the palette that gets out of the preset's way",
        colors: [
            Color::rgb(0x10, 0x10, 0x10),
            Color::rgb(0x3a, 0x3a, 0x3a),
            Color::rgb(0x70, 0x70, 0x70),
            Color::rgb(0xb4, 0xb4, 0xb4),
            Color::rgb(0xf2, 0xf2, 0xf2),
        ],
    },
    BuiltIn {
        name: "carpathian",
        character: "pine dusk → plum → rust → wheat; the dark-folk palette",
        colors: [
            Color::rgb(0x0f, 0x15, 0x13),
            Color::rgb(0x3a, 0x2c, 0x42),
            Color::rgb(0x8a, 0x40, 0x33),
            Color::rgb(0xc9, 0x85, 0x50),
            Color::rgb(0xec, 0xd6, 0xa8),
        ],
    },
];

/// Every built-in palette name, in registry order.
pub fn names() -> Vec<&'static str> {
    BUILT_INS.iter().map(|palette| palette.name).collect()
}

/// The built-in palette called `name`.
///
/// # Errors
///
/// [`Error::Config`] naming every palette that does exist, because a typo in
/// `--palette` is the user's argument and the list is what fixes it.
pub fn by_name(name: &str) -> Result<&'static BuiltIn> {
    BUILT_INS
        .iter()
        .find(|palette| palette.name == name)
        .ok_or_else(|| {
            Error::Config(format!(
                "unknown palette `{name}`; avz ships: {}. \
                 Inline colors work too: `--palette '#1a1a2e,#e94560'`",
                names().join(", "),
            ))
        })
}

/// The five linear colors `palette` stands for.
///
/// # Errors
///
/// [`Error::Config`] for a built-in name that does not exist, or an inline
/// palette outside `MIN_PALETTE_COLORS..=MAX_PALETTE_COLORS`.
pub fn resolve(palette: &Palette) -> Result<LinearPalette> {
    match palette {
        Palette::Named(name) => Ok(resample(&by_name(name)?.colors)),
        Palette::Inline(colors) => {
            // `Palette::inline` already refuses these, but the variant is public
            // and `resample` indexes `colors[0]`.
            if !(MIN_PALETTE_COLORS..=MAX_PALETTE_COLORS).contains(&colors.len()) {
                return Err(Error::Config(format!(
                    "an inline palette takes {MIN_PALETTE_COLORS} to {MAX_PALETTE_COLORS} \
                     colors, got {}",
                    colors.len(),
                )));
            }
            Ok(resample(colors))
        }
    }
}

/// Stretch `colors` across the five uniform slots, blending in Oklab.
///
/// The endpoints land on slots 0 and 4 exactly, as does any input color a slot
/// falls squarely on — `slot / 4 * (n - 1)` is exact in binary floating point
/// whenever it is a whole number, and the whole-number case skips the blend.
/// Five colors in therefore means the same five colors out.
///
/// Panics if `colors` is empty; [`resolve`] is the only caller and checks.
fn resample(colors: &[Color]) -> LinearPalette {
    let last = colors.len() - 1;

    std::array::from_fn(|slot| {
        let position = slot as f32 / (PALETTE_SLOTS - 1) as f32 * last as f32;
        let lower = position.floor();
        let fraction = position - lower;
        let lower = lower as usize;

        if fraction == 0.0 {
            return linear_rgba(colors[lower]);
        }
        blend(colors[lower], colors[lower + 1], fraction)
    })
}

/// `from` and `to` mixed `fraction` of the way apart, in Oklab, as linear RGBA.
fn blend(from: Color, to: Color, fraction: f32) -> [f32; 4] {
    let (from, to) = (linear_rgba(from), linear_rgba(to));
    let (lab, other) = (oklab(from), oklab(to));

    let mixed = oklab_to_linear([
        lerp(lab[0], other[0], fraction),
        lerp(lab[1], other[1], fraction),
        lerp(lab[2], other[2], fraction),
    ]);

    [
        mixed[0].clamp(0.0, 1.0),
        mixed[1].clamp(0.0, 1.0),
        mixed[2].clamp(0.0, 1.0),
        lerp(from[3], to[3], fraction),
    ]
}

fn lerp(from: f32, to: f32, fraction: f32) -> f32 {
    from + (to - from) * fraction
}

/// An sRGB config color as linear-space RGBA, which is what shaders blend in.
///
/// Shared with [`schema`](crate::render::schema), so a `color` preset parameter
/// reaches the shader in the same space the palette does.
pub(crate) fn linear_rgba(color: Color) -> [f32; 4] {
    [
        srgb_to_linear(color.r),
        srgb_to_linear(color.g),
        srgb_to_linear(color.b),
        f32::from(color.a) / 255.0,
    ]
}

/// The sRGB electro-optical transfer function, inverted: an 8-bit encoded
/// channel becomes the light it stands for.
pub(crate) fn srgb_to_linear(channel: u8) -> f32 {
    let encoded = f32::from(channel) / 255.0;
    if encoded <= 0.040_45 {
        encoded / 12.92
    } else {
        ((encoded + 0.055) / 1.055).powf(2.4)
    }
}

/// Linear sRGB → Oklab (Björn Ottosson, 2020).
fn oklab(rgba: [f32; 4]) -> [f32; 3] {
    let [r, g, b, _] = rgba;

    let l = 0.412_221_47 * r + 0.536_332_55 * g + 0.051_445_995 * b;
    let m = 0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b;
    let s = 0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b;

    let (l, m, s) = (l.cbrt(), m.cbrt(), s.cbrt());

    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

/// Oklab → linear sRGB. May leave the gamut; callers clamp.
fn oklab_to_linear(lab: [f32; 3]) -> [f32; 3] {
    let [lightness, a, b] = lab;

    let l = lightness + 0.396_337_78 * a + 0.215_803_76 * b;
    let m = lightness - 0.105_561_346 * a - 0.063_854_17 * b;
    let s = lightness - 0.089_484_18 * a - 1.291_485_5 * b;

    let (l, m, s) = (l * l * l, m * m * m, s * s * s);

    [
        4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s,
        -1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s,
        -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sRGB opto-electronic transfer function: the inverse of the one under
    /// test, written out separately so the round trip is a real check on the
    /// constants rather than a restatement of them.
    fn linear_to_srgb(light: f32) -> u8 {
        let encoded = if light <= 0.003_130_8 {
            light * 12.92
        } else {
            1.055 * light.powf(1.0 / 2.4) - 0.055
        };
        (encoded * 255.0).round().clamp(0.0, 255.0) as u8
    }

    fn inline(hexes: &[&str]) -> Palette {
        Palette::inline(
            hexes
                .iter()
                .map(|hex| hex.parse().expect("a legal color"))
                .collect(),
        )
        .expect("a legal inline palette")
    }

    fn close(actual: f32, expected: f32) -> bool {
        (actual - expected).abs() < 1e-4
    }

    /// A built-in fills every slot, and the colors reach the shader as light,
    /// not as the 8-bit encoding of light. Mid-grey is ~0.216 in linear, not
    /// 0.5: getting this wrong washes every palette out by a stop and a half.
    #[test]
    fn named_palette_resolves_to_five_linear_colors() {
        let resolved = resolve(&Palette::Named("mono".to_owned())).expect("`mono` ships");

        assert_eq!(resolved.len(), PALETTE_SLOTS);
        for slot in resolved {
            assert_eq!(slot[3], 1.0, "an opaque built-in stays opaque");
            for channel in &slot[..3] {
                assert!((0.0..=1.0).contains(channel), "{channel} is not a light");
            }
        }

        // `mono` slot 2 is sRGB 0x70; the shader must see the light it encodes.
        assert!(
            close(resolved[2][0], srgb_to_linear(0x70)),
            "slot 2 is {} in linear",
            resolved[2][0],
        );
        assert!(
            resolved[2][0] < 0.25,
            "sRGB 0x70 is dark light ({}), not 0.44",
            resolved[2][0],
        );

        // Five colors in, five colors out: no blend, no drift.
        for (slot, color) in resolved.iter().zip(by_name("mono").expect("ships").colors) {
            assert_eq!(*slot, linear_rgba(color));
        }
    }

    /// The whole point of the inline form: two colors, five slots, endpoints
    /// intact and a real blend between them.
    #[test]
    fn inline_two_colors_interpolate_to_five() {
        let dark: Color = "#1a1a2e".parse().expect("a color");
        let gold: Color = "#ffd93d".parse().expect("a color");

        let resolved = resolve(&inline(&["#1a1a2e", "#ffd93d"])).expect("two colors resolve");

        assert_eq!(resolved[0], linear_rgba(dark), "the first color is exact");
        assert_eq!(resolved[4], linear_rgba(gold), "the last color is exact");

        // The middle slot is the Oklab midpoint of the endpoints: a blend, not a
        // copy. Asserted on the Oklab axes rather than the RGB ones because a
        // hue-correct navy→gold blend passes the neutral axis, and its blue
        // channel overshoots *both* endpoints on the way. "Between on every RGB
        // channel" is false of the very interpolation a palette wants.
        let (from, to) = (oklab(resolved[0]), oklab(resolved[4]));
        let middle = oklab(resolved[2]);
        for axis in 0..3 {
            let midpoint = lerp(from[axis], to[axis], 0.5);
            assert!(
                close(middle[axis], midpoint),
                "Oklab axis {axis}: {} is not the midpoint {midpoint}",
                middle[axis],
            );
        }

        // Slots 1 and 3 walk monotonically from one endpoint to the other.
        let lightness: Vec<f32> = resolved.iter().map(|slot| oklab(*slot)[0]).collect();
        for pair in lightness.windows(2) {
            assert!(pair[0] < pair[1], "lightness must climb: {lightness:?}");
        }
    }

    /// Eight colors is the most the inline form takes; the endpoints and the
    /// slots that land squarely on an input color survive untouched.
    #[test]
    fn inline_eight_colors_resample_onto_the_five_slots() {
        let hexes = [
            "#000000", "#110000", "#220000", "#330000", "#440000", "#550000", "#660000", "#770000",
        ];
        let resolved = resolve(&inline(&hexes)).expect("eight colors resolve");

        assert_eq!(
            resolved[0],
            linear_rgba("#000000".parse().expect("a color"))
        );
        assert_eq!(
            resolved[4],
            linear_rgba("#770000".parse().expect("a color"))
        );
        for slot in resolved {
            for channel in &slot[..3] {
                assert!((0.0..=1.0).contains(channel), "{channel} left the gamut");
            }
        }
    }

    /// A blend between two in-gamut colors can leave the sRGB gamut in Oklab.
    /// It must not leave the uniform.
    #[test]
    fn a_resampled_palette_never_leaves_the_gamut() {
        let resolved = resolve(&inline(&["#00ff00", "#0000ff"])).expect("two colors resolve");

        for slot in resolved {
            for channel in slot {
                assert!((0.0..=1.0).contains(&channel), "{channel} is outside 0..=1",);
            }
        }
    }

    /// An unknown name is the user's argument. Say what does exist.
    #[test]
    fn unknown_palette_name_lists_valid_names() {
        let err = by_name("embers").expect_err("there is no `embers`");

        assert!(matches!(err, Error::Config(_)), "got {err:?}");
        let message = err.to_string();
        assert!(message.contains("embers"), "quote the typo: {message}");
        for name in names() {
            assert!(message.contains(name), "`{name}` is missing from {message}");
        }
    }

    /// Every hex color survives the trip into linear light and back.
    #[test]
    fn srgb_to_linear_round_trip_within_epsilon() {
        for channel in 0..=u8::MAX {
            let light = srgb_to_linear(channel);
            assert!((0.0..=1.0).contains(&light), "{channel} -> {light}");
            assert_eq!(linear_to_srgb(light), channel, "{channel} did not survive");
        }

        assert_eq!(srgb_to_linear(0), 0.0);
        assert_eq!(srgb_to_linear(255), 1.0);
        assert!(
            close(srgb_to_linear(0x80), 0.2158),
            "mid-grey is {} in linear, expected ~0.216",
            srgb_to_linear(0x80),
        );
    }

    /// Oklab is a detour, and a detour must arrive where it started.
    #[test]
    fn oklab_round_trips_through_linear_rgb() {
        for hex in ["#000000", "#ffffff", "#1a1a2e", "#e94560", "#3fa9c9"] {
            let color: Color = hex.parse().expect("a color");
            let linear = linear_rgba(color);
            let back = oklab_to_linear(oklab(linear));

            for channel in 0..3 {
                assert!(
                    close(back[channel], linear[channel]),
                    "{hex} channel {channel}: {} != {}",
                    back[channel],
                    linear[channel],
                );
            }
        }
    }

    /// Each built-in is five colors, darkest first, and carries the one-line
    /// character note the palettes are chosen by.
    #[test]
    fn every_built_in_is_a_dark_first_ramp_with_a_character_note() {
        for palette in BUILT_INS {
            assert!(!palette.character.is_empty(), "`{}`", palette.name);
            assert!(
                palette.colors.iter().all(|color| color.a == 0xff),
                "`{}`: a translucent slot would wash the preset out",
                palette.name,
            );

            let lightness: Vec<f32> = palette
                .colors
                .iter()
                .map(|color| oklab(linear_rgba(*color))[0])
                .collect();
            for pair in lightness.windows(2) {
                assert!(
                    pair[0] < pair[1],
                    "`{}` is not ordered darkest-first: {lightness:?}",
                    palette.name,
                );
            }

            // Slot 0 is the background: it has to stay out of the way.
            assert!(
                lightness[0] < 0.3,
                "`{}` has a background slot too bright to sit behind a preset",
                palette.name,
            );
        }

        assert_eq!(
            names(),
            ["ember", "glacier", "verdant", "mono", "carpathian"]
        );
    }

    /// Two palettes that resolved to the same colors would render the same video
    /// under two names.
    #[test]
    fn no_two_built_ins_resolve_to_the_same_colors() {
        for (index, palette) in BUILT_INS.iter().enumerate() {
            for other in &BUILT_INS[index + 1..] {
                assert_ne!(palette.colors, other.colors, "`{}`", palette.name);
            }
        }
    }

    /// The variant is public, so an empty one can be built. It must not index
    /// out of a zero-length slice.
    #[test]
    fn an_inline_palette_of_the_wrong_length_is_a_config_error() {
        let err = resolve(&Palette::Inline(Vec::new())).expect_err("no colors, no palette");
        assert!(matches!(err, Error::Config(_)), "got {err:?}");

        let too_many = vec![Color::rgb(0, 0, 0); MAX_PALETTE_COLORS + 1];
        resolve(&Palette::Inline(too_many)).expect_err("nine colors is too many");
    }
}
