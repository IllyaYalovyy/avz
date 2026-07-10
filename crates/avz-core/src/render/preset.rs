//! The preset registry and the pipeline that draws one.
//!
//! A preset is a WGSL file and a JSON parameter schema in
//! `crates/avz-core/presets/`, both embedded with `include_str!`, drawn as a
//! fullscreen triangle against the [`Globals`](crate::render::Globals) uniform
//! (`VISION.md` §6). Adding one is those two files and a row in [`PRESETS`] —
//! which lives in `presets/registry.rs` and is `include!`d here, so a new preset
//! touches nothing outside `presets/` (RFC-001 G3). No pipeline, binding, or
//! compositor code moves (`AGENTS.md`, rendering).
//!
//! The palette that fills `Globals::palette` arrives in RFC-001 Step 16.

use crate::render::globals::{GLOBALS_SIZE, Globals};
use crate::render::offscreen::{FRAME_FORMAT, Gpu, Offscreen};
use crate::render::schema::PresetSchema;
use crate::{Error, Result};

/// One visualizer: a name, a one-line description, its shader, and its schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Preset {
    /// What `--preset` and `visual.preset` name it.
    pub name: &'static str,
    /// The one-line description `avz presets` prints.
    pub description: &'static str,
    /// The WGSL source, embedded in the binary.
    pub source: &'static str,
    /// The JSON parameter schema, embedded in the binary. Parsed by
    /// [`Preset::schema`].
    pub schema: &'static str,
}

include!("../../presets/registry.rs");

impl Preset {
    /// The preset called `name`.
    ///
    /// # Errors
    ///
    /// [`Error::Config`] naming every preset that does exist, because a typo in
    /// `visual.preset` is the user's argument and they need the list to fix it.
    pub fn by_name(name: &str) -> Result<&'static Preset> {
        PRESETS
            .iter()
            .find(|preset| preset.name == name)
            .ok_or_else(|| {
                Error::Config(format!(
                    "unknown preset `{name}`; avz ships: {}",
                    names().join(", ")
                ))
            })
    }

    /// This preset's parameter schema, parsed.
    ///
    /// Parsed on demand rather than at startup: a render reads one schema, and
    /// `avz presets` reads them all once. Both are microseconds.
    ///
    /// # Errors
    ///
    /// [`Error::Config`] if the embedded JSON is malformed or self-contradictory.
    /// The schema ships inside the binary, so that is a bug rather than the
    /// user's mistake — `every_preset_ships_a_schema_that_parses` is what keeps
    /// it from reaching anyone.
    pub fn schema(&self) -> Result<PresetSchema> {
        PresetSchema::parse(self.name, self.schema)
    }
}

/// Every preset name, in registry order.
pub fn names() -> Vec<&'static str> {
    PRESETS.iter().map(|preset| preset.name).collect()
}

/// A preset's render pipeline, its uniform buffer, and its bind group.
///
/// Built once per render and reused for every frame: only the uniform's bytes
/// change between frames.
#[derive(Debug)]
pub struct Visualizer {
    pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    bindings: wgpu::BindGroup,
}

