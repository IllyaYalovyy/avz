//! The reference docs cannot drift from the code (#33).
//!
//! `docs/PRESETS.md` and `docs/CONFIGURATION.md` are hand-written prose, but
//! their *coverage* is checked here against the same embedded registry,
//! schemas, and example generator the binary ships. A preset, parameter, or
//! config key that the docs do not mention fails the suite, so adding one
//! without documenting it is a red test rather than a silent gap.
//!
//! The check is presence, not truth: prose still needs review. Presence is
//! what rots first.

use std::fs;
use std::path::PathBuf;
use std::sync::LazyLock;

use avz_core::config;
use avz_core::render::PRESETS;

/// A file under `docs/`, read once.
fn doc(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs")
        .join(name);
    fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("{} must exist for this suite: {err}", path.display()))
}

static PRESETS_DOC: LazyLock<String> = LazyLock::new(|| doc("PRESETS.md"));
static CONFIGURATION_DOC: LazyLock<String> = LazyLock::new(|| doc("CONFIGURATION.md"));

/// `haystack` contains `needle`, ignoring how either wraps its lines: markdown
/// re-flows prose, and a description or hint must be allowed to wrap without
/// counting as missing.
fn contains_unwrapped(haystack: &str, needle: &str) -> bool {
    let flatten = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
    flatten(haystack).contains(&flatten(needle))
}

#[test]
fn every_preset_is_documented_with_its_description() {
    for preset in PRESETS {
        assert!(
            PRESETS_DOC.contains(&format!("## `{}`", preset.name)),
            "docs/PRESETS.md has no `## \\`{}\\`` section",
            preset.name
        );
        assert!(
            contains_unwrapped(&PRESETS_DOC, preset.description),
            "docs/PRESETS.md does not carry `{}`'s one-line description \
             (it must match the registry verbatim): {}",
            preset.name,
            preset.description
        );
    }
}

#[test]
fn every_preset_parameter_is_documented() {
    for preset in PRESETS {
        let schema = preset.schema().expect("the shipped schema parses");
        for param in &schema.params {
            assert!(
                PRESETS_DOC.contains(&format!("`{}`", param.name)),
                "docs/PRESETS.md does not mention `{}`'s parameter `{}`",
                preset.name,
                param.name
            );
            assert!(
                PRESETS_DOC.contains(&param.kind.default_display()),
                "docs/PRESETS.md does not carry the default of `{}`'s `{}` \
                 ({}); the table is stale",
                preset.name,
                param.name,
                param.kind.default_display()
            );
        }
    }
}

#[test]
fn every_preset_perf_hint_is_documented() {
    for preset in PRESETS {
        let schema = preset.schema().expect("the shipped schema parses");
        if let Some(hint) = &schema.perf_hint {
            assert!(
                contains_unwrapped(&PRESETS_DOC, hint),
                "docs/PRESETS.md does not carry `{}`'s perf_hint verbatim",
                preset.name
            );
        }
    }
}

#[test]
fn every_preset_has_an_example_command() {
    for preset in PRESETS {
        assert!(
            PRESETS_DOC.contains(&format!("--preset {}", preset.name)),
            "docs/PRESETS.md has no `--preset {}` example command",
            preset.name
        );
    }
}

/// Every key and section the example config emits appears in the
/// configuration reference. The example is generated from the config structs,
/// so this transitively pins the docs to the code.
#[test]
fn every_config_key_is_documented() {
    for line in config::example().lines() {
        let line = line.trim();

        if line.starts_with('[') {
            assert!(
                CONFIGURATION_DOC.contains(line),
                "docs/CONFIGURATION.md does not mention the `{line}` section"
            );
            continue;
        }

        // `key = value` lines, including the commented-out optional ones the
        // example documents (`# image = ...`).
        let uncommented = line.strip_prefix("# ").unwrap_or(line);
        if let Some((key, _)) = uncommented.split_once(" = ") {
            if key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                assert!(
                    CONFIGURATION_DOC.contains(&format!("`{key}`")),
                    "docs/CONFIGURATION.md does not mention the `{key}` key"
                );
            }
        }
    }
}
