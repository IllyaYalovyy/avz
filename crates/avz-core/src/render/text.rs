//! The top layer: the title/artist card (`VISION.md` §5.3, layer 3).
//!
//! **Rasterized once.** The words never change mid-render, so `cosmic-text`
//! shapes and `swash` rasterizes them exactly once, into an alpha coverage
//! bitmap the size of the text block. From then on the render loop only animates
//! a quad's opacity and offset over that one texture.
//!
//! **The bundled font, and only the bundled font.** A [`FontSystem`] built from
//! `FontSystem::new()` would load whatever fonts the host happens to have
//! installed and pick a fallback by rules that differ between machines. Same
//! inputs plus same config must produce the same video (`AGENTS.md`,
//! determinism), so the database holds one face — the bundled OFL font, or the
//! one `[text] font` names — and shaping is [`Shaping::Basic`], which never asks
//! for a fallback. `scripts/quality.d/42-text-rasterizes-from-the-bundled-font.sh`
//! keeps it that way.

use std::path::Path;

use cosmic_text::{Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, SwashCache, fontdb};

use crate::config::{FontChoice, Position, Text as TextConfig};
use crate::render::globals::PALETTE_SLOTS;
use crate::render::layer::Layer;
use crate::render::offscreen::{FRAME_FORMAT, Gpu};
use crate::render::palette::LinearPalette;
use crate::{Error, Result};

/// The font a `[text] font = "auto"` render draws with: IBM Plex Sans Regular,
/// SIL Open Font License 1.1 (`assets/fonts/OFL.txt`).
const BUNDLED_FONT: &[u8] = include_bytes!("../../../../assets/fonts/IBMPlexSans-Regular.ttf");

/// The artist line, relative to the title's type size.
const ARTIST_SCALE: f32 = 0.6;

/// Leading, as a fraction of the type size. Both lines are one line each, so
/// this only ever separates the title from the artist.
const LINE_SPACING: f32 = 1.25;

/// The gap between the title's line box and the artist's, as a fraction of the
/// artist's type size.
const LINE_GAP: f32 = 0.2;

/// The words on the card, after `--title` and `--artist` have had their say.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CardText {
    /// The larger line.
    pub title: Option<String>,
    /// The smaller line beneath it.
    pub artist: Option<String>,
}

impl CardText {
    /// The card's words: the config's overrides, falling back to the ID3 tags.
    ///
    /// Field by field, not card by card. `--title` names a title and says
    /// nothing about the artist, so a file whose artist tag is the only one it
    /// carries keeps it (`VISION.md` §5.2).
    pub fn resolve(config: &TextConfig, title: Option<&str>, artist: Option<&str>) -> Self {
        let owned = |value: Option<&str>| value.map(str::to_owned);

        Self {
            title: config.title.clone().or_else(|| owned(title)),
            artist: config.artist.clone().or_else(|| owned(artist)),
        }
    }

    /// Whether there is nothing to draw.
    pub fn is_empty(&self) -> bool {
        self.title.is_none() && self.artist.is_none()
    }
}

/// The card's opacity envelope, in seconds of song time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Timing {
    /// When the fade in begins.
    in_at: f64,
    /// How long the card stays at full opacity.
    hold: f64,
    /// How long each fade lasts.
    fade: f64,
}

impl Timing {
    /// How opaque the card is `seconds` into the song, in `0.0..=1.0`.
    ///
    /// Four windows: dark until `in_at`, up over `fade`, `hold` seconds at full,
    /// down over `fade`, dark forever. `fade = 0` collapses the two ramps into
    /// cuts rather than dividing by zero, and `hold = 0` with it leaves a card
    /// that never shows — which is what a user who wrote both zeros asked for.
    ///
    /// Smoothstep rather than a linear ramp: a linear fade has a visible corner
    /// where it meets the hold, and the card is the one thing in the frame the
    /// eye is reading rather than watching.
    pub fn opacity(self, seconds: f64) -> f32 {
        let full_in = self.in_at + self.fade;
        let full_out = full_in + self.hold;
        let end = full_out + self.fade;

        if seconds < self.in_at || seconds >= end {
            return 0.0;
        }
        if seconds < full_in {
            return smoothstep((seconds - self.in_at) / self.fade);
        }
        if seconds < full_out {
            return 1.0;
        }
        smoothstep((end - seconds) / self.fade)
    }
}

/// The classic `3t² - 2t³` ease, on a `t` already known to be in `0.0..1.0`.
fn smoothstep(t: f64) -> f32 {
    (t * t * (3.0 - 2.0 * t)) as f32
}

