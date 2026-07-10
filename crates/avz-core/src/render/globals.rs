//! The uniform every preset receives.
//!
//! `VISION.md` §6 fixes the contract: animation time, frame size, seed, every
//! feature raw and enveloped, the onset impulse, the centroid, five palette
//! slots, and eight `vec4` parameter slots. A preset is one WGSL file against
//! this struct, which is why the struct is the thing that must not drift.
//!
//! [`Globals::to_bytes`] writes the WGSL uniform layout by hand rather than
//! transmuting a `#[repr(C)]` struct, because `avz-core` is `forbid(unsafe_code)`
//! and the padding is worth seeing. The layout naga computes for the WGSL
//! declaration is:
//!
//! ```text
//!   0  time: f32
//!   4  -- 4 bytes of padding (vec2<f32> aligns to 8)
//!   8  resolution: vec2<f32>
//!  16  seed: f32
//!  20  rms: f32          24  rms_env: f32
//!  28  bass: f32         32  bass_env: f32
//!  36  low_mid: f32      40  low_mid_env: f32
//!  44  mid: f32          48  mid_env: f32
//!  52  high: f32         56  high_env: f32
//!  60  air: f32          64  air_env: f32
//!  68  flux: f32
//!  72  onset: f32
//!  76  centroid: f32
//!  80  pal: array<vec4<f32>, 5>      (5 × 16 B)
//! 160  params: array<vec4<f32>, 8>   (8 × 16 B)
//! 288  -- end
//! ```
//!
//! The two arrays land on 16-byte boundaries without padding in front of them,
//! which is what WGSL's uniform address space demands of an array member.

use crate::analysis::FeatureFrame;
use crate::config::{Color, MAX_PALETTE_COLORS};
use crate::render::schema::PackedParams;

/// Palette slots the uniform carries (`VISION.md` §6: `pal: array<vec4, 5>`).
pub const PALETTE_SLOTS: usize = MAX_PALETTE_COLORS;

/// Preset parameter slots (`VISION.md` §6: `params: array<vec4, 8>`).
pub const PARAM_SLOTS: usize = 8;

/// Size of the encoded uniform in bytes. See the module docs for the layout.
pub const GLOBALS_SIZE: usize = 288;

/// One frame's worth of the `VISION.md` §6 uniform contract.
///
/// `time` is always `frame_index / fps` — never a wall clock, never a frame
/// delta (`AGENTS.md`, determinism). `seed` is a float in `0.0..1.0` derived
/// from the render's `u64` seed by [`seed_fraction`], so a shader can hash it
/// together with the fragment position and `time` and stay reproducible.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Globals {
    /// `frame_index / fps`, in seconds.
    pub time: f32,
    /// Frame size in pixels, as floats.
    pub resolution: [f32; 2],
    /// The render seed, mapped into `0.0..1.0`.
    pub seed: f32,
    /// The analyzed features of this frame.
    pub features: FeatureFrame,
    /// Five palette colors in **linear** space, RGBA.
    ///
    /// Linear because the render target is `Rgba8UnormSrgb` and encodes on
    /// write, so a shader that blends must blend in linear space.
    pub palette: [[f32; 4]; PALETTE_SLOTS],
    /// The active preset's parameters, packed by
    /// [`PresetSchema::resolve`](crate::render::PresetSchema::resolve).
    pub params: PackedParams,
}

impl Globals {
    /// The uniform for video frame `frame_index` of a song rendered at `fps`.
    ///
    /// `params` is packed once per render, not once per frame: a preset's
    /// parameters are configuration, and configuration does not move with the
    /// music.
    pub fn for_frame(
        frame_index: usize,
        fps: u32,
        resolution: (u32, u32),
        seed: u64,
        features: FeatureFrame,
        palette: [Color; PALETTE_SLOTS],
        params: PackedParams,
    ) -> Self {
        // The f64 divide is `FeatureTimeline::timestamp`'s, so the shader's clock
        // and the timeline's agree exactly on every frame.
        let time = (frame_index as f64 / f64::from(fps)) as f32;

        Self {
            time,
            resolution: [resolution.0 as f32, resolution.1 as f32],
            seed: seed_fraction(seed),
            features,
            palette: palette.map(linear_rgba),
            params,
        }
    }

    /// Encode into the bytes the WGSL `Globals` uniform expects.
    pub fn to_bytes(&self) -> [u8; GLOBALS_SIZE] {
        let mut out = Writer::default();

        out.scalar(self.time);
        out.pad(4);
        out.scalar(self.resolution[0]);
        out.scalar(self.resolution[1]);
        out.scalar(self.seed);

        let f = &self.features;
        for value in [
            f.rms,
            f.rms_env,
            f.bass,
            f.bass_env,
            f.low_mid,
            f.low_mid_env,
            f.mid,
            f.mid_env,
            f.high,
            f.high_env,
            f.air,
            f.air_env,
            f.flux,
            f.onset,
            f.centroid,
        ] {
            out.scalar(value);
        }

        for slot in self.palette.iter().chain(&self.params) {
            for channel in slot {
                out.scalar(*channel);
            }
        }

        out.finish()
    }
}

