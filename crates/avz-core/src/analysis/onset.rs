//! Spectral flux → onsets.
//!
//! An onset is a discrete hit: the frame a note, kick, or cymbal *arrives* on.
//! Presets spend them on flashes, spawns, and direction changes (`VISION.md`
//! §5.1). Two channels come out of this module:
//!
//! - a binary train, `hits[i]`, which is what the DSP tests assert on, and
//! - a decaying impulse, `impulse[i]`, which is what reaches the shader.
//!
//! The impulse is computed here rather than in WGSL because a shader sees one
//! frame at a time and would have to carry state across draws to decay anything
//! (`VISION.md` §6: the `onset` uniform is "1.0 at onset, exp decay").
//!
//! Everything here is a pure function of a flux track and an `fps`, so the
//! detection can be checked against flux written by hand (`docs/TESTING.md`).

use std::time::Duration;

/// How many median absolute deviations above the local median flux must rise
/// before a frame counts as a hit.
///
/// 2.5 is where RFC-001 Step 12 starts; the reference-track listening pass is
/// what moves it. Raising it misses soft hits, lowering it fires on texture.
pub const DEFAULT_K: f32 = 2.5;

/// Half-width of the centered window the median and MAD are taken over: the
/// "±1 s" of `VISION.md` §5.1.
///
/// Wide enough that a bar of music sets the threshold, narrow enough that a
/// quiet verse does not inherit the chorus's.
pub const DEFAULT_HALF_WINDOW: Duration = Duration::from_secs(1);

/// The shortest gap between two onsets.
///
/// One physical hit smears across the analysis windows that overlap it, so
/// without a refractory period a single snare reads as two or three. 100 ms is
/// below the shortest gap a listener hears as separate hits and above the smear.
pub const DEFAULT_REFRACTORY: Duration = Duration::from_millis(100);

/// Time constant of the impulse's exponential decay: the impulse falls to `1/e`
/// this long after a hit.
pub const DEFAULT_DECAY: Duration = Duration::from_millis(150);

/// The absolute flux a hit must clear on top of the local median and MAD.
///
/// Without it, detection collapses wherever the MAD is near zero — digital
/// silence, a held chord, a steady noise floor — because `median + k·MAD` is
/// then barely above the median and the loudest frame of the FFT's own
/// numerical noise clears it. Under a Gaussian noise floor `median + 2.5·MAD`
/// sits near the 91st percentile, so roughly one frame in eleven would "onset".
///
/// The scale is meaningful because magnitudes are amplitude-normalized (a
/// full-scale sine reads 1.0 at its bin) and flux sums them: an impulse of
/// amplitude `a` produces a flux near `2a` regardless of window length. So 0.05
/// gates out anything below roughly a 0.025-amplitude transient, about
/// −32 dBFS. Measured against it: a steady 1 kHz tone peaks at 8e-5 of flux, and
/// a click at a tenth of full scale reaches 0.51.
pub const DEFAULT_NOISE_FLOOR: f32 = 0.05;

/// How many past hits a preset can see at once
/// ([`FeatureTimeline::onset_history`](crate::analysis::FeatureTimeline::onset_history)).
///
/// The uniform's `onset` is one number about *this* frame, which is enough to
/// flash on the beat and nothing more. A preset that spawns something on a hit
/// and then lets it live — a particle burst, an expanding ring — must know when
/// the hits it is still drawing happened, because a fragment shader carries no
/// state between frames and re-derives the whole picture from scratch every
/// time.
///
/// 64 is generous on purpose. [`DEFAULT_REFRACTORY`] caps hits at ten a second,
/// so 64 slots hold at least the last 6.4 seconds of them — longer than any
/// lifetime a burst preset has business animating, and long enough that the
/// window never becomes the thing a preset author has to reason about.
pub const ONSET_SLOTS: usize = 64;

/// The birth time an unfilled [`ONSET_SLOTS`] slot carries.
///
/// A slot is empty at the start of a song, before enough hits have landed to
/// fill the window. Rather than a flag a shader has to test, the slot reports a
/// hit a thousand seconds before the song began: every age computed from it is
/// enormous, so every lifetime test a preset already writes rejects it. Not
/// `-inf`, which would make `time - birth` an infinity that `min`, `clamp`, and
/// `exp` each round differently on different drivers.
pub const NO_ONSET: f32 = -1000.0;

