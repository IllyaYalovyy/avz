//! The offscreen render target and the device it lives on.
//!
//! There is no window and no surface: avz renders into a texture, copies it into
//! a mapped buffer, and hands the bytes to the encoder (`VISION.md` §5.3). The
//! copy is where wgpu's 256-byte row alignment intrudes, and [`RowLayout`] is
//! the only thing that knows about it.

use crate::render::adapter::{self, AdapterChoice, AdapterKind, Selection};
use crate::render::readback::RowLayout;
use crate::{Error, Result};

/// The texture format every avz frame is rendered and read back in.
///
/// sRGB because shaders blend in linear space and the encoder wants sRGB bytes;
/// `Rgba8` because that is exactly what `ffmpeg -pix_fmt rgba` reads from stdin.
pub const FRAME_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// An open GPU device, plus what kind of adapter it was opened on.
#[derive(Debug)]
pub struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    kind: AdapterKind,
    fell_back_to_software: bool,
    adapter_name: String,
}

impl Gpu {
    /// Select an adapter per `choice` and open a device on it.
    ///
    /// # Errors
    ///
    /// [`Error::Render`] if no adapter satisfies `choice`, or if the adapter
    /// refuses a device. Both are terminal: there is no second renderer to try.
    pub fn new(choice: AdapterChoice) -> Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            // One code path (`AGENTS.md`, rendering). The other backends are not
            // compiled in, but saying so here keeps the invariant readable.
            backends: wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });

        Self::open(adapter::select(&instance, choice)?)
    }

    /// Open a device on an already-selected adapter.
    fn open(selection: Selection) -> Result<Self> {
        let adapter = selection.adapter();
        let adapter_name = adapter.get_info().name;

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("avz"),
            // Nothing avz renders needs an optional feature; asking for none is
            // what lets the same shaders run on lavapipe and on a discrete GPU.
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            ..Default::default()
        }))
        .map_err(|err| Error::Render(format!("cannot open a device on `{adapter_name}`: {err}")))?;

        Ok(Self {
            device,
            queue,
            kind: selection.kind(),
            fell_back_to_software: selection.fell_back_to_software(),
            adapter_name,
        })
    }

    /// The kind of adapter this device runs on.
    pub fn kind(&self) -> AdapterKind {
        self.kind
    }

    /// The adapter's own name, e.g. `llvmpipe (LLVM 20.1.0, 256 bits)`.
    pub fn adapter_name(&self) -> &str {
        &self.adapter_name
    }

    /// Whether `--adapter auto` had to settle for software rendering.
    ///
    /// `avz-cli` warns on this; `avz-core` never prints.
    pub fn fell_back_to_software(&self) -> bool {
        self.fell_back_to_software
    }

    /// The device, for building pipelines and textures.
    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    /// The queue, for submitting command buffers.
    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }
}

/// A `width × height` RGBA texture and the buffer its pixels are read back into.
///
/// Built once per render and reused for every frame: the texture is re-drawn and
/// the readback buffer re-copied, so a five-minute song allocates this once.
#[derive(Debug)]
pub struct Offscreen {
    layout: RowLayout,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    readback: wgpu::Buffer,
}

impl Offscreen {
    /// Create the render target and its readback buffer.
    ///
    /// # Errors
    ///
    /// [`Error::Render`] if the size is not a frame ([`RowLayout::new`]) or
    /// exceeds the device's `max_texture_dimension_2d`. Checking against the
    /// device's own limit turns a driver-level panic into a message naming the
    /// adapter that cannot do it.
    pub fn new(gpu: &Gpu, width: u32, height: u32) -> Result<Self> {
        let layout = RowLayout::new(width, height)?;

        let max = gpu.device().limits().max_texture_dimension_2d;
        if width > max || height > max {
            return Err(Error::Render(format!(
                "`{}` renders at most {max}x{max}, but the output is {width}x{height}",
                gpu.adapter_name(),
            )));
        }

        let texture = gpu.device().create_texture(&wgpu::TextureDescriptor {
            label: Some("avz frame"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FRAME_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        let readback = gpu.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("avz frame readback"),
            size: layout.buffer_size(),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        Ok(Self {
            layout,
            texture,
            view,
            readback,
        })
    }

    /// How this frame is laid out, padding and all.
    pub fn layout(&self) -> RowLayout {
        self.layout
    }

    /// The view presets and the compositor render into.
    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    /// Fill the whole frame with one linear-space RGBA color.
    ///
    /// The M1 tracer bullet's entire "shader": brightness follows loudness. Real
    /// presets arrive in RFC-001 Step 14 and draw through [`Offscreen::view`].
    ///
    /// The frame is [`FRAME_FORMAT`], so the color is encoded to sRGB on write.
    pub fn clear(&self, gpu: &Gpu, color: [f32; 4]) {
        let [r, g, b, a] = color.map(f64::from);

        let mut encoder = gpu
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("avz clear"),
            });

        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("avz clear"),
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

    /// Copy the rendered frame back as tightly packed RGBA bytes.
    ///
    /// # Errors
    ///
    /// [`Error::Render`] if the device is lost or the buffer cannot be mapped.
    pub fn read_rgba(&self, gpu: &Gpu) -> Result<Vec<u8>> {
        let mut frame = Vec::new();
        self.read_rgba_into(gpu, &mut frame)?;
        Ok(frame)
    }

    /// [`Offscreen::read_rgba`] into a caller-owned buffer, reusing its
    /// allocation across the thousands of frames of a song.
    pub fn read_rgba_into(&self, gpu: &Gpu, frame: &mut Vec<u8>) -> Result<()> {
        let mut encoder = gpu
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("avz readback"),
            });

        encoder.copy_texture_to_buffer(
            self.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &self.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.layout.padded_bytes_per_row()),
                    rows_per_image: Some(self.layout.height()),
                },
            },
            self.texture.size(),
        );
        gpu.queue().submit([encoder.finish()]);

        let slice = self.readback.slice(..);
        let (sender, receiver) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            // A disconnected receiver means read_rgba_into already gave up.
            let _ = sender.send(result);
        });

        gpu.device()
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|err| {
                Error::Render(format!("waiting for the frame readback failed: {err}"))
            })?;

        receiver
            .recv()
            .map_err(|_| Error::Render("the frame readback never completed".to_owned()))?
            .map_err(|err| Error::Render(format!("cannot map the frame readback buffer: {err}")))?;

        // The mapped view borrows the buffer, so it must be dropped before the
        // unmap below — which the end of this statement guarantees.
        let result = slice
            .get_mapped_range()
            .map_err(|err| Error::Render(format!("cannot read the frame readback buffer: {err}")))
            .and_then(|mapped| self.layout.unpad_into(&mapped, frame));

        self.readback.unmap();
        result
    }
}