/// A cursor that writes little-endian `f32`s into the uniform's byte layout.
#[derive(Debug)]
struct Writer {
    bytes: [u8; GLOBALS_SIZE],
    at: usize,
}

impl Default for Writer {
    fn default() -> Self {
        Self {
            bytes: [0; GLOBALS_SIZE],
            at: 0,
        }
    }
}

impl Writer {
    fn scalar(&mut self, value: f32) {
        self.bytes[self.at..self.at + 4].copy_from_slice(&value.to_le_bytes());
        self.at += 4;
    }

    /// Skip `count` bytes of alignment padding, leaving them zero.
    fn pad(&mut self, count: usize) {
        self.at += count;
    }

    fn finish(self) -> [u8; GLOBALS_SIZE] {
        assert_eq!(
            self.at, GLOBALS_SIZE,
            "the encoder and GLOBALS_SIZE disagree about the uniform layout"
        );
        self.bytes
    }
}

/// The seed a shader sees: a `f32` in `0.0..1.0`, from a `u64` render seed.
///
/// A `u64` does not survive the trip through an `f32`, so it is mixed down to 23
/// bits and laid straight into a mantissa. Every result is exactly representable,
/// so no adapter rounds it differently — which is the whole point of seeding.
pub fn seed_fraction(seed: u64) -> f32 {
    // splitmix64's finalizer: avalanches adjacent seeds into unrelated bits, so
    // `--seed 1` and `--seed 2` do not render nearly the same video.
    let mut z = seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^= z >> 31;

    // 23 mantissa bits under an exponent of 0 gives 1.0..2.0; subtract the one.
    f32::from_bits(0x3f80_0000 | (z >> 41) as u32) - 1.0
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

/// The sRGB electro-optical transfer function, inverted.
fn srgb_to_linear(channel: u8) -> f32 {
    let encoded = f32::from(channel) / 255.0;
    if encoded <= 0.040_45 {
        encoded / 12.92
    } else {
        ((encoded + 0.055) / 1.055).powf(2.4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn features() -> FeatureFrame {
        FeatureFrame::default()
    }

    fn white() -> [Color; PALETTE_SLOTS] {
        ["#ffffff".parse().expect("a color"); PALETTE_SLOTS]
    }

    /// A preset with every parameter at zero: the uniform layout is what these
    /// tests are about, not any particular preset's schema.
    fn no_params() -> PackedParams {
        [[0.0; 4]; PARAM_SLOTS]
    }

    /// Read the `f32` at `offset` out of an encoded uniform.
    fn at(bytes: &[u8; GLOBALS_SIZE], offset: usize) -> f32 {
        let mut four = [0; 4];
        four.copy_from_slice(&bytes[offset..offset + 4]);
        f32::from_le_bytes(four)
    }

    /// A `Globals` with one feature field set, so a scan finds its offset.
    fn with(field: impl FnOnce(&mut FeatureFrame)) -> [u8; GLOBALS_SIZE] {
        let mut frame = features();
        field(&mut frame);
        Globals {
            time: 0.0,
            resolution: [0.0, 0.0],
            seed: 0.0,
            features: frame,
            palette: [[0.0; 4]; PALETTE_SLOTS],
            params: [[0.0; 4]; PARAM_SLOTS],
        }
        .to_bytes()
    }

    /// The layout the WGSL `struct Globals` declares, member by member. A field
    /// inserted, reordered, or padded differently on either side of the boundary
    /// silently feeds every preset the wrong number.
    #[test]
    fn globals_layout_matches_wgsl() {
        assert_eq!(GLOBALS_SIZE, 288, "the whole uniform");
        assert_eq!(GLOBALS_SIZE % 16, 0, "a uniform struct aligns to 16 bytes");

        let bytes = Globals {
            time: 1.0,
            resolution: [1920.0, 1080.0],
            seed: 0.5,
            features: features(),
            palette: [[0.0; 4]; PALETTE_SLOTS],
            params: [[0.0; 4]; PARAM_SLOTS],
        }
        .to_bytes();

        assert_eq!(at(&bytes, 0), 1.0, "time at 0");
        assert_eq!(at(&bytes, 4), 0.0, "vec2<f32> aligns to 8: padding at 4");
        assert_eq!(at(&bytes, 8), 1920.0, "resolution.x at 8");
        assert_eq!(at(&bytes, 12), 1080.0, "resolution.y at 12");
        assert_eq!(at(&bytes, 16), 0.5, "seed at 16");

        let offsets = [
            (20, "rms", with(|f| f.rms = 1.0)),
            (24, "rms_env", with(|f| f.rms_env = 1.0)),
            (28, "bass", with(|f| f.bass = 1.0)),
            (32, "bass_env", with(|f| f.bass_env = 1.0)),
            (36, "low_mid", with(|f| f.low_mid = 1.0)),
            (40, "low_mid_env", with(|f| f.low_mid_env = 1.0)),
            (44, "mid", with(|f| f.mid = 1.0)),
            (48, "mid_env", with(|f| f.mid_env = 1.0)),
            (52, "high", with(|f| f.high = 1.0)),
            (56, "high_env", with(|f| f.high_env = 1.0)),
            (60, "air", with(|f| f.air = 1.0)),
            (64, "air_env", with(|f| f.air_env = 1.0)),
            (68, "flux", with(|f| f.flux = 1.0)),
            (72, "onset", with(|f| f.onset = 1.0)),
            (76, "centroid", with(|f| f.centroid = 1.0)),
        ];
        for (offset, name, bytes) in offsets {
            assert_eq!(at(&bytes, offset), 1.0, "{name} belongs at {offset}");
            let elsewhere = (0..GLOBALS_SIZE)
                .step_by(4)
                .filter(|&other| other != offset)
                .all(|other| at(&bytes, other) == 0.0);
            assert!(elsewhere, "{name} also wrote somewhere other than {offset}");
        }
    }

    /// Both arrays must start on a 16-byte boundary, and `pal` must hold exactly
    /// five `vec4`s before `params` begins.
    #[test]
    fn the_palette_and_param_arrays_sit_on_sixteen_byte_boundaries() {
        let mut globals = Globals {
            time: 0.0,
            resolution: [0.0, 0.0],
            seed: 0.0,
            features: features(),
            palette: [[0.0; 4]; PALETTE_SLOTS],
            params: [[0.0; 4]; PARAM_SLOTS],
        };
        globals.palette[0] = [1.0, 2.0, 3.0, 4.0];
        globals.palette[4] = [5.0, 6.0, 7.0, 8.0];
        globals.params[0] = [9.0, 0.0, 0.0, 0.0];
        globals.params[7] = [0.0, 0.0, 0.0, 10.0];

        let bytes = globals.to_bytes();

        assert_eq!(80 % 16, 0);
        assert_eq!(at(&bytes, 80), 1.0, "pal[0].r at 80");
        assert_eq!(at(&bytes, 92), 4.0, "pal[0].a at 92");
        assert_eq!(at(&bytes, 144), 5.0, "pal[4].r at 144");

        assert_eq!(160 % 16, 0);
        assert_eq!(at(&bytes, 160), 9.0, "params[0].x at 160");
        assert_eq!(at(&bytes, 284), 10.0, "params[7].w at 284");
    }

    /// `time` is `frame_index / fps` and nothing else (`AGENTS.md`, determinism).
    #[test]
    fn time_is_the_frame_index_over_the_frame_rate() {
        for (index, fps, expected) in [(0, 30, 0.0), (30, 30, 1.0), (45, 30, 1.5), (60, 24, 2.5)] {
            let globals =
                Globals::for_frame(index, fps, (320, 180), 0, features(), white(), no_params());
            assert_eq!(globals.time, expected, "frame {index} at {fps} fps");
        }
    }

    #[test]
    fn the_seed_is_a_fraction_of_one_and_two_seeds_differ() {
        for seed in [0, 1, 2, 7, u64::MAX] {
            let fraction = seed_fraction(seed);
            assert!(
                (0.0..1.0).contains(&fraction),
                "seed {seed} mapped to {fraction}"
            );
        }

        assert_ne!(seed_fraction(0), seed_fraction(1));
        assert_ne!(seed_fraction(1), seed_fraction(2));
        assert_eq!(seed_fraction(42), seed_fraction(42), "and it is a function");
    }

    /// The palette reaches the shader in linear space, because the render target
    /// encodes to sRGB on write.
    #[test]
    fn the_palette_reaches_the_shader_in_linear_space() {
        let colors: [Color; PALETTE_SLOTS] = [
            "#000000".parse().expect("a color"),
            "#ffffff".parse().expect("a color"),
            "#808080".parse().expect("a color"),
            "#ffffff".parse().expect("a color"),
            "#ffffff".parse().expect("a color"),
        ];
        let globals = Globals::for_frame(0, 30, (320, 180), 0, features(), colors, no_params());

        assert_eq!(globals.palette[0], [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(globals.palette[1], [1.0, 1.0, 1.0, 1.0]);
        // Mid-grey is ~0.216 in linear, not 0.5. Getting this wrong washes every
        // palette out by a stop and a half.
        assert!(
            (globals.palette[2][0] - 0.2158).abs() < 0.001,
            "sRGB 0x80 is {} in linear, expected ~0.216",
            globals.palette[2][0],
        );
    }
}