/// The ordinal an unfilled [`ONSET_SLOTS`] slot carries: no hit has this index.
pub const NO_ORDINAL: f32 = -1.0;

/// The recent-hit window a preset reads, as the renderer uploads it.
///
/// One `(birth, ordinal)` pair per slot, newest first: `[0]` and `[1]` are the
/// most recent hit's timestamp in seconds and its 0-based index in the song's
/// hits, `[2]` and `[3]` the hit before it, and so on. Unfilled slots hold
/// [`NO_ONSET`] and [`NO_ORDINAL`].
///
/// **Why the ordinal is in here.** A slot is a position in a sliding window, so
/// slot 3 names a different hit on the frame after a new one lands. A preset
/// that hashed the slot index would tear every particle of every live burst
/// across to a new position on every hit. The ordinal names the hit itself and
/// never moves, so it is what a burst's seeded hashes key on.
pub type OnsetHistory = [f32; ONSET_SLOTS * 2];

/// The window a song hands a preset before its first hit lands: every slot
/// empty. Also what the renderer fills the texture with before the first upload.
pub const EMPTY_HISTORY: OnsetHistory = {
    let mut history = [0.0; ONSET_SLOTS * 2];
    let mut slot = 0;
    while slot < ONSET_SLOTS {
        history[slot * 2] = NO_ONSET;
        history[slot * 2 + 1] = NO_ORDINAL;
        slot += 1;
    }
    history
};

/// The knobs of [`detect`]. Constants now; RFC-001 M3 plumbs them to config.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OnsetParams {
    /// Median absolute deviations above the local median. See [`DEFAULT_K`].
    pub k: f32,
    /// Half-width of the centered threshold window. See [`DEFAULT_HALF_WINDOW`].
    pub half_window: Duration,
    /// Shortest gap between onsets. See [`DEFAULT_REFRACTORY`].
    pub refractory: Duration,
    /// Impulse decay time constant. See [`DEFAULT_DECAY`].
    pub decay: Duration,
    /// Absolute flux gate. See [`DEFAULT_NOISE_FLOOR`].
    pub noise_floor: f32,
}

impl Default for OnsetParams {
    fn default() -> Self {
        Self {
            k: DEFAULT_K,
            half_window: DEFAULT_HALF_WINDOW,
            refractory: DEFAULT_REFRACTORY,
            decay: DEFAULT_DECAY,
            noise_floor: DEFAULT_NOISE_FLOOR,
        }
    }
}

/// The two onset channels of a whole song, one entry per video frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Onsets {
    /// `true` on the frames a hit was detected on.
    pub hits: Vec<bool>,
    /// 1.0 on a hit, decaying exponentially toward zero afterwards.
    pub impulse: Vec<f32>,
}

/// Find the hits in `flux` and decay an impulse from each one.
///
/// A frame is a hit when its flux is a local peak, clears
/// `median + k·MAD + noise_floor` of the flux in a centered window around it,
/// and no other hit landed within `refractory` of it. The window is clamped —
/// not padded — at the ends of the song, so the first and last second are
/// thresholded against the neighbours they actually have.
///
/// An empty flux track, or an `fps` of zero, yields no onsets rather than a
/// panic.
pub fn detect(flux: &[f32], fps: u32, params: OnsetParams) -> Onsets {
    let count = flux.len();
    let mut hits = vec![false; count];
    let mut impulse = vec![0.0f32; count];

    if count == 0 || fps == 0 {
        return Onsets { hits, impulse };
    }

    let half_window = frames_in(params.half_window, fps);
    // At least one frame, or a hit could re-trigger on the frame after itself.
    let refractory = frames_in(params.refractory, fps).max(1);
    let decay = decay_per_frame(params.decay, fps);

    // Only a struck frame may read exactly 1.0, which is what lets
    // `FeatureTimeline::is_onset` recover the binary train from the impulse.
    debug_assert!(decay < 1.0, "the impulse must strictly decay");

    let mut window = Vec::with_capacity(2 * half_window + 1);
    let mut last_hit: Option<usize> = None;
    let mut previous = 0.0f32;

    for index in 0..count {
        let low = index.saturating_sub(half_window);
        let high = (index + half_window + 1).min(count);
        window.clear();
        window.extend_from_slice(&flux[low..high]);

        let threshold = median_plus_k_mad(&mut window, params.k) + params.noise_floor;
        let rested = last_hit.is_none_or(|last| index - last >= refractory);
        let hit = flux[index] > threshold && is_local_peak(flux, index) && rested;

        if hit {
            last_hit = Some(index);
            previous = 1.0;
        } else {
            previous *= decay;
        }

        hits[index] = hit;
        impulse[index] = previous;
    }

    Onsets { hits, impulse }
}

