//! Orchestration: analysis → render → encode.
//!
//! Owns the two-pass flow and reports progress through the
//! [`Progress`](crate::Progress) callback trait. Analysis completes fully before
//! the first frame is rendered (`VISION.md` §4.2).
//!
//! The visualizer is still the M1 tracer bullet: a fullscreen clear whose
//! brightness follows loudness. Real presets arrive in RFC-001 Step 14 and
//! replace [`tracer_color`] alone — every other seam here is the final one.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::analysis::{self, FeatureFrame};
use crate::config::{Config, SampleRange};
use crate::encode::{EncodeSettings, Encoder, Ffmpeg};
use crate::render::{AdapterChoice, AdapterKind, Gpu, Offscreen};
use crate::{Error, Phase, Progress, Result};

/// Everything one `avz render` needs to know.
///
/// Borrowed rather than owned so the CLI can keep its parsed arguments and hand
/// the pipeline a view of them.
#[derive(Debug)]
pub struct RenderRequest<'a> {
    /// The mp3 to render. Decoded for analysis, muxed untouched into the output.
    pub input: &'a Path,
    /// Where the finished mp4 lands.
    pub output: &'a Path,
    /// The resolved configuration.
    pub config: &'a Config,
    /// Which Vulkan adapter to render on.
    pub adapter: AdapterChoice,
    /// Render only this excerpt of the song. `None` renders all of it.
    pub sample: Option<SampleRange>,
    /// The ffmpeg binary [`preflight`](crate::encode::preflight) approved.
    pub ffmpeg: &'a Ffmpeg,
}

/// What a finished render turned out to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderSummary {
    /// How many video frames were written.
    pub frames: u64,
    /// The frame rate they were written at.
    pub fps: u32,
    /// The adapter they were rendered on.
    pub adapter: AdapterKind,
    /// Where the mp4 was written.
    pub output: PathBuf,
}

impl RenderSummary {
    /// How long the rendered video plays for: `frames / fps`.
    pub fn duration(&self) -> Duration {
        Duration::from_secs_f64(self.frames as f64 / f64::from(self.fps))
    }
}

/// The one warning `VISION.md` §3 spells out, verbatim in spirit: say what
/// happened, what it costs, and how to silence it.
const SOFTWARE_FALLBACK_WARNING: &str = "no GPU adapter found, falling back to software rendering — expect roughly 5-15 fps \
     instead of hundreds; pass `--adapter software` to silence this";

/// Render `request.input` to `request.output`.
///
/// Analysis runs to completion first, then every frame in the requested range is
/// drawn, read back, and piped to ffmpeg. Nothing appears at the output path
/// until ffmpeg exits cleanly.
///
/// # Errors
///
/// [`Error::Input`] for a file that will not decode, [`Error::Config`] for a
/// sample range the song cannot satisfy, [`Error::Render`] for adapter or
/// readback failures, and [`Error::Encode`] if ffmpeg refuses or dies. A failure
/// anywhere leaves no output file and no partial one.
pub fn render(request: &RenderRequest<'_>, progress: &dyn Progress) -> Result<RenderSummary> {
    let config = request.config;
    let fps = config.output.fps;

    progress.phase_started(Phase::Analyzing, None);
    let audio = analysis::decode(request.input)?;
    let timeline = analysis::analyze(&audio, fps)?;
    let range = frame_range(timeline.len(), fps, request.sample)?;
    progress.phase_finished(Phase::Analyzing);

    let gpu = Gpu::new(request.adapter)?;
    progress.adapter_selected(gpu.kind(), gpu.adapter_name());
    if gpu.fell_back_to_software() {
        progress.warn(SOFTWARE_FALLBACK_WARNING);
    }

    let resolution = config.output.resolution;
    let target = Offscreen::new(&gpu, resolution.width, resolution.height)?;

    let settings = EncodeSettings {
        width: resolution.width,
        height: resolution.height,
        fps,
        codec: config.output.codec,
        quality: config.output.quality,
        audio_start: audio_start(range, fps),
    };
    let mut encoder = Encoder::start(request.ffmpeg, &settings, request.input, request.output)?;

    tracing::debug!(
        frames = range.len(),
        first = range.start,
        %resolution,
        fps,
        adapter = %gpu.kind(),
        "rendering"
    );

    progress.phase_started(Phase::Rendering, Some(range.len() as u64));
    let mut pixels = Vec::new();
    for index in range.start..range.end {
        target.clear(&gpu, tracer_color(timeline.frame(index)));
        target.read_rgba_into(&gpu, &mut pixels)?;
        encoder.write_frame(&pixels)?;
        progress.advance(Phase::Rendering, 1);
    }
    progress.phase_finished(Phase::Rendering);

    progress.phase_started(Phase::Finalizing, None);
    encoder.finish()?;
    progress.phase_finished(Phase::Finalizing);

    Ok(RenderSummary {
        frames: range.len() as u64,
        fps,
        adapter: gpu.kind(),
        output: request.output.to_path_buf(),
    })
}

