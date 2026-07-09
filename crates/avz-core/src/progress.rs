//! Progress reporting, as a callback trait.
//!
//! `avz-core` must not print. Long-running phases call into a [`Progress`]
//! implementation supplied by the caller; `avz-cli` renders that as an
//! `indicatif` bar, and a future GUI would render it as a widget.

/// The phases a render moves through, in order (`VISION.md` §8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Phase {
    /// Decoding audio and building the feature timeline.
    Analyzing,
    /// Rendering frames on the GPU and piping them to the encoder.
    Rendering,
    /// Waiting on ffmpeg to flush and finalize the container.
    Finalizing,
}

impl Phase {
    /// A lowercase label suitable for a progress bar or log line.
    pub fn label(self) -> &'static str {
        match self {
            Phase::Analyzing => "analyzing",
            Phase::Rendering => "rendering",
            Phase::Finalizing => "finalizing",
        }
    }
}

/// Callbacks the pipeline invokes as work proceeds.
///
/// Implementations must be cheap and must not block the pipeline. They may be
/// called from worker threads, hence `Send + Sync`.
pub trait Progress: Send + Sync {
    /// A phase began. `total` is the unit count when known (frames, windows).
    fn phase_started(&self, phase: Phase, total: Option<u64>);

    /// `units` more units of the current phase completed.
    fn advance(&self, phase: Phase, units: u64);

    /// A phase completed.
    fn phase_finished(&self, phase: Phase);

    /// An actionable, non-fatal warning the user should see.
    fn warn(&self, message: &str);
}

/// A [`Progress`] that discards everything. Useful in tests and library callers
/// that do not care.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopProgress;

impl Progress for NoopProgress {
    fn phase_started(&self, _phase: Phase, _total: Option<u64>) {}
    fn advance(&self, _phase: Phase, _units: u64) {}
    fn phase_finished(&self, _phase: Phase) {}
    fn warn(&self, _message: &str) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop_progress_is_usable_as_a_trait_object() {
        let progress: &dyn Progress = &NoopProgress;
        progress.phase_started(Phase::Analyzing, Some(10));
        progress.advance(Phase::Analyzing, 1);
        progress.phase_finished(Phase::Analyzing);
        progress.warn("nothing to see here");
    }

    #[test]
    fn phase_labels_are_stable() {
        assert_eq!(Phase::Analyzing.label(), "analyzing");
        assert_eq!(Phase::Rendering.label(), "rendering");
        assert_eq!(Phase::Finalizing.label(), "finalizing");
    }
}