impl From<&TextConfig> for Timing {
    fn from(config: &TextConfig) -> Self {
        Self {
            in_at: config.in_at.as_secs_f64(),
            hold: config.hold.as_secs_f64(),
            fade: config.fade.as_secs_f64(),
        }
    }
}

/// The top-left corner of a `block`-sized card in a `frame`-sized layer.
///
/// The margin holds the card off the edges it is anchored to and is not applied
/// to an axis it is centered on — a centered card is centered, not centered in
/// what the margins left over. Signed, because a card wider than its frame has a
/// negative corner and the shader clips it rather than this wrapping around.
fn place(position: Position, block: (u32, u32), frame: (u32, u32), margin: u32) -> (i32, i32) {
    use Position::{
        BottomCenter, BottomLeft, BottomRight, Center, CenterLeft, CenterRight, TopCenter, TopLeft,
        TopRight,
    };

    /// `start`, `middle`, or `end` of one axis.
    fn axis(anchor: i32, block: u32, frame: u32, margin: u32) -> i32 {
        let (block, frame, margin) = (block as i32, frame as i32, margin as i32);
        match anchor {
            -1 => margin,
            0 => (frame - block) / 2,
            _ => frame - margin - block,
        }
    }

    let (horizontal, vertical) = match position {
        TopLeft => (-1, -1),
        TopCenter => (0, -1),
        TopRight => (1, -1),
        CenterLeft => (-1, 0),
        Center => (0, 0),
        CenterRight => (1, 0),
        BottomLeft => (-1, 1),
        BottomCenter => (0, 1),
        BottomRight => (1, 1),
    };

    (
        axis(horizontal, block.0, frame.0, margin),
        axis(vertical, block.1, frame.1, margin),
    )
}

/// Which edge of the block the lines are flushed against.
///
/// Derived from the card's horizontal anchor, so a bottom-right card reads as a
/// right-aligned pair of lines rather than a left-aligned one hugging the right
/// edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Align {
    /// Flush left.
    Start,
    /// Centered.
    Middle,
    /// Flush right.
    End,
}

impl From<Position> for Align {
    fn from(position: Position) -> Self {
        match position {
            Position::TopLeft | Position::CenterLeft | Position::BottomLeft => Align::Start,
            Position::TopCenter | Position::Center | Position::BottomCenter => Align::Middle,
            Position::TopRight | Position::CenterRight | Position::BottomRight => Align::End,
        }
    }
}

/// A rasterized card: coverage, one byte per pixel, tightly packed and cropped
/// to the ink.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Raster {
    /// Width of the text block in pixels.
    pub width: u32,
    /// Height of the text block in pixels.
    pub height: u32,
    /// `width * height` coverage bytes, row-major.
    pub coverage: Vec<u8>,
}

/// The type sizes a `height`-pixel frame draws the card at.
///
/// Relative to the output height, so 720p, 1080p, and 4k look proportionate
/// rather than leaving the card a postage stamp on the largest of them.
fn type_sizes(height: u32, size: f32) -> (f32, f32) {
    let title = (height as f32 * size).max(1.0);
    (title, (title * ARTIST_SCALE).max(1.0))
}

/// Read the font `choice` names.
///
/// # Errors
///
/// [`Error::Input`] if a `[text] font` path cannot be read. The bundled font is
/// compiled in and cannot fail.
fn font_data(choice: &FontChoice) -> Result<Vec<u8>> {
    match choice {
        FontChoice::Auto => Ok(BUNDLED_FONT.to_vec()),
        FontChoice::Path(path) => read_font(path),
    }
}

fn read_font(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).map_err(|err| Error::Input(format!("`{}`: {err}", path.display())))
}

/// The locale shaping is done in.
///
/// Fixed rather than read from the environment: `LANG` is not an input to a
/// render, and two hosts must set the same words the same way.
const LOCALE: &str = "en-US";

/// A [`FontSystem`] holding exactly one face, and the family that names it.
///
/// # Errors
///
/// [`Error::Input`] if the bytes hold no font face `fontdb` can parse, naming
/// the file the user pointed `[text] font` at.
fn font_system(choice: &FontChoice, data: Vec<u8>) -> Result<(FontSystem, String)> {
    let mut db = fontdb::Database::new();
    db.load_font_data(data);

    let family = db
        .faces()
        .next()
        .and_then(|face| face.families.first())
        .map(|(name, _)| name.clone())
        .ok_or_else(|| {
            Error::Input(format!("{} holds no font face avz can read", named(choice)))
        })?;

    Ok((
        FontSystem::new_with_locale_and_db(LOCALE.to_owned(), db),
        family,
    ))
}

