//! The previous-frame texture, for trails and other feedback effects.
//!
//! The one binding a preset may ask the renderer for beyond the uniform
//! (`VISION.md` §6). A preset opts in with `"needs_feedback": true` in its
//! schema; the renderer then binds last frame's pixels at `@binding(1)` and a
//! sampler at `@binding(2)`, and clears them to transparent black before the
//! first frame.
//!
//! Generic, not `nebula`-specific: `ink`, `ribbons`, and every deferred preset
//! that wants a trail reuses exactly this (RFC-001 NG1).
//!
//! **Why one texture and not a ping-pong pair.** A pair exists to keep a shader
//! from sampling the attachment it is writing. avz never has that problem: the
//! preset draws into its [`Layer`], and the history below is a second texture the
//! layer is copied into afterwards. The copy is GPU-side and costs one frame of
//! bandwidth — the readback that follows it already costs more. Two histories
//! would buy a swap in place of a copy and a second full-resolution texture in
//! exchange.
//!
//! **The history is the visualizer's own layer, not the composited frame.** A
//! trail preset advects what *it* drew last frame, so what it samples is
//! premultiplied light with the background nowhere in it. Feeding back the
//! composited frame would smear the backdrop into every trail.

use crate::render::layer::Layer;
use crate::render::offscreen::{FRAME_FORMAT, Gpu};

/// Last frame's pixels, and the sampler a preset reads them through.
///
/// Built once per render, like [`Offscreen`], and sized to match it.
#[derive(Debug)]
pub struct Feedback {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    sampler: wgpu::Sampler,
}

impl Feedback {
    /// A `width × height` history texture, cleared to transparent black.
    ///
    /// The clear is explicit rather than left to wgpu's lazy zero-initialization,
    /// because "frame 0 sees black" is a contract presets are written against
    /// (`the_feedback_texture_is_black_on_the_first_frame` and
    /// `the_feedback_texture_is_transparent_on_the_first_frame`), not an accident
    /// of what the driver hands back.
    ///
    /// *Transparent* black, and not `wgpu::Color::BLACK`, whose alpha is 1. The
    /// history is a premultiplied layer (`VISION.md` §5.3) and before frame 0
    /// there is no layer, so its coverage is zero. Clearing the alpha to one
    /// makes every feedback render open by hiding the background behind a sheet
    /// of black that fades down over the first frames, and makes a preset that
    /// carries its state in the alpha channel — `ink` — start saturated.
    pub fn new(gpu: &Gpu, width: u32, height: u32) -> Self {
        let texture = gpu.device().create_texture(&wgpu::TextureDescriptor {
            label: Some("avz feedback"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            // The same format as the layer, so the copy below is a plain blit and
            // the shader samples in linear light like everything else.
            format: FRAME_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        let sampler = gpu.device().create_sampler(&wgpu::SamplerDescriptor {
            label: Some("avz feedback"),
            // Clamped, so a trail advected off the edge of the frame smears out
            // of it rather than wrapping around to the far side.
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            // Linear, because a trail is sampled between texels every frame and
            // nearest sampling would quantize the flow into visible stair-steps.
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let mut encoder = gpu
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("avz feedback clear"),
            });
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("avz feedback clear"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
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
        gpu.queue().submit([encoder.finish()]);

        Self {
            texture,
            view,
            sampler,
        }
    }

    /// The two bind-group layout entries a feedback preset adds, at bindings 1
    /// and 2. Declared beside [`Feedback::bindings`] so the pair cannot drift.
    pub fn layout_entries() -> [wgpu::BindGroupLayoutEntry; 2] {
        [
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
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ]
    }

    /// The two bind-group entries that fill [`Feedback::layout_entries`].
    pub fn bindings(&self) -> [wgpu::BindGroupEntry<'_>; 2] {
        [
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&self.view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(&self.sampler),
            },
        ]
    }

    /// Keep the layer just drawn into `target` as the next frame's history.
    ///
    /// Recorded into the same encoder as the draw, after the render pass ends:
    /// the shader sampled the *old* contents, and wgpu inserts the barrier that
    /// makes the overwrite wait for it.
    ///
    /// # Panics
    ///
    /// If `target` is not the layer this history was sized for. The visualizer
    /// takes its target at construction, so a mismatch is a caller bug rather
    /// than anything a user can provoke.
    pub fn capture(&self, encoder: &mut wgpu::CommandEncoder, target: &Layer) {
        let size = target.texture().size();
        assert_eq!(
            size,
            self.texture.size(),
            "the feedback texture was built for a different layer size",
        );

        encoder.copy_texture_to_texture(
            target.texture().as_image_copy(),
            self.texture.as_image_copy(),
            size,
        );
    }
}
