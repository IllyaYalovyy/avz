//! Preset parameter schemas: the JSON half of a preset (`VISION.md` §6).
//!
//! A preset is a WGSL file plus a schema, both embedded in the binary. The
//! schema names every parameter the shader reads, its type, its default, the
//! range it is allowed to take, and the `params: array<vec4<f32>, 8>` slot it
//! occupies. That one file is what `avz presets <name>` prints, what
//! `[visual.params]` and `--set` are validated against, and what turns a
//! `toml::Table` of user values into the bytes the uniform carries.
//!
//! Nothing here reaches for the GPU. [`PresetSchema::resolve`] runs before the
//! song is decoded, so a typo'd parameter costs a millisecond rather than a
//! five-minute analysis pass.

use serde::Deserialize;

use crate::config::{Color, closest};
use crate::render::globals::PARAM_SLOTS;
use crate::render::palette::linear_rgba;
use crate::{Error, Result};

/// Components in one `vec4<f32>` uniform slot.
pub const SLOT_COMPONENTS: usize = 4;

/// The `params` half of the uniform, packed and ready to encode.
pub type PackedParams = [[f32; SLOT_COMPONENTS]; PARAM_SLOTS];

/// Every parameter one preset exposes, in the order its schema declares them.
#[derive(Debug, Clone, PartialEq)]
pub struct PresetSchema {
    /// The preset the schema belongs to. Quoted in every error message.
    pub preset: String,
    /// The parameters, in declaration order — which is the order
    /// `avz presets <name>` prints them in.
    pub params: Vec<Param>,
    /// Whether the preset samples the previous frame (`VISION.md` §6).
    ///
    /// The one binding a preset may ask the renderer for beyond the uniform.
    /// Declaring it binds last frame's pixels at `@binding(1)` and a sampler at
    /// `@binding(2)`; on the first frame of a render they are black. A preset
    /// that leaves it false gets neither, and a shader that reaches for them
    /// anyway fails to build rather than sampling whatever was there.
    pub needs_feedback: bool,
    /// What to tell a user rendering this preset on lavapipe (`VISION.md` §7).
    pub perf_hint: Option<String>,
}

/// One tunable knob of a preset.
#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    /// The key under `[visual.params]`, and the word `--set` names.
    pub name: String,
    /// One line, printed by `avz presets <name>`.
    pub description: String,
    /// The type, its default, and the values it accepts.
    pub kind: ParamKind,
    /// Where the packed value lands in `Globals::params`.
    pub slot: Slot,
}

/// A parameter's type, together with everything that type constrains.
#[derive(Debug, Clone, PartialEq)]
pub enum ParamKind {
    /// A real number, packed as itself.
    Float {
        /// Value used when the config names no other.
        default: f32,
        /// Smallest accepted value, inclusive.
        min: f32,
        /// Largest accepted value, inclusive.
        max: f32,
    },
    /// A whole number, packed as an `f32` — the uniform holds nothing else.
    Int {
        /// Value used when the config names no other.
        default: i64,
        /// Smallest accepted value, inclusive.
        min: i64,
        /// Largest accepted value, inclusive.
        max: i64,
    },
    /// A switch, packed as `1.0` or `0.0`.
    Bool {
        /// Value used when the config names no other.
        default: bool,
    },
    /// One of a fixed set of names, packed as its index in `variants`.
    Enum {
        /// Value used when the config names no other. Always in `variants`.
        default: String,
        /// The accepted names, in the order the shader indexes them.
        variants: Vec<String>,
    },
    /// An sRGB color, packed as linear RGBA across a whole `vec4` slot.
    Color {
        /// Value used when the config names no other.
        default: Color,
    },
}

/// Where a parameter's value lands in `Globals::params`.
///
/// A [`ParamKind::Color`] occupies all four components of `index` and therefore
/// always starts at component 0; every other kind occupies one component.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slot {
    /// Which `vec4` of `params`, in `0..PARAM_SLOTS`.
    pub index: usize,
    /// Which component of that `vec4`: 0 is `x`, 3 is `w`.
    pub component: usize,
}

