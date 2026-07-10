//! The looped background video: a second ffmpeg, one frame per rendered frame.
//!
//! `VISION.md` §5.3 gives the background layer three forms, and this is the
//! third. A dedicated ffmpeg subprocess decodes the video, loops it forever,
//! fits it to the frame, and converts its frame rate, writing raw RGBA to its
//! stdout — so avz's side of the contract is "read `width * height * 4` bytes,
//! upload them, repeat". Looping, scaling, and fps conversion are the parts with
//! the sharp edges, and ffmpeg already knows how to do all three.
//!
//! **Muted by construction.** `-an` is passed to the reader, so the background
//! video's audio stream is never even decoded. The only sound in an avz render
//! is the original mp3, muxed with `-c:a copy` (`AGENTS.md`, audio).
//!
//! **A stall is an error, not a hang.** `VISION.md` §11 names a stalled decode
//! thread as a risk. The reader thread pushes frames into a *bounded* channel —
//! so a decoder faster than the renderer buffers two frames and then blocks,
//! rather than reading a five-minute loop into memory — and the render thread
//! waits on it with a timeout, so a decoder slower than [`FRAME_TIMEOUT`]
//! produces a message naming the video instead of a render that never returns.
//! `scripts/quality.d/43-the-background-video-reader-is-bounded-and-times-out.sh`
//! guards both halves, because a test cannot see the difference between a bounded
//! channel and an unbounded one.
//!
//! **The alpha channel is binary, and that is a choice.** The layer stack blends
//! premultiplied alpha (`VISION.md` §5.3), and premultiplied and straight alpha
//! agree exactly where alpha is 0 or 255. So the filter chain flattens the
//! source's own alpha (`format=rgb24`) *before* it scales, and only then pads a
//! `contain` letterbox with transparent black — which leaves every pixel either
//! opaque video or an empty bar, and lets the frame ffmpeg wrote be uploaded
//! byte for byte, with no per-frame premultiply in the way.
//!
//! **`blur` and `darken` still happen in light.** Where the image background
//! pays for them once per render, a video pays once per frame — so the default
//! (`blur = 0`, `darken = 0`) costs nothing at all, a darken alone is a
//! 256-entry lookup table, and only a blur takes the full trip through linear
//! f32 that `background.rs` explains.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::{self, BufRead as _, BufReader, Read as _};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdout, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel};
use std::thread::JoinHandle;
use std::time::Duration;

use image::{Rgba, Rgba32FImage, imageops};

use crate::config::Fit;
use crate::encode::Ffmpeg;
use crate::render::layer::Layer;
use crate::render::offscreen::Gpu;
use crate::render::palette::{linear_to_srgb, srgb_to_linear};
use crate::render::readback::BYTES_PER_PIXEL;
use crate::{Error, Result};

/// How many decoded frames may wait for the renderer.
///
/// Two: enough that the decode of frame `n + 1` overlaps the render of frame
/// `n`, and few enough that a background video longer than the song cannot be
/// read into memory. An unbounded channel would turn a decoder that outruns
/// lavapipe — which every decoder does — into a slow memory leak.
const FRAME_QUEUE: usize = 2;

/// How long a render waits for one background frame before giving up.
///
/// Generous, because the first frame arrives behind ffmpeg's startup and a seek
/// over a long file, and because a software render's own frames take hundreds of
/// milliseconds. Short enough that a wedged decoder is a message rather than a
/// job someone kills the next morning.
const FRAME_TIMEOUT: Duration = Duration::from_secs(30);

/// How many trailing stderr lines are kept to explain a failure. As in
/// [`Encoder`](crate::encode::Encoder): ffmpeg's diagnosis is at the end.
const STDERR_TAIL_LINES: usize = 8;

