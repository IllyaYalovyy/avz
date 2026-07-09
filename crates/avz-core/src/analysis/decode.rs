//! mp3 → mono f32 PCM.
//!
//! The first stage of the data flow (`VISION.md` §4.2). This decode exists only
//! so the analysis pass has samples to look at: the original mp3 stream is muxed
//! into the output untouched with `-c:a copy`, so nothing here can ever reach the
//! listener's ears.
//!
//! Everything is mixed down to one channel and kept at the file's native sample
//! rate. Mono because every feature in `VISION.md` §5.1 is a property of the
//! whole mix, and native rate because the analysis hop is derived from it
//! (`sample_rate / fps`), so resampling would only add error.

use std::fs::File;
use std::path::Path;
use std::time::Duration;

use symphonia::core::codecs::CodecParameters;
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::TrackType;
use symphonia::core::formats::probe::Hint;
use symphonia::core::io::MediaSourceStream;

use crate::{Error, Result};

/// A whole song, decoded to a single channel of `f32` samples.
///
/// Samples are nominally in `-1.0..=1.0`. mp3 is lossy, so a signal mastered to
/// the limit can decode a hair outside that range; the values are left as the
/// decoder produced them rather than clamped, because analysis normalizes
/// against the song's own dynamic range anyway (`VISION.md` §5.1) and a clamp
/// would only hide inter-sample overs from it.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedAudio {
    /// Mono samples: the average of the source channels, frame by frame.
    pub samples: Vec<f32>,
    /// The file's own sample rate, in Hz. Never resampled.
    pub sample_rate: u32,
}

impl DecodedAudio {
    /// How long the decoded audio plays for.
    ///
    /// Derived from the samples rather than read from the container, so the
    /// duration can never disagree with the buffer the analysis pass walks.
    pub fn duration(&self) -> Duration {
        Duration::from_secs_f64(self.samples.len() as f64 / f64::from(self.sample_rate))
    }
}

/// Decode an mp3 file to mono `f32` PCM at its native sample rate.
///
/// # Errors
///
/// [`Error::Input`] if the file is missing, unreadable, not an mp3, truncated,
/// or otherwise malformed. Every failure here is the user's file, never the
/// pipeline, so the CLI exits with code 3.
pub fn decode(path: impl AsRef<Path>) -> Result<DecodedAudio> {
    let path = path.as_ref();

    let file = File::open(path).map_err(|err| unopenable(path, &err))?;
    let stream = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|ext| ext.to_str()) {
        hint.with_extension(extension);
    }

    let mut format = symphonia::default::get_probe()
        .probe(&hint, stream, Default::default(), Default::default())
        .map_err(|err| undecodable(path, &err))?;

    let track = format
        .default_track(TrackType::Audio)
        .ok_or_else(|| Error::Input(format!("{}: no audio track", path.display())))?;
    let track_id = track.id;

    // Cloned so the immutable borrow of `format` ends before the packet loop
    // needs it mutably.
    let Some(CodecParameters::Audio(params)) = track.codec_params.clone() else {
        return Err(Error::Input(format!(
            "{}: the audio track declares no codec; avz reads mp3",
            path.display()
        )));
    };

    // `AudioDecoderOptions::gapless` defaults to true, so the decoder honours the
    // encoder delay and padding the LAME tag declares. Analysis and the final mux
    // then agree about where the song starts.
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&params, &AudioDecoderOptions::default())
        .map_err(|err| undecodable(path, &err))?;

    let mut samples = Vec::new();
    let mut interleaved = Vec::new();
    let mut sample_rate = None;

    while let Some(packet) = format
        .next_packet()
        .map_err(|err| undecodable(path, &err))?
    {
        if packet.track_id != track_id {
            continue;
        }

        // Strict on purpose: a packet the decoder rejects means the file is
        // damaged, and silently analyzing the rest would move every later frame
        // against audio the listener will still hear (`AGENTS.md`: make failures
        // explicit).
        let decoded = decoder
            .decode(&packet)
            .map_err(|err| undecodable(path, &err))?;
        if decoded.is_empty() {
            continue;
        }

        let rate = decoded.spec().rate();
        match sample_rate {
            Some(known) if known != rate => {
                return Err(Error::Input(format!(
                    "{}: sample rate changes from {known} Hz to {rate} Hz mid-stream",
                    path.display()
                )));
            }
            _ => sample_rate = Some(rate),
        }

        decoded.copy_to_vec_interleaved(&mut interleaved);
        mix_into(&mut samples, &interleaved, decoded.num_planes());
    }

    // A file can probe as mp3 and still yield nothing: an ID3 tag with no frames
    // behind it, or a stream whose header claims a zero sample rate.
    let sample_rate = match sample_rate {
        Some(rate) if rate > 0 && !samples.is_empty() => rate,
        _ => {
            return Err(Error::Input(format!(
                "{}: no audio could be decoded from this file",
                path.display()
            )));
        }
    };

    tracing::debug!(
        path = %path.display(),
        sample_rate,
        frames = samples.len(),
        "decoded audio to mono"
    );

    Ok(DecodedAudio {
        samples,
        sample_rate,
    })
}