/// The M1 tracer bullet's entire visualizer: brightness follows loudness.
///
/// Returned in linear space, because the render target is sRGB and encodes on
/// write. `rms` is used raw — normalization against the song's own dynamic range
/// arrives with RFC-001 Step 13, and a gain invented here would only have to be
/// removed then.
fn tracer_color(frame: FeatureFrame) -> [f32; 4] {
    let level = frame.rms.clamp(0.0, 1.0);
    [level, level, level, 1.0]
}

/// The half-open range of timeline frames a render covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FrameRange {
    /// The first video frame to render.
    start: usize,
    /// One past the last video frame to render.
    end: usize,
}

impl FrameRange {
    /// How many frames the range covers.
    fn len(self) -> usize {
        self.end - self.start
    }
}

/// Which timeline frames `sample` asks for, out of a song `frames` long.
///
/// # Errors
///
/// [`Error::Config`] if the range starts past the end of the song, or is too
/// short to contain a single video frame. Both are the user's arguments, not the
/// pipeline's problem — and both would otherwise reach ffmpeg as an empty video.
fn frame_range(frames: usize, fps: u32, sample: Option<SampleRange>) -> Result<FrameRange> {
    let Some(sample) = sample else {
        return Ok(FrameRange {
            start: 0,
            end: frames,
        });
    };

    let song_secs = frames as f64 / f64::from(fps);
    let start = frame_at(sample.start.as_secs_f64(), fps);
    if start >= frames {
        return Err(Error::Config(format!(
            "the sample starts at {:.2}s, but the song is only {song_secs:.2}s long",
            sample.start.as_secs_f64(),
        )));
    }

    let end = frame_at(sample.end.as_secs_f64(), fps).min(frames);
    if end <= start {
        return Err(Error::Config(format!(
            "the sample {:.3}s..{:.3}s is shorter than one frame at {fps} fps",
            sample.start.as_secs_f64(),
            sample.end.as_secs_f64(),
        )));
    }

    Ok(FrameRange { start, end })
}

/// How close a boundary must land to a frame timestamp to count as being on it.
///
/// `1.1s` at 30 fps is frame 33, but `1.1 * 30.0` is a hair above `33.0` in
/// binary floating point, and a bare `ceil` would answer 34. A microsecond of
/// slack is far below one frame at any frame rate avz will encode.
const FRAME_EPSILON: f64 = 1e-6;

/// The first video frame whose timestamp is at or after `secs`.
///
/// Frame `i` shows at `i / fps` (`FeatureTimeline::timestamp`), so this is the
/// inverse of that clock — the only way a sample boundary is turned into a frame
/// index anywhere in avz.
fn frame_at(secs: f64, fps: u32) -> usize {
    let exact = secs * f64::from(fps);
    (exact - FRAME_EPSILON).ceil().max(0.0) as usize
}

