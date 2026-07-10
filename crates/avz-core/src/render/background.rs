//! The bottom layer: what the frame is before the visualizer draws on it.
//!
//! `VISION.md` §5.3 lists three things the background layer can be: a solid
//! color or gradient from the palette, a static image, or a looped video. The
//! first two live here. The video is a layer of its own
//! ([`video`](crate::render::video)), because it has a frame per rendered frame
//! and this one is built once; under it, this layer is the bare palette backdrop
//! its letterbox bars show through.
//!
//! Before the compositor existed, every preset opened by painting `pal[0]` over
//! the frame and the render target was cleared to black behind it. That is now
//! this layer's job, and the presets draw light alone — which is what lets them
//! be transparent where they draw none.
//!
//! **The image sits on the backdrop, not instead of it.** `fit = "contain"`
//! letterboxes, and something has to fill the bars; a PNG with an alpha channel
//! has holes, and something has to show through them. Both are the same
//! question, and the palette backdrop is the same answer — so the layer is built
//! by drawing the fitted image *over* the backdrop rather than in place of it.
//!
//! **Built on the CPU, in linear light.** The layer is a function of the palette,
//! the image, and the frame size, so it is the same for every frame of a render:
//! computed once, uploaded once, and then it is just a texture the compositor
//! reads. Blurring and darkening sRGB bytes directly would darken the picture —
//! light is what averages and what dims — so every step between decoding the
//! image and encoding the layer happens in linear f32, and the sRGB transfer
//! function is applied exactly once, on the way out.

use std::io::Cursor;
use std::path::Path;

use image::imageops::{self, FilterType};
use image::{ImageReader, Rgb, Rgb32FImage, Rgba32FImage};

