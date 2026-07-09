//! Decode → `FeatureTimeline`.
//!
//! Windowed FFT over mono-mixed PCM, producing one feature frame per video
//! frame: band energies, RMS, spectral flux, onsets, centroid. This pass runs to
//! completion before rendering starts, which is what buys lookahead and global
//! normalization (`VISION.md` §5.1).
//!
//! Populated by RFC-001 Steps 5, 6, 11, 12, and 13.

pub mod decode;
pub mod features;

pub use decode::{DecodedAudio, decode};
pub use features::{FeatureFrame, FeatureTimeline, analyze};
