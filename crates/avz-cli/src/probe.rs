//! `avz probe song.mp3` — what avz sees in a file before it renders it.
//!
//! The report answers the question a user actually has: will the text card have
//! anything to say, is the artwork usable, and is this really the song I meant?
//! Missing tags are printed as missing. Only an unreadable file is an error
//! (`designs/USER-TASKS.md` UT-005).
//!
//! `avz-core` reads; this module writes. All formatting lives here because
//! nothing in core may talk to a terminal (`AGENTS.md`, core/cli split).

use std::io::{self, Write};
use std::path::Path;
use std::time::Duration;

use avz_core::meta::{CoverArt, TrackMeta};

use crate::cli::ProbeArgs;

/// Width of the label column, so values line up under each other. One wider than
/// the longest label, so `sample rate` keeps a space after it.
const LABEL_WIDTH: usize = 12;

/// Read `args.input` and print its metadata to stdout.
pub fn run(args: &ProbeArgs) -> anyhow::Result<()> {
    let meta = avz_core::meta::read(&args.input)?;

    let stdout = io::stdout();
    report(&mut stdout.lock(), &args.input, &meta)?;

    Ok(())
}

/// Write the human-readable report.
///
/// Takes a writer rather than printing directly so the layout is testable
/// without spawning the binary.
fn report(out: &mut impl Write, path: &Path, meta: &TrackMeta) -> io::Result<()> {
    writeln!(out, "{}", path.display())?;

    field(out, "title", meta.title.as_deref().unwrap_or(MISSING))?;
    field(out, "artist", meta.artist.as_deref().unwrap_or(MISSING))?;
    field(out, "album", meta.album.as_deref().unwrap_or(MISSING))?;
    field(out, "duration", &format_duration(meta.duration))?;

    let sample_rate = match meta.sample_rate {
        Some(hz) => format!("{hz} Hz"),
        None => MISSING.to_owned(),
    };
    field(out, "sample rate", &sample_rate)?;

    let channels = match meta.channels {
        Some(n) => format_channels(n),
        None => MISSING.to_owned(),
    };
    field(out, "channels", &channels)?;

    let bitrate = match meta.bitrate_kbps {
        Some(kbps) => format!("{kbps} kbps"),
        None => MISSING.to_owned(),
    };
    field(out, "bitrate", &bitrate)?;

    let cover = match &meta.cover {
        Some(cover) => format_cover(cover),
        None => NONE.to_owned(),
    };
    field(out, "cover art", &cover)
}

/// What an absent tag looks like. A value, not an error.
const MISSING: &str = "(missing)";

/// What an absent cover looks like. Distinct from a missing tag: the file simply
/// carries no artwork.
const NONE: &str = "(none)";

fn field(out: &mut impl Write, label: &str, value: &str) -> io::Result<()> {
    writeln!(out, "  {label:<LABEL_WIDTH$}{value}")
}

/// `0:05.04`, `3:47.50`, `1:01:01.00` — the way a music player shows a time.
///
/// Hundredths are kept because `--sample` ranges are specified at that
/// precision, and because a fixture five hundredths over five seconds should
/// look like one.
fn format_duration(duration: Duration) -> String {
    let total = duration.as_secs_f64();
    let hours = (total / 3600.0).floor() as u64;
    let minutes = ((total % 3600.0) / 60.0).floor() as u64;
    let seconds = total % 60.0;

    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:05.2}")
    } else {
        format!("{minutes}:{seconds:05.2}")
    }
}

/// Name the common channel counts; a user reads "stereo" faster than "2".
fn format_channels(channels: u8) -> String {
    match channels {
        1 => "1 (mono)".to_owned(),
        2 => "2 (stereo)".to_owned(),
        n => n.to_string(),
    }
}

/// `image/png, 256x256, 2.8 KiB` — mime, dimensions, and weight.
///
/// Any of the three may be unknown for artwork avz cannot parse; the parts that
/// are known are still worth printing.
fn format_cover(cover: &CoverArt) -> String {
    let mime = cover.mime.as_deref().unwrap_or("unknown type");

    let dimensions = match cover.dimensions {
        Some((width, height)) => format!("{width}x{height}"),
        None => "unreadable image".to_owned(),
    };

    format!("{mime}, {dimensions}, {}", format_bytes(cover.data.len()))
}