use crate::config::{Background as BackgroundConfig, BackgroundSource, Fit};
use crate::render::layer::Layer;
use crate::render::offscreen::Gpu;
use crate::render::palette::{LinearPalette, linear_to_srgb, srgb_to_linear};
use crate::{Error, Result};

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
        Background::from(self).layer(gpu, width, height, palette)
    }

    /// The backdrop as the linear-light canvas everything else is drawn onto.
    ///
    /// Interpolated in linear light, then encoded once — the same trip the
    /// shaders make. Mixing the two 8-bit encodings directly would darken the
    /// middle of every gradient by roughly a stop.
    fn canvas(self, width: u32, height: u32, palette: LinearPalette) -> Rgb32FImage {
        let mut canvas = Rgb32FImage::new(width, height);
        for y in 0..height {
            let row = Rgb(gradient(palette, self.fraction(y, height)));
            for x in 0..width {
                canvas.put_pixel(x, y, row);
            }
        }
        canvas
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

/// The whole background layer: the palette backdrop, an optional image fitted
/// over it, and the blur and darken that keep the visuals readable on top
/// (`VISION.md` §5.5, `[background]`).
#[derive(Debug, Clone)]
pub struct Background {
    /// What fills the frame where no image does.
    backdrop: Backdrop,
    /// The decoded image, premultiplied and in linear light.
    image: Option<Rgba32FImage>,
    /// How the image is fitted to the frame.
    fit: Fit,
    /// Gaussian standard deviation, in pixels of the *output* frame.
    blur: f32,
    /// How much light to take away, `0.0..=1.0`.
    darken: f32,
}

impl From<Backdrop> for Background {
    fn from(backdrop: Backdrop) -> Self {
        Self {
            backdrop,
            image: None,
            fit: Fit::Cover,
            blur: 0.0,
            darken: 0.0,
        }
    }
}

impl Background {
    /// Read `[background]`, decoding the image it names.
    ///
    /// Called before the song is decoded: a background image that does not exist
    /// is the user's argument, and they should hear about it in the first
    /// millisecond rather than after a five-minute analysis pass.
    ///
    /// A `background.video` is *not* decoded here: it has a frame per rendered
    /// frame and an ffmpeg of its own ([`BackgroundVideo`]). What this layer
    /// becomes for it is the palette backdrop alone — which is what a `contain`
    /// letterbox shows through, exactly as under an image. Its path is still
    /// checked, because that is the point of loading before the song is decoded.
    ///
    /// # Errors
    ///
    /// [`Error::Input`] if the image cannot be read or decoded, or if the video
    /// is not there, naming the path.
    ///
    /// [`BackgroundVideo`]: crate::render::BackgroundVideo
    pub fn load(config: &BackgroundConfig) -> Result<Self> {
        let image = match &config.source {
            None => None,
            Some(BackgroundSource::Image(path)) => Some(decode(path)?),
            Some(BackgroundSource::Video(path)) => {
                exists(path)?;
                None
            }
        };

        Ok(Self {
            backdrop: Backdrop::default(),
            image,
            fit: config.fit,
            blur: config.blur,
            darken: config.darken,
        })
    }

    /// The decoded image's pixel dimensions, or `None` when there is no image.
    ///
    /// The pipeline compares these against the frame: an image the renderer has
    /// to enlarge is a soft video and no error at all.
    pub fn image_size(&self) -> Option<(u32, u32)> {
        self.image.as_ref().map(|image| image.dimensions())
    }

    /// Build the `width × height` background layer for `palette`.
    ///
    /// Opaque, so a composited frame reaches ffmpeg with alpha 255 whatever the
    /// layers above it did.
    pub fn layer(&self, gpu: &Gpu, width: u32, height: u32, palette: LinearPalette) -> Layer {
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
    /// Backdrop, then the fitted image over it, then the blur, then the darken —
    /// in that order. The blur runs after the image lands so that a `contain`
    /// letterbox has a soft seam rather than a hard one, and the darken runs
    /// after the blur so that `darken = 1.0` is black however much the blur
    /// smeared.
    pub fn pixels(&self, width: u32, height: u32, palette: LinearPalette) -> Vec<u8> {
        let mut canvas = self.backdrop.canvas(width, height, palette);

        if let Some(image) = &self.image {
            draw(&mut canvas, image, self.fit);
        }
        if self.blur > 0.0 {
            canvas = imageops::fast_blur(&canvas, self.blur);
        }

        encode(&canvas, self.darken)
    }
}

/// Where a source image lands in the frame: which rectangle of it is drawn, and
/// which rectangle of the frame it is drawn into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Placement {
    /// `[x, y, width, height]` of the source rectangle that survives the fit.
    crop: [u32; 4],
    /// Left edge of the destination rectangle.
    x: u32,
    /// Top edge of the destination rectangle.
    y: u32,
    /// Destination width.
    width: u32,
    /// Destination height.
    height: u32,
}

/// How `fit` places a `source`-sized image in a `frame`-sized layer.
///
/// `cover` is expressed as a crop rather than as an oversized scale that the
/// frame then clips. The two draw the same pixels, but the scale is unbounded:
/// covering a 1920×1080 frame with a 1×1000 source needs a 1920× enlargement,
/// and the intermediate image is sixty gigabytes. Cropping the source to the
/// frame's aspect ratio first bounds every allocation by the frame.
fn place(fit: Fit, source: (u32, u32), frame: (u32, u32)) -> Placement {
    let (source_width, source_height) = source;
    let (frame_width, frame_height) = frame;
    let whole = [0, 0, source_width, source_height];

    // Whether the source is wider than the frame, compared as a ratio of ratios
    // so neither side has to become a float.
    let wider = u64::from(source_width) * u64::from(frame_height)
        > u64::from(frame_width) * u64::from(source_height);

    match fit {
        Fit::Stretch => Placement {
            crop: whole,
            x: 0,
            y: 0,
            width: frame_width,
            height: frame_height,
        },
        Fit::Cover => {
            let (width, height) = if wider {
                (
                    scaled(source_height, frame_width, frame_height),
                    source_height,
                )
            } else {
                (
                    source_width,
                    scaled(source_width, frame_height, frame_width),
                )
            };
            let width = width.clamp(1, source_width);
            let height = height.clamp(1, source_height);

            Placement {
                crop: [
                    (source_width - width) / 2,
                    (source_height - height) / 2,
                    width,
                    height,
                ],
                x: 0,
                y: 0,
                width: frame_width,
                height: frame_height,
            }
        }
        Fit::Contain => {
            let (width, height) = if wider {
                (
                    frame_width,
                    scaled(source_height, frame_width, source_width),
                )
            } else {
                (
                    scaled(source_width, frame_height, source_height),
                    frame_height,
                )
            };
            let width = width.clamp(1, frame_width);
            let height = height.clamp(1, frame_height);

            Placement {
                crop: whole,
                x: (frame_width - width) / 2,
                y: (frame_height - height) / 2,
                width,
                height,
            }
        }
    }
}

/// `value * numerator / denominator`, rounded to the nearest whole pixel.
///
/// In `u64`, because the three factors of a 4K frame overflow a `u32` long
/// before any of them is an unreasonable image.
fn scaled(value: u32, numerator: u32, denominator: u32) -> u32 {
    let denominator = u64::from(denominator).max(1);
    let rounded = (u64::from(value) * u64::from(numerator) * 2 + denominator) / (2 * denominator);
    u32::try_from(rounded).unwrap_or(u32::MAX)
}

/// Draw `image` over `canvas`, fitted, with the `over` operator.
///
/// The image is premultiplied, so `over` is `src + (1 - src.a) * dst` — the same
/// blend the compositor asks the GPU for one layer up
/// ([`Compositor`](crate::render::Compositor)).
fn draw(canvas: &mut Rgb32FImage, image: &Rgba32FImage, fit: Fit) {
    let placement = place(
        fit,
        (image.width(), image.height()),
        (canvas.width(), canvas.height()),
    );

    let [crop_x, crop_y, crop_width, crop_height] = placement.crop;
    let cropped = imageops::crop_imm(image, crop_x, crop_y, crop_width, crop_height).to_image();
    // Lanczos: a background is the one thing in the frame the eye can study, and
    // it is resampled once per render rather than once per frame.
    let scaled = imageops::resize(
        &cropped,
        placement.width,
        placement.height,
        FilterType::Lanczos3,
    );

    for (x, y, source) in scaled.enumerate_pixels() {
        let [red, green, blue, alpha] = source.0;
        // Lanczos overshoots, and an overshot premultiplied pixel claims to emit
        // more light than it covers. Clamping restores `rgb <= a`.
        let alpha = alpha.clamp(0.0, 1.0);
        let light = [red, green, blue].map(|channel| channel.clamp(0.0, alpha));

        let target = canvas.get_pixel_mut(placement.x + x, placement.y + y);
        for (behind, over) in target.0.iter_mut().zip(light) {
            *behind = over + (1.0 - alpha) * *behind;
        }
    }
}

/// Fail if `path` is not a file avz can open.
///
/// The video itself is ffmpeg's to read, and ffmpeg is not spawned until the
/// song has been analyzed. A typo'd `background.video` would otherwise cost a
/// full analysis pass before anyone mentioned it.
fn exists(path: &Path) -> Result<()> {
    std::fs::File::open(path)
        .map(drop)
        .map_err(|err| Error::Input(format!("`{}`: {err}", path.display())))
}

/// Read and decode a background image.
fn decode(path: &Path) -> Result<Rgba32FImage> {
    let bytes =
        std::fs::read(path).map_err(|err| Error::Input(format!("`{}`: {err}", path.display())))?;

    let decoded = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|err| Error::Input(format!("`{}`: {err}", path.display())))?
        .decode()
        .map_err(|err| {
            let message = format!(
                "`{}` is not a background image avz can read: {err}",
                path.display(),
            );
            // The most likely way to arrive here with a video is `--bg`, whose
            // owner needs the spelling that works, not just the failure.
            let message = if looks_like_video(path) {
                format!(
                    "{message}; a still image was expected — a looped, muted background \
                     video is `background.video`: pass `--set background.video={}` or \
                     set it in a config file",
                    path.display(),
                )
            } else {
                message
            };
            Error::Input(message)
        })?;

    Ok(premultiplied_linear(&decoded))
}

