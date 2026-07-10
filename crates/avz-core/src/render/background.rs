//! The bottom layer: what the frame is before the visualizer draws on it.
//!
//! `VISION.md` §5.3 lists three things the background layer can be: a solid
//! color or gradient from the palette, a static image, or a looped video. This
//! module is the first of them — the one that is always available, because a
//! palette always is. The image lands in RFC-001 Step 19 and the video in NG2;
//! both become another way to fill the same [`Layer`].
//!
//! Before the compositor existed, every preset opened by painting `pal[0]` over
//! the frame and the render target was cleared to black behind it. That is now
//! this layer's job, and the presets draw light alone — which is what lets them
//! be transparent where they draw none.
//!
//! **Built on the CPU.** The backdrop is a function of the palette and the frame
//! size, so it is the same for every frame of a render: computed once, uploaded
//! once, and then it is just a texture the compositor reads. That also makes the
//! gradient itself testable without a GPU ([`gradient`] below).

use crate::render::layer::Layer;
use crate::render::offscreen::Gpu;
use crate::render::palette::{LinearPalette, linear_to_srgb};

/// What the default background layer draws.
///
/// Both forms are built from the palette, which is why neither takes a color:
/// `--palette` is the one place a render's colors are chosen (`VISION.md` §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Backdrop {
    /// Palette slot 0, everywhere. The flattest thing a frame can sit on.
    Solid,
    /// Palette slot 0 at the top of the frame, slot 1 at the bottom.
    ///
    /// The default: a built-in palette is ordered darkest-first, so this is a
    /// dark sky lifting toward its first accent.
    #[default]
    Gradient,
}

impl Backdrop {
    /// Build the `width × height` background layer for `palette`.
    ///
    /// Opaque, so a composited frame reaches ffmpeg with alpha 255 whatever the
    /// layers above it did.
    pub fn layer(self, gpu: &Gpu, width: u32, height: u32, palette: LinearPalette) -> Layer {
        let layer = Layer::new(gpu, width, height, "avz background");
        let pixels = self.pixels(width, height, palette);

        gpu.queue().write_texture(
            layer.texture().as_image_copy(),
            &pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            layer.texture().size(),
        );
        gpu.queue().submit([]);

        layer
    }

    /// The layer's pixels, tightly packed sRGB RGBA.
    ///
    /// Interpolated in linear light, then encoded once — the same trip the
    /// shaders make. Mixing the two 8-bit encodings directly would darken the
    /// middle of every gradient by roughly a stop.
    fn pixels(self, width: u32, height: u32, palette: LinearPalette) -> Vec<u8> {
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);

        for y in 0..height {
            let row = gradient(palette, self.fraction(y, height));
            for _ in 0..width {
                pixels.extend_from_slice(&row);
            }
        }

        pixels
    }

    /// How far down the gradient row `y` sits, in `0.0..=1.0`.
    ///
    /// A one-pixel-tall frame is the top of the gradient rather than a division
    /// by zero.
    fn fraction(self, y: u32, height: u32) -> f32 {
        match self {
            Self::Solid => 0.0,
            Self::Gradient if height <= 1 => 0.0,
            Self::Gradient => y as f32 / (height - 1) as f32,
        }
    }
}

/// Palette slots 0 and 1, mixed `fraction` of the way apart in linear light, as
/// the opaque sRGB bytes a frame stores.
fn gradient(palette: LinearPalette, fraction: f32) -> [u8; 4] {
    let [top, bottom] = [palette[0], palette[1]];

    let mut row = [0u8, 0, 0, 255];
    for channel in 0..3 {
        let light = top[channel] + (bottom[channel] - top[channel]) * fraction;
        row[channel] = linear_to_srgb(light);
    }
    row
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Palette;
    use crate::render::palette::resolve;

    fn ember() -> LinearPalette {
        resolve(&Palette::Named("ember".to_owned())).expect("`ember` ships")
    }

    fn row(pixels: &[u8], width: u32, y: u32) -> [u8; 4] {
        let at = ((y * width) * 4) as usize;
        [pixels[at], pixels[at + 1], pixels[at + 2], pixels[at + 3]]
    }

    /// The endpoints are the palette slots themselves, not something near them:
    /// `--palette ember` must put `#1a1a2e` at the top of the frame.
    #[test]
    fn the_gradient_runs_from_palette_slot_zero_to_slot_one() {
        let pixels = Backdrop::Gradient.pixels(4, 9, ember());

        assert_eq!(row(&pixels, 4, 0), [0x1a, 0x1a, 0x2e, 0xff]);
        assert_eq!(row(&pixels, 4, 8), [0x53, 0x34, 0x83, 0xff]);
    }

    /// Mixed in light, not in the 8-bit encoding of light. The midpoint of
    /// `#1a1a2e` and `#533483` is `#40265f`-ish in linear and a good deal darker
    /// if the bytes are averaged directly.
    #[test]
    fn the_gradient_is_interpolated_in_linear_light() {
        let middle = gradient(ember(), 0.5);
        let naive = [(0x1a + 0x53) / 2, (0x1a + 0x34) / 2, (0x2e + 0x83) / 2];

        for channel in 0..3 {
            assert!(
                middle[channel] > naive[channel],
                "channel {channel} midpoint {} is no brighter than the byte average {}",
                middle[channel],
                naive[channel],
            );
        }
    }

    /// Every row of a gradient is flat, and every row of a solid backdrop is the
    /// same flat row.
    #[test]
    fn a_backdrop_never_varies_across_a_row() {
        for style in [Backdrop::Solid, Backdrop::Gradient] {
            let pixels = style.pixels(3, 4, ember());
            for y in 0..4 {
                let expected = row(&pixels, 3, y);
                for x in 0..3 {
                    let at = ((y * 3 + x) * 4) as usize;
                    assert_eq!(pixels[at..at + 4], expected, "{style:?} row {y} col {x}");
                }
            }
        }
    }

    #[test]
    fn a_solid_backdrop_is_palette_slot_zero_at_every_row() {
        let pixels = Backdrop::Solid.pixels(2, 5, ember());

        for y in 0..5 {
            assert_eq!(row(&pixels, 2, y), [0x1a, 0x1a, 0x2e, 0xff]);
        }
    }

    /// A one-pixel-tall frame is legal and must not divide by zero.
    #[test]
    fn a_single_row_frame_is_the_top_of_the_gradient() {
        let pixels = Backdrop::Gradient.pixels(1, 1, ember());

        assert_eq!(pixels, vec![0x1a, 0x1a, 0x2e, 0xff]);
    }

    /// The default is the gradient: it is what `avz render song.mp3` puts under
    /// the visualizer with no `[background]` section at all.
    #[test]
    fn the_default_backdrop_is_the_gradient() {
        assert_eq!(Backdrop::default(), Backdrop::Gradient);
    }
}
