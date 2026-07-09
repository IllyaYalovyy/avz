//! Typed errors for `avz-core`.
//!
//! `anyhow` stays in `avz-cli`; nothing here erases the error type. The CLI maps
//! these variants onto the process exit codes in `VISION.md` §8, so each variant
//! must stay classifiable as "the user's arguments", "the user's input file", or
//! "the pipeline broke".

use std::result;

/// Convenience alias for fallible core operations.
pub type Result<T, E = Error> = result::Result<T, E>;

/// Everything `avz-core` can fail with.
///
/// Deliberately exhaustive: `avz-cli` matches every variant to pick an exit
/// code, so adding one here is a compile error until it has been classified.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A command or code path that is planned but not yet built.
    #[error("`avz {command}` is not implemented yet")]
    NotImplemented {
        /// The user-facing command name, e.g. `render`.
        command: &'static str,
    },

    /// Bad configuration: unknown keys, out-of-range values, conflicting flags.
    #[error("invalid configuration: {0}")]
    Config(String),

    /// The input file is missing, unreadable, or not the format we expected.
    #[error("input problem: {0}")]
    Input(String),

    /// Analysis of decoded audio failed.
    #[error("analysis failed: {0}")]
    Analysis(String),

    /// GPU adapter selection, shader compilation, or frame readback failed.
    #[error("render failed: {0}")]
    Render(String),

    /// The ffmpeg subprocess is missing, died, or rejected our stream.
    #[error("encode failed: {0}")]
    Encode(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_implemented_names_the_command_it_refers_to() {
        let err = Error::NotImplemented { command: "render" };
        assert_eq!(err.to_string(), "`avz render` is not implemented yet");
    }
}
