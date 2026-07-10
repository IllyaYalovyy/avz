//! The recent hits of the song, as a texture a preset can re-simulate from.
//!
//! The third binding a preset may ask the renderer for beyond the uniform. A
//! preset opts in with `"needs_onsets": true` in its schema; the renderer then
//! binds a `64×1` texture at `@binding(4)` holding
//! [`FeatureTimeline::onset_history`](crate::analysis::FeatureTimeline::onset_history)
//! for the frame being drawn: the birth time and ordinal of each of the last
//! [`ONSET_SLOTS`] hits, newest first.
//!
//! Generic, not `particles`-specific, exactly as [`Feedback`](super::feedback) is
//! not `nebula`-specific and [`Spectrum`](super::spectrum) is not
//! `ribbons`-specific. Anything a preset spawns on a hit and then lets live —
//! a burst, an expanding ring, a hue that steps on every fourth kick — reads
//! this.
//!
//! **Why the uniform is not enough.** `Globals::onset` is a decaying impulse
//! about the frame being drawn. It is exactly what a flash needs, and it is
//! nothing at all to a particle that was spawned a second ago and is still in
//! the air: a fragment shader carries no state between frames, so it must
//! re-derive every live particle from the hit that spawned it. That hit's
//! timestamp is not in the uniform, and the analysis pass — which finishes
//! before a single frame is drawn (`VISION.md` §4.2) — already knows every one
//! of them.
//!
//! **Why not simulate into a texture instead.** A ping-ponged particle-state
//! texture, or a compute pass that integrates positions frame by frame, would
//! make frame `N` a function of the GPU's rounding on frames `0..N`. Golden
//! frames would then be a hash of the driver rather than of the shader
//! (`AGENTS.md`, determinism). Re-deriving from the hit list makes frame `N` a
//! pure function of frame `N`'s uniform and this row, which is the same property
//! every other preset has.
//!
//! **`Rg32Float`, read with `textureLoad`, with no sampler**, for the reason
//! [`Spectrum`](super::spectrum) gives: filtering a float texture needs a wgpu
//! feature lavapipe does not always carry, and interpolating between two hits
//! would be meaningless anyway. Two channels because a slot carries a birth time
//! *and* the ordinal of the hit that owns it — see
//! [`OnsetHistory`](crate::analysis::OnsetHistory) for why a burst's hashes must
//! key on the second and not on the slot.

use crate::analysis::{EMPTY_HISTORY, ONSET_SLOTS};
use crate::render::offscreen::Gpu;

/// The texture format the onset history is uploaded in.
///
/// Full `f32` precision because the red channel is a timestamp in seconds and
/// the shader subtracts it from `time`: five minutes into a song, an 8- or
/// 16-bit birth would quantize a burst's age into visible steps. The whole row
/// is 512 bytes.
const ONSET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rg32Float;

/// The song's recent hits, uploaded once per frame.
///
/// Built once per render, like [`Spectrum`](super::spectrum::Spectrum), and
/// rewritten in place by [`OnsetHistory::upload`] before each draw.
#[derive(Debug)]
pub struct OnsetHistory {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

impl OnsetHistory {
    /// A `64×1` onset-history texture, holding no hits until the first
    /// [`OnsetHistory::upload`].
    ///
    /// Filled with the sentinel explicitly rather than left to wgpu's lazy
    /// zero-initialization: a zeroed slot would claim a hit at time zero with
    /// ordinal zero, and a preset handed no history would open every render with
    /// a burst nothing played.
    pub fn new(gpu: &Gpu) -> Self {
        let texture = gpu.device().create_texture(&wgpu::TextureDescriptor {
            label: Some("avz onsets"),
            size: wgpu::Extent3d {
                width: ONSET_SLOTS as u32,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: ONSET_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        let onsets = Self { texture, view };
        onsets.write(gpu, &EMPTY_HISTORY);
        onsets
    }

    /// Replace the texture's contents with `history`, this frame's recent hits.
    ///
    /// Queued on the same queue as the draw that follows it, so the shader sees
    /// this frame's window and never the last one's.
    ///
    /// # Panics
    ///
    /// If `history` is not `2 × ONSET_SLOTS` long.
    /// [`FeatureTimeline::onset_history`](crate::analysis::FeatureTimeline::onset_history)
    /// always returns exactly that many, so a mismatch is a caller bug rather
    /// than anything a user can provoke.
    pub fn upload(&self, gpu: &Gpu, history: &[f32]) {
        assert_eq!(
            history.len(),
            ONSET_SLOTS * 2,
            "the onset history is {ONSET_SLOTS} slots of (birth, ordinal)",
        );

        self.write(gpu, history);
    }

    /// Write `history` into the texture, little-endian `f32`s in slot order.
    fn write(&self, gpu: &Gpu, history: &[f32]) {
        let bytes: Vec<u8> = history
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();

        gpu.queue().write_texture(
            self.texture.as_image_copy(),
            &bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(ONSET_SLOTS as u32 * 8),
                rows_per_image: Some(1),
            },
            self.texture.size(),
        );
    }

    /// The one bind-group layout entry an onset-history preset adds, at binding
    /// 4 — after the spectrum's 3, so a preset that asks for both declares the
    /// same binding numbers as one that asks for either alone.
    pub fn layout_entries() -> [wgpu::BindGroupLayoutEntry; 1] {
        [wgpu::BindGroupLayoutEntry {
            binding: 4,
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

    /// The bind-group entry that fills [`OnsetHistory::layout_entries`].
    pub fn bindings(&self) -> [wgpu::BindGroupEntry<'_>; 1] {
        [wgpu::BindGroupEntry {
            binding: 4,
            resource: wgpu::BindingResource::TextureView(&self.view),
        }]
    }
}
