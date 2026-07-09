//! The ffmpeg encoder subprocess: raw RGBA on stdin, a finished mp4 on disk.
//!
//! One ffmpeg per render, spawned before the first frame. avz writes tightly
//! packed RGBA frames to its stdin and lets it mux the untouched mp3 stream
//! alongside — `-c:a copy`, never a re-encode (`VISION.md` §5.4).
//!
//! Two failure modes drive the design:
//!
//! - **A half-written mp4.** ffmpeg writes `out.mp4.part`; [`Encoder::finish`]
//!   renames it only after a clean exit. Every other path — a broken pipe, a
//!   non-zero exit, a dropped [`Encoder`] — removes it.
//! - **A silent death.** ffmpeg's stderr is drained by a dedicated thread, so it
//!   can never block on a full pipe while avz waits for it to read a frame, and
//!   so its last words are available to explain whatever went wrong.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::fs;
use std::io::{self, BufRead as _, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdin, Command, ExitStatus, Stdio};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::config::Codec;
use crate::encode::Ffmpeg;
use crate::render::readback::BYTES_PER_PIXEL;
use crate::{Error, Result};

/// The suffix ffmpeg writes to, appended to the output path.
const PART_SUFFIX: &str = "part";

/// How many trailing stderr lines are kept to explain a failure.
///
/// ffmpeg's diagnosis is at the end; the banner and stream mapping are at the
/// start. A handful of lines is enough to name the codec that refused.
const STDERR_TAIL_LINES: usize = 8;

/// How the video stream is encoded.
///
/// Mirrors the resolved `[output]` config section, decoupled from it so the
/// encoder can be driven from a test without building a whole [`Config`].
///
/// [`Config`]: crate::config::Config
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncodeSettings {
    /// Frame width in pixels. Must be even.
    pub width: u32,
    /// Frame height in pixels. Must be even.
    pub height: u32,
    /// Frames per second, which is also the rate the frames are timestamped at.
    pub fps: u32,
    /// Video codec. v0.1 encodes x264 only (RFC-001 NG3).
    pub codec: Codec,
    /// CRF quality; lower is better.
    pub quality: u8,
    /// How far into the mp3 the muxed audio starts, for a `--sample` render.
    ///
    /// [`Duration::ZERO`] for a whole-song render. ffmpeg seeks the input and
    /// still copies the stream, so a sampled render costs no re-encode — but it
    /// starts at the mp3 frame nearest the offset, not at the exact sample,
    /// because a copied stream cannot be cut mid-frame.
    pub audio_start: Duration,
}

impl EncodeSettings {
    /// Bytes in one tightly packed RGBA frame.
    fn frame_bytes(&self) -> usize {
        self.width as usize * self.height as usize * BYTES_PER_PIXEL as usize
    }
}

/// The x264 speed/size tradeoff from `VISION.md` §5.4. Offline rendering already
/// costs minutes of GPU time, so the encoder may as well take its time too.
const X264_PRESET: &str = "slow";

/// The ffmpeg encoder name for `codec`.
///
/// # Errors
///
/// [`Error::Encode`] for the codecs RFC-001 NG3 defers past v0.1. ffmpeg would
/// otherwise fail with a message about an unknown encoder, minutes into a
/// render, and leave the user guessing which spelling it wanted.
fn video_encoder(codec: Codec) -> Result<&'static str> {
    match codec {
        Codec::X264 => Ok("libx264"),
        Codec::X265 | Codec::Av1 => Err(Error::Encode(format!(
            "codec `{}` is not supported yet; avz v0.1 encodes x264 only — use `--codec x264`",
            codec.as_str(),
        ))),
    }
}

