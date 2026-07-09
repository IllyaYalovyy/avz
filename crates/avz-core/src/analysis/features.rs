//! Mono PCM → [`FeatureTimeline`].
//!
//! One [`FeatureFrame`] per video frame, so the renderer can index features by
//! `frame_index` and never has to resample anything. This module currently
//! computes a single feature — `rms` — which is enough to drive the M1 tracer
//! bullet's brightness-follows-loudness shader. Bands, spectral flux, onsets,
//! centroid, envelopes, and normalization arrive in RFC-001 Steps 11–13 and slot
//! into the same struct.

use std::time::Duration;

use crate::analysis::DecodedAudio;
use crate::{Error, Result};

/// Nominal analysis window: 2048 samples ≈ 46 ms at 44.1 kHz (`VISION.md` §5.1).
///
/// A power of two because Steps 11–13 will run an FFT over this same window.
const NOMINAL_WINDOW: usize = 2048;

/// Every feature of one video frame, as plain floats.
///
/// Fixed-size and `Copy` on purpose: the whole struct is uploaded as a uniform
/// once per rendered frame (`VISION.md` §5.1).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FeatureFrame {
    /// Root mean square of the analysis window. Nominally `0.0..=1.0`.
    pub rms: f32,
}

/// The features of a whole song, one frame per video frame.
#[derive(Debug, Clone, PartialEq)]
pub struct FeatureTimeline {
    /// Video frames per second. The frames below are spaced `1/fps` apart.
    pub fps: u32,
    /// Frame `i` describes the audio around timestamp `i / fps`.
    pub frames: Vec<FeatureFrame>,
}

impl FeatureTimeline {
    /// How many video frames the timeline covers.
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    /// Whether the timeline covers no frames at all.
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// The presentation timestamp of frame `frame_index`: `frame_index / fps`.
    ///
    /// This is the only definition of animation time in avz. Nothing derives it
    /// from a wall clock or from frame deltas (`AGENTS.md`, determinism).
    pub fn timestamp(&self, frame_index: usize) -> Duration {
        Duration::from_secs_f64(frame_index as f64 / f64::from(self.fps))
    }

    /// How long the timeline plays for: `len() / fps`.
    pub fn duration(&self) -> Duration {
        self.timestamp(self.len())
    }

    /// The features of frame `frame_index`, clamped to the last frame.
    ///
    /// The renderer's frame count is derived from the same `fps` and duration,
    /// but a caller that rounds the other way must get the final frame's
    /// features rather than a panic — a black last frame on an otherwise loud
    /// song is a harder bug to see than a slightly-repeated one.
    pub fn frame(&self, frame_index: usize) -> FeatureFrame {
        match self.frames.last() {
            Some(&last) => *self.frames.get(frame_index).unwrap_or(&last),
            None => FeatureFrame::default(),
        }
    }
}

/// Build a [`FeatureTimeline`] with one frame per video frame at `fps`.
///
/// Each frame's analysis window is centered on that video frame's exact
/// timestamp, computed from the frame index rather than accumulated hop by hop,
/// so a sample rate that is not a whole multiple of `fps` (44.1 kHz at 24 fps,
/// say) cannot drift the analysis away from the picture over a long song.
///
/// # Errors
///
/// [`Error::Analysis`] if `fps` is zero or the audio carries no samples.
pub fn analyze(audio: &DecodedAudio, fps: u32) -> Result<FeatureTimeline> {
    if fps == 0 {
        return Err(Error::Analysis("fps must be at least 1".to_owned()));
    }
    if audio.sample_rate == 0 {
        return Err(Error::Analysis(
            "decoded audio reports a sample rate of 0 Hz".to_owned(),
        ));
    }
    if audio.samples.is_empty() {
        return Err(Error::Analysis("decoded audio has no samples".to_owned()));
    }

    let samples = &audio.samples;
    let rate = u64::from(audio.sample_rate);
    let frame_count = frame_count(samples.len(), audio.sample_rate, fps);
    let window = window_len(audio.sample_rate, fps);
    let lead = window / 2;

    let frames = (0..frame_count)
        .map(|index| {
            let center = center_sample(index, rate, u64::from(fps));
            let start = center.saturating_sub(lead);
            let end = (center + (window - lead)).min(samples.len());
            FeatureFrame {
                rms: rms(&samples[start..end]),
            }
        })
        .collect();

    tracing::debug!(fps, frames = frame_count, window, "built feature timeline");

    Ok(FeatureTimeline { fps, frames })
}