/// Append the mono mixdown of one interleaved buffer to `out`.
///
/// A trailing partial frame is dropped: it carries no complete moment in time,
/// and averaging it against absent channels would invent a sample.
fn mix_into(out: &mut Vec<f32>, interleaved: &[f32], channels: usize) {
    if channels == 0 {
        return;
    }

    let scale = 1.0 / channels as f32;
    out.extend(
        interleaved
            .chunks_exact(channels)
            .map(|frame| frame.iter().sum::<f32>() * scale),
    );
}

/// Explain why the file could not be opened.
fn unopenable(path: &Path, err: &std::io::Error) -> Error {
    use std::io::ErrorKind as Io;

    let path = path.display();

    match err.kind() {
        Io::NotFound => Error::Input(format!("{path}: no such file")),
        Io::PermissionDenied => Error::Input(format!("{path}: permission denied")),
        _ => Error::Input(format!("{path}: cannot be read: {err}")),
    }
}

/// Turn a `symphonia` failure into an input error the user can act on.
///
/// Every variant here means "this file", never "this machine": the pipeline has
/// not started yet and nothing but the bytes can be at fault. The distinction
/// worth drawing for the user is between a file that is not audio, a file that
/// stops early, and audio avz cannot read.
fn undecodable(path: &Path, err: &SymphoniaError) -> Error {
    use std::io::ErrorKind as Io;

    tracing::debug!(path = %path.display(), error = %err, "could not decode audio");

    let path = path.display();

    Error::Input(match err {
        SymphoniaError::IoError(io) if io.kind() == Io::UnexpectedEof => {
            format!("{path}: the file ends before the audio does; it is truncated")
        }
        SymphoniaError::IoError(io) => match io.kind() {
            Io::NotFound => format!("{path}: no such file"),
            Io::PermissionDenied => format!("{path}: permission denied"),
            _ => format!("{path}: cannot be read: {io}"),
        },
        SymphoniaError::Unsupported(_) => {
            format!("{path}: not a recognized audio file; avz reads mp3")
        }
        SymphoniaError::DecodeError(what) => format!("{path}: malformed audio stream: {what}"),
        SymphoniaError::LimitError(what) => format!("{path}: audio stream exceeds a limit: {what}"),
        // `symphonia::Error` is `#[non_exhaustive]`. Seeks and resets do not
        // arise in a straight decode, and a future variant would not either.
        other => format!("{path}: this audio stream cannot be decoded: {other}"),
    })
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;

    /// A committed CC0 fixture. See `assets/fixtures/README.md`.
    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/fixtures")
            .join(name)
    }

    /// One mp3 frame at 44.1 kHz is 1152 samples ≈ 26 ms. Frame quantization and
    /// encoder padding are the only reasons a decode may miss the nominal
    /// length, so nothing may be off by more than one frame.
    const ONE_MP3_FRAME: Duration = Duration::from_millis(26);

    fn rms(samples: &[f32]) -> f32 {
        let sum: f64 = samples.iter().map(|&s| f64::from(s) * f64::from(s)).sum();
        (sum / samples.len() as f64).sqrt() as f32
    }

    fn peak(samples: &[f32]) -> f32 {
        samples.iter().fold(0.0f32, |max, &s| max.max(s.abs()))
    }

    #[test]
    fn decodes_fixture_to_expected_duration() {
        let audio = decode(fixture("tone-tagged.mp3")).expect("the fixture decodes");

        let expected = Duration::from_secs(5);
        let decoded = audio.duration();
        let drift = decoded.abs_diff(expected);

        assert!(
            drift <= ONE_MP3_FRAME,
            "decoded {decoded:?}, expected {expected:?} within one mp3 frame"
        );
    }

    #[test]
    fn mono_output_length_matches_duration_times_rate() {
        let audio = decode(fixture("tone-tagged.mp3")).expect("the fixture decodes");

        let frames = audio.duration().as_secs_f64() * f64::from(audio.sample_rate);

        assert_eq!(audio.samples.len(), frames.round() as usize);
    }

    /// The mix is one channel even though the fixture is stereo. A decoder that
    /// forgot to mix down would return twice as many samples.
    #[test]
    fn a_stereo_source_decodes_to_one_channel() {
        let audio = decode(fixture("tone-tagged.mp3")).expect("the fixture decodes");

        assert_eq!(audio.sample_rate, 44_100);
        assert_eq!(audio.samples.len(), 5 * 44_100);
    }

    #[test]
    fn decoded_samples_stay_within_the_nominal_range() {
        let audio = decode(fixture("tone-tagged.mp3")).expect("the fixture decodes");

        assert!(
            audio.samples.iter().all(|s| (-1.0..=1.0).contains(s)),
            "peak was {}",
            peak(&audio.samples)
        );
        assert!(audio.samples.iter().all(|s| s.is_finite()));
    }

    /// The fixture carries a 1 kHz tone at amplitude 0.5 in the left channel and
    /// silence in the right. Averaging halves it. Taking channel 0, or summing
    /// the channels, would leave it at 0.5 — which is what this pins down.
    #[test]
    fn stereo_downmix_is_channel_average() {
        let audio = decode(fixture("tone-left-only.mp3")).expect("the fixture decodes");

        // 0.25·sin has RMS 0.25/√2. Lossy coding moves it by well under 0.01.
        let expected_rms = 0.25 / f32::sqrt(2.0);
        let got = rms(&audio.samples);

        assert!(
            (got - expected_rms).abs() < 0.01,
            "mono RMS {got}, expected {expected_rms} (channel average)"
        );
        assert!(
            peak(&audio.samples) < 0.35,
            "peak {} looks like one channel, not the average",
            peak(&audio.samples)
        );
    }

    fn mix_to_mono(interleaved: &[f32], channels: usize) -> Vec<f32> {
        let mut mono = Vec::new();
        mix_into(&mut mono, interleaved, channels);
        mono
    }

    /// A mono source has nothing to average, and must come through untouched.
    #[test]
    fn mono_source_passes_through_unmixed() {
        assert_eq!(mix_to_mono(&[0.25, -0.5, 1.0], 1), vec![0.25, -0.5, 1.0]);
    }

    #[test]
    fn channel_average_is_exact_for_interleaved_frames() {
        // Two stereo frames: (1.0, 0.0) and (-0.5, 0.5).
        assert_eq!(mix_to_mono(&[1.0, 0.0, -0.5, 0.5], 2), vec![0.5, 0.0]);

        // One 3-channel frame, chosen to average without rounding.
        assert_eq!(mix_to_mono(&[0.25, 0.5, 0.75], 3), vec![0.5]);
    }

    /// A trailing partial frame cannot be averaged and must not be invented.
    #[test]
    fn a_partial_trailing_frame_is_dropped_rather_than_guessed() {
        assert_eq!(mix_to_mono(&[1.0, 0.0, -1.0], 2), vec![0.5]);
    }

    #[test]
    fn a_missing_file_is_an_input_error() {
        let err = decode(fixture("no-such-song.mp3")).expect_err("a missing file cannot decode");

        assert!(matches!(err, Error::Input(_)), "got {err:?}");
        let msg = err.to_string();
        assert!(
            msg.contains("no-such-song.mp3"),
            "must name the file: {msg}"
        );
        assert!(
            msg.contains("no such file"),
            "must say what is wrong: {msg}"
        );
    }

    /// A file that ends mid-stream must be reported, not silently analyzed as a
    /// shorter song and not panicked over.
    #[test]
    fn truncated_mp3_yields_input_error_not_panic() {
        let whole = std::fs::read(fixture("tone-tagged.mp3")).expect("read fixture");
        let dir = tempfile::tempdir().expect("tempdir");
        // Not named "truncated": the message assertion below must not be able to
        // pass on the file name alone.
        let path = dir.path().join("half-a-song.mp3");
        std::fs::write(&path, &whole[..1000]).expect("write");

        let err = decode(&path).expect_err("a truncated mp3 cannot decode");

        assert!(matches!(err, Error::Input(_)), "got {err:?}");
        let msg = err.to_string();
        assert!(msg.contains("half-a-song.mp3"), "must name the file: {msg}");
        assert!(
            msg.contains("truncated"),
            "must say the file is incomplete, not `os error 22`: {msg}"
        );
    }

    #[test]
    fn non_mp3_bytes_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("not-audio.mp3");
        std::fs::write(&path, b"this is not an mp3").expect("write");

        let err = decode(&path).expect_err("a text file cannot decode");

        assert!(matches!(err, Error::Input(_)), "got {err:?}");
        let msg = err.to_string();
        assert!(msg.contains("not-audio.mp3"), "must name the file: {msg}");
        assert!(
            msg.contains("not a recognized audio file") || msg.contains("no audio"),
            "must say what is wrong, not `os error 22`: {msg}"
        );
    }
}
