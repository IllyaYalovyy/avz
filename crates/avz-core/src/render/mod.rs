//! Offscreen GPU rendering.
//!
//! One code path: wgpu → Vulkan → (hardware driver | lavapipe). Presets are WGSL
//! plus a parameter schema; the compositor stacks background, visualizer, and
//! text layers with premultiplied alpha; frames are read back as RGBA
//! (`VISION.md` §5.3, §6, §7).
//!
//! Animation time is always `frame_index / fps`, never wall clock. Readback
//! row-padding (256-byte alignment) is handled in exactly one place:
//! [`readback::RowLayout`].
//!
//! Populated by RFC-001 Steps 7, 14, 17, 18, 19, and 20.

pub mod adapter;
pub mod globals;
pub mod offscreen;
pub mod preset;
pub mod readback;
pub mod schema;

pub use adapter::{AdapterChoice, AdapterKind};
pub use globals::{GLOBALS_SIZE, Globals, PALETTE_SLOTS, PARAM_SLOTS};
pub use offscreen::{FRAME_FORMAT, Gpu, Offscreen};
pub use preset::{PRESETS, Preset, Visualizer};
pub use readback::RowLayout;
pub use schema::{PackedParams, Param, ParamKind, PresetSchema, SLOT_COMPONENTS, Slot};
