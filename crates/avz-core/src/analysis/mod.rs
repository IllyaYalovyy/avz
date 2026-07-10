//! Decode → `FeatureTimeline`.
//!
//! Windowed FFT over mono-mixed PCM, producing one feature frame per video
//! frame: band energies, RMS, spectral flux, onsets, centroid. This pass runs to
//! completion before rendering starts, which is what buys lookahead and global
//! normalization (`VISION.md` §5.1).
//!
//! Populated by RFC-001 Steps 5, 6, 11, 12, and 13.
//!
//! The DSP is split so each half can be tested against signals whose correct
//! answer is known analytically: [`spectrum`] reads features off one magnitude
//! spectrum, [`onset`] reads hits off a whole flux track, [`envelope`] rescales
//! and smooths a whole feature track, and [`features`] owns window placement and
//! the parallel drive loop that joins them.

pub mod decode;
pub mod envelope;
pub mod features;
pub mod onset;
pub mod spectrum;

pub use decode::{DecodedAudio, decode};
pub use envelope::EnvelopeParams;
pub use features::{FeatureFrame, FeatureTimeline, analyze, analyze_with};
pub use onset::{
    EMPTY_HISTORY, NO_ONSET, NO_ORDINAL, ONSET_SLOTS, OnsetHistory, OnsetParams, Onsets,
};
pub use spectrum::{BAND_COUNT, BAND_EDGES, SPECTRUM_BINS, SPECTRUM_RANGE_HZ};
