//! One layer of the compositor's stack.
//!
//! `VISION.md` §5.3 stacks three of these bottom to top — background,
//! visualizer, text — and composites them in one final pass. Each renders into
//! its own texture, so a layer never has to know what is under it, and the only
//! thing that does know is [`Compositor`](crate::render::Compositor).
//!
//! **Premultiplied alpha.** A layer stores `color × alpha`, not `color`. That is
//! what makes the `over` operator a plain `src + (1 - src.a) * dst` — one
//! multiply-add the fixed-function blender already does — and what keeps a
//! filtered or resampled layer from fringing toward black where it fades out.
//! For an emissive visualizer it also has a physical reading: the RGB is the
//! light the layer emits, and the alpha is how much of the layer beneath it that
//! light hides.
//!
//! Layers are [`FRAME_FORMAT`], the same sRGB format as the frame, so the
//! premultiplied light a shader writes is encoded once and decoded once on the
//! way into the blend — which happens in linear space, where light adds.

use crate::render::offscreen::{FRAME_FORMAT, Gpu};

/// A frame-sized, premultiplied-alpha RGBA texture that one layer draws into.
///
/// Built once per render and reused for every frame, like
/// [`Offscreen`](crate::render::Offscreen).
#[derive(Debug)]
pub struct Layer {
    label: String,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

impl Layer {
    /// A `width × height` layer, its contents undefined until something draws.
    ///
    /// `label` names the layer in GPU debug output and in the error a mismatched
    /// stack raises ([`Compositor::new`](crate::render::Compositor::new)).
    pub fn new(gpu: &Gpu, width: u32, height: u32, label: &str) -> Self {
        let texture = gpu.device().create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FRAME_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                // The compositor reads it.
                | wgpu::TextureUsages::TEXTURE_BINDING
                // `Feedback::capture` copies the visualizer layer out of it.
                | wgpu::TextureUsages::COPY_SRC
                // A backdrop is uploaded rather than drawn (`background.rs`).
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        Self {
            label: label.to_owned(),
            texture,
            view,
        }
    }

    /// What this layer is called.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// The layer's width in pixels.
    pub fn width(&self) -> u32 {
        self.texture.width()
    }

    /// The layer's height in pixels.
    pub fn height(&self) -> u32 {
        self.texture.height()
    }

    /// The view a layer's own pass renders into, and the compositor samples.
    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    /// Fill the whole layer with one premultiplied linear-space RGBA color.
    ///
    /// Premultiplied: half-covering white is `[0.5, 0.5, 0.5, 0.5]`. The color
    /// channels are encoded to sRGB on write ([`FRAME_FORMAT`]); the alpha is
    /// stored as it is given.
    pub fn clear(&self, gpu: &Gpu, premultiplied: [f32; 4]) {
        let [r, g, b, a] = premultiplied.map(f64::from);

        let mut encoder = gpu
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("avz layer clear"),
            });

        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("avz layer clear"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color { r, g, b, a }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });

        gpu.queue().submit([encoder.finish()]);
    }

    /// The texture behind [`Layer::view`].
    ///
    /// Crate-private: the compositor binds it, `Feedback::capture` copies it, and
    /// nothing else has business with it.
    pub(crate) fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }
}
