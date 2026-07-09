//! `--set key.path=value` overrides.
//!
//! An assignment is turned into a one-key nested TOML table and deserialized
//! through the same [`ConfigLayer`] schema as a config file, so `--set` inherits
//! unknown-key rejection, "did you mean" hints, and type checking instead of
//! reimplementing them and drifting.

use crate::{Error, Result};

use super::{ConfigLayer, suggestion};

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
        if let Some(near) = suggestion(err.message()) {
            message.push_str(&format!("\nhint: did you mean `{near}`?"));
        }
        Error::Config(message)
    })
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
