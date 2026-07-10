//! The analysis pass's performance budget, as an ignored smoke test.
//!
//! `VISION.md` §5.1 budgets low single-digit seconds to analyze a five-minute
//! song, which is what the reused FFT planner and the rayon-parallel windows buy.
//! Nothing else in the suite would notice if a future change planned an FFT per
//! window, or serialized the pass: the answers would all still be right.
//!
//! It lives here rather than beside the code it measures for two reasons. It
//! reads a wall clock, and `scripts/quality.d/90-animation-time-comes-from-the-frame-index.sh`
//! rightly forbids `Instant::now()` anywhere under `crates/avz-core/src`. And it
//! is `#[ignore]`d: a loaded machine would make it flake as a per-commit gate.
//!
//! Run it with `cargo test -p avz-core --test analysis_perf -- --ignored`.

use std::f64::consts::TAU;
use std::time::{Duration, Instant};

use avz_core::analysis::{DecodedAudio, analyze};

const RATE: u32 = 44_100;
const FPS: u32 = 30;
const SECONDS: f64 = 300.0;

/// A five-minute signal with content in every band, so no band's bins are
/// trivially zero and the FFT is doing real work.
fn five_minute_song() -> Vec<f32> {
    let count = (SECONDS * f64::from(RATE)) as usize;
    (0..count)
        .map(|n| {
            let t = n as f64 / f64::from(RATE);
            let bass = 0.5 * (TAU * 60.0 * t).sin();
            let mid = 0.3 * (TAU * 1_000.0 * t).sin();
            let air = 0.1 * (TAU * 11_000.0 * t).sin();
            (bass + mid + air) as f32
        })
        .collect()
}

#[test]
#[ignore = "perf smoke test: run with `cargo test -- --ignored`"]
fn analysis_of_a_five_minute_song_finishes_in_seconds() {
    let audio = DecodedAudio {
        samples: five_minute_song(),
        sample_rate: RATE,
    };

    let started = Instant::now();
    let timeline = analyze(&audio, FPS).expect("a five-minute song analyzes");
    let elapsed = started.elapsed();

    assert_eq!(timeline.len(), (SECONDS as u32 * FPS) as usize);
    assert!(
        elapsed < Duration::from_secs(5),
        "analyzing a five-minute song took {elapsed:?}, over the VISION §5.1 budget"
    );
}