/// What the background video is decoded into.
///
/// Mirrors the resolved `[output]` and `[background]` config, decoupled from
/// both so the reader can be driven from a test without a whole
/// [`Config`](crate::config::Config).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VideoSettings {
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Frames per second, which ffmpeg converts the source to.
    pub fps: u32,
    /// How the video is fitted to the frame.
    pub fit: Fit,
    /// Gaussian standard deviation, in pixels of the output frame.
    pub blur: f32,
    /// How much light to take away, `0.0..=1.0`.
    pub darken: f32,
}

impl VideoSettings {
    /// Bytes in one tightly packed RGBA frame.
    fn frame_bytes(&self) -> usize {
        self.width as usize * self.height as usize * BYTES_PER_PIXEL as usize
    }
}

/// The ffmpeg filter chain that turns any source into one `width × height` RGBA
/// frame, fitted per `fit`.
///
/// `format=rgb24` leads every chain, and is the reason the alpha that comes back
/// is binary: it discards whatever alpha the source had, before any resampling
/// can smear it. The `contain` pad then reintroduces alpha, and only as the
/// fully transparent black of a letterbox bar — which the compositor draws the
/// palette backdrop through, exactly as it does under a `contain` image.
///
/// `cover` is a scale-then-crop rather than a crop-then-scale because ffmpeg
/// bounds the intermediate by the frame either way: `force_original_aspect_ratio`
/// enlarges the short axis only until the frame is covered.
fn fit_filter(fit: Fit, width: u32, height: u32) -> String {
    match fit {
        Fit::Stretch => format!("format=rgb24,scale={width}:{height}"),
        Fit::Cover => format!(
            "format=rgb24,scale={width}:{height}:force_original_aspect_ratio=increase,\
             crop={width}:{height}"
        ),
        Fit::Contain => format!(
            "format=rgb24,scale={width}:{height}:force_original_aspect_ratio=decrease,\
             format=rgba,pad={width}:{height}:(ow-iw)/2:(oh-ih)/2:color=black@0"
        ),
    }
}

/// Build the argument vector for the background-video reader.
///
/// `-stream_loop -1` is an *input* option and must precede `-i`, or ffmpeg loops
/// nothing. `-vf`, `-r`, and the rawvideo format are *output* options describing
/// the bytes avz reads off stdout.
fn ffmpeg_args(source: &Path, settings: &VideoSettings) -> Vec<OsString> {
    let mut args: Vec<OsString> = ["-hide_banner", "-nostats", "-loglevel", "error"]
        .iter()
        .map(OsString::from)
        .collect();

    // Forever, so a loop shorter than the song never runs out and a loop longer
    // than it is simply cut off when avz stops reading.
    args.push("-stream_loop".into());
    args.push("-1".into());
    args.push("-i".into());
    args.push(source.into());

    args.extend(
        [
            // The background video's sound is ignored by construction: the only
            // audio in an avz render is the song, copied from the mp3.
            "-an",
            "-sn",
            "-dn",
            "-vf",
            &fit_filter(settings.fit, settings.width, settings.height),
            "-r",
            &settings.fps.to_string(),
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgba",
            "pipe:1",
        ]
        .iter()
        .map(OsString::from),
    );

    args
}

/// A running ffmpeg subprocess decoding a looped background video.
///
/// One frame per [`BackgroundVideo::next_frame`], forever. Dropping it kills
/// ffmpeg and joins its threads.
#[derive(Debug)]
pub struct BackgroundVideo {
    child: Child,
    /// `None` once dropped, which is what unblocks a reader stuck on a full queue.
    frames: Option<Receiver<Vec<u8>>>,
    reader: Option<JoinHandle<()>>,
    stderr: Option<JoinHandle<Vec<String>>>,
    program: PathBuf,
    source: PathBuf,
    width: u32,
    height: u32,
    timeout: Duration,
}

