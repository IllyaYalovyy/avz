//! Offscreen GPU rendering.
//!
//! One code path: wgpu → Vulkan → (hardware driver | lavapipe). Presets are WGSL
//! plus a parameter schema; the compositor stacks background, visualizer, and
//! text layers with premultiplied alpha; frames are read back as RGBA
//! (`VISION.md` §5.3, §6, §7).
//!
//! A render owns one [`Offscreen`] frame, one [`Layer`] per layer that exists,
//! and one [`Compositor`] that flattens them into it. A preset draws light and
//! coverage into the visualizer layer and never sees what is beneath it; the
//! [`Background`] under it is the palette [`Backdrop`], optionally with a fitted,
//! blurred, and darkened image over it, rather than the black the presets used to
//! paint for themselves. The [`TextCard`] on top of it is rasterized once and
//! then only animated.
//!
//! A preset draws against the uniform and, if its schema asks, against the three
//! optional textures `VISION.md` §6 allows it: the previous frame
//! ([`Feedback`]), this frame's coarse spectrum ([`Spectrum`]), and the song's
//! recent hits ([`OnsetHistory`]).
//!
//! Animation time is always `frame_index / fps`, never wall clock. Readback
//! row-padding (256-byte alignment) is handled in exactly one place:
//! [`readback::RowLayout`].
//!
//! Populated by RFC-001 Steps 7, 14, 17, 18, 19, and 20.

pub mod adapter;
pub mod background;
pub mod compositor;
pub mod feedback;
pub mod globals;
pub mod layer;
pub mod offscreen;
pub mod onsets;
pub mod palette;
pub mod preset;
pub mod readback;
pub mod schema;
pub mod spectrum;
pub mod text;

pub use adapter::{AdapterChoice, AdapterKind};
pub use background::{Backdrop, Background};
pub use compositor::Compositor;
pub use feedback::Feedback;
pub use globals::{GLOBALS_SIZE, Globals, PALETTE_SLOTS, PARAM_SLOTS};
pub use layer::Layer;
pub use offscreen::{FRAME_FORMAT, Gpu, Offscreen};
pub use onsets::OnsetHistory;
pub use palette::{BUILT_INS, BuiltIn, LinearPalette};
pub use preset::{PRESETS, Preset, Visualizer};
pub use readback::RowLayout;
pub use schema::{PackedParams, Param, ParamKind, PresetSchema, SLOT_COMPONENTS, Slot};
pub use spectrum::Spectrum;
pub use text::{Card, CardText, TextCard};