/// Build the argument vector for one render.
///
/// The shape is `VISION.md` §5.4: rawvideo on stdin, the mp3 as a second input,
/// one stream mapped from each, and the part file as the destination.
///
/// # Errors
///
/// [`Error::Encode`] if the codec is deferred or the frame geometry is one
/// ffmpeg cannot encode. Both are cheaper to catch here than in a subprocess.
fn ffmpeg_args(settings: &EncodeSettings, audio: &Path, part: &Path) -> Result<Vec<OsString>> {
    let encoder = video_encoder(settings.codec)?;

    if settings.fps == 0 {
        return Err(Error::Encode("frame rate must not be zero".to_owned()));
    }
    for (name, value) in [("width", settings.width), ("height", settings.height)] {
        if value == 0 {
            return Err(Error::Encode(format!("frame {name} must not be zero")));
        }
        // yuv420p subsamples chroma by two in both directions.
        if value % 2 != 0 {
            return Err(Error::Encode(format!(
                "frame {name} {value} must be even: the yuv420p output halves it"
            )));
        }
    }

    let size = format!("{}x{}", settings.width, settings.height);
    let mut args: Vec<OsString> = [
        // Overwrite a part file left by an interrupted render.
        "-y",
        "-hide_banner",
        // Progress belongs to the `Progress` callback, not to ffmpeg's stderr,
        // which avz keeps clear so a failure's last lines are the diagnosis.
        "-nostats",
        "-loglevel",
        "error",
        "-f",
        "rawvideo",
        "-pix_fmt",
        "rgba",
        "-s",
        &size,
        "-r",
        &settings.fps.to_string(),
        "-i",
        "pipe:0",
    ]
    .iter()
    .map(OsString::from)
    .collect();

    // An input option, so it must precede the mp3 and follow `pipe:0`: seeking
    // the rawvideo input would throw away frames avz already rendered.
    if !settings.audio_start.is_zero() {
        args.push("-ss".into());
        args.push(format!("{:.6}", settings.audio_start.as_secs_f64()).into());
    }
    args.push("-i".into());
    args.push(audio.into());

    args.extend(
        [
            "-map",
            "0:v",
            "-map",
            "1:a",
            "-c:v",
            encoder,
            "-preset",
            X264_PRESET,
            "-crf",
            &settings.quality.to_string(),
            "-pix_fmt",
            "yuv420p",
            // The audio promise: the original mp3 stream, muxed untouched.
            "-c:a",
            "copy",
            "-movflags",
            "+faststart",
            // The video ends when avz stops writing frames; the mp3 may run on.
            "-shortest",
            // ffmpeg picks the muxer from the output extension, and the part
            // file ends in `.part`. Name it, or ffmpeg refuses to open the file.
            "-f",
            "mp4",
        ]
        .iter()
        .map(OsString::from),
    );

    args.push(part.into());
    Ok(args)
}

/// The path ffmpeg writes before the rename: `out.mp4` → `out.mp4.part`.
fn part_path(output: &Path) -> PathBuf {
    let mut part = output.as_os_str().to_owned();
    part.push(".");
    part.push(PART_SUFFIX);
    PathBuf::from(part)
}

/// A running ffmpeg subprocess consuming raw RGBA frames.
///
/// Frames go in with [`Encoder::write_frame`]; the mp4 appears at the output
/// path when [`Encoder::finish`] returns `Ok`. Dropping an unfinished encoder
/// kills ffmpeg and removes the part file.
#[derive(Debug)]
pub struct Encoder {
    child: Child,
    /// `None` once stdin has been closed, which is ffmpeg's EOF.
    stdin: Option<ChildStdin>,
    /// `None` once the reader thread has been joined.
    stderr: Option<JoinHandle<Vec<String>>>,
    /// True once ffmpeg has been reaped; `Drop` then has nothing to clean up.
    reaped: bool,
    part: PathBuf,
    output: PathBuf,
    frame_bytes: usize,
    program: PathBuf,
}

impl Encoder {
    /// Spawn `ffmpeg` and leave it waiting for the first frame.
    ///
    /// `audio` is muxed with `-c:a copy`. Nothing is written to `output` until
    /// [`Encoder::finish`] succeeds.
    ///
    /// # Errors
    ///
    /// [`Error::Encode`] if the settings describe a video ffmpeg cannot encode,
    /// or if the process will not spawn. Preflight the binary with
    /// [`preflight`](super::preflight) first, so this is the rare case.
    pub fn start(
        ffmpeg: &Ffmpeg,
        settings: &EncodeSettings,
        audio: &Path,
        output: &Path,
    ) -> Result<Self> {
        let part = part_path(output);
        let args = ffmpeg_args(settings, audio, &part)?;
        let program = ffmpeg.program();

        let mut child = Command::new(program)
            .args(&args)
            .stdin(Stdio::piped())
            // ffmpeg writes the container to a file, never to stdout, and avz
            // must not inherit a stream that could interleave with the CLI's.
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| Error::Encode(format!("cannot run `{}`: {err}", program.display())))?;

        let stdin = child.stdin.take().expect("stdin was piped");
        let stderr = child.stderr.take().expect("stderr was piped");

        tracing::debug!(
            program = %program.display(),
            width = settings.width,
            height = settings.height,
            fps = settings.fps,
            part = %part.display(),
            "ffmpeg started"
        );

