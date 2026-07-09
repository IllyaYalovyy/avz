//! ID3 tags and cover art.
//!
//! Reads title, artist, album, and embedded artwork via `lofty`. Missing tags
//! are absent values, never errors: a song with no title still renders, it just
//! skips the text card (`VISION.md` §5.2). Only the *file* being unreadable or
//! unrecognizable is an error, and it is an [`Error::Input`] so the CLI can exit
//! with code 3.
//!
//! Reading tags never touches the audio samples. `avz probe` is expected to be
//! instant on a five-minute song.

use std::fmt;
use std::io::Cursor;
use std::path::Path;
use std::time::Duration;

use lofty::error::ErrorKind;
use lofty::file::{AudioFile as _, TaggedFileExt as _};
use lofty::picture::{Picture, PictureType};
use lofty::prelude::Accessor as _;
use lofty::tag::Tag;

use crate::{Error, Result};

/// What `avz` knows about an input file without decoding it.
///
/// Every tag is optional because every tag is genuinely optional in the wild.
/// The audio properties come from the stream itself, so they survive a file with
/// no tags at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackMeta {
    /// ID3 title, if present and non-blank.
    pub title: Option<String>,
    /// ID3 artist, if present and non-blank.
    pub artist: Option<String>,
    /// ID3 album, if present and non-blank.
    pub album: Option<String>,
    /// Playing time, derived from the audio stream.
    pub duration: Duration,
    /// Sample rate in Hz.
    pub sample_rate: Option<u32>,
    /// Channel count: 1 mono, 2 stereo.
    pub channels: Option<u8>,
    /// Audio bitrate in kbps.
    pub bitrate_kbps: Option<u32>,
    /// Embedded cover art, if the file carries any.
    pub cover: Option<CoverArt>,
}

/// An embedded cover image.
///
/// The bytes are kept as-is. `render` uploads them as a texture later
/// (`VISION.md` §5.2), so decoding them here would be wasted work — only the
/// dimensions are extracted, from the image header alone.
#[derive(Clone, PartialEq, Eq)]
pub struct CoverArt {
    /// The mime type the tag declared, e.g. `image/png`. Absent if the tag did
    /// not say and the bytes could not be recognized.
    pub mime: Option<String>,
    /// Pixel dimensions, if the image header could be parsed.
    pub dimensions: Option<(u32, u32)>,
    /// The raw image bytes, exactly as stored in the tag.
    pub data: Vec<u8>,
}

/// Print the size of the artwork, never the artwork itself.
impl fmt::Debug for CoverArt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CoverArt")
            .field("mime", &self.mime)
            .field("dimensions", &self.dimensions)
            .field("bytes", &self.data.len())
            .finish()
    }
}

/// Read tags, duration, and cover art from an audio file.
///
/// # Errors
///
/// [`Error::Input`] if the file is missing, unreadable, or not an audio format
/// `lofty` recognizes. Missing *tags* are not errors.
pub fn read(path: impl AsRef<Path>) -> Result<TrackMeta> {
    let path = path.as_ref();

    let file = lofty::read_from_path(path).map_err(|err| unreadable(path, &err))?;

    let properties = file.properties();

    // `primary_tag` is ID3v2 for mp3, falling back to ID3v1 — the order the
    // format itself prefers. `first_tag` covers containers with no primary.
    let tag = file.primary_tag().or_else(|| file.first_tag());

    let meta = TrackMeta {
        title: tag.and_then(|tag| present(tag.title().as_deref())),
        artist: tag.and_then(|tag| present(tag.artist().as_deref())),
        album: tag.and_then(|tag| present(tag.album().as_deref())),
        duration: properties.duration(),
        sample_rate: properties.sample_rate(),
        channels: properties.channels(),
        bitrate_kbps: properties.audio_bitrate(),
        cover: tag.and_then(cover_art),
    };

    tracing::debug!(
        path = %path.display(),
        duration_secs = meta.duration.as_secs_f64(),
        has_cover = meta.cover.is_some(),
        "read track metadata"
    );

    Ok(meta)
}

