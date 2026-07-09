//! Orchestration: analysis → render → encode.
//!
//! Owns the two-pass flow and reports progress through the
//! [`Progress`](crate::Progress) callback trait. Analysis completes fully before
//! the first frame is rendered (`VISION.md` §4.2).
//!
//! Populated by RFC-001 Step 9.
