//! The spectral half of feature extraction: a windowed FFT and the pure
//! functions that read features off its magnitude spectrum.
//!
//! Everything here is a pure function of a magnitude spectrum and a bin width,
//! which is what makes the DSP testable against signals whose correct answer is
//! known analytically (`docs/TESTING.md`). Frame timing, window placement, and
//! the parallel drive loop live in [`super::features`].

use std::f32::consts::TAU;
use std::sync::Arc;

use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};

/// The five bands of `VISION.md` §5.1, as half-open `[low, high)` Hz ranges.
///
/// Order is `bass`, `low_mid`, `mid`, `high`, `air`, and [`band_energies`]
/// returns them in that order. A bin belongs to the band containing its center
/// frequency, so the ranges never overlap and never share a bin.
pub const BAND_EDGES: [(f32, f32); BAND_COUNT] = [
    (20.0, 150.0),
    (150.0, 500.0),
    (500.0, 2_000.0),
    (2_000.0, 8_000.0),
    (8_000.0, 16_000.0),
];

/// How many bands [`band_energies`] returns.
pub const BAND_COUNT: usize = 5;

/// The width of one FFT bin, in Hz. Bin `k` is centered on `k * bin_hz`.
pub fn bin_hz(sample_rate: u32, size: usize) -> f32 {
    if size == 0 {
        return 0.0;
    }
    sample_rate as f32 / size as f32
}

/// A Hann-windowed forward FFT of a fixed size, planned once and shared.
///
/// The planner is the expensive part, so it runs once per song rather than once
/// per window (`VISION.md` §5.1). The planned FFT is behind an `Arc` and takes
/// `&self` to transform, which is what lets every rayon worker share this one
/// value; the mutable state a transform needs lives in a per-worker
/// [`Workspace`].
#[derive(Clone)]
pub struct Spectrograph {
    fft: Arc<dyn Fft<f32>>,
    /// Hann coefficients, one per input sample.
    window: Vec<f32>,
    /// Divides out the window's coherent gain, so a full-scale sine reads 1.0
    /// at its bin no matter how long the window is.
    scale: f32,
}

/// The scratch a [`Spectrograph`] transform needs. One per thread.
pub struct Workspace {
    signal: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
}

impl Spectrograph {
    /// Plan a Hann-windowed forward FFT over `size` samples.
    ///
    /// # Panics
    ///
    /// If `size` is zero.
    pub fn new(size: usize) -> Self {
        assert!(size > 0, "an FFT needs at least one sample");

        let fft = FftPlanner::new().plan_fft_forward(size);
        let window = hann(size);

        // Coherent gain: the peak of a windowed sine is attenuated by the mean
        // window coefficient, and a real signal splits its energy between the
        // positive and negative frequency, hence the 2.
        let sum: f32 = window.iter().sum();
        let scale = if sum > 0.0 { 2.0 / sum } else { 0.0 };

        Self { fft, window, scale }
    }

    /// The window length this spectrograph transforms.
    pub fn size(&self) -> usize {
        self.window.len()
    }

    /// How many magnitude bins [`Self::magnitudes`] produces: `size / 2 + 1`.
    pub fn bins(&self) -> usize {
        self.size() / 2 + 1
    }

    /// Fresh per-thread scratch for [`Self::magnitudes`].
    pub fn workspace(&self) -> Workspace {
        Workspace {
            signal: vec![Complex::new(0.0, 0.0); self.size()],
            scratch: vec![Complex::new(0.0, 0.0); self.fft.get_inplace_scratch_len()],
        }
    }

    /// The magnitude spectrum of `samples`, one value per bin up to Nyquist.
    ///
    /// Magnitudes are amplitudes, not powers: a full-scale sine reads about 1.0
    /// at its bin. The upper half of the transform mirrors the lower for a real
    /// signal and is dropped.
    ///
    /// # Panics
    ///
    /// If `samples` is not exactly [`Self::size`] long.
    pub fn magnitudes(&self, samples: &[f32], workspace: &mut Workspace) -> Vec<f32> {
        assert_eq!(
            samples.len(),
            self.size(),
            "a window must be exactly the planned FFT size"
        );

        for ((slot, &sample), &coefficient) in
            workspace.signal.iter_mut().zip(samples).zip(&self.window)
        {
            *slot = Complex::new(sample * coefficient, 0.0);
        }

        self.fft
            .process_with_scratch(&mut workspace.signal, &mut workspace.scratch);

        workspace.signal[..self.bins()]
            .iter()
            .map(|bin| bin.norm() * self.scale)
            .collect()
    }
}

/// Periodic Hann coefficients: `0.5 - 0.5·cos(2πn/N)`.
fn hann(size: usize) -> Vec<f32> {
    (0..size)
        .map(|n| 0.5 - 0.5 * (TAU * n as f32 / size as f32).cos())
        .collect()
}

