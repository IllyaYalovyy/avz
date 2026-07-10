//! Global normalization and attack/decay envelope followers.
//!
//! Two pure passes over a whole feature track, in this order:
//!
//! 1. [`normalize`] maps the track's own p5 → 0 and p95 → 1 and clamps, so a
//!    quiet folk record and a wall-of-sound master both hand the presets the
//!    full visual range (`VISION.md` §5.1). It needs the whole song, which is
//!    exactly what the two-pass architecture buys.
//! 2. [`follow`] smooths the normalized track with an attack/decay envelope,
//!    because raw features are twitchy and motion built on them reads as jitter
//!    rather than music.
//!
//! Normalization runs first because the follower is scale-free — it is a convex
//! combination of its input and its own last value — while normalization is
//! affine *and clamped*. Running the clamp last would let a decayed tail sit
//! above 1.0, and would make the attack and decay time constants mean something
//! different on every master.
//!
//! Both are pure functions of a track and an `fps`, so a step, a ramp, and a
//! hand-computed hundred-value vector are enough to check them (`docs/TESTING.md`).

use std::time::Duration;

/// How fast the envelope rises toward a feature that grew: it closes `1 - 1/e`
/// of the gap in this long.
///
/// 10 ms is under half a frame at 60 fps, so a hit reaches the shader on the
/// frame it lands on. It is a real time constant rather than an instantaneous
/// `max`, so a preset can ask for a slow swell in M3 without a new follower.
pub const DEFAULT_ATTACK: Duration = Duration::from_millis(10);

/// How slowly the envelope falls away from a feature that shrank: it keeps `1/e`
/// of the gap after this long.
///
/// 300 ms is the middle of the 200–400 ms `VISION.md` §5.1 asks for, and with
/// [`DEFAULT_ATTACK`] it is the surface the M2 reference-track listening pass
/// tunes. Shorter reads twitchy; longer smears the beat.
pub const DEFAULT_DECAY: Duration = Duration::from_millis(300);

/// The `visual.smoothing` at which [`EnvelopeParams::for_smoothing`] reproduces
/// [`DEFAULT_DECAY`] exactly.
///
/// It is `Config::default().visual.smoothing`, which
/// `the_default_smoothing_yields_the_default_decay` pins.
pub const NOMINAL_SMOOTHING: f32 = 0.35;

/// The narrowest p5..p95 spread [`normalize`] will stretch to the unit interval.
///
/// Below it the track carries no dynamic range worth mapping — digital silence,
/// a held sine, a constant — and dividing by the spread would amplify the FFT's
/// own numerical noise into full-scale flicker, or divide by zero outright. Such
/// a track normalizes to all zeros.
pub const NORMALIZE_EPSILON: f32 = 1e-6;

/// The knobs of [`follow`]. Constants now; RFC-001 Step 15 gives presets their
/// own.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EnvelopeParams {
    /// Rise time constant. See [`DEFAULT_ATTACK`].
    pub attack: Duration,
    /// Fall time constant. See [`DEFAULT_DECAY`].
    pub decay: Duration,
}

impl Default for EnvelopeParams {
    fn default() -> Self {
        Self {
            attack: DEFAULT_ATTACK,
            decay: DEFAULT_DECAY,
        }
    }
}

impl EnvelopeParams {
    /// The envelope `visual.smoothing` asks for: the "global envelope decay
    /// scale" of `VISION.md` §5.5.
    ///
    /// The decay time constant scales linearly with `smoothing`, anchored so
    /// that the config default [`NOMINAL_SMOOTHING`] gives [`DEFAULT_DECAY`].
    /// A `smoothing` of zero leaves the envelope tracking its feature exactly.
    /// The attack is not scaled: smoothing is what happens *after* a hit, and
    /// slowing the rise would move the hit off the beat.
    pub fn for_smoothing(smoothing: f32) -> Self {
        let scale = f64::from(smoothing.max(0.0)) / f64::from(NOMINAL_SMOOTHING);

        Self {
            decay: DEFAULT_DECAY.mul_f64(scale),
            ..Self::default()
        }
    }
}

/// Map `track`'s own p5 to 0 and p95 to 1, clamping the tails, in place.
///
/// This is the "global" in global normalization: the percentiles come from the
/// whole song, so the same passage reads the same however the render was sliced
/// with `--sample`. A track whose spread is under [`NORMALIZE_EPSILON`] — silence,
/// a constant, a held tone — becomes all zeros rather than `NaN` or a stretched
/// noise floor.
pub fn normalize(track: &mut [f32]) {
    let Some((low, high)) = percentiles(track) else {
        return;
    };

    if high - low <= NORMALIZE_EPSILON {
        track.fill(0.0);
        return;
    }

    let span = high - low;
    for value in track.iter_mut() {
        *value = ((*value - low) / span).clamp(0.0, 1.0);
    }
}

