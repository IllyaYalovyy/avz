//! Mono PCM → [`FeatureTimeline`].
//!
//! One [`FeatureFrame`] per video frame, so the renderer can index features by
//! `frame_index` and never has to resample anything. This module owns frame
//! timing and window placement; the FFT and the pure functions that read
//! features off a magnitude spectrum live in [`super::spectrum`]. Onsets and the
//! envelope/normalization passes arrive in RFC-001 Steps 12–13 and slot into the
//! same struct.

use std::time::Duration;

use rayon::prelude::*;

use crate::analysis::DecodedAudio;
use crate::analysis::spectrum::{self, Spectrograph};
use crate::{Error, Result};

/// Nominal analysis window: 2048 samples ≈ 46 ms at 44.1 kHz (`VISION.md` §5.1).
///
/// A power of two because the FFT runs over this same window.
const NOMINAL_WINDOW: usize = 2048;

/// Every feature of one video frame, as plain floats.
///
/// Fixed-size and `Copy` on purpose: the whole struct is uploaded as a uniform
/// once per rendered frame (`VISION.md` §5.1).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FeatureFrame {
    /// Root mean square of the analysis window. Nominally `0.0..=1.0`.
    pub rms: f32,
    /// Log power, 20–150 Hz. Kick response.
    pub bass: f32,
    /// Log power, 150–500 Hz. Body and thickness.
    pub low_mid: f32,
    /// Log power, 500–2000 Hz. Detail motion, vocals-ish.
    pub mid: f32,
    /// Log power, 2–8 kHz. Sparkle, cymbals.
    pub high: f32,
    /// Log power, 8–16 kHz. Fine grain, shimmer.
    pub air: f32,
    /// Positive spectral flux against the previous frame. Zero on frame 0.
    pub flux: f32,
    /// Spectral centroid over Nyquist, `0.0..=1.0`. Zero for a silent window.
    pub centroid: f32,
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

    let spectrograph = Spectrograph::new(window);
    let bin_hz = spectrum::bin_hz(audio.sample_rate, window);

    // Every window is independent of every other, so they transform in parallel.
    // An indexed range collects in index order whatever order the threads finish
    // in, which is what keeps a re-render byte-identical (`AGENTS.md`,
    // determinism). Each worker keeps its own window buffer and FFT scratch; the
    // planned FFT itself is shared.
    let (mut frames, spectra): (Vec<FeatureFrame>, Vec<Vec<f32>>) = (0..frame_count)
        .into_par_iter()
        .map_init(
            || (vec![0.0f32; window], spectrograph.workspace()),
            |(buffer, workspace), index| {
                let center = center_sample(index, rate, u64::from(fps));
                let heard = place_window(samples, center, buffer);
                let magnitudes = spectrograph.magnitudes(buffer, workspace);

                let [bass, low_mid, mid, high, air] = spectrum::band_energies(&magnitudes, bin_hz);

                let frame = FeatureFrame {
                    rms: rms(heard),
                    bass,
                    low_mid,
                    mid,
                    high,
                    air,
                    // Flux compares neighbours, so it is filled in below.
                    flux: 0.0,
                    centroid: spectrum::spectral_centroid(&magnitudes, bin_hz),
                };

                (frame, magnitudes)
            },
        )
        .unzip();

    // Frame 0 has nothing to differ from and keeps its 0.0. The spectra live no
    // longer than this pass — a five-minute song at 30 fps is about 37 MB of
    // them — which is why they are not part of the timeline.
    for (index, pair) in spectra.windows(2).enumerate() {
        frames[index + 1].flux = spectrum::spectral_flux(&pair[0], &pair[1]);
    }

    tracing::debug!(fps, frames = frame_count, window, "built feature timeline");

    Ok(FeatureTimeline { fps, frames })
}