        Ok(Self {
            child,
            stdin: Some(stdin),
            stderr: Some(drain_stderr(stderr)),
            reaped: false,
            part,
            output: output.to_path_buf(),
            frame_bytes: settings.frame_bytes(),
            program: program.to_path_buf(),
        })
    }

    /// Bytes in one tightly packed RGBA frame, as [`Encoder::write_frame`] wants
    /// it: no row padding, exactly `width * height * 4`.
    pub fn frame_bytes(&self) -> usize {
        self.frame_bytes
    }

    /// The path the finished mp4 will appear at.
    pub fn output(&self) -> &Path {
        &self.output
    }

    /// Send one tightly packed RGBA frame to ffmpeg.
    ///
    /// # Errors
    ///
    /// [`Error::Encode`] if `frame` is not [`Encoder::frame_bytes`] long — a
    /// wrong-sized frame would shear every frame after it — or if ffmpeg died,
    /// in which case the message carries its exit status and last words. Either
    /// way the part file is gone and the encoder must not be used again.
    pub fn write_frame(&mut self, frame: &[u8]) -> Result<()> {
        if frame.len() != self.frame_bytes {
            return Err(Error::Encode(format!(
                "frame is {} bytes, expected {} for one RGBA frame",
                frame.len(),
                self.frame_bytes,
            )));
        }

        let Some(stdin) = self.stdin.as_mut() else {
            return Err(Error::Encode(
                "cannot write a frame: ffmpeg's input is already closed".to_owned(),
            ));
        };

        match stdin.write_all(frame) {
            Ok(()) => Ok(()),
            // A closed pipe means ffmpeg is gone. Its stderr says why.
            Err(err) => Err(self.abort(&err)),
        }
    }

    /// Close ffmpeg's input, wait for it, and move the part file into place.
    ///
    /// # Errors
    ///
    /// [`Error::Encode`] if ffmpeg exits non-zero or the rename fails. In both
    /// cases the part file is removed rather than left looking like a video.
    pub fn finish(mut self) -> Result<()> {
        // Closing stdin is ffmpeg's EOF: it flushes and finalizes the container.
        drop(self.stdin.take());

        // Every way this can fail leaves a part file that is not a video, so the
        // cleanup lives here rather than on each error path.
        match self.wait_and_rename() {
            Ok(()) => {
                tracing::debug!(output = %self.output.display(), "ffmpeg finished");
                Ok(())
            }
            Err(err) => {
                self.discard_part();
                Err(err)
            }
        }
    }

    /// Collect ffmpeg's exit status and, if it succeeded, move the part file to
    /// the output path.
    fn wait_and_rename(&mut self) -> Result<()> {
        let status = self.wait()?;
        let stderr = self.stderr_tail();

        if !status.success() {
            return Err(Error::Encode(format!(
                "ffmpeg exited {status} without finishing the video{}",
                complaint(&stderr),
            )));
        }

        fs::rename(&self.part, &self.output).map_err(|err| {
            Error::Encode(format!(
                "cannot move `{}` into place at `{}`: {err}",
                self.part.display(),
                self.output.display(),
            ))
        })
    }

    /// Reap ffmpeg after a write failed, and explain the failure with its words.
    ///
    /// Always leaves the part file removed: whatever ffmpeg managed to write is
    /// not a video, and a truncated mp4 next to a non-zero exit code is exactly
    /// the outcome `VISION.md` §5.4 forbids.
    fn abort(&mut self, write_error: &io::Error) -> Error {
        drop(self.stdin.take());
        let status = self.wait().ok();
        let stderr = self.stderr_tail();
        self.discard_part();

        match status {
            Some(status) if !status.success() => Error::Encode(format!(
                "ffmpeg exited {status} while avz was still sending frames{}",
                complaint(&stderr),
            )),
            _ => Error::Encode(format!(
                "cannot send a frame to `{}`: {write_error}{}",
                self.program.display(),
                complaint(&stderr),
            )),
        }
    }

    /// Wait for ffmpeg, once.
    fn wait(&mut self) -> Result<ExitStatus> {
        let status = self.child.wait().map_err(|err| {
            Error::Encode(format!(
                "cannot wait for `{}`: {err}",
                self.program.display()
            ))
        });
        self.reaped = true;
        status
    }

    /// Join the stderr reader. Only call after ffmpeg has exited, or this blocks
    /// until it does.
    fn stderr_tail(&mut self) -> Vec<String> {
        self.stderr
            .take()
            .and_then(|reader| reader.join().ok())
            .unwrap_or_default()
    }

    /// Remove the part file, ignoring the case where it was never created.
    fn discard_part(&self) {
        match fs::remove_file(&self.part) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => tracing::warn!(
                part = %self.part.display(),
                %err,
                "cannot remove the partial output"
            ),
        }
    }
}