/// Where in the song the muxed audio must start: the first rendered frame's
/// timestamp.
///
/// Derived from the frame index rather than from the seconds the user typed, so
/// picture and sound start at the same instant even when `--sample 1.1s` names a
/// moment that falls between two frames.
fn audio_start(range: FrameRange, fps: u32) -> Duration {
    Duration::from_secs_f64(range.start as f64 / f64::from(fps))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Seconds;

    fn sample(start: &str, end: &str) -> SampleRange {
        format!("{start}..{end}").parse().expect("a sample range")
    }

    fn frame(rms: f32) -> FeatureFrame {
        FeatureFrame { rms }
    }

    #[test]
    fn silence_renders_black_and_full_scale_renders_white() {
        assert_eq!(tracer_color(frame(0.0)), [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(tracer_color(frame(1.0)), [1.0, 1.0, 1.0, 1.0]);
    }

    /// The M1 acceptance criterion, as an assertion: brightness follows loudness.
    #[test]
    fn a_louder_frame_renders_brighter_than_a_quieter_one() {
        let quiet = tracer_color(frame(0.1));
        let loud = tracer_color(frame(0.6));

        for channel in 0..3 {
            assert!(
                loud[channel] > quiet[channel],
                "channel {channel}: {loud:?} is not brighter than {quiet:?}"
            );
        }
    }

    /// mp3 decodes a hair outside `-1.0..=1.0`, and RMS is not yet normalized, so
    /// a level above one is reachable. It must clamp, not wrap or wash out.
    #[test]
    fn a_frame_hotter_than_full_scale_clamps_to_opaque_white() {
        assert_eq!(tracer_color(frame(1.4)), [1.0, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn every_rendered_frame_is_fully_opaque() {
        for rms in [0.0, 0.25, 1.0, 2.0] {
            assert_eq!(tracer_color(frame(rms))[3], 1.0, "rms {rms} is translucent");
        }
    }

    #[test]
    fn no_sample_renders_every_frame_of_the_song() {
        let range = frame_range(150, 30, None).expect("a whole song is a valid range");

        assert_eq!(range, FrameRange { start: 0, end: 150 });
        assert_eq!(range.len(), 150);
    }

    /// `--sample 60s` is shorthand for the first 60 seconds (`VISION.md` §3).
    #[test]
    fn a_bare_duration_samples_the_start_of_the_song() {
        let range = frame_range(300, 30, Some("2s".parse().expect("a sample range")))
            .expect("the first two seconds exist");

        assert_eq!(range, FrameRange { start: 0, end: 60 });
    }

    #[test]
    fn a_sample_range_selects_the_frames_that_cover_it() {
        let range = frame_range(300, 30, Some(sample("0:01", "0:03"))).expect("a valid range");

        assert_eq!(range, FrameRange { start: 30, end: 90 });
        assert_eq!(range.len(), 60);
    }

    /// A boundary lands on the first frame at or after it. `1.1s` at 30 fps is
    /// frame 33, and the binary representation of `1.1` must not make it 34.
    #[test]
    fn a_sample_boundary_lands_on_the_frame_whose_timestamp_it_names() {
        assert_eq!(frame_at(0.0, 30), 0);
        assert_eq!(frame_at(1.0, 30), 30);
        assert_eq!(frame_at(1.1, 30), 33);
        assert_eq!(frame_at(45.0, 30), 1_350);
        assert_eq!(frame_at(2.5, 24), 60);

        // Between two frames, the later one shows the sampled moment.
        assert_eq!(frame_at(1.001, 30), 31);
    }

    /// A sample that runs past the end of the song renders what exists rather
    /// than failing: `--sample 60s` on a 5-second song is a whole-song render.
    #[test]
    fn a_sample_that_overruns_the_song_is_clamped_to_its_last_frame() {
        let range = frame_range(150, 30, Some("60s".parse().expect("a sample range")))
            .expect("an overrunning sample renders what exists");

        assert_eq!(range, FrameRange { start: 0, end: 150 });
    }

    /// Starting past the end would render nothing at all, which reaches ffmpeg
    /// as an empty video. Say so in terms of the song the user gave.
    #[test]
    fn a_sample_that_starts_after_the_song_ends_is_a_config_error() {
        let err = frame_range(150, 30, Some(sample("6s", "8s")))
            .expect_err("a 5 second song has nothing at 6 seconds");

        assert!(matches!(err, Error::Config(_)), "got {err:?}");
        let msg = err.to_string();
        assert!(msg.contains("6.00s"), "must quote the sample start: {msg}");
        assert!(
            msg.contains("5.00s"),
            "must say how long the song is: {msg}"
        );
    }

    /// At 1 fps, `0.1s..0.2s` names no frame at all.
    #[test]
    fn a_sample_shorter_than_one_frame_is_a_config_error() {
        let err = frame_range(150, 1, Some(sample("100ms", "200ms")))
            .expect_err("a tenth of a second holds no frame at 1 fps");

        assert!(matches!(err, Error::Config(_)), "got {err:?}");
        assert!(err.to_string().contains("shorter than one frame"), "{err}");
    }

    /// Sound must start where the picture starts. The seconds the user typed may
    /// fall between two frames; the frame index never does.
    #[test]
    fn the_audio_starts_at_the_first_rendered_frames_timestamp() {
        let range = frame_range(300, 30, Some(sample("1.001s", "3s"))).expect("a valid range");

        assert_eq!(range.start, 31);
        assert_eq!(
            audio_start(range, 30),
            Duration::from_secs_f64(31.0 / 30.0),
            "the audio must not start at the 1.001s the user typed"
        );
    }

    #[test]
    fn a_whole_song_render_starts_the_audio_at_the_beginning() {
        let range = frame_range(150, 30, None).expect("a whole song");

        assert_eq!(audio_start(range, 30), Duration::ZERO);
    }

    #[test]
    fn a_summary_plays_for_as_long_as_its_frames_last() {
        let summary = RenderSummary {
            frames: 60,
            fps: 30,
            adapter: AdapterKind::Software,
            output: PathBuf::from("out.mp4"),
        };

        assert_eq!(summary.duration(), Duration::from_secs(2));
    }

    /// `Seconds` is what `SampleRange` is built from; a range that parses must
    /// keep the ordering the pipeline relies on.
    #[test]
    fn a_sample_range_always_ends_after_it_starts() {
        let range = sample("0:45", "1:45");

        assert_eq!(range.start, "45s".parse::<Seconds>().expect("45 seconds"));
        assert_eq!(range.duration_secs(), 60.0);
    }
}