/// `median + k · MAD` of `window`, where MAD is the median absolute deviation
/// from that median. Reorders `window` in place.
///
/// Median and MAD rather than mean and standard deviation because the window is
/// mostly non-onset frames plus a few enormous ones, and a mean threshold is
/// dragged up by the very hits it is supposed to find.
fn median_plus_k_mad(window: &mut [f32], k: f32) -> f32 {
    let centre = median(window);

    for value in window.iter_mut() {
        *value = (*value - centre).abs();
    }

    centre + k * median(window)
}

/// The upper median of `values`, which is the true median for the odd-length
/// windows every frame but the song's edges sees. Reorders `values`.
///
/// `total_cmp` gives a total order over floats, so the result does not depend on
/// the input order — the same song must analyze to the same timeline twice
/// (`AGENTS.md`, determinism).
fn median(values: &mut [f32]) -> f32 {
    if values.is_empty() {
        return 0.0;
    }

    let middle = values.len() / 2;
    *values.select_nth_unstable_by(middle, f32::total_cmp).1
}

/// Whether `flux[index]` is at least as large as both its neighbours.
///
/// A hit belongs on the frame the energy peaked, not the frame it started
/// rising on: without this the onset fires early and, worse, the refractory
/// period then masks the actual peak. Frames off either end of the song count as
/// silent, so the first and last frame can each still be a peak.
fn is_local_peak(flux: &[f32], index: usize) -> bool {
    let over_previous = index == 0 || flux[index] >= flux[index - 1];
    let over_next = index + 1 == flux.len() || flux[index] >= flux[index + 1];

    over_previous && over_next
}

/// How many video frames `duration` spans at `fps`, rounded to the nearest.
fn frames_in(duration: Duration, fps: u32) -> usize {
    (duration.as_secs_f64() * f64::from(fps)).round() as usize
}

