//! ID3 tags and cover art.
//!
//! Reads title, artist, album, and embedded artwork via `lofty`. Missing tags
//! warn and skip the text card rather than failing the render
//! (`VISION.md` §5.2).
//!
//! Populated by RFC-001 Step 4.