impl BackgroundVideo {
    /// Spawn ffmpeg and start decoding `source` into `settings`-shaped frames.
    ///
    /// Returns as soon as the process exists: a source ffmpeg cannot open is
    /// reported by the first [`BackgroundVideo::next_frame`], in ffmpeg's own
    /// words. The pipeline checks the path itself, before the song is decoded,
    /// so that is the rare case.
    ///
    /// # Errors
    ///
    /// [`Error::Render`] if the process will not spawn. Preflight the binary
    /// with [`preflight`](crate::encode::preflight) first.
    pub fn start(ffmpeg: &Ffmpeg, source: &Path, settings: &VideoSettings) -> Result<Self> {
        Self::start_with_timeout(ffmpeg, source, settings, FRAME_TIMEOUT)
    }

    /// [`BackgroundVideo::start`] with the stall timeout named.
    ///
    /// Only the tests name it: thirty seconds of a wedged decoder is a slow way
    /// to assert that the wedge is reported at all.
    fn start_with_timeout(
        ffmpeg: &Ffmpeg,
        source: &Path,
        settings: &VideoSettings,
        timeout: Duration,
    ) -> Result<Self> {
        let args = ffmpeg_args(source, settings);
        let program = ffmpeg.program();

        let mut child = Command::new(program)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| {
                Error::Render(format!(
                    "cannot run `{}` to decode `{}`: {err}",
                    program.display(),
                    source.display(),
                ))
            })?;

        let stdout = child.stdout.take().expect("stdout was piped");
        let stderr = child.stderr.take().expect("stderr was piped");

        tracing::debug!(
            program = %program.display(),
            args = ?args,
            source = %source.display(),
            "background video started"
        );

        let (sender, frames) = sync_channel(FRAME_QUEUE);

        Ok(Self {
            child,
            frames: Some(frames),
            reader: Some(read_frames(stdout, sender, *settings)),
            stderr: Some(drain_stderr(stderr)),
            program: program.to_path_buf(),
            source: source.to_path_buf(),
            width: settings.width,
            height: settings.height,
            timeout,
        })
    }

    /// The next frame of the loop: tightly packed, premultiplied sRGB RGBA.
    ///
    /// # Errors
    ///
    /// [`Error::Render`] if the decoder produced nothing for [`FRAME_TIMEOUT`],
    /// or if it stopped producing frames at all — which is ffmpeg having died,
    /// and its last words say why.
    pub fn next_frame(&mut self) -> Result<Vec<u8>> {
        let Some(frames) = self.frames.as_ref() else {
            return Err(Error::Render(format!(
                "the background video `{}` is no longer being decoded",
                self.source.display(),
            )));
        };

        match frames.recv_timeout(self.timeout) {
            Ok(frame) => Ok(frame),
            Err(RecvTimeoutError::Timeout) => Err(Error::Render(self.stalled())),
            Err(RecvTimeoutError::Disconnected) => Err(Error::Render(self.died())),
        }
    }

    /// Upload the next frame of the loop into `layer`.
    ///
    /// # Errors
    ///
    /// Whatever [`BackgroundVideo::next_frame`] failed with.
    pub fn draw(&mut self, gpu: &Gpu, layer: &Layer) -> Result<()> {
        let frame = self.next_frame()?;

        gpu.queue().write_texture(
            layer.texture().as_image_copy(),
            &frame,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(self.width * BYTES_PER_PIXEL),
                rows_per_image: Some(self.height),
            },
            layer.texture().size(),
        );
        gpu.queue().submit([]);

        Ok(())
    }

    /// What to say when the decoder produced no frame in time.
    ///
    /// Actionable, per `AGENTS.md`: the consequence, then the two ways out — a
    /// video ffmpeg can decode faster, or none at all.
    fn stalled(&self) -> String {
        format!(
            "the background video `{}` produced no frame in {}s, so the render cannot continue — \
             re-encode it as a plain h264 mp4, or drop `background.video` to render without it",
            self.source.display(),
            self.timeout.as_secs(),
        )
    }

    /// What to say when the decoder stopped, taking ffmpeg's last words with it.
    fn died(&mut self) -> String {
        let stderr = self.stderr_tail();
        format!(
            "`{}` stopped decoding the background video `{}`{}",
            self.program.display(),
            self.source.display(),
            complaint(&stderr),
        )
    }

    /// Join the stderr reader. Only call once the process has stopped writing,
    /// or this blocks until it does.
    fn stderr_tail(&mut self) -> Vec<String> {
        self.stderr
            .take()
            .and_then(|reader| reader.join().ok())
            .unwrap_or_default()
    }
}