/// Turn a `lofty` failure into an input error the user can act on.
///
/// The distinction that matters is "fix your filesystem" versus "give me a
/// different file". `lofty` blurs it: a truncated or garbage stream comes back
/// as an `io` error with `InvalidInput`, not as `UnknownFormat`, and
/// `Invalid argument (os error 22)` tells a user nothing. So the io kinds that
/// mean "these bytes are not audio" are folded into the format branch, and the
/// original error is logged for whoever is holding `--verbose`.
fn unreadable(path: &Path, err: &lofty::error::LoftyError) -> Error {
    use std::io::ErrorKind as Io;

    tracing::debug!(path = %path.display(), error = %err, "could not read metadata");

    let path = path.display();

    let not_audio = || {
        Error::Input(format!(
            "{path}: not a recognized audio file; avz reads mp3"
        ))
    };

    match err.kind() {
        ErrorKind::UnknownFormat => not_audio(),
        ErrorKind::Io(io) => match io.kind() {
            Io::NotFound => Error::Input(format!("{path}: no such file")),
            Io::PermissionDenied => Error::Input(format!("{path}: permission denied")),
            Io::InvalidData | Io::InvalidInput | Io::UnexpectedEof => not_audio(),
            _ => Error::Input(format!("{path}: cannot be read: {io}")),
        },
        _ => not_audio(),
    }
}