impl ParamKind {
    /// The word `avz presets <name>` prints in the type column.
    pub fn type_name(&self) -> &'static str {
        match self {
            ParamKind::Float { .. } => "float",
            ParamKind::Int { .. } => "int",
            ParamKind::Bool { .. } => "bool",
            ParamKind::Enum { .. } => "enum",
            ParamKind::Color { .. } => "color",
        }
    }

    /// The default, as the user would write it in a config file.
    pub fn default_display(&self) -> String {
        match self {
            ParamKind::Float { default, .. } => format!("{default}"),
            ParamKind::Int { default, .. } => format!("{default}"),
            ParamKind::Bool { default } => format!("{default}"),
            ParamKind::Enum { default, .. } => default.clone(),
            ParamKind::Color { default } => default.to_string(),
        }
    }

    /// The accepted values, as one short phrase.
    pub fn range_display(&self) -> String {
        match self {
            ParamKind::Float { min, max, .. } => format!("{min}..{max}"),
            ParamKind::Int { min, max, .. } => format!("{min}..{max}"),
            ParamKind::Bool { .. } => "true|false".to_owned(),
            ParamKind::Enum { variants, .. } => variants.join("|"),
            ParamKind::Color { .. } => "#rrggbb".to_owned(),
        }
    }

    /// How many components of its slot this kind occupies.
    fn width(&self) -> usize {
        match self {
            ParamKind::Color { .. } => SLOT_COMPONENTS,
            _ => 1,
        }
    }
}

impl PresetSchema {
    /// Parse `preset`'s schema from the JSON embedded beside its WGSL.
    ///
    /// # Errors
    ///
    /// [`Error::Config`] if the JSON is malformed, a default falls outside its
    /// own declared range, or two parameters claim the same uniform component.
    /// The schemas ship inside the binary, so any of those is a bug in the
    /// preset rather than in the user's config — `every_shipped_schema_parses`
    /// is what keeps it from ever reaching a user.
    pub fn parse(preset: &str, json: &str) -> Result<Self> {
        let raw: RawSchema = serde_json::from_str(json).map_err(|err| {
            Error::Config(format!("preset `{preset}` has a malformed schema: {err}"))
        })?;

        let bad = |reason: String| Error::Config(format!("preset `{preset}` schema: {reason}"));

        let params: Vec<Param> = raw
            .params
            .into_iter()
            .map(|param| param.validate().map_err(&bad))
            .collect::<Result<_>>()?;

        let mut claimed = [[false; SLOT_COMPONENTS]; PARAM_SLOTS];
        for param in &params {
            if params
                .iter()
                .filter(|other| other.name == param.name)
                .count()
                > 1
            {
                return Err(bad(format!("`{}` is declared twice", param.name)));
            }

            let Slot { index, component } = param.slot;
            let width = param.kind.width();
            if index >= PARAM_SLOTS || component + width > SLOT_COMPONENTS {
                return Err(bad(format!(
                    "`{}` claims slot [{index}, {component}], which is outside the \
                     {PARAM_SLOTS} x {SLOT_COMPONENTS} `params` uniform",
                    param.name,
                )));
            }

            for taken in &mut claimed[index][component..component + width] {
                if *taken {
                    return Err(bad(format!(
                        "`{}` claims a uniform component another parameter already uses",
                        param.name,
                    )));
                }
                *taken = true;
            }
        }

        Ok(Self {
            preset: preset.to_owned(),
            params,
            needs_feedback: raw.needs_feedback,
            perf_hint: raw.perf_hint,
        })
    }

    /// Validate `overrides` against the schema and pack the result.
    ///
    /// Parameters the user did not name keep their schema default. This is the
    /// only place a `[visual.params]` table becomes uniform bytes.
    ///
    /// # Errors
    ///
    /// [`Error::Config`] — exit code 2 — for a parameter the preset does not
    /// have (with a "did you mean" when one is close), a value of the wrong
    /// type, or a value outside the range the schema declares.
    pub fn resolve(&self, overrides: &toml::Table) -> Result<PackedParams> {
        for name in overrides.keys() {
            if self.params.iter().any(|param| &param.name == name) {
                continue;
            }

            let known: Vec<&str> = self.params.iter().map(|p| p.name.as_str()).collect();
            let mut message = format!(
                "unknown parameter `{name}` for preset `{}`; it accepts: {}",
                self.preset,
                known.join(", "),
            );
            if let Some(near) = closest(name, known.iter().copied()) {
                message.push_str(&format!("\nhint: did you mean `{near}`?"));
            }
            return Err(Error::Config(message));
        }

        let mut packed: PackedParams = [[0.0; SLOT_COMPONENTS]; PARAM_SLOTS];
        for param in &self.params {
            let value = match overrides.get(&param.name) {
                Some(value) => param.read(value)?,
                None => param.default_packed(),
            };

            let Slot { index, component } = param.slot;
            let width = param.kind.width();
            packed[index][component..component + width].copy_from_slice(&value[..width]);
        }

        Ok(packed)
    }
}