impl Drop for BackgroundVideo {
    /// An abandoned render leaves no ffmpeg decoding a video nobody is watching.
    ///
    /// The receiver goes first: the reader thread is almost always blocked on a
    /// full queue, and a dropped receiver is what turns its `send` into an error
    /// it can exit on. Killing ffmpeg first would leave it blocked on a `send`
    /// that nothing will ever take, and the `join` below would never return.
    fn drop(&mut self) {
        drop(self.frames.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        let _ = self.stderr_tail();
    }
}

/// Read whole frames off ffmpeg's stdout on a dedicated thread.
///
/// The thread is not optional, and neither is the bound on its channel. Reading
/// on the render thread would serialize decode behind render; reading into an
/// unbounded channel would let a decoder that outruns the renderer — which every
/// decoder outruns on lavapipe — buffer the whole loop in memory.
///
/// `blur` and `darken` are applied here rather than on the render thread, so the
/// one place a background video costs real CPU overlaps the GPU instead of
/// waiting for it.
fn read_frames(
    stdout: ChildStdout,
    sender: SyncSender<Vec<u8>>,
    settings: VideoSettings,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut stdout = BufReader::new(stdout);
        let darken = darken_lut(settings.darken);

        loop {
            let mut frame = vec![0u8; settings.frame_bytes()];
            // A short read is ffmpeg exiting mid-frame: there is no partial
            // frame to show, and `next_frame` reports the disconnect with the
            // process's own explanation.
            if stdout.read_exact(&mut frame).is_err() {
                return;
            }

            let frame = shade(frame, &settings, darken.as_ref());
            // The renderer is gone. So is the reason to decode.
            if sender.send(frame).is_err() {
                return;
            }
        }
    })
}

/// Blur and darken one decoded frame, in light rather than in encoded bytes.
///
/// Three paths, and the common one does nothing. `blur = 0, darken = 0` is the
/// default and the whole point of `-pix_fmt rgba`: ffmpeg already wrote the
/// bytes the layer wants. A darken alone never leaves the byte domain, because
/// `linear_to_srgb(srgb_to_linear(b) * keep)` is a function of one byte and
/// `keep` is constant for the whole render. Only a blur has to average light,
/// and it pays the trip through linear f32 that `background.rs` describes.
fn shade(frame: Vec<u8>, settings: &VideoSettings, darken: Option<&[u8; 256]>) -> Vec<u8> {
    if settings.blur <= 0.0 {
        let Some(darken) = darken else {
            return frame;
        };
        return dim(frame, darken);
    }

    let light = linear(&frame, settings.width, settings.height);
    encode(&imageops::fast_blur(&light, settings.blur), settings.darken)
}

/// `linear_to_srgb(srgb_to_linear(byte) * (1 - darken))` for every byte, or
/// `None` when `darken` takes no light away.
///
/// Built once per render. The alternative is two transfer functions and a
/// multiply on every colour channel of every pixel of every frame — sixty-two
/// million `powf` calls a second at 1080p30, for a table with 256 entries in it.
fn darken_lut(darken: f32) -> Option<[u8; 256]> {
    if darken <= 0.0 {
        return None;
    }

    let keep = 1.0 - darken;
    let mut lut = [0u8; 256];
    for (encoded, dimmed) in lut.iter_mut().enumerate() {
        // `encoded` indexes 0..256, so the cast is exact.
        *dimmed = linear_to_srgb(srgb_to_linear(encoded as u8) * keep);
    }
    Some(lut)
}