impl Visualizer {
    /// Compile `preset` and build everything it needs to draw.
    ///
    /// # Errors
    ///
    /// [`Error::Render`] if the shader does not compile or link. The presets are
    /// embedded, so that is a bug rather than bad user input — but a message
    /// beats a panic on a driver that rejects what naga accepted.
    pub fn new(gpu: &Gpu, preset: &Preset) -> Result<Self> {
        let device = gpu.device();

        let errors = device.push_error_scope(wgpu::ErrorFilter::Validation);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(preset.name),
            source: wgpu::ShaderSource::Wgsl(preset.source.into()),
        });

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("avz globals"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    // The driver checks the WGSL struct against this size, so a
                    // layout drift is caught at pipeline creation, not in pixels.
                    min_binding_size: wgpu::BufferSize::new(GLOBALS_SIZE as u64),
                },
                count: None,
            }],
        });

        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("avz globals"),
            size: GLOBALS_SIZE as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("avz globals"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(preset.name),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(preset.name),
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
                "preset `{}` does not build on `{}`: {err}",
                preset.name,
                gpu.adapter_name(),
            )));
        }

        Ok(Self {
            pipeline,
            uniforms,
            bindings,
        })
    }

    /// Draw one frame of the preset into `target`.
    ///
    /// The fullscreen triangle covers every pixel, so the frame needs no clear of
    /// its own; the `Clear` below only gives the pass a defined load op.
    pub fn draw(&self, gpu: &Gpu, target: &Offscreen, globals: &Globals) {
        gpu.queue()
            .write_buffer(&self.uniforms, 0, &globals.to_bytes());

        let mut encoder = gpu
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("avz visualizer"),
            });

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("avz visualizer"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target.view(),
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::schema::ParamKind;

    #[test]
    fn pulse_is_the_preset_the_default_config_names() {
        let preset = Preset::by_name("pulse").expect("pulse ships with avz");

        assert_eq!(preset.name, "pulse");
        assert!(!preset.description.is_empty(), "`avz presets` prints this");
        assert!(
            preset.source.contains("fn fs_main"),
            "the WGSL is embedded, not read from disk at runtime"
        );
    }

    /// A typo in `visual.preset` is the user's argument, and the fix is the list.
    #[test]
    fn an_unknown_preset_is_a_config_error_that_names_the_known_ones() {
        let err = Preset::by_name("pulze").expect_err("there is no `pulze`");

        assert!(matches!(err, Error::Config(_)), "got {err:?}");
        let message = err.to_string();
        assert!(message.contains("pulze"), "quote the typo: {message}");
        assert!(message.contains("pulse"), "name what does exist: {message}");
    }

    /// Every shipped preset carries a schema that parses, and a schema is not
    /// optional: it is the only thing `avz presets` and `--set` can validate
    /// against.
    #[test]
    fn every_preset_ships_a_schema_that_parses() {
        for preset in PRESETS {
            let schema = preset
                .schema()
                .unwrap_or_else(|err| panic!("preset `{}`: {err}", preset.name));

            assert_eq!(schema.preset, preset.name);
            assert!(
                !schema.params.is_empty(),
                "preset `{}` exposes nothing to tune",
                preset.name,
            );
        }
    }

    /// A default outside its own declared range would make `avz presets` print a
    /// lie and every default render illegal. Keeps future preset authors honest.
    #[test]
    fn schema_defaults_all_within_declared_ranges() {
        for preset in PRESETS {
            let schema = preset.schema().expect("a shipped schema parses");
            for param in &schema.params {
                match &param.kind {
                    ParamKind::Float { default, min, max } => assert!(
                        default >= min && default <= max,
                        "`{}.{}` defaults to {default}, outside {min}..{max}",
                        preset.name,
                        param.name,
                    ),
                    ParamKind::Int { default, min, max } => assert!(
                        default >= min && default <= max,
                        "`{}.{}` defaults to {default}, outside {min}..{max}",
                        preset.name,
                        param.name,
                    ),
                    ParamKind::Enum { default, variants } => assert!(
                        variants.contains(default),
                        "`{}.{}` defaults to `{default}`, not one of its variants",
                        preset.name,
                        param.name,
                    ),
                    ParamKind::Bool { .. } | ParamKind::Color { .. } => {}
                }
            }

            // `resolve` re-checks the same thing on the way into the uniform, so
            // an empty override table must be accepted by every shipped schema.
            schema
                .resolve(&toml::Table::new())
                .expect("the defaults pack");
        }
    }

    /// A schema slot nothing in the WGSL reads is a knob wired to nothing. The
    /// shader spells the accessor out, so the accessor is what to look for.
    #[test]
    fn every_schema_parameter_is_read_by_the_shader_that_declares_it() {
        const COMPONENTS: [&str; 4] = ["x", "y", "z", "w"];

        for preset in PRESETS {
            let schema = preset.schema().expect("a shipped schema parses");
            for param in &schema.params {
                let index = param.slot.index;
                let accessor = match param.kind {
                    // A color is the whole `vec4`, however the shader swizzles it.
                    ParamKind::Color { .. } => format!("params[{index}]"),
                    _ => format!("params[{index}].{}", COMPONENTS[param.slot.component]),
                };

                assert!(
                    preset.source.contains(&accessor),
                    "preset `{}` declares `{}` at `{accessor}`, but its WGSL never reads it",
                    preset.name,
                    param.name,
                );
            }
        }
    }

    /// Every shipped preset declares the `VISION.md` §6 uniform contract, whole.
    /// A preset that renamed a field would compile against its own struct and
    /// silently read the wrong feature.
    #[test]
    fn every_preset_declares_the_whole_globals_contract() {
        let contract = [
            "time: f32",
            "resolution: vec2<f32>",
            "seed: f32",
            "rms: f32",
            "rms_env: f32",
            "bass: f32",
            "bass_env: f32",
            "low_mid: f32",
            "low_mid_env: f32",
            "mid: f32",
            "mid_env: f32",
            "high: f32",
            "high_env: f32",
            "air: f32",
            "air_env: f32",
            "flux: f32",
            "onset: f32",
            "centroid: f32",
            "pal: array<vec4<f32>, 5>",
            "params: array<vec4<f32>, 8>",
        ];

        for preset in PRESETS {
            for member in contract {
                assert!(
                    preset.source.contains(member),
                    "preset `{}` is missing `{member}` from struct Globals",
                    preset.name,
                );
            }
        }
    }
}