/// How an error names the font the user chose.
fn named(choice: &FontChoice) -> String {
    match choice {
        FontChoice::Auto => "the bundled font".to_owned(),
        FontChoice::Path(path) => format!("`{}`", path.display()),
    }
}

/// One rasterized pixel of the card: where it lands, and how much of it is ink.
type Ink = (i32, i32, u8);

/// Shape and rasterize the card, once.
///
/// `None` when the words leave no ink at all — a title of nothing but spaces, or
/// glyphs the font does not have. There is no card to draw and the caller says
/// so rather than uploading a zero-sized texture.
fn rasterize(
    text: &CardText,
    fonts: &mut FontSystem,
    family: &str,
    layout: CardLayout,
) -> Option<Raster> {
    let mut cache = SwashCache::new();
    let attrs = Attrs::new().family(Family::Name(family));

    // Shape both lines before either is drawn: the block is as wide as its
    // widest line, and that is what the other line is aligned against.
    let lines: Vec<(&str, f32)> = [
        text.title.as_deref().map(|title| (title, layout.title)),
        text.artist.as_deref().map(|artist| (artist, layout.artist)),
    ]
    .into_iter()
    .flatten()
    .collect();

    let mut shaped = Vec::with_capacity(lines.len());
    let mut top = 0.0f32;
    for (line, size) in lines {
        let mut buffer = Buffer::new(fonts, Metrics::new(size, size * LINE_SPACING));
        // No wrap: the card is as wide as its words, and the frame clips it.
        buffer.set_size(None, None);
        // `Shaping::Basic` never reaches for a fallback font, which is the whole
        // point of a one-face database.
        buffer.set_text(line, &attrs, Shaping::Basic, None);
        buffer.shape_until_scroll(fonts, false);

        let width = buffer
            .layout_runs()
            .map(|run| run.line_w)
            .fold(0.0f32, f32::max);

        shaped.push((buffer, width, top));
        top += size * LINE_SPACING + layout.artist * LINE_GAP;
    }

    let block = shaped
        .iter()
        .map(|(_, width, _)| *width)
        .fold(0.0, f32::max);

    let mut ink: Vec<Ink> = Vec::new();
    for (buffer, width, top) in &mut shaped {
        let left = match layout.align {
            Align::Start => 0.0,
            Align::Middle => (block - *width) / 2.0,
            Align::End => block - *width,
        };
        let (dx, dy) = (left.round() as i32, top.round() as i32);

        buffer.draw(fonts, &mut cache, WHITE, |x, y, span, rows, color| {
            let coverage = color.a();
            if coverage == 0 {
                return;
            }
            for row in 0..rows as i32 {
                for column in 0..span as i32 {
                    ink.push((dx + x + column, dy + y + row, coverage));
                }
            }
        });
    }

    crop(&ink)
}

/// The color the coverage mask is drawn with.
///
/// Only its alpha survives: `SwashCache::with_pixels` replaces the alpha of the
/// base color with the glyph's coverage, and the card's actual color comes from
/// the palette, in the shader.
const WHITE: Color = Color::rgb(0xff, 0xff, 0xff);

/// The ink, cropped to its own bounding box.
fn crop(ink: &[Ink]) -> Option<Raster> {
    let (&(first_x, first_y, _), rest) = ink.split_first()?;
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (first_x, first_y, first_x, first_y);
    for &(x, y, _) in rest {
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
    }

    let width = (max_x - min_x + 1) as u32;
    let height = (max_y - min_y + 1) as u32;

    let mut coverage = vec![0u8; (width * height) as usize];
    for &(x, y, value) in ink {
        let at = ((y - min_y) as u32 * width + (x - min_x) as u32) as usize;
        // Overlapping antialiased glyphs cover a pixel once between them, not
        // twice: adding their coverage would darken every kerned pair.
        coverage[at] = coverage[at].max(value);
    }

    Some(Raster {
        width,
        height,
        coverage,
    })
}

/// How the card is set: the two type sizes, and which edge the lines flush to.
#[derive(Debug, Clone, Copy, PartialEq)]
struct CardLayout {
    /// Title type size in pixels.
    title: f32,
    /// Artist type size in pixels.
    artist: f32,
    /// Which edge the lines are flushed against.
    align: Align,
}

