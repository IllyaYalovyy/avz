//! The effects stage: color, brightness, zoom, and rotation over the finished
//! picture (RFC-002, issue #54).
//!
//! One pass between the compositor and the readback. The shader is dumb by
//! design: it samples the flattened frame at a transformed UV and multiplies
//! the color through a matrix — *all* of the arithmetic that decides what the
//! effects do lives here in Rust, per frame, where it is unit-testable
//! against hand-computed values.
//!
//! Fixed order, documented in `docs/CONFIGURATION.md`: geometry first (zoom
//! and rotation about the frame's center, in aspect-true coordinates,
//! clamp-to-edge at the fringe), then color (contrast, saturation, hue,
//! brightness, composed into one 3×3 + offset, applied in linear light), and
//! last the clip fade — [`fade_gain`] scaling that whole transform, so the
//! picture comes up from and goes down to black around everything else.
//!
//! Identity is free: when the config asks for nothing, [`config::Effects::is_identity`]
//! is true, the pass is never built, and the render is byte-identical to a
//! build without this module (RFC-002 G3).
//!
//! Determinism: the matrices are functions of frame time and the frame's own
//! features — `spin·time + sway·bass_env`, `1 + pulse·bass_env`,
//! `1 + flash·onset` — nothing integrates (AGENTS.md).

use crate::analysis::FeatureFrame;
use crate::config::Effects as EffectsConfig;
use crate::render::layer::Layer;
use crate::render::offscreen::{FRAME_FORMAT, Gpu, Offscreen};
use crate::{Error, Result};

const TAU: f32 = std::f32::consts::TAU;

/// Rec. 709 luma weights, in linear light — the gray axis the saturation and
/// hue matrices preserve.
const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];

/// The UV transform for one frame: the matrix that carries a *destination*
/// coordinate (centered, aspect-true) to the *source* coordinate to sample.
///
/// Zoom magnifies, so the sampler walks a smaller source region (divide);
/// rotation turns the picture by `+angle`, so the sampler turns by `-angle`.
/// Row-major: `[m00, m01, m10, m11]`.
pub fn uv_transform(effects: &EffectsConfig, features: &FeatureFrame, time: f32) -> [f32; 4] {
    let zoom = (effects.zoom * (1.0 + effects.pulse * features.bass_env)).max(1e-3);
    let angle = (effects.spin * time + effects.sway * features.bass_env) * TAU;

    let (sin, cos) = angle.sin_cos();
    let inv = 1.0 / zoom;
    // R(-angle) / zoom.
    [cos * inv, sin * inv, -sin * inv, cos * inv]
}

/// The color transform for one frame: a 3×3 matrix and an offset, composed
/// contrast → saturation → hue → brightness, all in linear light.
///
/// Returned row-major with each row's offset in the fourth column:
/// `[m00, m01, m02, off0, m10, ...]` — exactly the layout the shader reads.
pub fn color_transform(effects: &EffectsConfig, features: &FeatureFrame, time: f32) -> [f32; 12] {
    // Contrast pivots at mid-gray: c' = k·c + 0.5·(1 − k).
    let k = effects.contrast;
    let mut matrix = scale3(k);
    let mut offset = [0.5 * (1.0 - k); 3];

    // Saturation blends toward the luma gray axis: S = s·I + (1 − s)·L.
    let s = effects.saturation;
    let sat = [
        [
            s + (1.0 - s) * LUMA[0],
            (1.0 - s) * LUMA[1],
            (1.0 - s) * LUMA[2],
        ],
        [
            (1.0 - s) * LUMA[0],
            s + (1.0 - s) * LUMA[1],
            (1.0 - s) * LUMA[2],
        ],
        [
            (1.0 - s) * LUMA[0],
            (1.0 - s) * LUMA[1],
            s + (1.0 - s) * LUMA[2],
        ],
    ];
    (matrix, offset) = compose(sat, [0.0; 3], matrix, offset);

    // Hue rotates about the gray axis (the SVG `hueRotate` construction, with
    // the same luma weights as above so gray stays exactly gray).
    let hue = (effects.hue + effects.hue_drift * time) * TAU;
    (matrix, offset) = compose(hue_rotation(hue), [0.0; 3], matrix, offset);

    // Brightness last, lifted by the hit: b' = b·(1 + flash·onset).
    let b = effects.brightness * (1.0 + effects.flash * features.onset);
    (matrix, offset) = compose(scale3(b), [0.0; 3], matrix, offset);

    [
        matrix[0][0],
        matrix[0][1],
        matrix[0][2],
        offset[0],
        matrix[1][0],
        matrix[1][1],
        matrix[1][2],
        offset[1],
        matrix[2][0],
        matrix[2][1],
        matrix[2][2],
        offset[2],
    ]
}