/// Log power per band, in [`BAND_EDGES`] order.
///
/// Each band sums the power (magnitude squared) of the bins whose center
/// frequency falls inside it, then compresses the sum with `ln(1 + power)`.
/// Compressing the sum rather than each bin keeps a band's reading independent
/// of how many bins the sample rate happens to give it. Values are raw — the
/// global p5/p95 normalization is a later pass.
pub fn band_energies(magnitudes: &[f32], bin_hz: f32) -> [f32; BAND_COUNT] {
    let mut power = [0.0f64; BAND_COUNT];

    for (bin, &magnitude) in magnitudes.iter().enumerate() {
        let frequency = bin as f32 * bin_hz;
        if let Some(band) = band_of(frequency) {
            power[band] += f64::from(magnitude) * f64::from(magnitude);
        }
    }

    power.map(|sum| sum.ln_1p() as f32)
}

/// The band containing `frequency`, or `None` below 20 Hz or above 16 kHz.
fn band_of(frequency: f32) -> Option<usize> {
    BAND_EDGES
        .iter()
        .position(|&(low, high)| frequency >= low && frequency < high)
}

/// Half-wave-rectified spectral flux: how much the spectrum *grew* since
/// `previous`.
///
/// Rectified because onsets are energy arriving, not energy leaving: a note
/// ending should not read like a note starting.
///
/// # Panics
///
/// If the two spectra have different lengths.
pub fn spectral_flux(previous: &[f32], current: &[f32]) -> f32 {
    assert_eq!(
        previous.len(),
        current.len(),
        "flux compares two spectra of the same size"
    );

    let sum: f64 = previous
        .iter()
        .zip(current)
        .map(|(&was, &is)| f64::from((is - was).max(0.0)))
        .sum();

    sum as f32
}