/// Copy the analysis window centered on `center` into `buffer`, and return the
/// samples it heard.
///
/// The window is centered on the video frame's timestamp, except within half a
/// window of either end of the song, where it slides inward to stay full. It
/// does not zero-pad: a half-empty window reads about 3 dB quiet across every
/// band and, worse, the fill from one padded window to the next full one looks
/// exactly like an onset. Sliding trades that for at most half a window of
/// timing error on the song's first and last frames — 23 ms at 44.1 kHz — and
/// keeps the same promise `rms` already makes, that a song does not fade in and
/// out at its edges.
///
/// A song shorter than one window is zero-padded, because there is nothing to
/// slide.
fn place_window<'a>(samples: &'a [f32], center: usize, buffer: &mut [f32]) -> &'a [f32] {
    let window = buffer.len();
    let lead = window / 2;

    let start = center
        .saturating_sub(lead)
        .min(samples.len().saturating_sub(window));
    let end = (start + window).min(samples.len());
    let heard = &samples[start..end];

    buffer[..heard.len()].copy_from_slice(heard);
    buffer[heard.len()..].fill(0.0);

    heard
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

    /// The five band energies of the middle frame of a `freq` Hz tone, in the
    /// band order of `VISION.md` §5.1: bass, low_mid, mid, high, air.
    ///
    /// The middle frame, because its window sits wholly inside the signal.
    fn bands_of_a_tone(freq: f64, sample_rate: u32) -> [f32; 5] {
        let timeline = analyze(&audio(sine(freq, 0.9, 1.0, sample_rate), sample_rate), 30)
            .expect("a sine analyzes");
        let frame = timeline.frame(timeline.len() / 2);

        [frame.bass, frame.low_mid, frame.mid, frame.high, frame.air]
    }

    /// `bands[index]` must tower over every other band.
    fn assert_band_dominates(bands: [f32; 5], index: usize, factor: f32) {
        for (other, energy) in bands.iter().enumerate().filter(|(o, _)| *o != index) {
            assert!(
                bands[index] > energy * factor,
                "band {index} has energy {} but band {other} has {energy}; \
                 expected at least {factor}x dominance. bands: {bands:?}",
                bands[index]
            );
        }
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

    /// The canonical band-mapping test (`docs/TESTING.md` risk matrix): a kick
    /// drum's fundamental must reach the band presets route to kick response and
    /// nothing else. Checked at both common sample rates, because the bin → band
    /// edges are computed from the rate and an implementation that hard-coded
    /// 44.1 kHz would still pass at 44.1 kHz.
    #[test]
    fn sine_at_60hz_lights_up_bass_band_only() {
        for rate in [44_100, 48_000] {
            assert_band_dominates(bands_of_a_tone(60.0, rate), 0, 5.0);
        }
    }

    #[test]
    fn sine_at_1khz_dominates_mid() {
        for rate in [44_100, 48_000] {
            assert_band_dominates(bands_of_a_tone(1_000.0, rate), 2, 5.0);
        }
    }

    #[test]
    fn sine_at_12khz_dominates_air() {
        for rate in [44_100, 48_000] {
            assert_band_dominates(bands_of_a_tone(12_000.0, rate), 4, 5.0);
        }
    }

    /// Bands are sums over disjoint bin ranges, so two tones in two bands must
    /// light both — an implementation that reported only the loudest band, or
    /// that leaked one tone across every band, fails here.
    #[test]
    fn two_tone_signal_lights_both_bands() {
        let mut samples = sine(60.0, 0.45, 1.0, RATE);
        for (sample, high) in samples.iter_mut().zip(sine(5_000.0, 0.45, 1.0, RATE)) {
            *sample += high;
        }

        let timeline = analyze(&audio(samples, RATE), 30).expect("two tones analyze");
        let frame = timeline.frame(15);

        for quiet in [frame.low_mid, frame.mid, frame.air] {
            assert!(
                frame.bass > quiet * 5.0 && frame.high > quiet * 5.0,
                "bass {} and high {} should both tower over {quiet}",
                frame.bass,
                frame.high
            );
        }
    }

    /// Flux measures spectral *change*. A tone that never changes has none, and
    /// the first frame has nothing to differ from.
    #[test]
    fn steady_tone_has_near_zero_flux() {
        let timeline =
            analyze(&audio(sine(1_000.0, 0.9, 2.0, RATE), RATE), 30).expect("a sine analyzes");

        assert_eq!(
            timeline.frame(0).flux,
            0.0,
            "the first frame has no history"
        );
        for (index, frame) in timeline.frames.iter().enumerate() {
            // Against a switch spike of ~5.8 (see the test below), and an
            // observed steady-state flux of ~1e-4: this is a wide margin around
            // the FFT's own numerical noise, not a tuned constant.
            assert!(
                frame.flux < 0.01,
                "frame {index} of a steady tone has flux {}",
                frame.flux
            );
        }
    }

    /// The onset signal (#12) is built on this: when the spectrum changes, flux
    /// must spike on the frame that saw the change and stay quiet elsewhere.
    #[test]
    fn tone_switch_spikes_flux_at_switch_frame() {
        let mut samples = sine(200.0, 0.9, 1.0, RATE);
        samples.extend(sine(4_000.0, 0.9, 1.0, RATE));
        let timeline = analyze(&audio(samples, RATE), 30).expect("analyzes");

        // The switch is one second in: frame 30, whose window straddles it.
        let (spike_at, spike) = timeline
            .frames
            .iter()
            .enumerate()
            .map(|(index, frame)| (index, frame.flux))
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .expect("frames exist");
        let steady = timeline
            .frames
            .iter()
            .enumerate()
            .filter(|(index, _)| index.abs_diff(30) > 2)
            .map(|(_, frame)| frame.flux)
            .fold(0.0f32, f32::max);

        assert!(
            spike_at.abs_diff(30) <= 1,
            "the loudest flux is at frame {spike_at}, not at the switch"
        );
        assert!(
            spike > steady * 10.0,
            "flux spiked to {spike} at the switch but reaches {steady} on a steady tone"
        );
    }

    /// Centroid is the magnitude-weighted mean frequency over Nyquist, so a
    /// bright tone must read higher than a dark one — and both must stay in
    /// `0..=1` regardless of sample rate.
    #[test]
    fn centroid_higher_for_higher_tone() {
        for rate in [44_100, 48_000] {
            let dark = analyze(&audio(sine(200.0, 0.9, 1.0, rate), rate), 30).expect("analyzes");
            let bright =
                analyze(&audio(sine(8_000.0, 0.9, 1.0, rate), rate), 30).expect("analyzes");

            let dark = dark.frame(15).centroid;
            let bright = bright.frame(15).centroid;

            assert!(
                bright > dark * 5.0,
                "dark {dark}, bright {bright} at {rate} Hz"
            );
            assert!((0.0..=1.0).contains(&dark) && (0.0..=1.0).contains(&bright));
        }
    }

    /// An all-zero spectrum has no mean frequency. Dividing by its zero total
    /// magnitude would put a `NaN` in a uniform and paint a frame black — or
    /// worse, propagate through the envelopes in #13.
    #[test]
    fn silence_centroid_is_zero_not_nan() {
        let timeline = analyze(&audio(silence(1.0, RATE), RATE), 30).expect("silence analyzes");

        for frame in &timeline.frames {
            assert_eq!(frame.centroid, 0.0);
        }
    }

    #[test]
    fn silence_has_no_nans_in_any_feature() {
        let timeline = analyze(&audio(silence(1.0, RATE), RATE), 30).expect("silence analyzes");

        for frame in &timeline.frames {
            for value in [
                frame.rms,
                frame.bass,
                frame.low_mid,
                frame.mid,
                frame.high,
                frame.air,
                frame.flux,
                frame.centroid,
            ] {
                assert!(value.is_finite(), "{frame:?}");
                assert_eq!(value, 0.0, "silence is not silent: {frame:?}");
            }
        }
    }

    /// The windows are analyzed in parallel. A reduction whose result depended
    /// on thread scheduling would make every render irreproducible
    /// (`AGENTS.md`, determinism).
    #[test]
    fn the_same_song_analyzes_to_the_same_timeline_twice() {
        let mut samples = sine(60.0, 0.7, 1.0, RATE);
        samples.extend(sine(3_000.0, 0.4, 1.0, RATE));
        let audio = audio(samples, RATE);

        let once = analyze(&audio, 30).expect("analyzes");
        let twice = analyze(&audio, 30).expect("analyzes");

        assert_eq!(once, twice);
    }

    /// The edge windows slide inward instead of zero-padding, so a song that
    /// starts at full volume reads at full volume on frame 0. A padded window
    /// would read about 3 dB quiet and then "grow" into the next frame — an
    /// onset the music never played.
    #[test]
    fn the_first_and_last_frames_read_as_loud_as_the_middle_of_the_song() {
        let timeline =
            analyze(&audio(sine(60.0, 0.9, 1.0, RATE), RATE), 30).expect("a sine analyzes");

        let middle = timeline.frame(15).bass;
        let first = timeline.frame(0).bass;
        let last = timeline.frame(timeline.len() - 1).bass;

        for (name, edge) in [("first", first), ("last", last)] {
            assert!(
                (edge - middle).abs() < middle * 0.05,
                "the {name} frame reads {edge} against {middle} mid-song"
            );
        }
    }

    /// A song shorter than one analysis window has nothing to slide toward, so
    /// its single window is zero-padded. It must still analyze rather than panic
    /// on a short slice.
    #[test]
    fn a_song_shorter_than_one_window_still_analyzes() {
        let timeline =
            analyze(&audio(sine(1_000.0, 0.9, 0.01, RATE), RATE), 30).expect("a short song");

        assert!(!timeline.is_empty());
        assert!(
            timeline
                .frames
                .iter()
                .all(|f| f.mid.is_finite() && f.mid > 0.0)
        );
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

    /// The fixture through the real mp3 decoder, not a synthesized array: a
    /// 60 Hz kick every half second, decaying under a steady 1 kHz tone
    /// (RFC-001 Q1). It is the one signal here that has been through a lossy
    /// codec, so it is where an implementation that only survives clean sines
    /// would come apart.
    ///
    /// Two things must be true of it, and they are the two things M2 exists for:
    /// the kick is separable from the tone, and flux marks the beat.
    #[test]
    fn the_fixtures_kick_separates_from_its_tone_and_spikes_flux_on_the_beat() {
        let decoded = crate::analysis::decode(fixture("tone-tagged.mp3")).expect("fixture decodes");
        let timeline = analyze(&decoded, 30).expect("the fixture analyzes");

        // At 30 fps a kick every half second lands on every 15th frame.
        let kicks = (15..timeline.len()).step_by(15);

        for kick in kicks {
            assert!(
                timeline.frame(kick).flux > 0.5,
                "no onset at the kick on frame {kick}: flux {}",
                timeline.frame(kick).flux
            );

            // Struck, the kick buries the tone; fourteen frames later it has
            // decayed below it. A band mapping that mixed the two would show
            // neither crossing.
            let struck = timeline.frame(kick);
            let decayed = timeline.frame(kick + 14);
            assert!(
                struck.bass > struck.mid * 5.0,
                "frame {kick}: bass {} does not tower over mid {}",
                struck.bass,
                struck.mid
            );
            assert!(
                decayed.bass < decayed.mid,
                "frame {}: bass {} has not decayed below mid {}",
                kick + 14,
                decayed.bass,
                decayed.mid
            );
        }

        // Between the kicks the bass is only ever fading, and fading energy is
        // not an onset.
        for frame in (5..15).chain(20..30) {
            assert!(
                timeline.frame(frame).flux < 0.01,
                "frame {frame} decays but reports flux {}",
                timeline.frame(frame).flux
            );
        }
    }
}