/// Smooth `track` with an attack/decay envelope follower at `fps`.
///
/// The envelope moves toward each sample by a fraction of the gap: quickly when
/// the feature grew, slowly when it shrank. Both fractions come from time
/// constants, so a hit swells and fades over the same milliseconds whatever the
/// frame rate — the same rule `onset::detect` follows for its impulse.
///
/// The result stays inside the range of `track` and zero, because every step is
/// a convex combination of the input and the envelope's own last value. So a
/// normalized track yields a normalized envelope, with no second clamp.
///
/// An `fps` of zero yields zeros rather than a division by zero.
pub fn follow(track: &[f32], fps: u32, params: EnvelopeParams) -> Vec<f32> {
    if fps == 0 {
        return vec![0.0; track.len()];
    }

    let attack = gap_left_per_frame(params.attack, fps);
    let decay = gap_left_per_frame(params.decay, fps);

    let mut envelope = 0.0f32;
    track
        .iter()
        .map(|&value| {
            let remaining = if value > envelope { attack } else { decay };
            envelope = value + (envelope - value) * remaining;
            envelope
        })
        .collect()
}

/// The p5 and p95 of `track`, or `None` if it is empty.
///
/// Sorted rather than selected twice: a five-minute song is nine thousand frames
/// and eight tracks, which sorts in microseconds, and one sort is obviously
/// order-independent — the same song must analyze to the same timeline twice
/// (`AGENTS.md`, determinism).
fn percentiles(track: &[f32]) -> Option<(f32, f32)> {
    if track.is_empty() {
        return None;
    }

    let mut sorted = track.to_vec();
    sorted.sort_unstable_by(f32::total_cmp);

    Some((percentile(&sorted, 0.05), percentile(&sorted, 0.95)))
}

/// The value `quantile` of the way through `sorted`, by nearest rank.
///
/// Rank `quantile · (n - 1)` rounded, so `quantile` 0 and 1 give the extremes
/// and a single-valued track gives that value twice.
fn percentile(sorted: &[f32], quantile: f64) -> f32 {
    let last = sorted.len() - 1;
    let rank = (quantile * last as f64).round() as usize;

    sorted[rank]
}

