//! FFmpeg subprocess management.
//!
//! FFmpeg is a subprocess, never a crate. Raw RGBA frames go to its stdin; the
//! original mp3 stream is muxed with `-c:a copy` and never re-encoded. Output is
//! written to `out.mp4.part` and renamed on success (`VISION.md` §5.4).
//!
//! Populated by RFC-001 Steps 3 and 8.