impl Param {
    /// The `visual.params.<name>` key errors quote, so the user can find it.
    fn key(&self) -> String {
        format!("visual.params.{}", self.name)
    }

    /// This parameter's default, packed.
    fn default_packed(&self) -> [f32; SLOT_COMPONENTS] {
        let mut packed = [0.0; SLOT_COMPONENTS];
        match &self.kind {
            ParamKind::Float { default, .. } => packed[0] = *default,
            ParamKind::Int { default, .. } => packed[0] = *default as f32,
            ParamKind::Bool { default } => packed[0] = f32::from(*default),
            ParamKind::Enum { default, variants } => {
                let index = variants.iter().position(|v| v == default).unwrap_or(0);
                packed[0] = index as f32;
            }
            ParamKind::Color { default } => packed = linear_rgba(*default),
        }
        packed
    }

    /// Interpret one user-supplied TOML value as this parameter.
    fn read(&self, value: &toml::Value) -> Result<[f32; SLOT_COMPONENTS]> {
        let key = self.key();
        let wrong_type = || {
            Error::Config(format!(
                "`{key}` is a {}, got {}",
                self.kind.type_name(),
                describe(value),
            ))
        };
        let out_of_range = |got: String| {
            Error::Config(format!(
                "`{key}` must be within {}, got {got}",
                self.kind.range_display(),
            ))
        };

        let mut packed = [0.0; SLOT_COMPONENTS];
        match &self.kind {
            ParamKind::Float { min, max, .. } => {
                // A bare `--set bass_drive=2` is a TOML integer, and refusing it
                // would be pedantry: every integer is a float.
                let number = match value {
                    toml::Value::Float(number) => *number,
                    toml::Value::Integer(number) => *number as f64,
                    _ => return Err(wrong_type()),
                };
                let number = number as f32;
                if !number.is_finite() || number < *min || number > *max {
                    return Err(out_of_range(format!("{number}")));
                }
                packed[0] = number;
            }
            ParamKind::Int { min, max, .. } => {
                // A float is not an int: `ring_count = 4.5` is a mistake worth
                // naming rather than a 4 the user never asked for.
                let toml::Value::Integer(number) = value else {
                    return Err(wrong_type());
                };
                if number < min || number > max {
                    return Err(out_of_range(format!("{number}")));
                }
                packed[0] = *number as f32;
            }
            ParamKind::Bool { .. } => {
                let toml::Value::Boolean(flag) = value else {
                    return Err(wrong_type());
                };
                packed[0] = f32::from(*flag);
            }
            ParamKind::Enum { variants, .. } => {
                let toml::Value::String(name) = value else {
                    return Err(wrong_type());
                };
                let index = variants.iter().position(|v| v == name).ok_or_else(|| {
                    let mut message = format!(
                        "`{key}` must be one of: {}, got `{name}`",
                        variants.join(", "),
                    );
                    if let Some(near) = closest(name, variants.iter().map(String::as_str)) {
                        message.push_str(&format!("\nhint: did you mean `{near}`?"));
                    }
                    Error::Config(message)
                })?;
                packed[0] = index as f32;
            }
            ParamKind::Color { .. } => {
                let toml::Value::String(hex) = value else {
                    return Err(wrong_type());
                };
                let color: Color = hex.parse().map_err(|err| {
                    Error::Config(format!("`{key}`: {}", crate::Error::from(err)))
                })?;
                packed = linear_rgba(color);
            }
        }

        Ok(packed)
    }
}

/// What kind of TOML value the user actually wrote, for a type-mismatch message.
fn describe(value: &toml::Value) -> String {
    match value {
        toml::Value::String(text) => format!("the string `{text}`"),
        toml::Value::Integer(number) => format!("the integer `{number}`"),
        toml::Value::Float(number) => format!("the float `{number}`"),
        toml::Value::Boolean(flag) => format!("the boolean `{flag}`"),
        other => format!("a {}", other.type_str()),
    }
}

/// The JSON a `presets/<name>.json` file holds, before it is validated.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSchema {
    #[serde(default)]
    params: Vec<RawParam>,
    #[serde(default)]
    needs_feedback: bool,
    #[serde(default)]
    perf_hint: Option<String>,
}