/// Whether a file that failed to decode as an image looks like a video.
///
/// Judged by extension alone, and only after the image sniff has already
/// failed: the point is to route the user to `background.video`, where ffmpeg
/// — not avz — is the judge of the contents.
fn looks_like_video(path: &Path) -> bool {
    const VIDEO_EXTENSIONS: [&str; 9] = [
        "mp4", "m4v", "mkv", "webm", "mov", "avi", "mpg", "mpeg", "wmv",
    ];

    path.extension()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|extension| {
            VIDEO_EXTENSIONS
                .iter()
                .any(|video| extension.eq_ignore_ascii_case(video))
        })
}

/// An sRGB image as premultiplied linear light.
fn premultiplied_linear(decoded: &image::DynamicImage) -> Rgba32FImage {
    let source = decoded.to_rgba8();
    let mut linear = Rgba32FImage::new(source.width(), source.height());

    for (x, y, pixel) in source.enumerate_pixels() {
        let [r, g, b, a] = pixel.0;
        let coverage = f32::from(a) / 255.0;
        linear.put_pixel(
            x,
            y,
            image::Rgba([
                srgb_to_linear(r) * coverage,
                srgb_to_linear(g) * coverage,
                srgb_to_linear(b) * coverage,
                coverage,
            ]),
        );
    }

    linear
}

