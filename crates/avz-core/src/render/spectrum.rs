//! This frame's spectrum, as a texture a preset can read across the frame.
//!
//! The second binding a preset may ask the renderer for beyond the uniform
//! (`VISION.md` §6: "spectrum texture (512×1)"). A preset opts in with
//! `"needs_spectrum": true` in its schema; the renderer then binds a `512×1`
//! texture at `@binding(3)` holding the coarse, log-spaced spectrum
//! [`coarse_spectrum`](crate::analysis::spectrum::coarse_spectrum) averaged for
//! the frame being drawn, normalized into `0.0..=1.0` over the whole song.
//!
//! Generic, not `ribbons`-specific, exactly as [`Feedback`](super::feedback)
//! is not `nebula`-specific: a preset that wants to place light by frequency
//! rather than by band reuses this.
//!
//! **Why the uniform is not enough.** `Globals` carries five band energies, and
//! five numbers cannot draw a spectrum. A ribbon reads a different bucket at
//! every column of the frame, so what it needs is an array indexed by a value it
//! computes in the fragment shader — which is a texture.
//!
//! **`R32Float`, read with `textureLoad`, with no sampler.** Filtering a
//! floating-point texture needs a wgpu feature (`FLOAT32_FILTERABLE`) that
//! lavapipe and the low-end hardware avz targets do not all carry, and hardware
//! filtering rounds differently on different drivers — which is precisely what
//! golden frames must not depend on (`AGENTS.md`, determinism). So the texture is
//! sampled with `textureLoad`, and any interpolation between buckets is
//! arithmetic the shader writes down. That also spares the bind group a sampler:
//! the spectrum is one binding, not two.

use crate::analysis::SPECTRUM_BINS;
use crate::render::offscreen::Gpu;

/// The texture format the spectrum is uploaded in.
///
/// Full `f32` precision because the ribbon reads it as a displacement: an 8-bit
/// bucket would quantize a slow swell into visible stair-steps, and the whole
/// row is 2 KiB either way.
const SPECTRUM_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R32Float;

/// The frame's coarse spectrum, uploaded once per frame.
///
/// Built once per render, like [`Feedback`](super::feedback::Feedback), and
/// rewritten in place by [`Spectrum::upload`] before each draw.
#[derive(Debug)]
pub struct Spectrum {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

impl Spectrum {
    /// A `512×1` spectrum texture, silent until the first [`Spectrum::upload`].
    ///
    /// Zero-filled explicitly rather than left to wgpu's lazy initialization,
    /// because "a preset that is handed no spectrum sees silence" is a contract
    /// worth being able to state.
    pub fn new(gpu: &Gpu) -> Self {
        let texture = gpu.device().create_texture(&wgpu::TextureDescriptor {
            label: Some("avz spectrum"),
            size: wgpu::Extent3d {
                width: SPECTRUM_BINS as u32,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SPECTRUM_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        let spectrum = Self { texture, view };
        spectrum.write(gpu, &[0.0; SPECTRUM_BINS]);
        spectrum
    }

    /// Replace the texture's contents with `bins`, this frame's spectrum.
    ///
    /// Queued on the same queue as the draw that follows it, so the shader sees
    /// this frame's row and never the last one's.
    ///
    /// # Panics
    ///
    /// If `bins` is not [`SPECTRUM_BINS`] long.
    /// [`FeatureTimeline::spectrum`](crate::analysis::FeatureTimeline::spectrum)
    /// always returns exactly that many, so a mismatch is a caller bug rather
    /// than anything a user can provoke.
    pub fn upload(&self, gpu: &Gpu, bins: &[f32]) {
        assert_eq!(
            bins.len(),
            SPECTRUM_BINS,
            "the spectrum texture is {SPECTRUM_BINS} buckets wide",
        );

        self.write(gpu, bins);
    }

    /// Write `bins` into the texture. The bytes are little-endian `f32`s, laid
    /// out the way `Globals::to_bytes` lays out the uniform's.
    fn write(&self, gpu: &Gpu, bins: &[f32]) {
        let bytes: Vec<u8> = bins.iter().flat_map(|bin| bin.to_le_bytes()).collect();

        gpu.queue().write_texture(
            self.texture.as_image_copy(),
            &bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(SPECTRUM_BINS as u32 * 4),
                rows_per_image: Some(1),
            },
            self.texture.size(),
        );
    }

    /// The one bind-group layout entry a spectrum preset adds, at binding 3.
    ///
    /// Three rather than one, so that a preset asking for the spectrum alone
    /// declares the same binding number as one asking for the spectrum and the
    /// previous frame both. Bindings need not be contiguous; they must be stable.
    pub fn layout_entries() -> [wgpu::BindGroupLayoutEntry; 1] {
        [wgpu::BindGroupLayoutEntry {
            binding: 3,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                // Unfilterable: read with `textureLoad`, never `textureSample`.
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        }]
    }

    /// The bind-group entry that fills [`Spectrum::layout_entries`].
    pub fn bindings(&self) -> [wgpu::BindGroupEntry<'_>; 1] {
        [wgpu::BindGroupEntry {
            binding: 3,
            resource: wgpu::BindingResource::TextureView(&self.view),
        }]
    }
}
