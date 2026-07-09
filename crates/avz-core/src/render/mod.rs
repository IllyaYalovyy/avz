//! Offscreen GPU rendering.
//!
//! One code path: wgpu → Vulkan → (hardware driver | lavapipe). Presets are WGSL
//! plus a parameter schema; the compositor stacks background, visualizer, and
//! text layers with premultiplied alpha; frames are read back as RGBA
//! (`VISION.md` §5.3, §6, §7).
//!
//! Animation time is always `frame_index / fps`, never wall clock. Readback
//! row-padding (256-byte alignment) is handled in exactly one place.
//!
//! Populated by RFC-001 Steps 7, 14, 17, 18, 19, and 20.