/// One JSON parameter. The type-specific keys are optional here and required by
/// [`RawParam::validate`], so a missing `max` names the parameter it belongs to
/// instead of a byte offset.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawParam {
    name: String,
    description: String,
    #[serde(rename = "type")]
    kind: String,
    slot: [usize; 2],
    default: serde_json::Value,
    min: Option<serde_json::Value>,
    max: Option<serde_json::Value>,
    variants: Option<Vec<String>>,
}

impl RawParam {
    /// Turn the JSON into a [`Param`], or say what the schema got wrong.
    fn validate(self) -> std::result::Result<Param, String> {
        let name = self.name;
        let bad = |reason: &str| format!("`{name}`: {reason}");

        if name.trim().is_empty() {
            return Err("a parameter has no name".to_owned());
        }
        if self.description.trim().is_empty() {
            return Err(bad("has no description; `avz presets` prints one"));
        }

        let number = |value: Option<&serde_json::Value>, what: &str| -> Result<f64, String> {
            value
                .and_then(serde_json::Value::as_f64)
                .ok_or_else(|| bad(&format!("needs a numeric `{what}`")))
        };
        let integer = |value: Option<&serde_json::Value>, what: &str| -> Result<i64, String> {
            value
                .and_then(serde_json::Value::as_i64)
                .ok_or_else(|| bad(&format!("needs an integer `{what}`")))
        };

        let kind = match self.kind.as_str() {
            "float" => {
                let default = number(Some(&self.default), "default")? as f32;
                let min = number(self.min.as_ref(), "min")? as f32;
                let max = number(self.max.as_ref(), "max")? as f32;
                if min > max {
                    return Err(bad("declares a min above its max"));
                }
                if default < min || default > max {
                    return Err(bad(&format!(
                        "defaults to {default}, outside its own range {min}..{max}"
                    )));
                }
                ParamKind::Float { default, min, max }
            }
            "int" => {
                let default = integer(Some(&self.default), "default")?;
                let min = integer(self.min.as_ref(), "min")?;
                let max = integer(self.max.as_ref(), "max")?;
                if min > max {
                    return Err(bad("declares a min above its max"));
                }
                if default < min || default > max {
                    return Err(bad(&format!(
                        "defaults to {default}, outside its own range {min}..{max}"
                    )));
                }
                ParamKind::Int { default, min, max }
            }
            "bool" => {
                let default = self
                    .default
                    .as_bool()
                    .ok_or_else(|| bad("needs a boolean `default`"))?;
                ParamKind::Bool { default }
            }
            "enum" => {
                let variants = self
                    .variants
                    .filter(|variants| !variants.is_empty())
                    .ok_or_else(|| bad("needs a non-empty `variants`"))?;
                let default = self
                    .default
                    .as_str()
                    .ok_or_else(|| bad("needs a string `default`"))?
                    .to_owned();
                if !variants.contains(&default) {
                    return Err(bad(&format!("defaults to `{default}`, not a variant")));
                }
                ParamKind::Enum { default, variants }
            }
            "color" => {
                let default: Color = self
                    .default
                    .as_str()
                    .ok_or_else(|| bad("needs a `#rrggbb` string `default`"))?
                    .parse()
                    .map_err(|err| bad(&format!("{err}")))?;
                ParamKind::Color { default }
            }
            other => return Err(bad(&format!("has unknown type `{other}`"))),
        };

        Ok(Param {
            name,
            description: self.description,
            kind,
            slot: Slot {
                index: self.slot[0],
                component: self.slot[1],
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every parameter type, each in its own uniform component, plus the color
    /// that claims a whole `vec4`. Used wherever a test needs a schema that is
    /// not one avz ships.
    fn every_type() -> PresetSchema {
        PresetSchema::parse(
            "test",
            r##"{
              "perf_hint": "on software rendering, keep `count` under 8",
              "params": [
                {"name":"drive","type":"float","default":1.0,"min":0.0,"max":4.0,
                 "slot":[0,0],"description":"How hard the kick drives."},
                {"name":"count","type":"int","default":4,"min":1,"max":32,
                 "slot":[0,1],"description":"How many rings."},
                {"name":"flash","type":"bool","default":true,
                 "slot":[0,2],"description":"Whether an onset flashes."},
                {"name":"mode","type":"enum","default":"rings","variants":["disc","rings","grid"],
                 "slot":[0,3],"description":"What the preset draws."},
                {"name":"tint","type":"color","default":"#ffffff",
                 "slot":[1,0],"description":"The color of the core."}
              ]
            }"##,
        )
        .expect("the test schema is well formed")
    }

    fn table(source: &str) -> toml::Table {
        source.parse().expect("a TOML table")
    }

    /// A default outside its own range would silently render something the
    /// schema says is illegal — and `avz presets` would print a lie.
    #[test]
    fn a_schema_whose_default_is_outside_its_own_range_is_rejected() {
        let err = PresetSchema::parse(
            "test",
            r##"{"params":[{"name":"drive","type":"float","default":9.0,"min":0.0,"max":4.0,
                "slot":[0,0],"description":"d"}]}"##,
        )
        .expect_err("9.0 is not within 0..4");

        let message = err.to_string();
        assert!(message.contains("drive"), "{message}");
        assert!(message.contains("outside its own range"), "{message}");
    }