/// Where a frame sits in the *clip* — the video being written — rather than in
/// the song.
///
/// The distinction is the whole point of this type. `spin` and `hue_drift` read
/// song time, so that `--sample 1s..2s` previews exactly what the full render
/// draws at those timestamps. A fade cannot: it belongs to the clip's own first
/// and last frames, so a sampled second fades up at *its* start, not at the
/// song's.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClipTime {
    /// Seconds since the clip's first frame.
    pub elapsed: f32,
    /// The clip's whole length, in seconds.
    pub duration: f32,
}

/// The clip fade's gain for one frame: 0 at the first frame of a fade in, 1 once
/// it is up, and back to 0 at the clip's last frame under a fade out.
///
/// The two ramps combine with `min`, not by multiplying. On a clip shorter than
/// `fade_in + fade_out` the windows overlap, and multiplying would darken the
/// middle twice over — the picture would dip toward black exactly where it
/// should be brightest. Taking the smaller ramp keeps the curve up-then-down and
/// respects both fades.
pub fn fade_gain(effects: &EffectsConfig, clip: ClipTime) -> f32 {
    let fade_in = effects.fade_in.as_secs_f64() as f32;
    let fade_out = effects.fade_out.as_secs_f64() as f32;

    // A clip with no length has no start to fade from and no end to fade to.
    if clip.duration <= 0.0 {
        return 1.0;
    }

    let up = if fade_in > 0.0 {
        clip.elapsed / fade_in
    } else {
        1.0
    };
    let down = if fade_out > 0.0 {
        (clip.duration - clip.elapsed) / fade_out
    } else {
        1.0
    };

    up.min(down).clamp(0.0, 1.0)
}

/// `k`·I.
fn scale3(k: f32) -> [[f32; 3]; 3] {
    [[k, 0.0, 0.0], [0.0, k, 0.0], [0.0, 0.0, k]]
}

/// The affine composition `after ∘ before`: apply `before`, then `after`.
fn compose(
    after: [[f32; 3]; 3],
    after_offset: [f32; 3],
    before: [[f32; 3]; 3],
    before_offset: [f32; 3],
) -> ([[f32; 3]; 3], [f32; 3]) {
    let mut matrix = [[0.0; 3]; 3];
    let mut offset = after_offset;
    for row in 0..3 {
        for column in 0..3 {
            for i in 0..3 {
                matrix[row][column] += after[row][i] * before[i][column];
            }
        }
        for i in 0..3 {
            offset[row] += after[row][i] * before_offset[i];
        }
    }
    (matrix, offset)
}

/// Rotation about the gray axis by `angle` radians — the standard
/// `feColorMatrix hueRotate` construction on [`LUMA`].
fn hue_rotation(angle: f32) -> [[f32; 3]; 3] {
    let (sin, cos) = angle.sin_cos();
    let [lr, lg, lb] = LUMA;
    [
        [
            lr + cos * (1.0 - lr) - sin * lr,
            lg - cos * lg - sin * lg,
            lb - cos * lb + sin * (1.0 - lb),
        ],
        [
            lr - cos * lr + sin * 0.143,
            lg + cos * (1.0 - lg) + sin * 0.140,
            lb - cos * lb - sin * 0.283,
        ],
        [
            lr - cos * lr - sin * (1.0 - lr),
            lg - cos * lg + sin * lg,
            lb + cos * (1.0 - lb) + sin * lb,
        ],
    ]
}

