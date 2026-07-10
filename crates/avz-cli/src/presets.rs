//! `avz presets` — discovery (`VISION.md` §3, UT-004).
//!
//! With no argument it lists every preset avz ships. With a name it prints that
//! preset's full parameter schema: name, type, default, accepted range, and the
//! one-line description the schema carries.
//!
//! The formatting lives here rather than in `avz-core`, which never prints
//! (`AGENTS.md`, core/cli split). Core hands over a typed [`PresetSchema`]; this
//! module decides what a terminal sees.

use avz_core::render::{PRESETS, Preset, PresetSchema};

use crate::cli::PresetsArgs;

/// Two spaces between columns: enough to read, not enough to scan past.
const GUTTER: usize = 2;

/// List every preset, or print one preset's schema.
pub fn run(args: &PresetsArgs) -> anyhow::Result<()> {
    match &args.name {
        Some(name) => {
            let preset = Preset::by_name(name)?;
            print!("{}", describe(preset, &preset.schema()?));
        }
        None => print!("{}", list(PRESETS)),
    }

    Ok(())
}

/// Every preset, one per line, name and description in aligned columns.
fn list(presets: &[Preset]) -> String {
    let width = presets
        .iter()
        .map(|preset| preset.name.len())
        .max()
        .unwrap_or(0);

    let mut out = String::new();
    for preset in presets {
        out.push_str(&format!(
            "{:width$}{}{}\n",
            preset.name,
            " ".repeat(GUTTER),
            preset.description,
        ));
    }
    out.push_str("\nRun `avz presets <name>` for a preset's parameters.\n");
    out
}