/// How far the card rises into place, as a fraction of the title's type size.
///
/// The offset half of "animated opacity/offset" (`VISION.md` §5.3): the card
/// drifts up as it fades in and sinks back as it fades out, which reads as the
/// card arriving rather than a rectangle of letters appearing.
const RISE: f32 = 0.35;

/// The palette slot the card is set in.
///
/// The last one: every palette is ordered darkest-first (`render::palette`), so
/// the brightest accent is the one that reads against the backdrop beneath it.
const CARD_SLOT: usize = PALETTE_SLOTS - 1;

/// The quad, and the coverage it reads through.
///
/// Inline rather than an `include_str!` of a `.wgsl` file, for the same reason
/// the compositor's shader is: `presets/` is the only place avz embeds shaders
/// from, and this is not a preset.
const CARD_WGSL: &str = r"
struct Card {
    // Linear-space color of the type; `w` is the opacity of the whole card.
    color: vec4<f32>,
    // Where the block sits in the frame: `xy` top-left, `zw` size, in pixels.
    rect: vec4<f32>,
};

@group(0) @binding(0) var<uniform> card: Card;
@group(0) @binding(1) var coverage: texture_2d<f32>;

@vertex
fn vs_main(@builtin(vertex_index) vertex: u32) -> @builtin(position) vec4<f32> {
    let index = i32(vertex);
    let x = f32(index / 2) * 4.0 - 1.0;
    let y = f32(index & 1) * 4.0 - 1.0;
    return vec4<f32>(x, y, 0.0, 1.0);
}

// Premultiplied light, like every layer: the type emits `color * alpha` and
// hides `alpha` of what is beneath it. Off the block, it emits and hides
// nothing, which is what lets the card be the top of the stack.
@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let local = vec2<i32>(position.xy) - vec2<i32>(card.rect.xy);
    let size = vec2<i32>(card.rect.zw);

    if (local.x < 0 || local.y < 0 || local.x >= size.x || local.y >= size.y) {
        return vec4<f32>(0.0, 0.0, 0.0, 0.0);
    }

    let alpha = textureLoad(coverage, local, 0).r * card.color.w;
    return vec4<f32>(card.color.rgb * alpha, alpha);
}
";

/// The card as the CPU builds it: coverage, where it sits, and when it shows.
///
/// Shaped and rasterized once, before the song is decoded — a `[text] font` that
/// does not exist is the user's argument, and they should hear about it in the
/// first millisecond rather than after a five-minute analysis pass.
#[derive(Debug, Clone, PartialEq)]
pub struct Card {
    /// The coverage mask of the whole text block.
    raster: Raster,
    /// The block's resting top-left corner in the frame.
    origin: (i32, i32),
    /// How far below `origin` the card starts, in pixels.
    rise: f32,
    /// The opacity envelope, in song seconds.
    timing: Timing,
}

impl Card {
    /// Shape and rasterize `words` for a `frame`-sized layer.
    ///
    /// `Ok(None)` when the words leave no ink — the font has none of their
    /// glyphs, say. There is nothing to draw, and the caller drops the layer
    /// rather than compositing a blank one.
    ///
    /// # Errors
    ///
    /// [`Error::Input`] if `[text] font` names a file that cannot be read or
    /// holds no font face.
    pub fn prepare(
        config: &TextConfig,
        words: &CardText,
        frame: (u32, u32),
    ) -> Result<Option<Self>> {
        let (title, artist) = type_sizes(frame.1, config.size);

        let (mut fonts, family) = font_system(&config.font, font_data(&config.font)?)?;
        let Some(raster) = rasterize(
            words,
            &mut fonts,
            &family,
            CardLayout {
                title,
                artist,
                align: config.position.into(),
            },
        ) else {
            return Ok(None);
        };

        let margin = (frame.1 as f32 * config.margin).round() as u32;
        let origin = place(
            config.position,
            (raster.width, raster.height),
            frame,
            margin,
        );

        Ok(Some(Self {
            raster,
            origin,
            rise: title * RISE,
            timing: Timing::from(config),
        }))
    }
}

/// The title/artist card on the GPU: one coverage texture, one animated quad.
///
/// Built once per render, like every other layer's drawing apparatus. The
/// texture never changes, and [`TextCard::draw`] only rewrites the eight floats
/// that say where the block sits and how opaque it is.
#[derive(Debug)]
pub struct TextCard {
    pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    bindings: wgpu::BindGroup,
    /// The block's size in pixels.
    block: (u32, u32),
    /// The block's resting top-left corner.
    origin: (i32, i32),
    /// How far below `origin` the card starts, in pixels.
    rise: f32,
    /// The opacity envelope, in song seconds.
    timing: Timing,
    /// The color of the type, in linear light.
    color: [f32; 3],
}