/// The fraction of the gap to the input that survives one frame, for a follower
/// with time constant `tau` at `fps`: `exp(-1 / (tau · fps))`.
///
/// Derived from a time constant rather than given as a per-frame factor, so the
/// envelope takes the same number of milliseconds to move at every `fps`. A
/// `tau` of zero leaves no gap: the envelope is the input.
fn gap_left_per_frame(tau: Duration, fps: u32) -> f32 {
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

    /// A step from 0.0 to 1.0 at frame `at`, `frames` long.
    fn step_up(frames: usize, at: usize) -> Vec<f32> {
        (0..frames)
            .map(|index| if index >= at { 1.0 } else { 0.0 })
            .collect()
    }

    /// A step from 1.0 down to 0.0 at frame `at`, `frames` long.
    fn step_down(frames: usize, at: usize) -> Vec<f32> {
        (0..frames)
            .map(|index| if index >= at { 0.0 } else { 1.0 })
            .collect()
    }

    /// Seeded, because a re-run must exercise the same signal (`AGENTS.md`,
    /// determinism).
    fn pseudo_random(count: usize, seed: u64) -> Vec<f32> {
        let mut state = seed;
        (0..count)
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                (state >> 40) as f32 / (1u64 << 24) as f32
            })
            .collect()
    }

    /// The definition, on a vector whose percentiles can be counted by hand:
    /// `0..100`, whose p5 is 5 and p95 is 95 at rank `q · 99` rounded. Everything
    /// between maps linearly, and the tails clamp rather than run negative or
    /// past one.
    #[test]
    fn p5_p95_mapping_matches_hand_computed_vector() {
        let mut track: Vec<f32> = (0..100).map(|value| value as f32).collect();

        normalize(&mut track);

        // rank(0.05) = round(4.95) = 5, rank(0.95) = round(94.05) = 94.
        let (low, high) = (5.0f32, 94.0f32);
        assert_eq!(track[5], 0.0, "p5 maps to the bottom of the range");
        assert_eq!(track[94], 1.0, "p95 maps to the top of the range");

        // Everything between p5 and p95 maps linearly; the tails clamp rather
        // than running negative or past one.
        for (value, &normalized) in track.iter().enumerate() {
            let expected = ((value as f32 - low) / (high - low)).clamp(0.0, 1.0);
            assert!(
                (normalized - expected).abs() < 1e-6,
                "value {value} normalized to {normalized} rather than {expected}"
            );
        }
    }

    /// The risk-matrix row: normalization must not divide by a zero spread.
    #[test]
    fn silence_normalizes_without_nan() {
        let mut track = vec![0.0f32; 120];

        normalize(&mut track);

        assert!(track.iter().all(|value| *value == 0.0), "{track:?}");
    }

    /// A held chord, digital silence, and a constant all have no dynamic range.
    /// Stretching their p5..p95 to the unit interval would amplify the FFT's own
    /// numerical noise into full-scale flicker.
    #[test]
    fn constant_signal_normalizes_to_zeros_not_nan() {
        for level in [0.0f32, 0.5, 1.0, 42.0] {
            let mut track = vec![level; 120];

            normalize(&mut track);

            assert!(
                track.iter().all(|value| value.is_finite() && *value == 0.0),
                "a constant {level} normalized to {track:?}"
            );
        }

        // Just inside the epsilon: still no range worth mapping.
        let mut track = vec![1.0f32; 120];
        track[60] = 1.0 + NORMALIZE_EPSILON / 2.0;
        normalize(&mut track);
        assert!(track.iter().all(|value| *value == 0.0), "{track:?}");
    }

    #[test]
    fn an_empty_track_normalizes_to_nothing() {
        let mut track: Vec<f32> = Vec::new();

        normalize(&mut track);

        assert!(track.is_empty());
    }

    /// Two masters of the same performance, twenty decibels apart, must hand the
    /// presets the same motion — that is the whole point of normalizing against
    /// the song's own dynamic range (`VISION.md` §4.2).
    #[test]
    fn a_quiet_master_and_a_loud_one_normalize_to_the_same_track() {
        let shape: Vec<f32> = (0..200).map(|index| (index % 17) as f32 / 17.0).collect();

        let mut quiet: Vec<f32> = shape.iter().map(|value| value * 0.01).collect();
        let mut loud: Vec<f32> = shape.iter().map(|value| value * 1.0).collect();
        normalize(&mut quiet);
        normalize(&mut loud);

        for (index, (&q, &l)) in quiet.iter().zip(&loud).enumerate() {
            assert!((q - l).abs() < 1e-5, "frame {index}: {q} against {l}");
        }
    }

    /// A step into a feature is answered within the attack budget: the envelope
    /// closes 90% of the gap after `ceil(ln(10) · τ · fps)` frames, because
    /// `exp(-n / (τ · fps))` is the gap it has left.
    #[test]
    fn step_input_env_reaches_90pct_within_attack_budget() {
        // A deliberately slow attack, so the budget is many frames and a
        // follower that simply copied its input could not pass.
        for (fps, attack) in [
            (30u32, Duration::from_millis(10)),
            (60, Duration::from_millis(10)),
            (30, Duration::from_millis(100)),
            (60, Duration::from_millis(250)),
        ] {
            let params = EnvelopeParams {
                attack,
                ..EnvelopeParams::default()
            };
            let track = step_up(4 * fps as usize, fps as usize);
            let env = follow(&track, fps, params);

            let budget =
                (f64::from(10.0f32).ln() * attack.as_secs_f64() * f64::from(fps)).ceil() as usize;
            let hit = fps as usize;

            assert!(
                env[hit + budget] >= 0.9,
                "{fps} fps, {attack:?} attack: {budget} frames on, the envelope \
                 is only at {}",
                env[hit + budget]
            );
            // And not before: an instantaneous follower is not an envelope.
            if budget > 1 {
                assert!(
                    env[hit + budget - 2] < 0.9,
                    "{fps} fps, {attack:?} attack: the envelope reached 0.9 early"
                );
            }
        }
    }

    /// The release is the knob that makes motion musical, so it is pinned in
    /// *time*: one decay constant after the feature falls away, the envelope
    /// still holds `1/e` of it, at every frame rate.
    #[test]
    fn release_tail_matches_decay_time_constant() {
        for fps in [24u32, 30, 60] {
            let params = EnvelopeParams::default();
            let track = step_down(4 * fps as usize, fps as usize);
            let env = follow(&track, fps, params);

            let tau = params.decay.as_secs_f32();
            // The last frame the feature was still present on. `step_down` puts
            // the first zero at `fps`, by which point one decay step has run.
            let release = fps as usize - 1;
            let after = release + (tau * fps as f32).round() as usize;

            let inverse_e = std::f32::consts::E.recip();
            assert!(
                (env[after] - inverse_e).abs() < inverse_e * 0.1,
                "{fps} fps: one time constant after the release the envelope is \
                 {} rather than {inverse_e}",
                env[after]
            );

            // The whole tail is `exp(-t/τ)`, not only the point at τ.
            for ahead in 1..=fps as usize {
                let expected = (-(ahead as f32 / fps as f32) / tau).exp();
                assert!(
                    (env[release + ahead] - expected).abs() <= expected * 0.05,
                    "{fps} fps, {ahead} frames after the release: {} against {expected}",
                    env[release + ahead]
                );
            }
        }
    }

    /// `visual.smoothing` is the global decay scale (`VISION.md` §5.5): doubling
    /// it must hold the tail up for twice as long, and zeroing it must leave the
    /// envelope tracking its feature exactly.
    #[test]
    fn smoothing_config_scales_decay() {
        let track = step_down(4 * FPS as usize, FPS as usize);

        let nominal = follow(
            &track,
            FPS,
            EnvelopeParams::for_smoothing(NOMINAL_SMOOTHING),
        );
        let smoother = follow(
            &track,
            FPS,
            EnvelopeParams::for_smoothing(2.0 * NOMINAL_SMOOTHING),
        );

        for frame in FPS as usize + 1..track.len() {
            assert!(
                smoother[frame] > nominal[frame],
                "frame {frame}: smoothing 0.7 fell to {} while 0.35 held {}",
                smoother[frame],
                nominal[frame]
            );
        }

        let none = follow(&track, FPS, EnvelopeParams::for_smoothing(0.0));
        assert_eq!(none[FPS as usize], 0.0, "no smoothing, no tail");
    }

    /// The config default must reproduce the documented default, or the two
    /// numbers drift apart and the docs stop describing the code.
    #[test]
    fn the_default_smoothing_yields_the_default_decay() {
        assert_eq!(
            EnvelopeParams::for_smoothing(NOMINAL_SMOOTHING),
            EnvelopeParams::default()
        );
        assert_eq!(
            crate::config::Config::default().visual.smoothing,
            NOMINAL_SMOOTHING
        );
    }

    /// The property that lets the envelope skip a second clamp: every step is a
    /// convex combination of the input and the last envelope, so a normalized
    /// track can only produce a normalized envelope. Checked over pseudo-random
    /// tracks, steps, and ramps.
    #[test]
    fn env_never_exceeds_input_peak_or_drops_below_zero() {
        let tracks = [
            pseudo_random(500, 7),
            pseudo_random(500, 11),
            step_up(200, 50),
            step_down(200, 50),
            (0..200).map(|index| index as f32 / 199.0).collect(),
            vec![1.0; 100],
            vec![0.0; 100],
        ];

        for (which, track) in tracks.iter().enumerate() {
            let peak = track.iter().copied().fold(0.0f32, f32::max);

            for fps in [1u32, 24, 30, 60, 240] {
                for env in follow(track, fps, EnvelopeParams::default()) {
                    assert!(
                        env.is_finite() && (0.0..=peak).contains(&env),
                        "track {which} at {fps} fps produced an envelope of {env} \
                         against a peak of {peak}"
                    );
                }
            }
        }
    }

    /// Attack is faster than decay, so the envelope answers a hit at once and
    /// lets go of it slowly. Reversed, motion would lag every beat and then snap.
    #[test]
    fn the_envelope_rises_faster_than_it_falls() {
        let up = follow(&step_up(120, 30), FPS, EnvelopeParams::default());
        let down = follow(&step_down(120, 30), FPS, EnvelopeParams::default());

        // One frame into the step, the rise has closed almost all of its gap
        // while the fall has given up barely a tenth of the feature it held.
        assert!(up[30] > 0.95, "the attack is sluggish: {}", up[30]);
        assert!(down[30] > 0.85, "the decay is twitchy: {}", down[30]);
    }

    #[test]
    fn a_zero_fps_track_envelopes_to_zeros_rather_than_a_division_by_zero() {
        let env = follow(&step_up(10, 2), 0, EnvelopeParams::default());

        assert_eq!(env, vec![0.0; 10]);
    }

    #[test]
    fn an_empty_track_envelopes_to_nothing() {
        assert!(follow(&[], FPS, EnvelopeParams::default()).is_empty());
    }

    /// A zero time constant makes the envelope its input, rather than dividing
    /// by zero.
    #[test]
    fn a_zero_time_constant_tracks_the_input_exactly() {
        let params = EnvelopeParams {
            attack: Duration::ZERO,
            decay: Duration::ZERO,
        };
        let track = pseudo_random(50, 3);

        assert_eq!(follow(&track, FPS, params), track);
    }

    #[test]
    fn a_percentile_of_a_single_valued_track_is_that_value() {
        assert_eq!(percentiles(&[7.0]), Some((7.0, 7.0)));
        assert_eq!(percentiles(&[]), None);
    }

    /// Percentiles read the track's values, not the order they arrived in.
    #[test]
    fn percentiles_do_not_depend_on_the_order_of_the_track() {
        let ascending: Vec<f32> = (0..100).map(|value| value as f32).collect();
        let descending: Vec<f32> = ascending.iter().rev().copied().collect();

        assert_eq!(percentiles(&ascending), percentiles(&descending));
        assert_eq!(percentiles(&ascending), Some((5.0, 94.0)));
    }
}
