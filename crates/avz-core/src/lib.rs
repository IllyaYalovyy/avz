//! Core library for `avz`: mp3 in, abstract music-reactive video out.
//!
//! This crate is UI-agnostic on purpose. It performs no terminal I/O and knows
//! nothing about `clap`, `indicatif`, or process exit codes — progress is
//! reported through the [`Progress`] callback trait and failures surface as the
//! typed [`Error`] enum. That separation is what lets a GUI or batch
//! orchestrator sit on top later without refactoring the pipeline.
//!
//! See `VISION.md` §4 for the architecture and `AGENTS.md` for the invariants
//! that keep it intact.

#![forbid(unsafe_code)]

pub mod analysis;
pub mod config;
pub mod encode;
mod error;
pub mod meta;
pub mod pipeline;
mod progress;
pub mod render;

pub use error::{Error, Result};
pub use progress::{NoopProgress, Phase, Progress};