/// Apply a `color_transform` result to one linear RGB value — the reference
/// the tests compare against, and handy for callers that want a CPU preview.
pub fn apply_color(transform: &[f32; 12], rgb: [f32; 3]) -> [f32; 3] {
    let mut out = [0.0; 3];
    for row in 0..3 {
        out[row] = transform[row * 4] * rgb[0]
            + transform[row * 4 + 1] * rgb[1]
            + transform[row * 4 + 2] * rgb[2]
            + transform[row * 4 + 3];
    }
    out
}

/// The GPU pass. Built once per render, only when the config is not identity.
pub struct EffectsPass {
    pipeline: wgpu::RenderPipeline,
    bindings: wgpu::BindGroup,
    uniform: wgpu::Buffer,
}

/// The uniform block the shader reads: the UV matrix, the three color rows
/// with their offsets, and the frame's aspect ratio.
const UNIFORM_FLOATS: usize = 4 + 12 + 4;

impl EffectsPass {
    /// Build the pass that reads `source` — the compositor's flattened frame.
    ///
    /// # Errors
    ///
    /// [`Error::Render`] if the shader fails validation, which is a bug in
    /// this module rather than in any input.
    pub fn new(gpu: &Gpu, source: &Layer) -> Result<Self> {
        let device = gpu.device();
        let errors = device.push_error_scope(wgpu::ErrorFilter::Validation);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("avz effects"),
            source: wgpu::ShaderSource::Wgsl(EFFECTS_WGSL.into()),
        });

        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("avz effects"),
            size: (UNIFORM_FLOATS * 4) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Clamp-to-edge: the fringe a zoom-out or rotation exposes smears the
        // edge pixel instead of cutting to black (RFC-002 Q1).
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("avz effects"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("avz effects"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
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
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("avz effects"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(source.view()),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("avz effects"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("avz effects"),
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
                "the effects pass does not build on `{}`: {err}",
                gpu.adapter_name(),
            )));
        }

        Ok(Self {
            pipeline,
            bindings,
            uniform,
        })
    }

    /// Transform the flattened frame into `target` for one video frame.
    pub fn apply(
        &self,
        gpu: &Gpu,
        target: &Offscreen,
        effects: &EffectsConfig,
        features: &FeatureFrame,
        time: f32,
        clip: ClipTime,
    ) {
        let uv = uv_transform(effects, features, time);
        // The fade scales the finished color transform — matrix *and* offsets,
        // so it scales the transform's output rather than only its slope. That
        // makes it the last thing applied, after contrast, saturation, hue, and
        // brightness, which is what "fade the picture to black" means. No shader
        // change: the uniform block is the same twelve floats, smaller.
        let gain = fade_gain(effects, clip);
        let color = color_transform(effects, features, time).map(|float| float * gain);
        let layout = target.layout();
        let aspect = layout.width() as f32 / layout.height() as f32;

        let mut floats = [0.0f32; UNIFORM_FLOATS];
        floats[..4].copy_from_slice(&uv);
        floats[4..16].copy_from_slice(&color);
        floats[16] = aspect;

        let mut bytes = [0u8; UNIFORM_FLOATS * 4];
        for (slot, float) in floats.iter().enumerate() {
            bytes[slot * 4..slot * 4 + 4].copy_from_slice(&float.to_le_bytes());
        }
        gpu.queue().write_buffer(&self.uniform, 0, &bytes);

        let mut encoder = gpu
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("avz effects"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("avz effects"),
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
}

/// Inline rather than an `include_str!` of a `.wgsl` file: `presets/` is the
/// only home for standalone shader files, and this is core machinery.
const EFFECTS_WGSL: &str = r#"
struct Post {
    // R(-angle)/zoom, row-major.
    uv: vec4<f32>,
    // Color rows, offset in w.
    tone0: vec4<f32>,
    tone1: vec4<f32>,
    tone2: vec4<f32>,
    // x is the frame's aspect ratio; the rest is padding.
    frame: vec4<f32>,
}

@group(0) @binding(0) var<uniform> post: Post;
@group(0) @binding(1) var source: texture_2d<f32>;
@group(0) @binding(2) var source_sampler: sampler;

struct Vertex {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) vertex: u32) -> Vertex {
    let index = i32(vertex);
    let x = f32(index / 2) * 4.0 - 1.0;
    let y = f32(index & 1) * 4.0 - 1.0;
    return Vertex(vec4<f32>(x, y, 0.0, 1.0), vec2<f32>(x, -y) * 0.5 + 0.5);
}