impl TextCard {
    /// Upload `card` and build the pass that animates it over `target`.
    ///
    /// # Errors
    ///
    /// [`Error::Render`] if the card's pass does not build on this adapter.
    pub fn new(gpu: &Gpu, card: &Card, palette: LinearPalette) -> Result<Self> {
        let raster = &card.raster;
        let block = (raster.width, raster.height);

        let device = gpu.device();
        let errors = device.push_error_scope(wgpu::ErrorFilter::Validation);

        let coverage = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("avz text coverage"),
            size: wgpu::Extent3d {
                width: raster.width,
                height: raster.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        gpu.queue().write_texture(
            coverage.as_image_copy(),
            &raster.coverage,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(raster.width),
                rows_per_image: Some(raster.height),
            },
            coverage.size(),
        );
        gpu.queue().submit([]);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("avz text card"),
            source: wgpu::ShaderSource::Wgsl(CARD_WGSL.into()),
        });

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("avz text card"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(CARD_UNIFORM_SIZE as u64),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });

        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("avz text card"),
            size: CARD_UNIFORM_SIZE as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("avz text card"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniforms.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(
                        &coverage.create_view(&wgpu::TextureViewDescriptor::default()),
                    ),
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("avz text card"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("avz text card"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: FRAME_FORMAT,
                    // The triangle covers the whole layer and writes every pixel
                    // of it, so there is nothing to blend against.
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        if let Some(err) = pollster::block_on(errors.pop()) {
            return Err(Error::Render(format!(
                "the text card does not build on `{}`: {err}",
                gpu.adapter_name(),
            )));
        }

        let [red, green, blue, _] = palette[CARD_SLOT];

        Ok(Self {
            pipeline,
            uniforms,
            bindings,
            block,
            origin: card.origin,
            rise: card.rise,
            timing: card.timing,
            color: [red, green, blue],
        })
    }

    /// Draw the card into `target` as it looks on frame `frame_index`.
    ///
    /// The clock is `frame_index / fps` and nothing else (`AGENTS.md`,
    /// determinism), so frames may be drawn in any order and a `--sample` render
    /// draws the card exactly where the full render draws it.
    pub fn draw(&self, gpu: &Gpu, target: &Layer, frame_index: usize, fps: u32) {
        let opacity = self.timing.opacity(frame_index as f64 / f64::from(fps));
        // Below its resting place while fading, in place while held.
        let rise = self.rise * (1.0 - opacity);

        gpu.queue()
            .write_buffer(&self.uniforms, 0, &self.uniform(opacity, rise));

        let mut encoder = gpu
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("avz text card"),
            });

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("avz text card"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target.view(),
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bindings, &[]);
            pass.draw(0..3, 0..1);
        }

        gpu.queue().submit([encoder.finish()]);
    }

    /// The eight floats the shader reads: the color and opacity, then the block.
    fn uniform(&self, opacity: f32, rise: f32) -> [u8; CARD_UNIFORM_SIZE] {
        let [red, green, blue] = self.color;
        let fields = [
            red,
            green,
            blue,
            opacity,
            self.origin.0 as f32,
            self.origin.1 as f32 + rise.round(),
            self.block.0 as f32,
            self.block.1 as f32,
        ];

        let mut bytes = [0u8; CARD_UNIFORM_SIZE];
        for (slot, field) in bytes.chunks_exact_mut(4).zip(fields) {
            slot.copy_from_slice(&field.to_le_bytes());
        }
        bytes
    }
}