/// One preset's schema, as `avz presets <name>` prints it.
fn describe(preset: &Preset, schema: &PresetSchema) -> String {
    let mut out = format!("{} — {}\n", preset.name, preset.description);

    if let Some(hint) = &schema.perf_hint {
        out.push_str(&format!("\nperformance: {hint}\n"));
    }

    let rows: Vec<[String; 4]> = schema
        .params
        .iter()
        .map(|param| {
            [
                param.name.clone(),
                param.kind.type_name().to_owned(),
                param.kind.default_display(),
                param.kind.range_display(),
            ]
        })
        .collect();

    let header = ["PARAMETER", "TYPE", "DEFAULT", "RANGE"];
    let mut widths = header.map(str::len);
    for row in &rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.len());
        }
    }

    let gutter = " ".repeat(GUTTER);
    out.push('\n');
    out.push_str(&format!(
        "{}{gutter}DESCRIPTION\n",
        header
            .iter()
            .zip(widths)
            .map(|(cell, width)| format!("{cell:width$}"))
            .collect::<Vec<_>>()
            .join(&gutter),
    ));

    for (row, param) in rows.iter().zip(&schema.params) {
        out.push_str(&format!(
            "{}{gutter}{}\n",
            row.iter()
                .zip(widths)
                .map(|(cell, width)| format!("{cell:width$}"))
                .collect::<Vec<_>>()
                .join(&gutter),
            param.description,
        ));
    }

    out.push_str(&format!(
        "\nSet one with `--set {}=<value>`, or under `[visual.params]` in a config file.\n",
        schema
            .params
            .first()
            .map_or("<name>", |param| param.name.as_str()),
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema(json: &str) -> PresetSchema {
        PresetSchema::parse("test", json).expect("a well-formed schema")
    }

    fn preset() -> Preset {
        Preset {
            name: "test",
            description: "a preset for testing",
            source: "",
            schema: "",
        }
    }

    /// UT-004, first half: one line per preset, name and description.
    #[test]
    fn the_listing_names_every_preset_and_describes_it() {
        let text = list(PRESETS);

        for shipped in PRESETS {
            assert!(text.contains(shipped.name), "{text}");
            assert!(text.contains(shipped.description), "{text}");
        }
        assert_eq!(
            text.lines().filter(|line| line.contains("  ")).count(),
            PRESETS.len(),
            "one aligned row per preset:\n{text}"
        );
    }

    /// UT-004, second half: name, type, default, range, and description — for
    /// every type a schema can declare, because each renders its range its own way.
    #[test]
    fn the_schema_print_shows_every_column_for_every_type() {
        let schema = schema(
            r##"{"params":[
                {"name":"drive","type":"float","default":1.0,"min":0.0,"max":4.0,
                 "slot":[0,0],"description":"How hard the kick drives."},
                {"name":"count","type":"int","default":4,"min":1,"max":32,
                 "slot":[0,1],"description":"How many rings."},
                {"name":"flash","type":"bool","default":true,
                 "slot":[0,2],"description":"Whether an onset flashes."},
                {"name":"mode","type":"enum","default":"rings","variants":["disc","rings"],
                 "slot":[0,3],"description":"What the preset draws."},
                {"name":"tint","type":"color","default":"#e94560",
                 "slot":[1,0],"description":"The color of the core."}
            ]}"##,
        );

        let text = describe(&preset(), &schema);

        assert!(text.contains("test — a preset for testing"), "{text}");
        assert!(text.contains("PARAMETER"), "{text}");
        assert!(text.contains("DESCRIPTION"), "{text}");

        for (name, kind, default, range, description) in [
            ("drive", "float", "1", "0..4", "How hard the kick drives."),
            ("count", "int", "4", "1..32", "How many rings."),
            (
                "flash",
                "bool",
                "true",
                "true|false",
                "Whether an onset flashes.",
            ),
            (
                "mode",
                "enum",
                "rings",
                "disc|rings",
                "What the preset draws.",
            ),
            (
                "tint",
                "color",
                "#e94560",
                "#rrggbb",
                "The color of the core.",
            ),
        ] {
            let row = text
                .lines()
                .find(|line| line.starts_with(name))
                .unwrap_or_else(|| panic!("no row for `{name}`:\n{text}"));

            for cell in [kind, default, range, description] {
                assert!(
                    row.contains(cell),
                    "`{name}` row is missing `{cell}`: {row}"
                );
            }
        }
    }

    /// The columns line up, or the table is worse than no table.
    #[test]
    fn the_schema_columns_are_aligned() {
        let schema = schema(
            r##"{"params":[
                {"name":"a","type":"float","default":1.0,"min":0.0,"max":4.0,
                 "slot":[0,0],"description":"short name"},
                {"name":"a_much_longer_name","type":"int","default":4,"min":1,"max":32,
                 "slot":[0,1],"description":"long name"}
            ]}"##,
        );

        let text = describe(&preset(), &schema);
        let rows: Vec<&str> = text
            .lines()
            .filter(|line| line.starts_with("PARAMETER") || line.starts_with('a'))
            .collect();
        assert_eq!(rows.len(), 3, "a header and two rows:\n{text}");

        // Where the type column begins: past the name, past the padding.
        let type_column = |row: &str| {
            let name = row.split(' ').next().expect("a name").len();
            name + row[name..]
                .find(|c: char| c != ' ')
                .expect("a second column")
        };

        let columns: Vec<usize> = rows.iter().map(|row| type_column(row)).collect();
        assert!(
            columns.iter().all(|at| *at == columns[0]),
            "the type column starts at {columns:?}, not one offset:\n{text}"
        );
    }

    /// `VISION.md` §7: a preset that is pathological on lavapipe says so.
    #[test]
    fn a_perf_hint_is_printed_when_the_schema_carries_one() {
        let without = schema(
            r##"{"params":[{"name":"a","type":"float","default":1.0,"min":0.0,"max":4.0,
                "slot":[0,0],"description":"d"}]}"##,
        );
        assert!(!describe(&preset(), &without).contains("performance"));

        let with = schema(
            r##"{"perf_hint":"on software rendering, consider count <= 1000",
                 "params":[{"name":"a","type":"float","default":1.0,"min":0.0,"max":4.0,
                 "slot":[0,0],"description":"d"}]}"##,
        );
        assert!(
            describe(&preset(), &with)
                .contains("performance: on software rendering, consider count <= 1000"),
        );
    }

    /// The print tells the reader how to change what they just read.
    #[test]
    fn the_schema_print_says_how_to_set_a_parameter() {
        let schema = schema(
            r##"{"params":[{"name":"drive","type":"float","default":1.0,"min":0.0,"max":4.0,
                "slot":[0,0],"description":"d"}]}"##,
        );

        let text = describe(&preset(), &schema);
        assert!(text.contains("--set drive=<value>"), "{text}");
        assert!(text.contains("[visual.params]"), "{text}");
    }
}