/// Magnitude-weighted mean frequency, normalized by Nyquist to `0.0..=1.0`.
///
/// A silent window has no mean frequency, so it reads 0.0 rather than `NaN` —
/// a `NaN` here would reach a shader uniform and paint a frame black.
pub fn spectral_centroid(magnitudes: &[f32], bin_hz: f32) -> f32 {
    let mut total = 0.0f64;
    let mut weighted = 0.0f64;

    for (bin, &magnitude) in magnitudes.iter().enumerate() {
        total += f64::from(magnitude);
        weighted += f64::from(magnitude) * f64::from(bin as f32 * bin_hz);
    }

    let nyquist = f64::from(bin_hz) * magnitudes.len().saturating_sub(1) as f64;
    if total <= 0.0 || nyquist <= 0.0 {
        return 0.0;
    }

    ((weighted / total) / nyquist).clamp(0.0, 1.0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 44_100;
    const SIZE: usize = 2048;

    /// A `freq` Hz sine of amplitude `amplitude`, exactly one FFT window long.
    fn windowed_sine(freq: f32, amplitude: f32) -> Vec<f32> {
        (0..SIZE)
            .map(|n| amplitude * (TAU * freq * n as f32 / RATE as f32).sin())
            .collect()
    }

    fn magnitudes_of(samples: &[f32]) -> Vec<f32> {
        let spectrograph = Spectrograph::new(samples.len());
        spectrograph.magnitudes(samples, &mut spectrograph.workspace())
    }

    /// The window's coherent gain is divided out, so a bin's magnitude is the
    /// amplitude of the sine that lives there. Without the correction the peak
    /// would scale with the window length, and every threshold downstream would
    /// silently become a function of `fps`.
    ///
    /// The tone sits exactly on bin 46, because a tone between two bins is split
    /// between them (scalloping loss) and would read about 0.85 whatever the
    /// normalization.
    #[test]
    fn a_full_scale_sine_reads_unit_amplitude_at_its_bin() {
        let on_bin = 46.0 * bin_hz(RATE, SIZE);
        let magnitudes = magnitudes_of(&windowed_sine(on_bin, 1.0));

        let peak = magnitudes.iter().copied().fold(0.0f32, f32::max);
        assert!((peak - 1.0).abs() < 0.01, "peak magnitude {peak}");
    }

    /// The peak lands on the bin nearest the tone: 1000 Hz / 21.53 Hz ≈ bin 46.
    #[test]
    fn a_tone_peaks_at_the_bin_holding_its_frequency() {
        let magnitudes = magnitudes_of(&windowed_sine(1_000.0, 0.8));

        let peak = magnitudes
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(bin, _)| bin)
            .expect("bins exist");

        let expected = (1_000.0 / bin_hz(RATE, SIZE)).round() as usize;
        assert_eq!(peak, expected);
    }

    #[test]
    fn a_real_spectrum_keeps_only_the_bins_up_to_nyquist() {
        let spectrograph = Spectrograph::new(SIZE);

        assert_eq!(spectrograph.bins(), SIZE / 2 + 1);
        assert_eq!(
            spectrograph
                .magnitudes(&windowed_sine(440.0, 0.5), &mut spectrograph.workspace())
                .len(),
            SIZE / 2 + 1
        );
    }

    /// Bin centers, not bin edges, decide the band — and the boundaries are
    /// half-open, so 150 Hz is `low_mid` and never counted twice.
    #[test]
    fn a_bin_belongs_to_the_band_holding_its_center_frequency() {
        assert_eq!(band_of(20.0), Some(0));
        assert_eq!(band_of(149.9), Some(0));
        assert_eq!(band_of(150.0), Some(1));
        assert_eq!(band_of(1_999.9), Some(2));
        assert_eq!(band_of(2_000.0), Some(3));
        assert_eq!(band_of(15_999.9), Some(4));
    }

    /// Below the bass edge and above the air edge, a bin belongs to nobody: DC
    /// and rumble must not inflate `bass`.
    #[test]
    fn dc_and_ultrasound_belong_to_no_band() {
        assert_eq!(band_of(0.0), None);
        assert_eq!(band_of(19.9), None);
        assert_eq!(band_of(16_000.0), None);
        assert_eq!(band_of(22_050.0), None);
    }

    /// `ln(1 + p)` of the band's total power, so a band whose power doubles
    /// grows but does not double: the compression is the point.
    #[test]
    fn a_band_reports_the_log_of_its_summed_power() {
        let hz = bin_hz(RATE, SIZE);
        // Two bins inside `mid` (bins 46 and 47 sit at ~991 and ~1013 Hz),
        // each carrying an amplitude of 2.0, so the band power is 8.0.
        let mut magnitudes = vec![0.0f32; SIZE / 2 + 1];
        magnitudes[46] = 2.0;
        magnitudes[47] = 2.0;

        let bands = band_energies(&magnitudes, hz);

        assert!((bands[2] - 8.0f32.ln_1p()).abs() < 1e-5, "{bands:?}");
        assert_eq!(bands[0], 0.0);
        assert_eq!(bands[4], 0.0);
    }

    #[test]
    fn a_silent_spectrum_has_no_band_energy_and_no_nans() {
        let bands = band_energies(&vec![0.0; SIZE / 2 + 1], bin_hz(RATE, SIZE));

        assert!(bands.iter().all(|&band| band == 0.0), "{bands:?}");
    }

    /// Energy that leaves the spectrum is not an onset.
    #[test]
    fn flux_is_half_wave_rectified() {
        assert_eq!(spectral_flux(&[0.0, 1.0], &[0.5, 0.25]), 0.5);
        assert_eq!(spectral_flux(&[1.0, 1.0], &[0.0, 0.0]), 0.0);
        assert_eq!(spectral_flux(&[0.0, 0.0], &[0.0, 0.0]), 0.0);
    }

    #[test]
    fn flux_between_identical_spectra_is_zero() {
        let magnitudes = magnitudes_of(&windowed_sine(440.0, 0.7));

        assert_eq!(spectral_flux(&magnitudes, &magnitudes), 0.0);
    }

    /// The centroid of a single tone is that tone's frequency, over Nyquist.
    #[test]
    fn the_centroid_of_a_lone_tone_is_its_own_frequency() {
        let hz = bin_hz(RATE, SIZE);
        let mut magnitudes = vec![0.0f32; SIZE / 2 + 1];
        magnitudes[100] = 1.0;

        let nyquist = hz * (SIZE / 2) as f32;
        let expected = (100.0 * hz) / nyquist;

        assert!((spectral_centroid(&magnitudes, hz) - expected).abs() < 1e-6);
    }

    /// Two equal tones put the centroid exactly between them, which is what
    /// makes it a *mean* frequency rather than a peak picker.
    #[test]
    fn the_centroid_of_two_equal_tones_sits_between_them() {
        let hz = bin_hz(RATE, SIZE);
        let mut magnitudes = vec![0.0f32; SIZE / 2 + 1];
        magnitudes[100] = 0.5;
        magnitudes[300] = 0.5;

        let nyquist = hz * (SIZE / 2) as f32;
        let expected = (200.0 * hz) / nyquist;

        assert!((spectral_centroid(&magnitudes, hz) - expected).abs() < 1e-6);
    }

    #[test]
    fn the_centroid_of_a_silent_spectrum_is_zero_not_nan() {
        let centroid = spectral_centroid(&vec![0.0; SIZE / 2 + 1], bin_hz(RATE, SIZE));

        assert_eq!(centroid, 0.0);
        assert!(centroid.is_finite());
    }

    /// A degenerate one-bin spectrum has a Nyquist of zero. It must read 0.0,
    /// not `inf`.
    #[test]
    fn a_single_bin_spectrum_has_no_centroid_rather_than_an_infinity() {
        assert_eq!(spectral_centroid(&[1.0], bin_hz(RATE, 1)), 0.0);
    }

    /// Hann is periodic, not symmetric: `w[0]` is 0 and the coefficients sum to
    /// half the window length. A symmetric window would divide the coherent
    /// gain by the wrong constant.
    #[test]
    fn the_hann_window_is_periodic_and_sums_to_half_its_length() {
        let window = hann(SIZE);

        assert_eq!(window[0], 0.0);
        assert!((window[SIZE / 2] - 1.0).abs() < 1e-6);
        assert!((window.iter().sum::<f32>() - SIZE as f32 / 2.0).abs() < 0.01);
    }
}