@fragment
fn fs_main(in: Vertex) -> @location(0) vec4<f32> {
    // Geometry: rotate and zoom about the center, in aspect-true units so a
    // rotation is a rotation and not a shear.
    let aspect = vec2<f32>(post.frame.x, 1.0);
    let centered = (in.uv - 0.5) * aspect;
    let at = vec2<f32>(
        dot(post.uv.xy, centered),
        dot(post.uv.zw, centered),
    );
    let sampled = textureSample(source, source_sampler, at / aspect + 0.5);

    // Color: one affine transform in linear light; alpha passes through.
    let rgb = vec3<f32>(
        dot(post.tone0.xyz, sampled.rgb) + post.tone0.w,
        dot(post.tone1.xyz, sampled.rgb) + post.tone1.w,
        dot(post.tone2.xyz, sampled.rgb) + post.tone2.w,
    );
    return vec4<f32>(max(rgb, vec3<f32>(0.0)), sampled.a);
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> EffectsConfig {
        EffectsConfig::default()
    }

    fn near(a: f32, b: f32) {
        assert!((a - b).abs() < 1e-4, "{a} !~ {b}");
    }

    fn near3(a: [f32; 3], b: [f32; 3]) {
        for i in 0..3 {
            assert!((a[i] - b[i]).abs() < 1e-3, "{a:?} !~ {b:?}");
        }
    }

    #[test]
    fn the_default_config_is_the_identity() {
        assert!(identity().is_identity());

        let mut zoomed = identity();
        zoomed.zoom = 1.2;
        assert!(!zoomed.is_identity());

        let mut flashed = identity();
        flashed.flash = 0.5;
        assert!(!flashed.is_identity());
    }

    #[test]
    fn identity_transforms_change_nothing() {
        let frame = FeatureFrame::default();
        let uv = uv_transform(&identity(), &frame, 3.7);
        near(uv[0], 1.0);
        near(uv[1], 0.0);
        near(uv[2], 0.0);
        near(uv[3], 1.0);

        let color = color_transform(&identity(), &frame, 3.7);
        near3(apply_color(&color, [0.2, 0.5, 0.9]), [0.2, 0.5, 0.9]);
    }

    #[test]
    fn zoom_shrinks_the_sampled_region() {
        let mut effects = identity();
        effects.zoom = 2.0;
        let uv = uv_transform(&effects, &FeatureFrame::default(), 0.0);
        near(uv[0], 0.5);
        near(uv[3], 0.5);
        near(uv[1], 0.0);
    }

    #[test]
    fn the_kick_pulses_the_zoom() {
        let mut effects = identity();
        effects.pulse = 0.5;
        let quiet = uv_transform(&effects, &FeatureFrame::default(), 0.0);
        near(quiet[0], 1.0);

        let loud = FeatureFrame {
            bass_env: 1.0,
            ..FeatureFrame::default()
        };
        let kicked = uv_transform(&effects, &loud, 0.0);
        near(kicked[0], 1.0 / 1.5);
    }

    #[test]
    fn a_quarter_turn_of_spin_samples_the_perpendicular_axis() {
        let mut effects = identity();
        effects.spin = 0.25;
        // One second in: angle = TAU/4, so the sampler reads R(-90°).
        let uv = uv_transform(&effects, &FeatureFrame::default(), 1.0);
        near(uv[0], 0.0);
        near(uv[1], 1.0);
        near(uv[2], -1.0);
        near(uv[3], 0.0);
    }

    #[test]
    fn brightness_scales_and_a_hit_lifts_it() {
        let mut effects = identity();
        effects.brightness = 2.0;
        effects.flash = 0.5;

        let calm = color_transform(&effects, &FeatureFrame::default(), 0.0);
        near3(apply_color(&calm, [0.1, 0.2, 0.3]), [0.2, 0.4, 0.6]);

        let hit = FeatureFrame {
            onset: 1.0,
            ..FeatureFrame::default()
        };
        let flashed = color_transform(&effects, &hit, 0.0);
        near3(apply_color(&flashed, [0.1, 0.1, 0.1]), [0.3, 0.3, 0.3]);
    }

    #[test]
    fn contrast_pivots_at_mid_gray() {
        let mut effects = identity();
        effects.contrast = 2.0;
        let color = color_transform(&effects, &FeatureFrame::default(), 0.0);
        near3(apply_color(&color, [0.5, 0.5, 0.5]), [0.5, 0.5, 0.5]);
        near3(apply_color(&color, [0.75, 0.25, 0.5]), [1.0, 0.0, 0.5]);
    }

    #[test]
    fn zero_saturation_is_the_luma_gray() {
        let mut effects = identity();
        effects.saturation = 0.0;
        let color = color_transform(&effects, &FeatureFrame::default(), 0.0);
        let luma = 0.2126 * 0.8 + 0.0722 * 0.4;
        near3(apply_color(&color, [0.8, 0.0, 0.4]), [luma, luma, luma]);
    }

    #[test]
    fn hue_leaves_gray_alone_and_turns_red_toward_green() {
        let mut effects = identity();
        effects.hue = 1.0 / 3.0;
        let color = color_transform(&effects, &FeatureFrame::default(), 0.0);

        near3(apply_color(&color, [0.5, 0.5, 0.5]), [0.5, 0.5, 0.5]);

        let turned = apply_color(&color, [1.0, 0.0, 0.0]);
        assert!(
            turned[1] > turned[0] && turned[1] > turned[2],
            "a third of a turn should carry red's energy to green: {turned:?}"
        );
    }

    #[test]
    fn hue_drift_walks_with_the_song_clock() {
        let mut effects = identity();
        effects.hue_drift = 1.0 / 3.0;
        let at_zero = color_transform(&effects, &FeatureFrame::default(), 0.0);
        near3(apply_color(&at_zero, [0.2, 0.5, 0.9]), [0.2, 0.5, 0.9]);

        let later = color_transform(&effects, &FeatureFrame::default(), 3.0);
        near3(apply_color(&later, [0.2, 0.5, 0.9]), [0.2, 0.5, 0.9]);
    }

    #[test]
    fn the_composed_matrix_matches_sequential_application() {
        let mut effects = identity();
        effects.contrast = 1.4;
        effects.saturation = 0.6;
        effects.brightness = 1.2;
        let composed = color_transform(&effects, &FeatureFrame::default(), 0.0);

        let sample = [0.3f32, 0.6, 0.1];
        // By hand, in the documented order: contrast, saturation, brightness.
        let contrasted: Vec<f32> = sample.iter().map(|c| c * 1.4 + 0.5 * (1.0 - 1.4)).collect();
        let luma = 0.2126 * contrasted[0] + 0.7152 * contrasted[1] + 0.0722 * contrasted[2];
        let saturated: Vec<f32> = contrasted.iter().map(|c| luma + (c - luma) * 0.6).collect();
        let expected = [saturated[0] * 1.2, saturated[1] * 1.2, saturated[2] * 1.2];

        near3(apply_color(&composed, sample), expected);
    }

    fn clip(elapsed: f32, duration: f32) -> ClipTime {
        ClipTime { elapsed, duration }
    }

    fn seconds(text: &str) -> crate::config::Seconds {
        text.parse().expect("a test duration parses")
    }

    #[test]
    fn no_fade_is_a_gain_of_one() {
        let effects = identity();
        for elapsed in [0.0, 0.5, 5.0, 10.0] {
            near(fade_gain(&effects, clip(elapsed, 10.0)), 1.0);
        }
    }

    #[test]
    fn a_clip_fade_makes_the_config_ask_for_a_pass() {
        // Without this, a config whose only effect is a fade would be skipped
        // as the identity and the fade would silently do nothing.
        let mut faded = identity();
        faded.fade_in = seconds("1s");
        assert!(!faded.is_identity());
    }

    #[test]
    fn the_fade_in_ramps_from_black_to_full() {
        let mut effects = identity();
        effects.fade_in = seconds("2s");

        near(fade_gain(&effects, clip(0.0, 10.0)), 0.0);
        near(fade_gain(&effects, clip(1.0, 10.0)), 0.5);
        near(fade_gain(&effects, clip(2.0, 10.0)), 1.0);
        // Up is up: the ramp does not keep climbing past its window.
        near(fade_gain(&effects, clip(7.0, 10.0)), 1.0);
    }

    #[test]
    fn the_fade_out_lands_on_black_at_the_clips_end() {
        let mut effects = identity();
        effects.fade_out = seconds("2s");

        near(fade_gain(&effects, clip(0.0, 10.0)), 1.0);
        near(fade_gain(&effects, clip(8.0, 10.0)), 1.0);
        near(fade_gain(&effects, clip(9.0, 10.0)), 0.5);
        near(fade_gain(&effects, clip(10.0, 10.0)), 0.0);
    }

    #[test]
    fn overlapping_fades_take_the_smaller_gain() {
        // Three seconds each on a four-second clip: the windows overlap for two
        // seconds in the middle. Multiplying the ramps would darken that middle
        // — the brightest part of the clip — so `fade_gain` takes the min.
        let mut effects = identity();
        effects.fade_in = seconds("3s");
        effects.fade_out = seconds("3s");

        near(fade_gain(&effects, clip(0.0, 4.0)), 0.0);
        near(fade_gain(&effects, clip(1.0, 4.0)), 1.0 / 3.0);
        // The crossover, and the clip's brightest frame.
        near(fade_gain(&effects, clip(2.0, 4.0)), 2.0 / 3.0);
        near(fade_gain(&effects, clip(3.0, 4.0)), 1.0 / 3.0);
        near(fade_gain(&effects, clip(4.0, 4.0)), 0.0);
    }

    #[test]
    fn a_zero_length_clip_has_nothing_to_fade() {
        let mut effects = identity();
        effects.fade_in = seconds("1s");
        effects.fade_out = seconds("1s");
        near(fade_gain(&effects, clip(0.0, 0.0)), 1.0);
    }

    #[test]
    fn a_fade_scales_the_whole_color_transform() {
        // Contrast puts a non-zero offset in the transform's fourth column; the
        // fade has to scale that too, or a faded frame would settle toward
        // mid-gray instead of going to black.
        let mut effects = identity();
        effects.contrast = 1.4;
        effects.brightness = 1.2;
        let transform = color_transform(&effects, &FeatureFrame::default(), 0.0);

        let sample = [0.3f32, 0.6, 0.1];
        let lit = apply_color(&transform, sample);

        for gain in [0.0f32, 0.25, 0.5, 1.0] {
            let faded = transform.map(|float| float * gain);
            near3(
                apply_color(&faded, sample),
                [lit[0] * gain, lit[1] * gain, lit[2] * gain],
            );
        }
    }
}