    /// Two parameters in one component means the second silently overwrites the
    /// first, and one of the two knobs does nothing.
    #[test]
    fn two_parameters_may_not_claim_the_same_uniform_component() {
        let err = PresetSchema::parse(
            "test",
            r##"{"params":[
                {"name":"a","type":"float","default":0.0,"min":0.0,"max":1.0,"slot":[0,0],"description":"d"},
                {"name":"b","type":"float","default":0.0,"min":0.0,"max":1.0,"slot":[0,0],"description":"d"}
            ]}"##,
        )
        .expect_err("both claim params[0].x");

        assert!(err.to_string().contains("already uses"), "{err}");
    }

    /// A color is four floats, so it cannot start halfway through a `vec4`.
    #[test]
    fn a_color_cannot_start_partway_through_its_slot() {
        let err = PresetSchema::parse(
            "test",
            r##"{"params":[{"name":"tint","type":"color","default":"#fff","slot":[0,2],"description":"d"}]}"##,
        )
        .expect_err("a color at component 2 would run off the end of the vec4");

        assert!(err.to_string().contains("outside"), "{err}");
    }

    #[test]
    fn a_slot_beyond_the_uniform_is_rejected() {
        let err = PresetSchema::parse(
            "test",
            r##"{"params":[{"name":"a","type":"float","default":0.0,"min":0.0,"max":1.0,
                "slot":[8,0],"description":"d"}]}"##,
        )
        .expect_err("there are only eight slots");

        assert!(err.to_string().contains("outside"), "{err}");
    }

    /// A typo in a parameter name is the user's argument, and the fix is the
    /// name they meant.
    #[test]
    fn unknown_param_rejected_with_suggestion() {
        let err = every_type()
            .resolve(&table("drve = 2.0"))
            .expect_err("`drve` is not a parameter");

        let message = err.to_string();
        assert!(matches!(err, Error::Config(_)), "exit 2, not exit 4");
        assert!(message.contains("unknown parameter `drve`"), "{message}");
        assert!(message.contains("did you mean `drive`"), "{message}");
    }

    /// "Out of range" without the range is a riddle. Say what is allowed.
    #[test]
    fn out_of_range_value_names_the_allowed_range() {
        let err = every_type()
            .resolve(&table("drive = 9.0"))
            .expect_err("9.0 is above the declared maximum");

        let message = err.to_string();
        assert!(message.contains("visual.params.drive"), "{message}");
        assert!(message.contains("0..4"), "must name the range: {message}");
        assert!(message.contains('9'), "must quote the value: {message}");

        let err = every_type()
            .resolve(&table("count = 99"))
            .expect_err("99 is above the declared maximum");
        assert!(err.to_string().contains("1..32"), "{err}");
    }

    /// `--set drive=2` is a TOML integer. Refusing it would be pedantry.
    #[test]
    fn a_float_parameter_accepts_a_bare_integer() {
        let packed = every_type().resolve(&table("drive = 2")).expect("2 is 2.0");
        assert_eq!(packed[0][0], 2.0);
    }

    /// `count = 4.5` is a mistake worth naming, not a 4 nobody asked for.
    #[test]
    fn an_int_parameter_rejects_a_float() {
        let err = every_type()
            .resolve(&table("count = 4.5"))
            .expect_err("4.5 rings is not a number of rings");

        let message = err.to_string();
        assert!(message.contains("is a int"), "{message}");
        assert!(message.contains("4.5"), "{message}");
    }

    #[test]
    fn a_bool_parameter_rejects_the_string_true() {
        let err = every_type()
            .resolve(&table("flash = \"true\""))
            .expect_err("a quoted `true` is a string");

        assert!(err.to_string().contains("is a bool"), "{err}");
    }

    #[test]
    fn an_enum_value_outside_its_variants_is_rejected_with_a_suggestion() {
        let err = every_type()
            .resolve(&table("mode = \"ringz\""))
            .expect_err("`ringz` is not a variant");

        let message = err.to_string();
        assert!(message.contains("disc, rings, grid"), "{message}");
        assert!(message.contains("did you mean `rings`"), "{message}");
    }

    /// Each type reaches the component its schema names, and nothing else.
    #[test]
    fn every_type_packs_into_the_component_its_schema_declares() {
        let packed = every_type()
            .resolve(&table(
                "drive = 2.5\ncount = 7\nflash = false\nmode = \"grid\"\ntint = \"#000000\"\n",
            ))
            .expect("every value is legal");

        assert_eq!(packed[0][0], 2.5, "float into params[0].x");
        assert_eq!(packed[0][1], 7.0, "int into params[0].y, as an f32");
        assert_eq!(packed[0][2], 0.0, "false into params[0].z");
        assert_eq!(packed[0][3], 2.0, "the enum's index into params[0].w");
        assert_eq!(packed[1], [0.0, 0.0, 0.0, 1.0], "black, opaque, in linear");

        for slot in &packed[2..] {
            assert_eq!(*slot, [0.0; 4], "an unclaimed slot stays zero");
        }
    }

    /// A color reaches the shader in linear space, like the palette does.
    #[test]
    fn a_color_parameter_is_linearized_across_its_whole_slot() {
        let packed = every_type()
            .resolve(&table("tint = \"#808080\""))
            .expect("a legal color");

        assert!(
            (packed[1][0] - 0.2158).abs() < 0.001,
            "sRGB 0x80 is ~0.216 in linear, got {}",
            packed[1][0],
        );
        assert_eq!(packed[1][3], 1.0, "opaque");
    }

    #[test]
    fn a_malformed_color_names_the_key_it_belongs_to() {
        let err = every_type()
            .resolve(&table("tint = \"puce\""))
            .expect_err("`puce` is not hex");

        let message = err.to_string();
        assert!(message.contains("visual.params.tint"), "{message}");
        assert!(message.contains("#rrggbb"), "{message}");
    }

    /// A parameter the config never names keeps the schema's default.
    #[test]
    fn missing_parameters_take_their_schema_defaults() {
        let defaults = every_type().resolve(&toml::Table::new()).expect("defaults");
        let partial = every_type().resolve(&table("drive = 1.0")).expect("legal");

        assert_eq!(
            defaults[0],
            [1.0, 4.0, 1.0, 1.0],
            "drive, count, flash, mode"
        );
        assert_eq!(
            defaults, partial,
            "naming a parameter at its default changes nothing"
        );
    }

    /// `VISION.md` §7: the schema may carry a note for software rendering.
    #[test]
    fn a_perf_hint_is_optional_and_survives_parsing() {
        assert_eq!(
            every_type().perf_hint.as_deref(),
            Some("on software rendering, keep `count` under 8"),
        );

        let bare = PresetSchema::parse("test", r##"{"params":[]}"##).expect("an empty schema");
        assert!(bare.perf_hint.is_none());
        assert!(bare.params.is_empty());
    }

    /// `VISION.md` §6: the previous-frame texture is opt-in, and a preset that
    /// says nothing must not be handed a binding its shader never declared.
    #[test]
    fn feedback_is_off_unless_the_schema_asks_for_it() {
        assert!(!every_type().needs_feedback, "no `needs_feedback` key");

        let asked = PresetSchema::parse("test", r##"{"needs_feedback":true,"params":[]}"##)
            .expect("a schema may ask for the previous frame");
        assert!(asked.needs_feedback);
    }

    #[test]
    fn an_unknown_schema_key_is_a_malformed_schema() {
        let err = PresetSchema::parse("test", r##"{"parameters":[]}"##)
            .expect_err("`params`, not `parameters`");
        assert!(err.to_string().contains("malformed schema"), "{err}");
    }

    #[test]
    fn a_parameter_without_a_description_is_rejected() {
        let err = PresetSchema::parse(
            "test",
            r##"{"params":[{"name":"a","type":"float","default":0.0,"min":0.0,"max":1.0,
                "slot":[0,0],"description":"  "}]}"##,
        )
        .expect_err("`avz presets` would print a blank line");

        assert!(err.to_string().contains("description"), "{err}");
    }
}