/// How many video frames it takes to cover `len` samples.
///
/// Rounded up: a final partial frame still shows on screen, so it still needs
/// features.
fn frame_count(len: usize, sample_rate: u32, fps: u32) -> usize {
    let frames = (len as u64 * u64::from(fps)).div_ceil(u64::from(sample_rate));
    frames as usize
}

/// The sample index at video frame `index`'s exact timestamp, `index / fps`.
///
/// Integer math from the frame index, rounded to the nearest sample. Adding a
/// truncated hop per frame would instead shed up to one sample per frame — a
/// second of drift over a four-minute song at 44.1 kHz and 24 fps.
fn center_sample(index: usize, sample_rate: u64, fps: u64) -> usize {
    ((index as u64 * sample_rate + fps / 2) / fps) as usize
}

/// The analysis window, in samples.
///
/// [`NOMINAL_WINDOW`] normally, widened to the hop when a low `fps` spaces video
/// frames further apart than that — otherwise the audio between windows would be
/// analyzed by nobody, and a hit landing in the gap would go unseen.
fn window_len(sample_rate: u32, fps: u32) -> usize {
    let hop = u64::from(sample_rate).div_ceil(u64::from(fps)) as usize;
    NOMINAL_WINDOW.max(hop)
}

/// Root mean square of a window. Empty windows are silent, not `NaN`.
///
/// Accumulated in `f64` so the result does not depend on how many samples the
/// window happens to hold at the edges of the song.
fn rms(window: &[f32]) -> f32 {
    if window.is_empty() {
        return 0.0;
    }

    let sum: f64 = window.iter().map(|&s| f64::from(s) * f64::from(s)).sum();
    (sum / window.len() as f64).sqrt() as f32
}

#[cfg(test)]
mod tests {
    use std::f64::consts::TAU;
    use std::path::{Path, PathBuf};

    use super::*;

    const RATE: u32 = 44_100;

    fn audio(samples: Vec<f32>, sample_rate: u32) -> DecodedAudio {
        DecodedAudio {
            samples,
            sample_rate,
        }
    }

    /// `amplitude`-scaled sine of `freq` Hz, `seconds` long.
    fn sine(freq: f64, amplitude: f64, seconds: f64, sample_rate: u32) -> Vec<f32> {
        let count = (seconds * f64::from(sample_rate)) as usize;
        (0..count)
            .map(|n| (amplitude * (TAU * freq * n as f64 / f64::from(sample_rate)).sin()) as f32)
            .collect()
    }

    fn silence(seconds: f64, sample_rate: u32) -> Vec<f32> {
        vec![0.0; (seconds * f64::from(sample_rate)) as usize]
    }

