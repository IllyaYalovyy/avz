//! `--set key.path=value` overrides.
//!
//! An assignment is turned into a one-key nested TOML table and deserialized
//! through the same [`ConfigLayer`] schema as a config file, so `--set` inherits
//! unknown-key rejection, "did you mean" hints, and type checking instead of
//! reimplementing them and drifting.
//!
//! Preset parameters get a shorthand. `visual.params.bass_drive=2.0` is the path
//! the config file uses, but `bass_drive=2.0` and `pulse.bass_drive=2.0` mean the
//! same thing, because a key that names no config section can only be a parameter
//! of the preset being rendered. The parameter's *name* is still validated later,
//! against the active preset's schema — this layer only decides which table the
//! value lands in.

use crate::render::preset;
use crate::{Error, Result};

use super::{ConfigLayer, closest};

/// The tables `ConfigLayer` declares. A first segment outside this set is a
/// preset parameter, not a mistyped section — unless it is nothing at all.
const SECTIONS: [&str; 5] = ["output", "visual", "background", "text", "effects"];

/// Parse one `key.path=value` assignment into a layer.
pub(super) fn layer_from_assignment(assignment: &str) -> Result<ConfigLayer> {
    let invalid = |reason: &str| {
        Error::Config(format!(
            "invalid `--set {assignment}`: {reason}, expected `key.path=value` \
             (e.g. `visual.intensity=1.4`)"
        ))
    };

    let (path, raw) = assignment
        .split_once('=')
        .ok_or_else(|| invalid("no `=` found"))?;

    let segments: Vec<&str> = path.trim().split('.').map(str::trim).collect();
    if segments.iter().any(|segment| segment.is_empty()) {
        return Err(invalid("the key path has an empty segment"));
    }

    let segments = expand_shorthand(assignment, &segments)?;

    // Build the table from the leaf up: `visual.params.bass_drive=2.0` becomes
    // `{ visual = { params = { bass_drive = 2.0 } } }`.
    let mut value = parse_scalar(raw.trim());
    for segment in segments.iter().rev() {
        let mut table = toml::Table::new();
        table.insert((*segment).to_owned(), value);
        value = toml::Value::Table(table);
    }

    value.try_into::<ConfigLayer>().map_err(|err| {
        let mut message = format!("invalid `--set {assignment}`: {}", err.message());
        if let Some(near) = super::suggestion(err.message()) {
            message.push_str(&format!("\nhint: did you mean `{near}`?"));
        }
        Error::Config(message)
    })
}

/// Rewrite a preset-parameter shorthand into the `visual.params.<name>` path the
/// config schema knows, and leave every other path alone.
///
/// # Errors
///
/// [`Error::Config`] for a first segment that is neither a config section nor a
/// preset — because `outputt.fps=30` is a typo worth naming, and silently filing
/// it under `visual.params` would report it as an unknown *parameter* instead.
fn expand_shorthand<'a>(assignment: &str, segments: &[&'a str]) -> Result<Vec<&'a str>> {
    let head = segments[0];
    if SECTIONS.contains(&head) {
        return Ok(segments.to_vec());
    }

    let presets = preset::names();
    let parameter = match segments.len() {
        1 => Some(head),
        2 if presets.contains(&head) => Some(segments[1]),
        _ => None,
    };

    if let Some(parameter) = parameter {
        return Ok(vec!["visual", "params", parameter]);
    }

    let candidates: Vec<&str> = SECTIONS
        .iter()
        .copied()
        .chain(presets.iter().copied())
        .collect();
    let mut message = format!(
        "invalid `--set {assignment}`: `{head}` is neither a config section ({}) \
         nor a preset ({}); a bare `--set name=value` sets a parameter of the \
         preset being rendered",
        SECTIONS.join(", "),
        presets.join(", "),
    );
    if let Some(near) = closest(head, candidates.into_iter()) {
        message.push_str(&format!("\nhint: did you mean `{near}`?"));
    }
    Err(Error::Config(message))
}

/// Interpret the right-hand side of an assignment as a TOML scalar.
///
/// `1.4` is a float, `true` is a bool, `["#fff"]` is an array. Anything TOML
/// cannot parse — `nebula`, `720p`, `art/forest.png` — is taken as a bare
/// string, which is what makes the unquoted `--set visual.preset=nebula` work.
/// A value that really must be a string can be quoted: `--set x='"true"'`.
fn parse_scalar(raw: &str) -> toml::Value {
    toml::from_str::<toml::Table>(&format!("value = {raw}"))
        .ok()
        .and_then(|mut table| table.remove("value"))
        .unwrap_or_else(|| toml::Value::String(raw.to_owned()))
}