/// Two `vec4<f32>`: the color and the block rectangle.
const CARD_UNIFORM_SIZE: usize = 32;

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> TextConfig {
        crate::config::Config::default().text
    }

    /// The card as a render rasterizes it, at a type size big enough to read in
    /// an assertion.
    fn raster(text: &CardText, align: Align) -> Option<Raster> {
        let (mut fonts, family) = font_system(
            &FontChoice::Auto,
            font_data(&FontChoice::Auto).expect("bundled"),
        )
        .expect("the bundled font parses");
        rasterize(
            text,
            &mut fonts,
            &family,
            CardLayout {
                title: 32.0,
                artist: 20.0,
                align,
            },
        )
    }

    fn card(title: Option<&str>, artist: Option<&str>) -> CardText {
        CardText {
            title: title.map(str::to_owned),
            artist: artist.map(str::to_owned),
        }
    }

    /// The bundled font ships, parses, and is the one a default render uses.
    #[test]
    fn the_bundled_font_is_a_parsable_face() {
        let (_, family) = font_system(
            &FontChoice::Auto,
            font_data(&FontChoice::Auto).expect("read"),
        )
        .expect("the bundled font parses");

        assert_eq!(family, "IBM Plex Sans");
    }

    /// A `[text] font` pointing at something that is not a font is the user's
    /// argument, and the error names the path they typed.
    #[test]
    fn a_font_path_that_is_not_a_font_is_an_input_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("not-a-font.ttf");
        std::fs::write(&path, b"this is not a font").expect("write");

        let choice = FontChoice::Path(path);
        let data = font_data(&choice).expect("the file reads");
        let err = font_system(&choice, data).expect_err("those bytes hold no face");

        assert!(matches!(err, Error::Input(_)), "got {err:?}");
        assert!(err.to_string().contains("not-a-font.ttf"), "{err}");
    }

    #[test]
    fn a_missing_font_path_is_an_input_error_naming_the_path() {
        let err = font_data(&FontChoice::Path("/no/such/Inter.ttf".into()))
            .expect_err("there is no such font");

        assert!(matches!(err, Error::Input(_)), "got {err:?}");
        assert!(err.to_string().contains("Inter.ttf"), "{err}");
    }

    /// The card is rasterized to ink: a block with something drawn in it, no
    /// wider than the words could possibly be, cropped to what was drawn.
    #[test]
    fn a_card_rasterizes_to_a_block_of_ink_cropped_to_its_bounds() {
        let raster = raster(&card(Some("Sine Tones"), Some("avz")), Align::Start)
            .expect("two lines of latin text leave ink");

        assert_eq!(
            raster.coverage.len(),
            (raster.width * raster.height) as usize
        );
        assert!(
            raster.coverage.iter().any(|&byte| byte > 0),
            "the card is blank",
        );

        // Cropped: every edge of the block touches ink somewhere.
        let row = |y: u32| {
            (0..raster.width).any(|x| raster.coverage[(y * raster.width + x) as usize] > 0)
        };
        let column = |x: u32| {
            (0..raster.height).any(|y| raster.coverage[(y * raster.width + x) as usize] > 0)
        };
        assert!(row(0) && row(raster.height - 1), "a blank row survived");
        assert!(
            column(0) && column(raster.width - 1),
            "a blank column survived"
        );
    }

    /// Both lines are drawn, and the artist sits beneath the title.
    #[test]
    fn the_artist_is_set_beneath_the_title_and_smaller_than_it() {
        let both = raster(&card(Some("Sine Tones"), Some("avz")), Align::Start).expect("ink");
        let title_only = raster(&card(Some("Sine Tones"), None), Align::Start).expect("ink");
        let artist_only = raster(&card(None, Some("avz")), Align::Start).expect("ink");

        assert!(
            both.height > title_only.height + artist_only.height / 2,
            "the artist line is not below the title: {} vs {} + {}",
            both.height,
            title_only.height,
            artist_only.height,
        );
        assert!(
            artist_only.height < title_only.height,
            "the artist is set no smaller than the title",
        );
    }

    /// Type size scales the card. Without this, `[text] size` and the frame
    /// height could both be plumbed through and quietly ignored.
    #[test]
    fn a_larger_type_size_rasterizes_a_larger_card() {
        let (mut fonts, family) = font_system(
            &FontChoice::Auto,
            font_data(&FontChoice::Auto).expect("read"),
        )
        .expect("parses");
        let words = card(Some("Sine Tones"), Some("avz"));

        let small = rasterize(
            &words,
            &mut fonts,
            &family,
            CardLayout {
                title: 16.0,
                artist: 10.0,
                align: Align::Start,
            },
        )
        .expect("ink");
        let large = rasterize(
            &words,
            &mut fonts,
            &family,
            CardLayout {
                title: 64.0,
                artist: 40.0,
                align: Align::Start,
            },
        )
        .expect("ink");

        assert!(
            large.width > small.width * 2,
            "{} vs {}",
            large.width,
            small.width
        );
        assert!(large.height > small.height * 2);
    }

    /// `type_sizes` is the only place the frame height becomes a type size, so
    /// 720p and 4k are the same picture at different scales.
    #[test]
    fn type_size_is_a_fraction_of_the_frame_height() {
        assert_eq!(type_sizes(1080, 0.05), (54.0, 54.0 * ARTIST_SCALE));
        assert_eq!(type_sizes(2160, 0.05), (108.0, 108.0 * ARTIST_SCALE));
        // A frame too small to hold a glyph still asks for a legal type size.
        assert_eq!(type_sizes(4, 0.05), (1.0, 1.0));
    }

    /// Nothing to draw is `None`, not a zero-sized texture wgpu refuses.
    #[test]
    fn words_that_leave_no_ink_rasterize_to_nothing() {
        assert_eq!(raster(&card(Some("\u{a0}"), None), Align::Start), None);
        assert_eq!(raster(&CardText::default(), Align::Start), None);
    }

    /// Where the ink of the artist line sits across the block, as `0.0..=1.0`.
    ///
    /// The artist is the short line, so it is the one alignment moves. It owns
    /// the bottom third of the block; the title owns the rest.
    fn artist_ink_center(raster: &Raster) -> f64 {
        let first_row = raster.height * 2 / 3;
        let mut weight = 0.0;
        let mut moment = 0.0;
        for y in first_row..raster.height {
            for x in 0..raster.width {
                let coverage = f64::from(raster.coverage[(y * raster.width + x) as usize]);
                weight += coverage;
                moment += coverage * f64::from(x);
            }
        }
        assert!(weight > 0.0, "the bottom of the block holds no ink");
        moment / weight / f64::from(raster.width - 1)
    }

    /// The short line is flushed to the edge the card is anchored to. A card set
    /// bottom-right whose artist hugged the left edge of the block would look
    /// like a bug, and nothing else here would see it.
    #[test]
    fn alignment_flushes_the_short_line_to_the_chosen_edge() {
        let words = card(Some("Sine Tones"), Some("avz"));

        let left = artist_ink_center(&raster(&words, Align::Start).expect("ink"));
        let middle = artist_ink_center(&raster(&words, Align::Middle).expect("ink"));
        let right = artist_ink_center(&raster(&words, Align::End).expect("ink"));

        assert!(left < 0.2, "a left-aligned artist sits at {left}");
        assert!(
            (0.4..=0.6).contains(&middle),
            "a centered artist sits at {middle}"
        );
        assert!(right > 0.8, "a right-aligned artist sits at {right}");
    }

    /// The card the compositor uploads is a function of the words, the font, and
    /// the type size — never of a clock, a hash map, or the host's font
    /// directory (`AGENTS.md`, determinism).
    #[test]
    fn the_same_card_rasterizes_to_the_same_bytes_twice() {
        let words = card(Some("Sine Tones"), Some("avz test fixture"));

        assert_eq!(raster(&words, Align::Start), raster(&words, Align::Start));
    }

    /// A card anchored to an edge is set flush against it; a centered one is
    /// centered.
    #[test]
    fn alignment_follows_the_horizontal_anchor() {
        assert_eq!(Align::from(Position::BottomLeft), Align::Start);
        assert_eq!(Align::from(Position::Center), Align::Middle);
        assert_eq!(Align::from(Position::TopRight), Align::End);
    }

    /// The envelope `VISION.md` §5.5 configures: nothing before `in_at`, a fade
    /// up, `hold` seconds of the card, a fade down, nothing after.
    #[test]
    fn opacity_envelope_matches_in_hold_fade_windows() {
        // in at 1s, fade 0.5s, hold 2s: up over 1.0..1.5, full to 3.5, down to 4.0.
        let timing = Timing {
            in_at: 1.0,
            hold: 2.0,
            fade: 0.5,
        };

        assert_eq!(timing.opacity(0.0), 0.0, "before the card is asked for");
        assert_eq!(timing.opacity(0.999), 0.0);
        assert_eq!(timing.opacity(1.0), 0.0, "the fade starts from nothing");

        let rising = timing.opacity(1.25);
        assert!(
            (rising - 0.5).abs() < 1e-5,
            "the middle of the fade in is half opacity, got {rising}"
        );

        assert_eq!(timing.opacity(1.5), 1.0, "the hold begins fully opaque");
        assert_eq!(timing.opacity(2.5), 1.0, "and stays that way");
        assert_eq!(timing.opacity(3.5), 1.0, "until the hold is over");

        let falling = timing.opacity(3.75);
        assert!(
            (falling - 0.5).abs() < 1e-5,
            "the middle of the fade out is half opacity, got {falling}"
        );

        assert_eq!(timing.opacity(4.0), 0.0, "and the card is gone");
        assert_eq!(timing.opacity(60.0), 0.0, "and stays gone");
    }

    /// A card is either on screen or it is not; nothing in between escapes the
    /// unit interval, whatever the shader would do with it.
    #[test]
    fn the_opacity_envelope_never_leaves_the_unit_interval_or_doubles_back() {
        let timing = Timing::from(&config());

        let mut previous = 0.0;
        let mut peaked = false;
        for frame in 0..300 {
            let opacity = timing.opacity(f64::from(frame) / 30.0);
            assert!(
                (0.0..=1.0).contains(&opacity),
                "frame {frame} has opacity {opacity}"
            );

            if opacity < previous {
                peaked = true;
            } else if peaked {
                assert_eq!(
                    opacity, 0.0,
                    "frame {frame} brightens again after the fade out",
                );
            }
            previous = opacity;
        }
        assert!(peaked, "the default card never fades out");
    }

    /// `fade = 0` is a cut, not a division by zero.
    #[test]
    fn a_card_with_no_fade_cuts_in_and_out() {
        let timing = Timing {
            in_at: 1.0,
            hold: 2.0,
            fade: 0.0,
        };

        assert_eq!(timing.opacity(0.999), 0.0);
        assert_eq!(timing.opacity(1.0), 1.0);
        assert_eq!(timing.opacity(2.999), 1.0);
        assert_eq!(timing.opacity(3.0), 0.0);
    }

    /// The nine anchors of `VISION.md` §5.3, with the margin held off every edge
    /// the card touches and never applied to an axis it is centered on.
    #[test]
    fn nine_grid_positions_and_margins_math() {
        const BLOCK: (u32, u32) = (100, 20);
        const MARGIN: u32 = 10;

        for frame in [(400, 200), (1920, 1080)] {
            let (width, height) = frame;
            let left = 10;
            let middle = (width as i32 - 100) / 2;
            let right = width as i32 - 10 - 100;
            let top = 10;
            let center = (height as i32 - 20) / 2;
            let bottom = height as i32 - 10 - 20;

            let cases = [
                (Position::TopLeft, (left, top)),
                (Position::TopCenter, (middle, top)),
                (Position::TopRight, (right, top)),
                (Position::CenterLeft, (left, center)),
                (Position::Center, (middle, center)),
                (Position::CenterRight, (right, center)),
                (Position::BottomLeft, (left, bottom)),
                (Position::BottomCenter, (middle, bottom)),
                (Position::BottomRight, (right, bottom)),
            ];

            for (position, expected) in cases {
                assert_eq!(
                    place(position, BLOCK, frame, MARGIN),
                    expected,
                    "{position:?} in a {width}x{height} frame",
                );
            }
        }
    }

    /// A card wider than the frame hangs off the edge rather than wrapping
    /// around it: the shader clips what does not fit.
    #[test]
    fn a_card_too_large_for_its_frame_is_placed_at_a_negative_offset() {
        let (x, y) = place(Position::Center, (200, 100), (100, 50), 4);

        assert_eq!((x, y), (-50, -25));
    }

    /// `--title` beats the ID3 title, and only the title: an override nobody
    /// passed must not erase the tag beside it.
    #[test]
    fn overrides_beat_id3_values() {
        let tagged = CardText::resolve(&config(), Some("Sine Tones"), Some("avz"));
        assert_eq!(tagged.title.as_deref(), Some("Sine Tones"));
        assert_eq!(tagged.artist.as_deref(), Some("avz"));

        let overridden = CardText::resolve(
            &TextConfig {
                title: Some("Cold Design".to_owned()),
                ..config()
            },
            Some("Sine Tones"),
            Some("avz"),
        );
        assert_eq!(overridden.title.as_deref(), Some("Cold Design"));
        assert_eq!(
            overridden.artist.as_deref(),
            Some("avz"),
            "`--title` alone must not take the artist with it",
        );

        // An override on a file with no tags at all is the whole card.
        let untagged = CardText::resolve(
            &TextConfig {
                artist: Some("Nobody".to_owned()),
                ..config()
            },
            None,
            None,
        );
        assert_eq!(untagged.title, None);
        assert_eq!(untagged.artist.as_deref(), Some("Nobody"));
    }

    /// Both tags missing and nothing overriding them: there is no card, and the
    /// pipeline is the one that has to say so.
    #[test]
    fn a_card_with_neither_tag_nor_override_is_empty() {
        assert!(CardText::resolve(&config(), None, None).is_empty());
        assert!(!CardText::resolve(&config(), Some("Sine Tones"), None).is_empty());
        assert!(!CardText::resolve(&config(), None, Some("avz")).is_empty());
    }
}