/// Palette slots 0 and 1, mixed `fraction` of the way apart, in linear light.
fn gradient(palette: LinearPalette, fraction: f32) -> [f32; 3] {
    let [top, bottom] = [palette[0], palette[1]];

    let mut row = [0.0f32; 3];
    for channel in 0..3 {
        row[channel] = top[channel] + (bottom[channel] - top[channel]) * fraction;
    }
    row
}

/// Dim `canvas` by `darken` and encode it as the opaque sRGB bytes a frame
/// stores.
///
/// Dimming is a multiply in *light*: `darken = 0.5` halves the photons, which is
/// a good deal brighter than halving the 8-bit channel.
fn encode(canvas: &Rgb32FImage, darken: f32) -> Vec<u8> {
    let keep = 1.0 - darken;

    let mut pixels = Vec::with_capacity((canvas.width() * canvas.height() * 4) as usize);
    for pixel in canvas.pixels() {
        for channel in pixel.0 {
            pixels.push(linear_to_srgb(channel * keep));
        }
        pixels.push(255);
    }
    pixels
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Palette;
    use crate::render::palette::resolve;

    fn ember() -> LinearPalette {
        resolve(&Palette::Named("ember".to_owned())).expect("`ember` ships")
    }

    /// The layer a bare `[background]` builds: the backdrop and nothing over it.
    fn backdrop_pixels(
        style: Backdrop,
        width: u32,
        height: u32,
        palette: LinearPalette,
    ) -> Vec<u8> {
        Background::from(style).pixels(width, height, palette)
    }

    fn row(pixels: &[u8], width: u32, y: u32) -> [u8; 4] {
        let at = ((y * width) * 4) as usize;
        [pixels[at], pixels[at + 1], pixels[at + 2], pixels[at + 3]]
    }

    fn pixel(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * width + x) * 4) as usize;
        [pixels[at], pixels[at + 1], pixels[at + 2], pixels[at + 3]]
    }

    /// A `width × height` image of one opaque sRGB color, as `Background` holds
    /// one: premultiplied linear light.
    fn solid_image(width: u32, height: u32, color: [u8; 4]) -> Rgba32FImage {
        let mut source = image::RgbaImage::new(width, height);
        for pixel in source.pixels_mut() {
            *pixel = image::Rgba(color);
        }
        premultiplied_linear(&image::DynamicImage::ImageRgba8(source))
    }

    fn with_image(image: Rgba32FImage, fit: Fit) -> Background {
        Background {
            image: Some(image),
            fit,
            ..Backdrop::default().into()
        }
    }

    /// The endpoints are the palette slots themselves, not something near them:
    /// `--palette ember` must put `#1a1a2e` at the top of the frame.
    #[test]
    fn the_gradient_runs_from_palette_slot_zero_to_slot_one() {
        let pixels = backdrop_pixels(Backdrop::Gradient, 4, 9, ember());

        assert_eq!(row(&pixels, 4, 0), [0x1a, 0x1a, 0x2e, 0xff]);
        assert_eq!(row(&pixels, 4, 8), [0x53, 0x34, 0x83, 0xff]);
    }

    /// Mixed in light, not in the 8-bit encoding of light. The midpoint of
    /// `#1a1a2e` and `#533483` is `#40265f`-ish in linear and a good deal darker
    /// if the bytes are averaged directly.
    #[test]
    fn the_gradient_is_interpolated_in_linear_light() {
        let middle = gradient(ember(), 0.5).map(linear_to_srgb);
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
            let pixels = backdrop_pixels(style, 3, 4, ember());
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
        let pixels = backdrop_pixels(Backdrop::Solid, 2, 5, ember());

        for y in 0..5 {
            assert_eq!(row(&pixels, 2, y), [0x1a, 0x1a, 0x2e, 0xff]);
        }
    }

    /// A one-pixel-tall frame is legal and must not divide by zero.
    #[test]
    fn a_single_row_frame_is_the_top_of_the_gradient() {
        let pixels = backdrop_pixels(Backdrop::Gradient, 1, 1, ember());

        assert_eq!(pixels, vec![0x1a, 0x1a, 0x2e, 0xff]);
    }

    /// The default is the gradient: it is what `avz render song.mp3` puts under
    /// the visualizer with no `[background]` section at all.
    #[test]
    fn the_default_backdrop_is_the_gradient() {
        assert_eq!(Backdrop::default(), Backdrop::Gradient);
    }

    /// `[background]` with no `image` key is the backdrop, unchanged. The image
    /// path must not cost anything to a render that names no image.
    #[test]
    fn a_background_with_no_image_is_the_backdrop_alone() {
        let config = crate::config::Config::default();
        let background = Background::load(&config.background).expect("no image, nothing to read");

        assert_eq!(
            background.pixels(4, 4, ember()),
            backdrop_pixels(Backdrop::default(), 4, 4, ember()),
        );
    }

    /// `stretch` fills the frame with the whole image, aspect ratio be damned.
    /// Nothing of the backdrop survives.
    #[test]
    fn a_stretched_image_covers_every_pixel_of_the_frame() {
        let background = with_image(solid_image(2, 8, [0x20, 0xc0, 0x40, 0xff]), Fit::Stretch);

        let pixels = background.pixels(6, 3, ember());

        for y in 0..3 {
            for x in 0..6 {
                assert_eq!(pixel(&pixels, 6, x, y), [0x20, 0xc0, 0x40, 0xff]);
            }
        }
    }

    /// `cover` fills the frame too, by cropping the overhanging axis rather than
    /// distorting it.
    #[test]
    fn a_covering_image_leaves_no_backdrop_showing() {
        let background = with_image(solid_image(8, 2, [0x20, 0xc0, 0x40, 0xff]), Fit::Cover);

        let pixels = background.pixels(4, 4, ember());

        for y in 0..4 {
            for x in 0..4 {
                assert_eq!(pixel(&pixels, 4, x, y), [0x20, 0xc0, 0x40, 0xff]);
            }
        }
    }

    /// `contain` fits the whole image inside the frame, and the bars it leaves
    /// are the palette backdrop rather than black.
    #[test]
    fn a_contained_image_letterboxes_onto_the_palette_backdrop() {
        // A 4×1 image inside an 8×8 frame occupies the middle two rows.
        let background = with_image(solid_image(4, 1, [0x20, 0xc0, 0x40, 0xff]), Fit::Contain);

        let pixels = background.pixels(8, 8, ember());
        let backdrop = backdrop_pixels(Backdrop::default(), 8, 8, ember());

        for y in [0, 1, 6, 7] {
            assert_eq!(
                pixel(&pixels, 8, 4, y),
                pixel(&backdrop, 8, 4, y),
                "row {y} is a letterbox bar and must show the backdrop",
            );
        }
        assert_eq!(pixel(&pixels, 8, 4, 4), [0x20, 0xc0, 0x40, 0xff]);
    }

    /// An image the same shape as the frame is not cropped, whatever the fit.
    #[test]
    fn an_image_shaped_like_the_frame_is_neither_cropped_nor_letterboxed() {
        for fit in [Fit::Cover, Fit::Contain, Fit::Stretch] {
            let placement = place(fit, (16, 9), (32, 18));

            assert_eq!(placement.crop, [0, 0, 16, 9], "{fit:?} cropped the source");
            assert_eq!(
                (placement.x, placement.y),
                (0, 0),
                "{fit:?} offset the image"
            );
            assert_eq!((placement.width, placement.height), (32, 18), "{fit:?}");
        }
    }

    /// `cover` crops the axis that overhangs, centered, and never the other one.
    #[test]
    fn cover_crops_the_overhanging_axis_and_centers_what_is_left() {
        // A wide source into a square frame: the sides go.
        assert_eq!(place(Fit::Cover, (100, 50), (50, 50)).crop, [25, 0, 50, 50],);
        // A tall source into a square frame: the top and bottom go.
        assert_eq!(place(Fit::Cover, (50, 100), (50, 50)).crop, [0, 25, 50, 50],);
    }

    /// `contain` scales to whichever axis binds first, and centers the bars.
    #[test]
    fn contain_fits_the_binding_axis_and_centers_the_rest() {
        let wide = place(Fit::Contain, (100, 50), (50, 50));
        assert_eq!((wide.width, wide.height), (50, 25));
        assert_eq!((wide.x, wide.y), (0, 12));

        let tall = place(Fit::Contain, (50, 100), (50, 50));
        assert_eq!((tall.width, tall.height), (25, 50));
        assert_eq!((tall.x, tall.y), (12, 0));
    }

    /// A frame far larger than the image must not round a destination axis down
    /// to zero pixels, which `resize` rejects.
    #[test]
    fn a_sliver_of_an_image_still_occupies_at_least_one_pixel() {
        let placement = place(Fit::Contain, (1, 1000), (1920, 1080));

        assert_eq!(placement.width, 1);
        assert_eq!(placement.height, 1080);
    }

    /// The alpha channel is coverage, not decoration: a hole in the image shows
    /// the backdrop through it, exactly as a `contain` letterbox does.
    #[test]
    fn a_transparent_image_lets_the_backdrop_through() {
        let background = with_image(solid_image(4, 4, [0xff, 0x00, 0x00, 0x00]), Fit::Stretch);

        let pixels = background.pixels(4, 4, ember());

        assert_eq!(pixels, backdrop_pixels(Backdrop::default(), 4, 4, ember()));
    }

    /// Half-covering red over the backdrop is the `over` operator in light, not
    /// the red the image stores.
    #[test]
    fn a_half_transparent_image_blends_with_the_backdrop() {
        let background = with_image(solid_image(2, 2, [0xff, 0x00, 0x00, 0x80]), Fit::Stretch);

        let pixels = background.pixels(2, 2, ember());
        let backdrop = backdrop_pixels(Backdrop::default(), 2, 2, ember());

        let [r, _, b, a] = pixel(&pixels, 2, 0, 0);
        assert_eq!(a, 0xff, "the background layer is opaque");
        assert!(r > pixel(&backdrop, 2, 0, 0)[0], "the red must show");
        assert!(r < 0xff, "and it must not show at full strength");
        assert!(b > 0, "the backdrop's blue must survive under it");
    }

    /// `darken` takes away light, not encoded bytes. Half the photons of white
    /// is `#bc` in sRGB; halving the byte would give `#80`, a stop and a half
    /// too dark.
    #[test]
    fn darken_dims_the_light_rather_than_the_encoded_byte() {
        let background = Background {
            darken: 0.5,
            ..with_image(solid_image(1, 1, [0xff, 0xff, 0xff, 0xff]), Fit::Stretch)
        };

        let pixels = background.pixels(1, 1, ember());

        assert_eq!(pixels[0], linear_to_srgb(0.5));
        assert!(pixels[0] > 0xb0 && pixels[0] < 0xc0, "got {}", pixels[0]);
    }

    /// The end of the range is black, whatever was under it.
    #[test]
    fn darken_of_one_leaves_black() {
        let background = Background {
            darken: 1.0,
            ..with_image(solid_image(2, 2, [0xff, 0xff, 0xff, 0xff]), Fit::Stretch)
        };

        assert_eq!(background.pixels(2, 2, ember()), [0u8, 0, 0, 255].repeat(4));
    }

    /// `blur = 0` is the identity, not "a kernel of radius zero" that a Gaussian
    /// approximation might still smear.
    #[test]
    fn a_blur_of_zero_leaves_the_image_untouched() {
        let sharp = with_image(solid_image(4, 4, [0x00, 0x00, 0x00, 0xff]), Fit::Contain);

        assert_eq!(
            sharp.pixels(8, 8, ember()),
            Background {
                blur: 0.0,
                ..sharp.clone()
            }
            .pixels(8, 8, ember()),
        );
    }

    /// A blur is an average of light. A black-and-white checkerboard blurred to
    /// flat grey is `#bc`-ish, the sRGB encoding of half the photons — averaging
    /// the bytes would give `#80`.
    #[test]
    fn a_blur_averages_light_rather_than_encoded_bytes() {
        let mut checker = image::RgbaImage::new(32, 32);
        for (x, y, pixel) in checker.enumerate_pixels_mut() {
            let value = if (x + y) % 2 == 0 { 0xff } else { 0x00 };
            *pixel = image::Rgba([value, value, value, 0xff]);
        }
        let background = Background {
            blur: 8.0,
            ..with_image(
                premultiplied_linear(&image::DynamicImage::ImageRgba8(checker)),
                Fit::Stretch,
            )
        };

        let pixels = background.pixels(32, 32, ember());
        let middle = pixel(&pixels, 32, 16, 16)[0];

        assert!(
            (i32::from(middle) - i32::from(linear_to_srgb(0.5))).abs() <= 4,
            "a blurred checkerboard is half the light, which encodes to {}, not {middle}",
            linear_to_srgb(0.5),
        );
    }

    /// A blur spreads light outward: a lone bright square is darker at its
    /// center and brighter outside it than it was.
    #[test]
    fn a_blur_spreads_light_beyond_the_shape_that_emitted_it() {
        let mut source = image::RgbaImage::new(16, 16);
        for (x, y, pixel) in source.enumerate_pixels_mut() {
            let inside = (6..10).contains(&x) && (6..10).contains(&y);
            let value = if inside { 0xff } else { 0x00 };
            *pixel = image::Rgba([value, value, value, 0xff]);
        }
        let sharp = with_image(
            premultiplied_linear(&image::DynamicImage::ImageRgba8(source)),
            Fit::Stretch,
        );
        let soft = Background {
            blur: 3.0,
            ..sharp.clone()
        };

        let sharp = sharp.pixels(16, 16, ember());
        let soft = soft.pixels(16, 16, ember());

        assert!(
            pixel(&soft, 16, 8, 8)[0] < pixel(&sharp, 16, 8, 8)[0],
            "the middle of the square must lose light to its surroundings",
        );
        assert!(
            pixel(&soft, 16, 3, 8)[0] > pixel(&sharp, 16, 3, 8)[0],
            "and the surroundings must gain it",
        );
    }

    /// Blurring a flat field is a no-op: an edge-clamped kernel must not pull
    /// black in from outside the frame and vignette every background.
    #[test]
    fn a_blur_of_a_flat_field_darkens_no_edge() {
        let background = Background {
            blur: 4.0,
            ..with_image(solid_image(8, 8, [0x80, 0x80, 0x80, 0xff]), Fit::Stretch)
        };

        let pixels = background.pixels(8, 8, ember());
        let middle = pixel(&pixels, 8, 4, 4);

        for y in 0..8 {
            for x in 0..8 {
                assert_eq!(pixel(&pixels, 8, x, y), middle, "at ({x}, {y})");
            }
        }
    }

    /// The same background twice is the same bytes twice: nothing here reads a
    /// clock or an unseeded RNG (`AGENTS.md`, determinism).
    #[test]
    fn the_same_background_builds_the_same_pixels_twice() {
        let background = Background {
            blur: 2.0,
            darken: 0.3,
            ..with_image(solid_image(6, 3, [0x30, 0x60, 0x90, 0xc0]), Fit::Cover)
        };

        assert_eq!(
            background.pixels(9, 9, ember()),
            background.pixels(9, 9, ember()),
        );
    }

    /// A background video that does not exist is the user's argument too, and it
    /// must be caught here rather than by an ffmpeg spawned after the song has
    /// been decoded and the first frame drawn.
    #[test]
    fn a_missing_background_video_is_an_input_error_naming_the_path() {
        let config = BackgroundConfig {
            source: Some(BackgroundSource::Video("/no/such/smoke.mp4".into())),
            fit: Fit::Cover,
            blur: 0.0,
            darken: 0.0,
        };

        let err = Background::load(&config).expect_err("there is no such file");

        assert!(matches!(err, Error::Input(_)), "got {err:?}");
        assert!(err.to_string().contains("smoke.mp4"), "{err}");
    }

    /// A background image that does not exist is the user's argument, and the
    /// error names the path they typed.
    #[test]
    fn a_missing_background_image_is_an_input_error_naming_the_path() {
        let config = BackgroundConfig {
            source: Some(BackgroundSource::Image("/no/such/forest.png".into())),
            fit: Fit::Cover,
            blur: 0.0,
            darken: 0.0,
        };

        let err = Background::load(&config).expect_err("there is no such file");

        assert!(matches!(err, Error::Input(_)), "got {err:?}");
        assert!(err.to_string().contains("forest.png"), "{err}");
    }

    /// A file that is not an image avz can read fails as an input problem, not
    /// as a render failure two minutes later.
    #[test]
    fn a_file_that_is_not_an_image_is_an_input_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("forest.png");
        std::fs::write(&path, b"not a png").expect("write");

        let config = BackgroundConfig {
            source: Some(BackgroundSource::Image(path)),
            fit: Fit::Cover,
            blur: 0.0,
            darken: 0.0,
        };

        let err = Background::load(&config).expect_err("those bytes decode to nothing");

        assert!(matches!(err, Error::Input(_)), "got {err:?}");
        assert!(err.to_string().contains("forest.png"), "{err}");
    }

    /// `background.video` is decoded per frame by
    /// [`BackgroundVideo`](crate::render::BackgroundVideo), not here. This layer
    /// is then the palette backdrop the video's letterbox bars show through, and
    /// it must hold no image of its own.
    #[test]
    fn a_background_video_leaves_this_layer_as_the_backdrop_it_shows_through() {
        let dir = tempfile::tempdir().expect("tempdir");
        let source = dir.path().join("smoke.mp4");
        std::fs::write(&source, b"not really a video, but it exists").expect("write");

        let config = BackgroundConfig {
            source: Some(BackgroundSource::Video(source)),
            fit: Fit::Cover,
            blur: 0.0,
            darken: 0.0,
        };

        let background = Background::load(&config).expect("a video needs no decoding here");

        assert_eq!(background.image_size(), None);
    }

    /// Found in the v0.1 e2e pass (#30): `--bg loop.mp4` failed with a message
    /// that never said `background.video` exists. When the file that failed to
    /// decode smells like a video, the error must hand over the spelling that
    /// works, not just the fact of the failure (`AGENTS.md`, actionable
    /// warnings).
    #[test]
    fn a_video_handed_to_background_image_points_at_background_video() {
        let dir = tempfile::tempdir().expect("tempdir");
        let clip = dir.path().join("loop.mp4");
        std::fs::write(&clip, b"\x00\x00\x00\x18ftypisom, not an image").expect("write");

        let err = decode(&clip).expect_err("an mp4 is not an image");
        let message = err.to_string();
        assert!(
            message.contains("still image"),
            "say what `background.image` wanted: {message}"
        );
        assert!(
            message.contains("`background.video`"),
            "name the key that takes a video: {message}"
        );
        assert!(
            message.contains("--set background.video="),
            "say how to pass it from the command line: {message}"
        );
    }

    /// The hint is earned by the extension. A corrupt image is its own problem,
    /// and pointing its owner at `background.video` would send them the wrong
    /// way.
    #[test]
    fn a_corrupt_image_does_not_get_the_video_hint() {
        let dir = tempfile::tempdir().expect("tempdir");
        let broken = dir.path().join("art.png");
        std::fs::write(&broken, b"not a png at all").expect("write");

        let err = decode(&broken).expect_err("garbage is not an image");
        let message = err.to_string();
        assert!(
            !message.contains("background.video"),
            "a bad image is not a video problem: {message}"
        );
    }

    #[test]
    fn the_video_hint_is_judged_by_extension_case_insensitively() {
        assert!(looks_like_video(Path::new("LOOP.MP4")));
        assert!(looks_like_video(Path::new("clip.WebM")));
        assert!(!looks_like_video(Path::new("art.png")));
        assert!(!looks_like_video(Path::new("no-extension")));
    }
}