/// Dim every colour channel through `lut`, leaving coverage alone.
///
/// Alpha is coverage, not light: a darkened letterbox bar is still a bar. And
/// because the alpha of a decoded frame is 0 or 255 and nothing between,
/// dimming the colour channels of a premultiplied pixel keeps `rgb <= a`.
fn dim(mut frame: Vec<u8>, lut: &[u8; 256]) -> Vec<u8> {
    for pixel in frame.chunks_exact_mut(BYTES_PER_PIXEL as usize) {
        for channel in &mut pixel[..3] {
            *channel = lut[usize::from(*channel)];
        }
    }
    frame
}

/// A decoded frame as premultiplied linear light.
///
/// Premultiplying is a no-op on the alpha ffmpeg was asked for — 0 or 255, never
/// between — but it is written out anyway, because the blur that follows is only
/// correct on premultiplied values, and a future filter chain that softened an
/// edge would otherwise fringe toward black.
fn linear(frame: &[u8], width: u32, height: u32) -> Rgba32FImage {
    let mut image = Rgba32FImage::new(width, height);

    for (pixel, target) in frame
        .chunks_exact(BYTES_PER_PIXEL as usize)
        .zip(image.pixels_mut())
    {
        let coverage = f32::from(pixel[3]) / 255.0;
        *target = Rgba([
            srgb_to_linear(pixel[0]) * coverage,
            srgb_to_linear(pixel[1]) * coverage,
            srgb_to_linear(pixel[2]) * coverage,
            coverage,
        ]);
    }

    image
}

/// Dim `image` by `darken` and encode it as the premultiplied sRGB bytes a layer
/// stores.
///
/// The mirror of [`linear`], and of `background.rs`'s own `encode`: dimming is a
/// multiply in light, and the transfer function is applied exactly once, here.
fn encode(image: &Rgba32FImage, darken: f32) -> Vec<u8> {
    let keep = 1.0 - darken;

    let mut frame = Vec::with_capacity(image.as_raw().len());
    for pixel in image.pixels() {
        let coverage = pixel.0[3].clamp(0.0, 1.0);
        for light in &pixel.0[..3] {
            // A blur of premultiplied light cannot emit more than it covers, and
            // neither may its encoding.
            frame.push(linear_to_srgb(light.clamp(0.0, coverage) * keep));
        }
        frame.push((coverage * 255.0).round() as u8);
    }
    frame
}

/// Read ffmpeg's stderr on a dedicated thread, keeping the last few lines.
///
/// Same reasoning as [`Encoder`](crate::encode::Encoder)'s: ffmpeg blocks once
/// its stderr pipe fills, and it blocks before writing the frame avz is blocked
/// reading.
fn drain_stderr(stderr: ChildStderr) -> JoinHandle<Vec<String>> {
    std::thread::spawn(move || {
        let mut tail = VecDeque::with_capacity(STDERR_TAIL_LINES);

        for line in BufReader::new(stderr).lines().map_while(io::Result::ok) {
            if line.trim().is_empty() {
                continue;
            }
            tracing::debug!(target: "avz::ffmpeg", "{line}");
            if tail.len() == STDERR_TAIL_LINES {
                tail.pop_front();
            }
            tail.push_back(line);
        }

        tail.into()
    })
}