/// Byte counts a human can compare at a glance.
fn format_bytes(bytes: usize) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;

    let bytes = bytes as f64;

    if bytes < KIB {
        format!("{bytes:.0} B")
    } else if bytes < MIB {
        format!("{:.1} KiB", bytes / KIB)
    } else {
        format!("{:.1} MiB", bytes / MIB)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rendered(meta: &TrackMeta) -> String {
        let mut out = Vec::new();
        report(&mut out, Path::new("song.mp3"), meta).expect("writing to a Vec cannot fail");
        String::from_utf8(out).expect("report is utf-8")
    }

    fn tagged() -> TrackMeta {
        TrackMeta {
            title: Some("Sine Tones".to_owned()),
            artist: Some("avz test fixture".to_owned()),
            album: Some("Public Domain Tones".to_owned()),
            duration: Duration::from_millis(5_041),
            sample_rate: Some(44_100),
            channels: Some(2),
            bitrate_kbps: Some(64),
            cover: Some(CoverArt {
                mime: Some("image/png".to_owned()),
                dimensions: Some((256, 256)),
                data: vec![0; 2_766],
            }),
        }
    }

    fn untagged() -> TrackMeta {
        TrackMeta {
            title: None,
            artist: None,
            album: None,
            duration: Duration::from_millis(5_041),
            sample_rate: Some(44_100),
            channels: Some(2),
            bitrate_kbps: Some(64),
            cover: None,
        }
    }

    #[test]
    fn a_tagged_file_reports_every_field() {
        let report = rendered(&tagged());

        assert_eq!(
            report,
            "\
song.mp3
  title       Sine Tones
  artist      avz test fixture
  album       Public Domain Tones
  duration    0:05.04
  sample rate 44100 Hz
  channels    2 (stereo)
  bitrate     64 kbps
  cover art   image/png, 256x256, 2.7 KiB
"
        );
    }

    /// The UT-005 contract: absent tags are reported, not fatal.
    #[test]
    fn missing_tags_render_as_missing_and_missing_art_as_none() {
        let report = rendered(&untagged());

        assert!(report.contains("title       (missing)"), "{report}");
        assert!(report.contains("artist      (missing)"), "{report}");
        assert!(report.contains("album       (missing)"), "{report}");
        assert!(report.contains("cover art   (none)"), "{report}");
        // The audio is still described.
        assert!(report.contains("sample rate 44100 Hz"), "{report}");
    }

    #[test]
    fn durations_read_like_a_music_player() {
        assert_eq!(format_duration(Duration::from_millis(5_041)), "0:05.04");
        assert_eq!(format_duration(Duration::from_millis(227_500)), "3:47.50");
        assert_eq!(format_duration(Duration::from_secs(3_661)), "1:01:01.00");
        assert_eq!(format_duration(Duration::ZERO), "0:00.00");
    }

    #[test]
    fn channel_counts_are_named_where_a_name_exists() {
        assert_eq!(format_channels(1), "1 (mono)");
        assert_eq!(format_channels(2), "2 (stereo)");
        assert_eq!(format_channels(6), "6");
    }

    #[test]
    fn byte_counts_scale_to_readable_units() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1_023), "1023 B");
        assert_eq!(format_bytes(1_024), "1.0 KiB");
        assert_eq!(format_bytes(2_766), "2.7 KiB");
        assert_eq!(format_bytes(3_145_728), "3.0 MiB");
    }

    /// Artwork avz cannot parse still reports what it does know.
    #[test]
    fn unparsable_cover_art_reports_type_and_size_anyway() {
        let cover = CoverArt {
            mime: Some("image/tiff".to_owned()),
            dimensions: None,
            data: vec![0; 100],
        };

        assert_eq!(format_cover(&cover), "image/tiff, unreadable image, 100 B");
    }

    #[test]
    fn cover_art_with_no_declared_mime_says_so() {
        let cover = CoverArt {
            mime: None,
            dimensions: Some((64, 64)),
            data: vec![0; 100],
        };

        assert_eq!(format_cover(&cover), "unknown type, 64x64, 100 B");
    }
}
