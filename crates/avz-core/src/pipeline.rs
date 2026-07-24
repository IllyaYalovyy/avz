//! Orchestration: analysis → render → encode.
//!
//! Owns the two-pass flow and reports progress through the
//! [`Progress`](crate::Progress) callback trait. Analysis completes fully before
//! the first frame is rendered (`VISION.md` §4.2).
//!
//! The visualizer is the preset `config.visual.preset` names, drawn against the
//! `VISION.md` §6 uniform contract. Almost everything a preset sees comes from
//! [`Globals`]: the palette, the frame's features, and `frame_index / fps` as
//! the only clock. The exceptions are the three optional textures — the previous
//! frame, which the renderer keeps for itself, and this frame's coarse spectrum
//! and the song's recent hits, both of which come from the timeline beside its
//! features.
//!
//! Each frame is a layer stack flattened by the [`Compositor`]: the background —
//! the palette backdrop, with `background.image` fitted over it — a looped
//! `background.video` over that if there is one, the visualizer's premultiplied
//! light over that, and the title/artist card on top.
//!
//! The background video is the one part of a frame that does not come from the
//! frame index: it has a clock of its own, and a second ffmpeg turns it into one
//! frame per rendered frame. So a render always draws the loop from its first
//! frame, `--sample` included — an excerpt previews the visuals it will get, not
//! the seconds of the loop the full render would have reached by then.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::analysis::{self, EnvelopeParams};
use crate::config::{BackgroundSource, Config, Resolution, SampleRange};
use crate::encode::{EncodeSettings, Encoder, Ffmpeg};
use crate::meta;
use crate::render::{
    AdapterChoice, AdapterKind, Background, BackgroundVideo, Card, CardText, ClipTime, Compositor,
    EffectsPass, Globals, Gpu, Layer, Offscreen, Preset, TextCard, VideoSettings, Visualizer,
    palette,
};
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
    // Before decoding a five-minute song: a typo'd preset name, an unknown
    // parameter, a value outside its range, an unknown palette, or a background
    // image that is not there are all the user's arguments, and they should hear
    // about them in the first millisecond.
    let preset = Preset::by_name(&config.visual.preset)?;
    let params = preset.schema()?.resolve(&config.visual.params)?;
    let palette = palette::resolve(&config.visual.palette)?;
    // A codec this ffmpeg was not built to encode is worth hearing about before
    // the song is analyzed, not after: ffmpeg would say the same thing itself,
    // an hour into a software render.
    crate::encode::ensure_encoder(request.ffmpeg, config.output.codec)?;
    let background = Background::load(&config.background)?;
    let resolution = config.output.resolution;
    warn_if_background_is_upscaled(config, &background, resolution, progress);
    // Shaped and rasterized here too, on the CPU, for the same reason: a font
    // that is not there, and a song with no tags to name, are both things the
    // user can act on before the render begins.
    let card = text_card(
        config,
        request.input,
        (resolution.width, resolution.height),
        progress,
    )?;

    progress.phase_started(Phase::Analyzing, None);
    let audio = analysis::decode(request.input)?;
    // The whole song, never the `--sample` excerpt: the p5/p95 normalization is
    // global by definition, so an excerpt must look the way it does in the render
    // it previews.
    let envelope = EnvelopeParams::for_smoothing(config.visual.smoothing);
    let timeline = analysis::analyze_with(&audio, fps, envelope)?;
    let range = frame_range(timeline.len(), fps, request.sample)?;
    progress.phase_finished(Phase::Analyzing);

    let gpu = Gpu::new(request.adapter)?;
    progress.adapter_selected(gpu.kind(), gpu.adapter_name());
    if gpu.fell_back_to_software() {
        progress.warn(SOFTWARE_FALLBACK_WARNING);
    }

    let target = Offscreen::new(&gpu, resolution.width, resolution.height)?;

    // The effects stage (RFC-002): built only when the config asks for one.
    // At identity the compositor writes straight to the readback target and
    // the render is byte-identical to a build without the stage.
    let effects = if config.effects.is_identity() {
        None
    } else {
        let flat = Layer::new(&gpu, resolution.width, resolution.height, "flattened");
        let pass = EffectsPass::new(&gpu, &flat)?;
        Some((flat, pass))
    };

    // The layer stack, bottom to top (`VISION.md` §5.3). The background layer is
    // the palette backdrop with `background.image` fitted over it; a
    // `background.video` is a layer of its own directly above it, redrawn every
    // frame, and its `contain` letterbox bars are how the backdrop still shows.
    // The text card sits on top of the visuals, and is absent altogether when
    // there is nothing to draw.
    let background = background.layer(&gpu, resolution.width, resolution.height, palette);
    let mut video = background_video(request, resolution, fps, &gpu)?;
    let visual = Layer::new(&gpu, resolution.width, resolution.height, "avz visualizer");
    let visualizer = Visualizer::new(&gpu, preset, &visual)?;

    let text = card
        .as_ref()
        .map(|card| {
            let layer = Layer::new(&gpu, resolution.width, resolution.height, "avz text card");
            TextCard::new(&gpu, card, palette).map(|card| (card, layer))
        })
        .transpose()?;

    let layers: Vec<&Layer> = [&background]
        .into_iter()
        .chain(video.iter().map(|(_, layer)| layer))
        .chain([&visual])
        .chain(text.iter().map(|(_, layer)| layer))
        .collect();
    let compositor = Compositor::new(&gpu, &layers)?;

    // `auto` is the file's own name, hashed (`VISION.md` §5.3). Logged because
    // it is the one input to a render the user never typed, and the one they
    // need in hand to reproduce a video they liked.
    let seed = config.visual.seed.resolve(request.input);
    tracing::debug!(seed, requested = %config.visual.seed, "seeded");

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
        preset = preset.name,
        adapter = %gpu.kind(),
        "rendering"
    );

    progress.phase_started(Phase::Rendering, Some(range.len() as u64));
    let mut pixels = Vec::new();
    for index in range.start..range.end {
        // The song's own frame index, not the excerpt's: `--sample 1s..2s` must
        // draw the same pixels the full render draws at those timestamps.
        let globals = Globals::for_frame(
            index,
            fps,
            (resolution.width, resolution.height),
            seed,
            timeline.frame(index),
            palette,
            params,
        );
        // One frame of the loop per rendered frame, in order, from its first —
        // `--sample` moves the picture and the sound, never the background loop,
        // which has a clock of its own and no timestamp in the song.
        if let Some((video, layer)) = video.as_mut() {
            video.draw(&gpu, layer)?;
        }
        visualizer.draw(
            &gpu,
            &visual,
            &globals,
            timeline.spectrum(index),
            &timeline.onset_history(index),
        );
        // The card was rasterized once; all that moves is the quad's opacity and
        // its offset (`VISION.md` §5.3).
        if let Some((card, layer)) = &text {
            card.draw(&gpu, layer, index, fps);
        }
        match &effects {
            None => compositor.composite(&gpu, &target),
            Some((flat, pass)) => {
                compositor.composite_into(&gpu, flat);
                pass.apply(
                    &gpu,
                    &target,
                    &config.effects,
                    &timeline.frame(index),
                    index as f32 / fps as f32,
                    // Song time above, clip time here: the fade belongs to the
                    // video being written, so `--sample 1s..2s` fades up at its
                    // own first frame rather than a second into an excerpt that
                    // is already over.
                    ClipTime {
                        elapsed: (index - range.start) as f32 / fps as f32,
                        duration: range.len() as f32 / fps as f32,
                    },
                );
            }
        }
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

/// The looped background video and the layer it draws into, or `None` when
/// `[background]` names no video.
///
/// Started here rather than beside [`Background::load`], which runs before the
/// song is decoded: a decoder spawned there would spend the whole analysis pass
/// blocked on a queue nobody is reading. What `load` does check is that the file
/// is there, which is the part the user can act on.
///
/// # Errors
///
/// [`Error::Render`] if ffmpeg will not spawn.
fn background_video(
    request: &RenderRequest<'_>,
    resolution: Resolution,
    fps: u32,
    gpu: &Gpu,
) -> Result<Option<(BackgroundVideo, Layer)>> {
    let config = &request.config.background;
    let Some(BackgroundSource::Video(source)) = &config.source else {
        return Ok(None);
    };

    let settings = VideoSettings {
        width: resolution.width,
        height: resolution.height,
        fps,
        fit: config.fit,
        blur: config.blur,
        darken: config.darken,
    };
    let video = BackgroundVideo::start(request.ffmpeg, source, &settings)?;
    let layer = Layer::new(
        gpu,
        resolution.width,
        resolution.height,
        "avz background video",
    );

    Ok(Some((video, layer)))
}

/// The card `[text]` asks for, shaped and rasterized, or `None` if there is none.
///
/// Three ways to have no card, and only one of them is silent. `enabled = false`
/// is what the user asked for. A song with neither tag and no override is the
/// risk-matrix row `docs/TESTING.md` names: warn, skip the card, and render the
/// video they came for. Words the font cannot draw at all are the same outcome
/// by a different route.
///
/// # Errors
///
/// [`Error::Input`] if `[text] font` names a file that is not a font, or if the
/// song's tags cannot be read at all — which is the song being unreadable, and
/// the decoder would have said so a moment later.
fn text_card(
    config: &Config,
    input: &Path,
    frame: (u32, u32),
    progress: &dyn Progress,
) -> Result<Option<Card>> {
    if !config.text.enabled {
        return Ok(None);
    }

    let tags = meta::read(input)?;
    let words = CardText::resolve(&config.text, tags.title.as_deref(), tags.artist.as_deref());
    if words.is_empty() {
        progress.warn(&no_words_warning(input));
        return Ok(None);
    }

    let card = Card::prepare(&config.text, &words, frame)?;
    if card.is_none() {
        progress.warn(&no_ink_warning(&config.text.font));
    }
    Ok(card)
}

/// What to say when the song names neither a title nor an artist.
///
/// Actionable, per `AGENTS.md`: what happened, and the two flags that answer it.
fn no_words_warning(input: &Path) -> String {
    format!(
        "`{}` has no ID3 title or artist, so the text card was skipped — pass \
         `--title` and `--artist` to set them, or `--no-text` to silence this",
        input.display(),
    )
}

/// Say so when `background.image` is smaller than the frame it has to fill.
fn warn_if_background_is_upscaled(
    config: &Config,
    background: &Background,
    resolution: Resolution,
    progress: &dyn Progress,
) {
    let (Some(BackgroundSource::Image(path)), Some(image)) =
        (&config.background.source, background.image_size())
    else {
        return;
    };

    let frame = (resolution.width, resolution.height);
    if needs_upscaling(image, frame) {
        progress.warn(&upscale_warning(path, image, frame));
    }
}

/// Whether `image` has to be enlarged on some axis to cover `frame`.
///
/// True on either axis, whatever the fit mode does with the other: `cover` and
/// `stretch` enlarge the short axis past its pixels, and `contain` enlarges the
/// binding one. A background bigger than the frame on both axes is only ever
/// downsampled, which costs nothing anyone can see.
fn needs_upscaling(image: (u32, u32), frame: (u32, u32)) -> bool {
    image.0 < frame.0 || image.1 < frame.1
}

/// What to say when the background image is smaller than the frame it fills.
///
/// Nothing errors here — the image is simply stretched, and the render comes
/// back soft. The blur is the honest way out: an image that was going to be
/// blurred anyway loses nothing by being enlarged first.
fn upscale_warning(path: &Path, image: (u32, u32), frame: (u32, u32)) -> String {
    format!(
        "`{}` is {}x{}, smaller than the {}x{} frame, so it will be upscaled and look \
         soft — supply an image at least {}x{}, or `--set background.blur=6` to hide it",
        path.display(),
        image.0,
        image.1,
        frame.0,
        frame.1,
        frame.0,
        frame.1,
    )
}

/// What to say when the words are there and the font cannot draw any of them.
fn no_ink_warning(font: &crate::config::FontChoice) -> String {
    let font = match font {
        crate::config::FontChoice::Auto => "the bundled font".to_owned(),
        crate::config::FontChoice::Path(path) => format!("`{}`", path.display()),
    };
    format!(
        "{font} has no glyphs for the title or artist, so the text card was \
         skipped — pass `--set text.font=PATH` a font that covers them, or \
         `--no-text` to silence this",
    )
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
    use crate::config::{FontChoice, Palette, Seconds, Seed};

    fn sample(start: &str, end: &str) -> SampleRange {
        format!("{start}..{end}").parse().expect("a sample range")
    }

    /// `AGENTS.md`, CLI invariants: a warning names the consequence and the
    /// action. The em dash separates the two halves, and the action always
    /// quotes the flag or config key that answers it.
    fn is_actionable(warning: &str) -> bool {
        warning.contains('—') && warning.contains('`')
    }

    /// The canonical warning `AGENTS.md` writes out: what happened, what it
    /// costs, and how to silence it. Pinned as a string so it stays folklore-free.
    #[test]
    fn warning_text_for_software_fallback_matches_contract() {
        let warning = SOFTWARE_FALLBACK_WARNING;

        assert!(warning.contains("no GPU adapter found"), "{warning}");
        assert!(warning.contains("software rendering"), "{warning}");
        assert!(warning.contains("fps"), "the cost is named: {warning}");
        assert!(
            warning.contains("`--adapter software` to silence this"),
            "the action is named: {warning}",
        );
        assert!(is_actionable(warning));
    }

    /// Every warning the pipeline can emit, held to the same shape. A new
    /// warning that only says what happened fails here rather than in a render
    /// the user cannot act on.
    #[test]
    fn every_pipeline_warning_names_a_consequence_and_an_action() {
        let warnings = [
            SOFTWARE_FALLBACK_WARNING.to_owned(),
            no_words_warning(Path::new("song.mp3")),
            no_ink_warning(&FontChoice::Auto),
            upscale_warning(Path::new("art/forest.png"), (800, 600), (1920, 1080)),
        ];

        for warning in warnings {
            assert!(is_actionable(&warning), "not actionable: {warning}");
        }
    }

    /// A background smaller than the frame is stretched over it, and the render
    /// comes back soft. Nothing errors, so nothing tells the user unless this does.
    #[test]
    fn a_background_smaller_than_the_frame_warns_that_it_will_be_upscaled() {
        let warning = upscale_warning(Path::new("art/forest.png"), (800, 600), (1920, 1080));

        assert!(warning.contains("art/forest.png"), "{warning}");
        assert!(warning.contains("800x600"), "the image size: {warning}");
        assert!(warning.contains("1920x1080"), "the frame size: {warning}");
        assert!(
            warning.contains("background.blur"),
            "the action that hides the softness: {warning}",
        );
    }

    /// Either axis short of the frame is an upscale on that axis, whatever the
    /// fit mode does with the other one.
    #[test]
    fn only_an_image_short_of_the_frame_on_some_axis_is_upscaled() {
        assert!(!needs_upscaling((1920, 1080), (1920, 1080)));
        assert!(!needs_upscaling((3840, 2160), (1920, 1080)));
        assert!(needs_upscaling((1920, 1079), (1920, 1080)));
        assert!(needs_upscaling((1919, 1080), (1920, 1080)));
        assert!(
            needs_upscaling((4000, 100), (1920, 1080)),
            "a wide sliver is still stretched vertically",
        );
    }

    /// The seed the shader is handed is the one the config asked for, resolved
    /// against the song being rendered. `Seed::resolve` is tested against every
    /// path shape in `config::tests`; this pins that the pipeline asks it at
    /// all, and asks it about the *input*.
    #[test]
    fn the_render_seed_is_the_configured_seed_resolved_against_the_input() {
        let song = Path::new("/music/cold-design/winter.mp3");

        assert_eq!(Seed::Fixed(7).resolve(song), 7);
        assert_eq!(
            Seed::Auto.resolve(song),
            Seed::Auto.resolve(Path::new("elsewhere/winter.mp3")),
            "a song moved between directories renders the same video",
        );
        assert_ne!(Seed::Auto.resolve(song), Seed::Auto.resolve(Path::new("x")));
    }

    /// The zero-config render has to resolve. A default naming a palette nobody
    /// ships would fail every `avz render song.mp3`.
    #[test]
    fn the_default_config_names_a_palette_that_resolves() {
        let config = Config::default();

        assert_eq!(config.visual.palette, Palette::Named("ember".to_owned()));
        palette::resolve(&config.visual.palette).expect("the default palette resolves");
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