/// The per-frame factor that decays an impulse to `1/e` after `tau`: with a
/// frame lasting `1/fps` seconds, that is `exp(-1 / (tau · fps))`.
///
/// Derived from a time constant rather than given as a factor so the visual
/// decay lasts the same number of milliseconds at every `fps`.
fn decay_per_frame(tau: Duration, fps: u32) -> f32 {
    let seconds = tau.as_secs_f64();
    if seconds <= 0.0 {
        return 0.0;
    }

    (-1.0 / (seconds * f64::from(fps))).exp() as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    const FPS: u32 = 30;

    /// A flat flux track of `level`, `frames` long, with a spike of `peak` at
    /// each index in `spikes`.
    fn flux(frames: usize, level: f32, spikes: &[usize], peak: f32) -> Vec<f32> {
        let mut track = vec![level; frames];
        for &index in spikes {
            track[index] = peak;
        }
        track
    }

    fn hits_of(track: &[f32]) -> Vec<usize> {
        detect(track, FPS, OnsetParams::default())
            .hits
            .iter()
            .enumerate()
            .filter(|&(_, &hit)| hit)
            .map(|(index, _)| index)
            .collect()
    }

    /// The definition, on the smallest signal that carries it: a spike that
    /// clears the local median by more than `k` MADs is a hit, and its
    /// neighbours are not.
    #[test]
    fn a_spike_above_the_local_median_and_mad_is_a_hit() {
        assert_eq!(hits_of(&flux(120, 0.1, &[40, 80], 5.0)), vec![40, 80]);
    }

    /// A track with no variation has a MAD of zero, so `median + k·MAD` is the
    /// median itself. Only the noise floor stands between that and an onset on
    /// every frame.
    #[test]
    fn a_flat_flux_track_has_no_onsets() {
        assert!(hits_of(&flux(120, 0.1, &[], 0.0)).is_empty());
        assert!(hits_of(&flux(120, 0.0, &[], 0.0)).is_empty());
        assert!(hits_of(&flux(120, 100.0, &[], 0.0)).is_empty());
    }

    /// A bump that clears the local statistics but not the absolute floor is
    /// texture, not a hit. The FFT's own numerical noise lives down here.
    #[test]
    fn a_bump_below_the_noise_floor_is_not_a_hit() {
        let just_under = DEFAULT_NOISE_FLOOR * 0.9;
        assert!(hits_of(&flux(120, 0.0, &[60], just_under)).is_empty());

        let just_over = DEFAULT_NOISE_FLOOR * 1.1;
        assert_eq!(hits_of(&flux(120, 0.0, &[60], just_over)), vec![60]);
    }

    /// The threshold is local, which is the whole point of the adaptive window:
    /// the same spike is a hit in a quiet passage and noise in a loud one.
    #[test]
    fn the_threshold_follows_the_passage_it_sits_in() {
        let mut track = vec![0.0f32; 300];
        // A dense first half whose flux swings around 2.0, then a quiet second.
        for (index, value) in track[..150].iter_mut().enumerate() {
            *value = if index % 2 == 0 { 1.0 } else { 3.0 };
        }
        track[40] = 4.0;
        track[220] = 4.0;

        let hits = hits_of(&track);

        assert!(
            !hits.contains(&40),
            "4.0 is unremarkable among flux that swings to 3.0: {hits:?}"
        );
        assert!(
            hits.contains(&220),
            "4.0 towers over a silent passage: {hits:?}"
        );
    }

    /// Two triggers a frame apart are one physical hit smeared across two
    /// analysis windows. At 30 fps the 100 ms refractory period spans 3 frames.
    #[test]
    fn refractory_period_merges_double_triggers() {
        let mut track = vec![0.0f32; 120];
        track[60] = 5.0;
        track[62] = 4.0;

        assert_eq!(hits_of(&track), vec![60]);
    }

    /// Far enough apart, they are two hits. The boundary is exactly the
    /// refractory period, so 3 frames at 30 fps is a second onset and 2 is not.
    #[test]
    fn hits_a_refractory_period_apart_are_both_kept() {
        let mut track = vec![0.0f32; 120];
        track[60] = 5.0;
        track[63] = 4.0;

        assert_eq!(hits_of(&track), vec![60, 63]);
    }

    /// The energy of a hit builds over a window or two. The onset belongs on the
    /// peak: firing on the rising edge would be early, and the refractory period
    /// would then swallow the peak itself.
    #[test]
    fn a_hit_lands_on_the_peak_not_the_rising_edge() {
        let mut track = vec![0.0f32; 120];
        track[59] = 1.0;
        track[60] = 5.0;
        track[61] = 2.0;

        assert_eq!(hits_of(&track), vec![60]);
    }

    /// `exp(-t/τ)`, sampled at the frames. Checked in *time*, not in frames, so
    /// the same hit fades over the same 150 ms whatever the fps.
    #[test]
    fn onset_impulse_decays_exponentially() {
        for fps in [30u32, 60] {
            let track = flux(4 * fps as usize, 0.0, &[fps as usize], 5.0);
            let onsets = detect(&track, fps, OnsetParams::default());
            let hit = fps as usize;

            assert_eq!(onsets.impulse[hit], 1.0, "the struck frame is 1.0");

            let tau = DEFAULT_DECAY.as_secs_f32();
            for ahead in 1..=fps as usize {
                let elapsed = ahead as f32 / fps as f32;
                let expected = (-elapsed / tau).exp();
                let got = onsets.impulse[hit + ahead];

                assert!(
                    (got - expected).abs() <= expected.max(1e-6) * 0.05,
                    "{fps} fps, {ahead} frames after the hit: {got} against {expected}"
                );
            }

            // One time constant on, the impulse has fallen to 1/e. Unless
            // `τ · fps` is a whole number the grid straddles that instant, so
            // the crossing is pinned between the frames either side of it.
            let exact = tau * fps as f32;
            let inverse_e = std::f32::consts::E.recip();
            let before = onsets.impulse[hit + exact.floor() as usize];
            let after = onsets.impulse[hit + exact.ceil() as usize];

            assert!(
                before + 1e-5 >= inverse_e && after <= inverse_e + 1e-5,
                "{fps} fps: 1/e should fall between {before} and {after}"
            );
        }
    }

    /// The impulse restarts at full scale on every hit rather than accumulating,
    /// so a shader can read it as "how recently was there a hit".
    #[test]
    fn every_hit_restarts_the_impulse_at_one() {
        let onsets = detect(
            &flux(120, 0.0, &[30, 60, 90], 5.0),
            FPS,
            OnsetParams::default(),
        );

        for hit in [30, 60, 90] {
            assert_eq!(onsets.impulse[hit], 1.0);
        }
    }

    /// The impulse is the binary train plus a decay, and the decay is strictly
    /// shrinking — so a frame reads exactly 1.0 if and only if it was struck.
    /// `FeatureTimeline::is_onset` depends on this.
    #[test]
    fn only_a_struck_frame_reads_a_full_impulse() {
        let onsets = detect(
            &flux(200, 0.02, &[30, 33, 90], 5.0),
            FPS,
            OnsetParams::default(),
        );

        for (index, (&hit, &impulse)) in onsets.hits.iter().zip(&onsets.impulse).enumerate() {
            assert_eq!(hit, impulse >= 1.0, "frame {index}: impulse {impulse}");
        }
    }

    /// The first and last second have no full window around them. Clamping, not
    /// padding: a padded window would drag the median toward zero and turn the
    /// song's opening chord into an onset.
    #[test]
    fn the_threshold_window_clamps_at_the_song_edges() {
        // Shorter than a single half-window at 30 fps.
        let hits = hits_of(&flux(10, 0.0, &[0, 9], 5.0));

        assert_eq!(hits, vec![0, 9], "a hit on the very first or last frame");
    }

    #[test]
    fn an_empty_flux_track_has_no_onsets() {
        let onsets = detect(&[], FPS, OnsetParams::default());

        assert!(onsets.hits.is_empty());
        assert!(onsets.impulse.is_empty());
    }

    #[test]
    fn a_zero_fps_track_has_no_onsets_rather_than_a_division_by_zero() {
        let onsets = detect(&flux(120, 0.0, &[60], 5.0), 0, OnsetParams::default());

        assert!(onsets.hits.iter().all(|&hit| !hit));
        assert!(onsets.impulse.iter().all(|&value| value == 0.0));
    }

    #[test]
    fn a_median_is_the_middle_value_however_the_window_arrived() {
        assert_eq!(median(&mut [3.0, 1.0, 2.0]), 2.0);
        assert_eq!(median(&mut [1.0, 2.0, 3.0]), 2.0);
        assert_eq!(median(&mut [100.0, 0.0, 0.0, 0.0, 0.0]), 0.0);
        assert_eq!(median(&mut []), 0.0);
    }

    /// A window of mostly-zero flux with a few enormous spikes has a median of
    /// zero and a MAD of zero — a mean and standard deviation would both be
    /// dragged up by the spikes they exist to find.
    #[test]
    fn the_median_and_mad_ignore_the_spikes_they_are_measuring() {
        let mut window = [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 9.0, 9.0];

        assert_eq!(median_plus_k_mad(&mut window, 2.5), 0.0);
    }

    #[test]
    fn a_hundred_millisecond_refractory_is_three_frames_at_thirty_fps() {
        assert_eq!(frames_in(DEFAULT_REFRACTORY, 30), 3);
        assert_eq!(frames_in(DEFAULT_REFRACTORY, 60), 6);
        assert_eq!(frames_in(DEFAULT_HALF_WINDOW, 30), 30);
    }

    /// A time constant of zero kills the impulse the frame after the hit rather
    /// than dividing by zero.
    #[test]
    fn a_zero_decay_constant_is_an_instant_decay_not_a_division_by_zero() {
        let params = OnsetParams {
            decay: Duration::ZERO,
            ..OnsetParams::default()
        };
        let onsets = detect(&flux(120, 0.0, &[60], 5.0), FPS, params);

        assert_eq!(onsets.impulse[60], 1.0);
        assert_eq!(onsets.impulse[61], 0.0);
    }
}