/// ffmpeg's own words about why it stopped, if it said anything.
fn complaint(stderr: &[String]) -> String {
    if stderr.is_empty() {
        return String::new();
    }
    format!(" — ffmpeg said: {}", stderr.join("; "))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;
    use crate::encode::preflight;

    fn settings() -> VideoSettings {
        VideoSettings {
            width: 320,
            height: 180,
            fps: 30,
            fit: Fit::Cover,
            blur: 0.0,
            darken: 0.0,
        }
    }

    /// The argv as a `Vec<String>`, for readable assertions.
    fn args(settings: &VideoSettings) -> Vec<String> {
        ffmpeg_args(Path::new("loops/smoke.mp4"), settings)
            .into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    /// Does `argv` contain `needle` as a contiguous run of arguments?
    fn contains_run(argv: &[String], needle: &[&str]) -> bool {
        argv.windows(needle.len()).any(|window| window == needle)
    }

    /// `-stream_loop -1` is an input option: after `-i` it applies to nothing,
    /// and a background video shorter than the song would run out mid-render.
    #[test]
    fn the_loop_is_requested_before_the_input_it_loops() {
        let argv = args(&settings());

        assert!(contains_run(&argv, &["-stream_loop", "-1"]), "{argv:?}");
        let loop_flag = argv
            .iter()
            .position(|arg| arg == "-stream_loop")
            .expect("-stream_loop");
        let input = argv.iter().position(|arg| arg == "-i").expect("-i");
        assert!(
            loop_flag < input,
            "the loop must precede its input: {argv:?}"
        );
    }

    /// avz reads `width * height * 4` bytes per frame and nothing else, at the
    /// render's own frame rate — ffmpeg converts the source's.
    #[test]
    fn frames_arrive_as_rawvideo_rgba_on_stdout_at_the_rendered_rate() {
        let argv = args(&settings());

        assert!(
            contains_run(&argv, &["-f", "rawvideo", "-pix_fmt", "rgba", "pipe:1"]),
            "{argv:?}"
        );
        assert!(contains_run(&argv, &["-r", "30"]), "{argv:?}");
    }

    /// The background video is muted by construction (`VISION.md` §5.3). Its
    /// audio stream must never be decoded, let alone mixed into the render.
    #[test]
    fn the_background_videos_audio_is_never_decoded() {
        let argv = args(&settings());

        assert!(argv.iter().any(|arg| arg == "-an"), "{argv:?}");
        for banned in ["-c:a", "-map", "-acodec"] {
            assert!(
                !argv.iter().any(|arg| arg == banned),
                "`{banned}` would give the background video a say in the sound: {argv:?}",
            );
        }
    }

    /// The reader writes to stdout, so nothing may name an output file: an argv
    /// that did would overwrite whatever it named.
    #[test]
    fn the_reader_writes_only_to_the_pipe() {
        let argv = args(&settings());

        assert_eq!(argv.last().map(String::as_str), Some("pipe:1"), "{argv:?}");
        assert!(!argv.iter().any(|arg| arg == "-y"), "{argv:?}");
    }

    /// Every fit mode discards the source's alpha before it resamples, which is
    /// what makes the frame's own alpha binary — and premultiplied alpha and
    /// straight alpha the same bytes.
    #[test]
    fn every_fit_flattens_the_sources_alpha_before_it_scales() {
        for fit in [Fit::Cover, Fit::Contain, Fit::Stretch] {
            let filter = fit_filter(fit, 1920, 1080);

            assert!(
                filter.starts_with("format=rgb24,scale="),
                "{fit:?}: {filter}",
            );
        }
    }

    /// `cover` crops, `contain` letterboxes, `stretch` distorts — and only
    /// `contain` introduces transparency, in the bars the backdrop shows through.
    #[test]
    fn each_fit_mode_asks_ffmpeg_for_the_geometry_it_promises() {
        let cover = fit_filter(Fit::Cover, 1920, 1080);
        assert!(
            cover.contains("force_original_aspect_ratio=increase"),
            "{cover}"
        );
        assert!(cover.contains("crop=1920:1080"), "{cover}");
        assert!(!cover.contains("pad="), "cover never letterboxes: {cover}");

        let contain = fit_filter(Fit::Contain, 1920, 1080);
        assert!(
            contain.contains("force_original_aspect_ratio=decrease"),
            "{contain}"
        );
        assert!(
            contain.contains("color=black@0"),
            "the bars are transparent: {contain}"
        );

        let stretch = fit_filter(Fit::Stretch, 1920, 1080);
        assert_eq!(stretch, "format=rgb24,scale=1920:1080");
    }

    #[test]
    fn a_frame_is_four_bytes_per_pixel() {
        assert_eq!(settings().frame_bytes(), 320 * 180 * 4);
    }

    /// The default render pays nothing for a background video beyond the read:
    /// no lookup table, no copy, no colour conversion.
    #[test]
    fn a_frame_that_is_neither_blurred_nor_darkened_is_the_bytes_ffmpeg_wrote() {
        let frame = vec![7u8, 33, 200, 255];
        let settings = VideoSettings {
            width: 1,
            height: 1,
            ..settings()
        };

        assert_eq!(darken_lut(settings.darken), None);
        assert_eq!(shade(frame.clone(), &settings, None), frame);
    }

    /// `darken = 0.5` halves the *photons*, which is a good deal brighter than
    /// halving the encoded byte. The same claim `background.rs` pins for images:
    /// half the light of white is `#bc`, not `#80`.
    #[test]
    fn darken_dims_the_light_rather_than_the_encoded_byte() {
        let lut = darken_lut(0.5).expect("a darken above zero has a table");

        assert_eq!(lut[255], 0xbc, "half the light of white");
        assert_eq!(lut[0], 0, "half of nothing is nothing");
        assert_ne!(lut[255], 0x80, "that would be halving the byte");
    }

    #[test]
    fn darken_of_one_leaves_black_and_darken_of_zero_leaves_the_frame_alone() {
        let lut = darken_lut(1.0).expect("a full darken has a table");
        assert!(lut.iter().all(|&byte| byte == 0), "all light is taken away");

        assert_eq!(darken_lut(0.0), None, "no light is taken away");
    }

    /// Coverage is not light. A darkened letterbox bar is still a bar, and a
    /// darkened opaque pixel is still opaque.
    #[test]
    fn darkening_a_frame_leaves_its_coverage_alone() {
        let lut = darken_lut(0.5).expect("a table");
        let frame = dim(vec![255, 255, 255, 255, 0, 0, 0, 0], &lut);

        assert_eq!(frame, [0xbc, 0xbc, 0xbc, 255, 0, 0, 0, 0]);
    }

    /// A blur averages light, not encoded bytes. Two pixels, one white and one
    /// black, blurred together: the answer is the encoding of half the light of
    /// white, and never the average of the two bytes.
    #[test]
    fn a_blur_averages_light_rather_than_encoded_bytes() {
        let settings = VideoSettings {
            width: 2,
            height: 1,
            blur: 4.0,
            ..settings()
        };
        let frame = vec![255, 255, 255, 255, 0, 0, 0, 255];

        let blurred = shade(frame, &settings, None);

        // A wide blur over two pixels drives both to the mean of the light.
        for byte in [blurred[0], blurred[4]] {
            assert!(
                (0xba..=0xbe).contains(&byte),
                "half the light of white is around 0xbc, not 0x80: {byte:#x}",
            );
        }
        assert_eq!(blurred[3], 255, "an opaque frame stays opaque");
        assert_eq!(blurred[7], 255);
    }

    /// A blur and a darken compose: the light is averaged first, then dimmed.
    #[test]
    fn a_blurred_frame_is_darkened_in_light_too() {
        let settings = VideoSettings {
            width: 2,
            height: 1,
            blur: 4.0,
            darken: 1.0,
            ..settings()
        };
        let frame = vec![255, 255, 255, 255, 0, 0, 0, 255];

        let blurred = shade(frame, &settings, darken_lut(1.0).as_ref());

        assert!(
            blurred[..3].iter().all(|&byte| byte == 0),
            "`darken = 1` is black however much the blur smeared: {blurred:?}",
        );
    }

    /// Write an executable stand-in for ffmpeg with the given shell body.
    ///
    /// The same trick `encode/preflight.rs` uses, and for the same reason: a
    /// real ffmpeg cannot be made to stall on cue.
    ///
    /// A body that blocks forever must `exec` rather than fork, so that the pid
    /// avz kills is the pid holding stdout. A real ffmpeg is that process; a
    /// shell that forked one is not, and its child would keep the pipe open long
    /// after `Drop` killed the shell.
    fn fake_ffmpeg(dir: &Path, body: &str) -> Ffmpeg {
        let path = dir.join("ffmpeg");
        let script = format!(
            "#!/bin/sh
if [ \"$1\" = '-version' ]; then
    echo 'ffmpeg version 7.1.5 Copyright (c) 2000-2026'
    exit 0
fi
{body}
"
        );
        fs::write(&path, script).expect("write fake ffmpeg");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod");
        wait_until_executable(&path);

        preflight(&path).expect("the fake ffmpeg identifies itself as ffmpeg")
    }

    /// Wait out `ETXTBSY` on the script we just wrote. See `encode/preflight.rs`.
    fn wait_until_executable(path: &Path) {
        for _ in 0..1_000 {
            match Command::new(path).arg("-version").output() {
                Err(err) if err.kind() == io::ErrorKind::ExecutableFileBusy => {
                    std::thread::sleep(Duration::from_millis(1));
                }
                _ => return,
            }
        }
        panic!("{}: still busy after a second", path.display());
    }

    /// `VISION.md` §11 names a stalled decode thread as a risk of this design.
    /// A decoder that stops producing frames must end the render with a message,
    /// not with a process that never returns.
    ///
    /// The stand-in writes one frame and then sleeps, which is exactly the shape
    /// of the failure: the render begins, the picture appears, and then nothing.
    #[test]
    fn a_decoder_that_stops_producing_frames_times_out_and_names_the_video() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ffmpeg = fake_ffmpeg(dir.path(), "head -c 4 /dev/zero\nexec sleep 60");
        let settings = VideoSettings {
            width: 1,
            height: 1,
            ..settings()
        };

        let mut video = BackgroundVideo::start_with_timeout(
            &ffmpeg,
            Path::new("loops/smoke.mp4"),
            &settings,
            Duration::from_millis(200),
        )
        .expect("the stand-in spawns");

        video.next_frame().expect("the one frame it wrote arrives");
        let err = video.next_frame().expect_err("nothing follows it");

        assert!(matches!(err, Error::Render(_)), "got {err:?}");
        let msg = err.to_string();
        assert!(msg.contains("smoke.mp4"), "name the video: {msg}");
        assert!(msg.contains("no frame in"), "say what happened: {msg}");
        assert!(
            msg.contains("`background.video`"),
            "say what to do next: {msg}",
        );
    }

    /// A decoder that exits takes the render with it, and its last words explain
    /// why. Waiting `FRAME_TIMEOUT` for a process that is already gone would be
    /// thirty seconds of nothing.
    #[test]
    fn a_decoder_that_exits_ends_the_render_with_ffmpegs_own_complaint() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ffmpeg = fake_ffmpeg(dir.path(), "echo 'No such file or directory' >&2\nexit 1");

        let mut video = BackgroundVideo::start(&ffmpeg, Path::new("loops/smoke.mp4"), &settings())
            .expect("the stand-in spawns");

        let err = video.next_frame().expect_err("there are no frames");

        assert!(matches!(err, Error::Render(_)), "got {err:?}");
        let msg = err.to_string();
        assert!(msg.contains("smoke.mp4"), "name the video: {msg}");
        assert!(
            msg.contains("No such file or directory"),
            "surface ffmpeg's own words: {msg}",
        );
    }

    /// The queue is bounded, so a decoder that outruns the renderer blocks
    /// instead of buffering the loop into memory. A stand-in that writes frames
    /// as fast as the pipe takes them must therefore still be running — and
    /// still blocked — when the render drops it.
    #[test]
    fn a_dropped_background_video_kills_ffmpeg_and_joins_its_threads() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ffmpeg = fake_ffmpeg(dir.path(), "exec cat /dev/zero");
        let settings = VideoSettings {
            width: 1,
            height: 1,
            ..settings()
        };

        let mut video = BackgroundVideo::start(&ffmpeg, Path::new("loops/smoke.mp4"), &settings)
            .expect("the stand-in spawns");
        video.next_frame().expect("frames flow");

        // The reader thread is blocked on a full queue by now. Dropping must not
        // deadlock: the receiver goes before the kill.
        drop(video);
    }
}