impl Drop for Encoder {
    /// An abandoned render leaves no ffmpeg running and no part file behind.
    fn drop(&mut self) {
        if self.reaped {
            return;
        }

        drop(self.stdin.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = self.stderr_tail();
        self.discard_part();
    }
}

/// Read ffmpeg's stderr on a dedicated thread, keeping the last few lines.
///
/// The thread is not optional. ffmpeg blocks once its stderr pipe fills, and it
/// blocks before reading the frame avz is blocked writing — a deadlock with no
/// diagnostic. Draining it continuously also means the tail is already in hand
/// when a failure needs explaining.
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

/// ffmpeg's own words about why it failed, if it said anything.
fn complaint(stderr: &[String]) -> String {
    if stderr.is_empty() {
        return String::new();
    }
    format!(" — ffmpeg said: {}", stderr.join("; "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> EncodeSettings {
        EncodeSettings {
            width: 320,
            height: 180,
            fps: 30,
            codec: Codec::X264,
            quality: 18,
            audio_start: Duration::ZERO,
        }
    }

    /// The argv as a `Vec<String>`, for readable assertions.
    fn args(settings: &EncodeSettings) -> Vec<String> {
        ffmpeg_args(settings, Path::new("song.mp3"), Path::new("out.mp4.part"))
            .expect("x264 at a legal size builds an argv")
            .into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    /// Does `argv` contain `needle` as a contiguous run of arguments?
    fn contains_run(argv: &[String], needle: &[&str]) -> bool {
        argv.windows(needle.len()).any(|window| window == needle)
    }

    #[test]
    fn video_arrives_as_rawvideo_rgba_on_stdin_at_the_configured_size_and_rate() {
        let argv = args(&settings());

        assert!(
            contains_run(
                &argv,
                &[
                    "-f", "rawvideo", "-pix_fmt", "rgba", "-s", "320x180", "-r", "30", "-i",
                    "pipe:0"
                ]
            ),
            "the video input must describe the frames avz actually writes: {argv:?}"
        );
    }

    /// The invariant from `AGENTS.md`: the original mp3 stream is muxed, never
    /// re-encoded. Nothing in the argv may name an audio encoder.
    #[test]
    fn the_audio_stream_is_copied_and_never_reencoded() {
        let argv = args(&settings());

        assert!(contains_run(&argv, &["-c:a", "copy"]), "{argv:?}");
        assert!(contains_run(&argv, &["-i", "song.mp3"]), "{argv:?}");
        assert!(contains_run(&argv, &["-map", "1:a"]), "{argv:?}");
        for banned in ["libmp3lame", "aac", "libopus", "libfdk_aac", "-b:a"] {
            assert!(
                !argv.iter().any(|arg| arg == banned),
                "`{banned}` re-encodes the audio: {argv:?}"
            );
        }
    }

    #[test]
    fn frames_are_written_to_the_part_file_never_straight_to_the_output() {
        let argv = args(&settings());

        assert_eq!(
            argv.last().map(String::as_str),
            Some("out.mp4.part"),
            "ffmpeg writes the part file; avz renames it: {argv:?}"
        );
        assert!(
            !argv.iter().any(|arg| arg == "out.mp4"),
            "the final path must never reach ffmpeg: {argv:?}"
        );
    }

    /// ffmpeg guesses the muxer from the output extension, and `.part` is not
    /// one it knows. Without an explicit `-f mp4` it refuses to open the file.
    #[test]
    fn the_output_muxer_is_named_because_the_part_suffix_hides_it() {
        let argv = args(&settings());

        let muxer = argv
            .iter()
            .rposition(|arg| arg == "-f")
            .expect("the output format is named");
        assert_eq!(
            argv.get(muxer + 1).map(String::as_str),
            Some("mp4"),
            "{argv:?}"
        );
    }

    #[test]
    fn quality_is_the_crf_and_the_pixel_format_is_broadly_playable() {
        let argv = args(&EncodeSettings {
            quality: 23,
            ..settings()
        });

        assert!(contains_run(&argv, &["-c:v", "libx264"]), "{argv:?}");
        assert!(contains_run(&argv, &["-crf", "23"]), "{argv:?}");
        assert!(contains_run(&argv, &["-pix_fmt", "yuv420p"]), "{argv:?}");
        assert!(
            contains_run(&argv, &["-movflags", "+faststart"]),
            "{argv:?}"
        );
    }

    /// RFC-001 NG3 defers x265 and av1. Refusing them here beats emitting an
    /// argv ffmpeg rejects with a message about an unknown encoder.
    #[test]
    fn a_deferred_codec_is_refused_and_names_the_one_that_works() {
        for codec in [Codec::X265, Codec::Av1] {
            let err = ffmpeg_args(
                &EncodeSettings {
                    codec,
                    ..settings()
                },
                Path::new("song.mp3"),
                Path::new("out.mp4.part"),
            )
            .expect_err("v0.1 encodes x264 only");

            assert!(matches!(err, Error::Encode(_)), "got {err:?}");
            let msg = err.to_string();
            assert!(
                msg.contains(codec.as_str()),
                "name the codec asked for: {msg}"
            );
            assert!(msg.contains("x264"), "name the codec that works: {msg}");
        }
    }

    /// yuv420p subsamples chroma by two, so an odd dimension makes ffmpeg fail
    /// deep in the encoder. Say so before a single frame is rendered.
    #[test]
    fn an_odd_frame_dimension_is_refused_before_ffmpeg_starts() {
        for (width, height) in [(321, 180), (320, 181)] {
            let err = ffmpeg_args(
                &EncodeSettings {
                    width,
                    height,
                    ..settings()
                },
                Path::new("song.mp3"),
                Path::new("out.mp4.part"),
            )
            .expect_err("yuv420p needs even dimensions");

            assert!(matches!(err, Error::Encode(_)), "got {err:?}");
            assert!(err.to_string().contains("even"), "{err}");
        }
    }

    #[test]
    fn a_zero_dimension_or_frame_rate_is_refused() {
        for settings in [
            EncodeSettings {
                width: 0,
                ..settings()
            },
            EncodeSettings {
                height: 0,
                ..settings()
            },
            EncodeSettings {
                fps: 0,
                ..settings()
            },
        ] {
            let err = ffmpeg_args(&settings, Path::new("song.mp3"), Path::new("out.mp4.part"))
                .expect_err("a video with no pixels or no frames is not a video");
            assert!(matches!(err, Error::Encode(_)), "got {err:?}");
        }
    }

    /// `--sample 0:45..1:45` renders the frames of that minute, so the mp3 must
    /// be muxed from the same instant. `-ss` is an *input* option: it has to sit
    /// in front of the audio input, never in front of the rawvideo one, which
    /// always starts at its first frame.
    #[test]
    fn a_sampled_render_seeks_the_audio_input_and_still_copies_the_stream() {
        let argv = args(&EncodeSettings {
            audio_start: Duration::from_secs(45),
            ..settings()
        });

        assert!(
            contains_run(&argv, &["-ss", "45.000000", "-i", "song.mp3"]),
            "the seek must apply to the mp3 input: {argv:?}"
        );
        assert!(contains_run(&argv, &["-c:a", "copy"]), "{argv:?}");

        let video_input = argv.iter().position(|arg| arg == "pipe:0").expect("stdin");
        let seek = argv.iter().position(|arg| arg == "-ss").expect("the seek");
        assert!(
            seek > video_input,
            "seeking the rawvideo input would drop rendered frames: {argv:?}"
        );
    }

    /// A sub-second sample start survives the argv: `-ss 1` would silently move
    /// the audio a whole second away from the picture.
    #[test]
    fn a_fractional_audio_start_keeps_its_precision() {
        let argv = args(&EncodeSettings {
            audio_start: Duration::from_secs_f64(31.0 / 30.0),
            ..settings()
        });

        assert!(contains_run(&argv, &["-ss", "1.033333"]), "{argv:?}");
    }

    /// A whole-song render must not seek at all: `-ss 0` on an mp3 is not free,
    /// and an argv that always seeks cannot be read as "this render did not".
    #[test]
    fn a_whole_song_render_does_not_seek_the_audio() {
        let argv = args(&settings());

        assert!(
            !argv.iter().any(|arg| arg == "-ss"),
            "nothing to seek past: {argv:?}"
        );
    }

    #[test]
    fn a_frame_is_four_bytes_per_pixel() {
        assert_eq!(settings().frame_bytes(), 320 * 180 * 4);
    }

    /// The part file sits beside the output, so the rename never crosses a
    /// filesystem — and it keeps the output's extension, so nothing mistakes it
    /// for a playable mp4.
    #[test]
    fn the_part_file_is_the_output_path_plus_a_suffix() {
        assert_eq!(
            part_path(Path::new("/tmp/out.mp4")),
            Path::new("/tmp/out.mp4.part")
        );
        assert_eq!(part_path(Path::new("noext")), Path::new("noext.part"));
    }

    #[test]
    fn a_silent_ffmpeg_gets_no_complaint_appended() {
        assert_eq!(complaint(&[]), "");
        assert_eq!(
            complaint(&["one".to_owned(), "two".to_owned()]),
            " — ffmpeg said: one; two"
        );
    }
}