    /// A committed CC0 fixture. See `assets/fixtures/README.md`.
    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/fixtures")
            .join(name)
    }

    #[test]
    fn one_feature_frame_per_video_frame() {
        let timeline = analyze(&audio(silence(5.0, RATE), RATE), 30).expect("silence analyzes");

        assert_eq!(timeline.len(), 5 * 30);
        assert_eq!(timeline.fps, 30);
    }

    /// A song that stops mid-frame still shows that frame, so it still needs
    /// features. Truncating instead of rounding up would leave the last video
    /// frame with nothing to render from.
    #[test]
    fn a_partial_final_video_frame_still_gets_a_feature_frame() {
        let one_second_and_one_sample = silence(1.0, RATE).len() + 1;
        let timeline = analyze(&audio(vec![0.0; one_second_and_one_sample], RATE), 30)
            .expect("silence analyzes");

        assert_eq!(timeline.len(), 31);
    }

    /// The hop invariant. Analysis frame `i` must sit on video frame `i`'s
    /// timestamp to within half a sample, forever — including at 24 fps, where
    /// 44.1 kHz gives a fractional hop of 1837.5 samples, and at 23 fps, where it
    /// gives 1917.39. An implementation that adds a truncated hop per frame is
    /// exact at `i = 1` and a full second late by the end of a long song.
    #[test]
    fn analysis_frames_never_drift_from_the_video_frame_clock() {
        let sweep = (0..2_000).chain([100_000, 1_000_000, 10_000_000]);

        for (rate, fps) in [
            (44_100u32, 24u32),
            (44_100, 23),
            (44_100, 25),
            (44_100, 30),
            (48_000, 30),
        ] {
            for index in sweep.clone() {
                let center = center_sample(index, u64::from(rate), u64::from(fps));

                // `center` must be the sample nearest to `index / fps` seconds.
                // Compared as exact integers, scaled by `fps` to clear the
                // fraction: the point of the invariant is that no rounding error
                // accumulates, and a float comparison would smuggle some back in.
                let want = index as i128 * i128::from(rate);
                let got = center as i128 * i128::from(fps);
                let drift = (got - want).abs();

                assert!(
                    2 * drift <= i128::from(fps),
                    "frame {index} at {rate} Hz / {fps} fps drifted {} samples \
                     from its video frame timestamp",
                    drift as f64 / f64::from(fps)
                );
            }
        }
    }

    /// A steady tone has one true RMS. Every frame must report it, including the
    /// first and last, whose windows hang off the ends of the song and must be
    /// clamped rather than zero-padded — padding would fade the song in and out.
    #[test]
    fn a_constant_sine_has_the_same_rms_on_every_frame() {
        let amplitude = 0.8;
        let timeline = analyze(&audio(sine(1_000.0, amplitude, 2.0, RATE), RATE), 30)
            .expect("a sine analyzes");

        let expected = (amplitude / f64::sqrt(2.0)) as f32;
        for (index, frame) in timeline.frames.iter().enumerate() {
            assert!(
                (frame.rms - expected).abs() < 0.02,
                "frame {index} has rms {}, expected {expected}",
                frame.rms
            );
        }
    }

    /// RMS of a constant is that constant: the one window whose answer needs no
    /// trigonometry.
    #[test]
    fn a_dc_signal_has_an_rms_equal_to_its_amplitude() {
        let timeline = analyze(&audio(vec![0.5; RATE as usize], RATE), 30).expect("dc analyzes");

        for frame in &timeline.frames {
            assert!((frame.rms - 0.5).abs() < 1e-6, "rms {}", frame.rms);
        }
    }

    #[test]
    fn silence_has_zero_rms_and_no_nans() {
        let timeline = analyze(&audio(silence(1.0, RATE), RATE), 30).expect("silence analyzes");

        assert!(timeline.frames.iter().all(|f| f.rms == 0.0));
        assert!(timeline.frames.iter().all(|f| f.rms.is_finite()));
    }

    #[test]
    fn a_loud_passage_reads_louder_than_a_quiet_one() {
        let mut samples = sine(440.0, 0.1, 1.0, RATE);
        samples.extend(sine(440.0, 0.9, 1.0, RATE));
        let timeline = analyze(&audio(samples, RATE), 30).expect("analyzes");

        let quiet = timeline.frame(15).rms;
        let loud = timeline.frame(45).rms;

        assert!(loud > quiet * 5.0, "quiet {quiet}, loud {loud}");
    }

    /// At 5 fps the hop is 8820 samples, four times the nominal window. A hit
    /// landing between two fixed 2048-sample windows would be analyzed by
    /// nobody, and the visuals would simply not see it.
    #[test]
    fn no_audio_falls_between_windows_when_the_hop_exceeds_the_window() {
        let mut samples = silence(1.0, RATE);
        // Sample 5000 sits between frame 0's centre (0) and frame 1's (8820),
        // and outside a 2048-sample window around either.
        samples[5_000..5_064].fill(1.0);

        let timeline = analyze(&audio(samples, RATE), 5).expect("analyzes");

        assert!(
            timeline.frames.iter().any(|f| f.rms > 0.0),
            "the burst at sample 5000 was never analyzed"
        );
    }

    /// The window is centered on its video frame's timestamp, so audio just
    /// before that timestamp belongs to that frame and not to the one before it.
    /// A window that started at the timestamp instead — the other natural STFT
    /// convention — would report this burst a frame early, and every visual would
    /// hit 40 ms behind the music.
    #[test]
    fn a_burst_lands_on_the_video_frame_nearest_it() {
        let fps = 24;
        // Frame 60 at 24 fps is 2.5 s, which is sample 110250 exactly. Put the
        // burst just before it: still this frame's moment, but earlier than it.
        let burst = 110_250 - 500;

        let mut samples = silence(3.0, RATE);
        samples[burst..burst + 64].fill(1.0);
        let timeline = analyze(&audio(samples, RATE), fps).expect("analyzes");

        let loud: Vec<usize> = timeline
            .frames
            .iter()
            .enumerate()
            .filter(|(_, frame)| frame.rms > 0.0)
            .map(|(index, _)| index)
            .collect();

        assert_eq!(loud, vec![60], "the burst belongs to frame 60 alone");
    }

    #[test]
    fn timestamps_derive_from_frame_index_over_fps() {
        let timeline = analyze(&audio(silence(2.0, RATE), RATE), 30).expect("analyzes");

        assert_eq!(timeline.timestamp(0), Duration::ZERO);
        assert_eq!(timeline.timestamp(30), Duration::from_secs(1));
        assert_eq!(timeline.duration(), Duration::from_secs(2));
    }

    /// A caller whose frame count rounds the other way must see the last frame,
    /// not a panic.
    #[test]
    fn a_frame_lookup_past_the_end_clamps_to_the_last_frame() {
        let timeline = analyze(&audio(vec![0.5; RATE as usize], RATE), 30).expect("analyzes");

        let last = *timeline.frames.last().expect("frames exist");
        assert_eq!(timeline.frame(timeline.len()), last);
        assert_eq!(timeline.frame(usize::MAX), last);
    }

    #[test]
    fn zero_fps_is_an_analysis_error() {
        let err = analyze(&audio(silence(1.0, RATE), RATE), 0).expect_err("0 fps has no frames");

        assert!(matches!(err, Error::Analysis(_)), "got {err:?}");
        assert!(err.to_string().contains("fps"), "{err}");
    }

    #[test]
    fn audio_without_samples_is_an_analysis_error() {
        let err = analyze(&audio(Vec::new(), RATE), 30).expect_err("no samples, no features");

        assert!(matches!(err, Error::Analysis(_)), "got {err:?}");
    }

    #[test]
    fn a_zero_sample_rate_is_an_analysis_error_rather_than_a_division_by_zero() {
        let err = analyze(&audio(vec![0.5; 100], 0), 30).expect_err("0 Hz is not a sample rate");

        assert!(matches!(err, Error::Analysis(_)), "got {err:?}");
    }

    /// End to end over the committed fixture: 5 s of a 60 Hz kick under a 1 kHz
    /// tone at 30 fps is 150 video frames, all of them audible.
    #[test]
    fn the_fixture_analyzes_to_one_frame_per_video_frame() {
        let decoded = crate::analysis::decode(fixture("tone-tagged.mp3")).expect("fixture decodes");

        let timeline = analyze(&decoded, 30).expect("the fixture analyzes");

        assert_eq!(timeline.len(), 150);
        assert!(timeline.frames.iter().all(|f| f.rms.is_finite()));
        assert!(timeline.frames.iter().all(|f| (0.0..=1.0).contains(&f.rms)));
        assert!(
            timeline.frames.iter().all(|f| f.rms > 0.01),
            "the fixture is never silent"
        );
    }
}
