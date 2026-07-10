//! The final pass: the layer stack, composited into the frame.
//!
//! `VISION.md` §5.3 defines three layers — background, visualizer, text — drawn
//! bottom to top "in one final pass". That is what this is: one render pass into
//! [`Offscreen`](crate::render::Offscreen), one fullscreen triangle per layer,
//! premultiplied `over` blending between them.
//!
//! Nothing here knows what a layer *is*. A [`Layer`] is a frame-sized texture of
//! premultiplied light, and the compositor takes a slice of them. The background
//! image and the text card are two more entries in that slice and no more code
//! in this file; the looped background video (RFC-001 NG2) will be a third.
//!
//! **Absent layers cost nothing.** A stack the caller did not put a layer into
//! has no bind group and no draw call for it — not a black quad, not a cleared
//! texture nobody looks at. The frame is cleared to transparent black once and
//! then only the layers that exist are drawn over it.
//!
//! **Why `textureLoad` and not a sampler.** Every layer is exactly frame-sized,
//! so the fragment at pixel `(x, y)` wants texel `(x, y)` and nothing else. A
//! sampler would interpolate between texels that a correct render never asks
//! for, and would need a filterable-binding declaration that says the opposite
//! of what the pass does.

use crate::render::layer::Layer;
use crate::render::offscreen::{FRAME_FORMAT, Gpu, Offscreen};
use crate::{Error, Result};

/// The fullscreen triangle and the one texel it reads.
///
/// Inline rather than an `include_str!` of a `.wgsl` file: `presets/` is the only
/// place avz embeds shaders from, and this is not a preset
/// (`scripts/quality.d/96-a-preset-is-only-files-in-presets.sh`).
const COMPOSITE_WGSL: &str = r"
@group(0) @binding(0) var layer: texture_2d<f32>;

@vertex
fn vs_main(@builtin(vertex_index) vertex: u32) -> @builtin(position) vec4<f32> {
    let index = i32(vertex);
    let x = f32(index / 2) * 4.0 - 1.0;
    let y = f32(index & 1) * 4.0 - 1.0;
    return vec4<f32>(x, y, 0.0, 1.0);
}

// The layer already stores premultiplied light; the blend state does the rest.
@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    return textureLoad(layer, vec2<i32>(position.xy), 0);
}
";

/// The `over` operator on premultiplied colors: `src + (1 - src.a) * dst`, on
/// the color channels and on alpha alike.
///
/// Named so the one decision this module makes is readable in one place.
const PREMULTIPLIED_OVER: wgpu::BlendState = wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING;

/// The layer stack of one render, and the pipeline that flattens it.
///
/// Built once per render: the bind groups below point at textures that are
/// re-drawn every frame, not replaced.
#[derive(Debug)]
pub struct Compositor {
    pipeline: wgpu::RenderPipeline,
    /// One per layer, bottom first.
    bindings: Vec<wgpu::BindGroup>,
    /// The frame size every layer agreed on.
    size: Option<(u32, u32)>,
}

impl Compositor {
    /// Build the pass that composites `layers`, bottom first.
    ///
    /// # Errors
    ///
    /// [`Error::Render`] if the layers are not all the same size, naming the one
    /// that disagrees. A compositor that quietly sampled outside a smaller layer
    /// would shear the picture rather than fail.
    pub fn new(gpu: &Gpu, layers: &[&Layer]) -> Result<Self> {
        let device = gpu.device();

        let size = match layers.split_first() {
            None => None,
            Some((first, rest)) => {
                let size = (first.width(), first.height());
                for layer in rest {
                    if (layer.width(), layer.height()) != size {
                        return Err(Error::Render(format!(
                            "layer `{}` is {}x{}, but the frame is {}x{}",
                            layer.label(),
                            layer.width(),
                            layer.height(),
                            size.0,
                            size.1,
                        )));
                    }
                }
                Some(size)
            }
        };

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("avz compositor"),
            source: wgpu::ShaderSource::Wgsl(COMPOSITE_WGSL.into()),
        });

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("avz compositor"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });

        let bindings = layers
            .iter()
            .map(|layer| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some(layer.label()),
                    layout: &layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(layer.view()),
                    }],
                })
            })
            .collect();

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("avz compositor"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("avz compositor"),
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
                    blend: Some(PREMULTIPLIED_OVER),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        Ok(Self {
            pipeline,
            bindings,
            size,
        })
    }

    /// How many layers this stack draws.
    pub fn layers(&self) -> usize {
        self.bindings.len()
    }

    /// Flatten the stack into `target`, bottom layer first.
    ///
    /// The frame is cleared to transparent black, which is the identity of the
    /// `over` operator: whatever the bottom layer is, it lands unmodified. The
    /// default backdrop is opaque, so a finished frame has alpha 255 and ffmpeg
    /// never sees anything else.
    ///
    /// # Panics
    ///
    /// If `target` is not the size the layers were built for. Both come from
    /// `config.output.resolution` in one function, so a mismatch is a caller bug.
    pub fn composite(&self, gpu: &Gpu, target: &Offscreen) {
        let frame = target.layout();
        if let Some(size) = self.size {
            assert_eq!(
                size,
                (frame.width(), frame.height()),
                "the layer stack was built for a different frame size",
            );
        }

        let mut encoder = gpu
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("avz compositor"),
            });

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("avz compositor"),
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
            for bindings in &self.bindings {
                pass.set_bind_group(0, bindings, &[]);
                pass.draw(0..3, 0..1);
            }
        }

        gpu.queue().submit([encoder.finish()]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one decision in this module, asserted rather than trusted to a wgpu
    /// constant that could be reinterpreted. A `SrcAlpha` source factor here
    /// would double-apply alpha to every premultiplied layer.
    #[test]
    fn the_blend_is_the_premultiplied_over_operator() {
        let over = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        };

        assert_eq!(PREMULTIPLIED_OVER.color, over);
        assert_eq!(
            PREMULTIPLIED_OVER.alpha, over,
            "alpha composites with the same operator, or a stack of \
             half-transparent layers never reaches opaque",
        );
    }
}
