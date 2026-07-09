//! TOML configuration: schema, validation, and merging.
//!
//! Precedence is fixed: CLI flags > `--set` overrides > `--config` file > preset
//! defaults > built-in defaults. Unknown keys are rejected with "did you mean"
//! suggestions rather than silently ignored (`VISION.md` §5.5).
//!
//! Populated by RFC-001 Step 2.