/// A tag value counts as present only when it says something.
///
/// Taggers write empty and whitespace-only frames. Treating those as titles
/// would put a blank text card on the video.
fn present(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

/// Pick the picture that represents the release.
///
/// A tag may carry several pictures — front cover, back cover, band photo. The
/// front cover is what a text card or an art-reactive preset wants; anything
/// else is a fallback so a file that only stores `Other` still shows artwork.
/// Selection is by picture type and then document order, never by iteration
/// order of a hash map, because a preset may sample it (`AGENTS.md`,
/// determinism).
fn cover_art(tag: &Tag) -> Option<CoverArt> {
    let pictures = tag.pictures();

    let picture = pictures
        .iter()
        .find(|picture| picture.pic_type() == PictureType::CoverFront)
        .or_else(|| pictures.first())?;

    Some(describe(picture))
}

/// Describe a picture without decoding it.
fn describe(picture: &Picture) -> CoverArt {
    let data = picture.data().to_vec();

    CoverArt {
        mime: picture.mime_type().map(|mime| mime.as_str().to_owned()),
        dimensions: dimensions(&data),
        data,
    }
}

/// Read `(width, height)` out of an image header.
///
/// Returns `None` for formats `image` was not built with, or for artwork that is
/// simply corrupt. Neither is worth failing a probe over: the mime type and byte
/// count still tell the user what is embedded.
fn dimensions(data: &[u8]) -> Option<(u32, u32)> {
    image::ImageReader::new(Cursor::new(data))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use lofty::picture::MimeType;
    use lofty::tag::TagType;

    use super::*;

    /// A committed CC0 fixture. See `assets/fixtures/README.md`.
    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/fixtures")
            .join(name)
    }

    /// A 1×1 PNG, the smallest thing `image` will report dimensions for.
    const PNG_1X1: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f,
        0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    fn picture(pic_type: PictureType, data: &[u8]) -> Picture {
        Picture::new_unchecked(pic_type, Some(MimeType::Png), None, data.to_vec())
    }

    #[test]
    fn tagged_mp3_yields_title_artist_and_album() {
        let meta = read(fixture("tone-tagged.mp3")).expect("the fixture is readable");

        assert_eq!(meta.title.as_deref(), Some("Sine Tones"));
        assert_eq!(meta.artist.as_deref(), Some("avz test fixture"));
        assert_eq!(meta.album.as_deref(), Some("Public Domain Tones"));
    }

    #[test]
    fn tagged_mp3_reports_cover_art_with_mime_and_dimensions() {
        let meta = read(fixture("tone-tagged.mp3")).expect("the fixture is readable");

        let cover = meta.cover.expect("the fixture embeds cover art");
        assert_eq!(cover.mime.as_deref(), Some("image/png"));
        assert_eq!(cover.dimensions, Some((256, 256)));
        assert!(!cover.data.is_empty());
    }

    #[test]
    fn duration_and_stream_properties_come_from_the_audio() {
        let meta = read(fixture("tone-tagged.mp3")).expect("the fixture is readable");

        // The fixture is 5 s of tones. mp3 frames quantize the length, so allow
        // a frame's worth of slack rather than pinning an exact value.
        let secs = meta.duration.as_secs_f64();
        assert!((4.95..=5.10).contains(&secs), "duration was {secs}s");

        assert_eq!(meta.sample_rate, Some(44_100));
        assert_eq!(meta.channels, Some(2));
        assert_eq!(meta.bitrate_kbps, Some(64));
    }

    #[test]
    fn untagged_mp3_reports_missing_tags_instead_of_failing() {
        let meta = read(fixture("tone-untagged.mp3")).expect("an untagged mp3 still probes");

        assert_eq!(meta.title, None);
        assert_eq!(meta.artist, None);
        assert_eq!(meta.album, None);
        assert_eq!(meta.cover, None);

        // The audio properties survive the absence of any tag.
        assert_eq!(meta.sample_rate, Some(44_100));
        assert!(meta.duration.as_secs_f64() > 4.9);
    }

    #[test]
    fn a_missing_file_is_an_input_error() {
        let path = fixture("no-such-song.mp3");

        let err = read(&path).expect_err("a missing file cannot be probed");

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

    /// Named `.mp3` but full of garbage: lofty trusts the extension, tries to
    /// parse, and fails deep inside the mpeg reader with an io error. The user
    /// must still be told the file is the problem, not their disk.
    #[test]
    fn a_file_that_lies_about_being_an_mp3_is_an_input_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("not-audio.mp3");
        std::fs::write(&path, b"this is not an mp3").expect("write");

        let err = read(&path).expect_err("a text file cannot be probed");

        assert!(matches!(err, Error::Input(_)), "got {err:?}");
        let msg = err.to_string();
        assert!(msg.contains("not-audio.mp3"), "must name the file: {msg}");
        assert!(
            msg.contains("not a recognized audio file"),
            "must say what is wrong, not `os error 22`: {msg}"
        );
    }

    /// No extension to trust and no magic bytes to match: the `UnknownFormat`
    /// branch. Both paths must reach the same sentence.
    #[test]
    fn a_file_of_an_unknown_format_is_an_input_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("mystery.dat");
        std::fs::write(&path, b"neither mp3 nor flac nor anything else").expect("write");

        let err = read(&path).expect_err("an unknown format cannot be probed");

        let msg = err.to_string();
        assert!(msg.contains("not a recognized audio file"), "got {msg}");
    }

    #[test]
    fn blank_and_whitespace_tag_values_count_as_missing() {
        assert_eq!(present(None), None);
        assert_eq!(present(Some("")), None);
        assert_eq!(present(Some("   \t ")), None);
        assert_eq!(
            present(Some("  Sine Tones ")),
            Some("Sine Tones".to_owned())
        );
    }

    #[test]
    fn front_cover_wins_over_other_pictures_regardless_of_order() {
        let mut tag = Tag::new(TagType::Id3v2);
        tag.push_picture(picture(PictureType::Media, b"not the cover".as_slice()));
        tag.push_picture(picture(PictureType::CoverFront, PNG_1X1));

        let cover = cover_art(&tag).expect("a tag with pictures has a cover");

        assert_eq!(cover.data, PNG_1X1);
        assert_eq!(cover.dimensions, Some((1, 1)));
    }

    #[test]
    fn a_tag_without_a_front_cover_falls_back_to_the_first_picture() {
        let mut tag = Tag::new(TagType::Id3v2);
        tag.push_picture(picture(PictureType::Other, PNG_1X1));
        tag.push_picture(picture(PictureType::Media, b"second".as_slice()));

        let cover = cover_art(&tag).expect("any picture beats no picture");

        assert_eq!(cover.data, PNG_1X1);
    }

    #[test]
    fn a_tag_with_no_pictures_has_no_cover() {
        let tag = Tag::new(TagType::Id3v2);

        assert_eq!(cover_art(&tag), None);
    }

    /// Unparsable artwork is still reported — mime and byte count are useful.
    #[test]
    fn artwork_that_is_not_a_readable_image_reports_no_dimensions() {
        let mut tag = Tag::new(TagType::Id3v2);
        tag.push_picture(picture(PictureType::CoverFront, b"garbage".as_slice()));

        let cover = cover_art(&tag).expect("a picture frame exists");

        assert_eq!(cover.dimensions, None);
        assert_eq!(cover.mime.as_deref(), Some("image/png"));
        assert_eq!(cover.data, b"garbage");
    }

    /// Cover art is often megabytes. `{:?}` on a `TrackMeta` must stay readable.
    #[test]
    fn debug_output_summarizes_cover_bytes_rather_than_printing_them() {
        let cover = CoverArt {
            mime: Some("image/png".to_owned()),
            dimensions: Some((1, 1)),
            data: PNG_1X1.to_vec(),
        };

        let debug = format!("{cover:?}");

        assert!(
            debug.contains(&format!("bytes: {}", PNG_1X1.len())),
            "got {debug}"
        );
        assert!(
            !debug.contains("data"),
            "raw bytes leaked into Debug: {debug}"
        );
    }
}
