//! Golden frames: every shipped preset, rendered to a hash (`docs/TESTING.md`).
//!
//! A shader regression is invisible everywhere else. Unit tests cover the DSP,
//! `pipeline_render.rs` covers which frames were drawn, and `render_e2e.rs`
//! covers whether the mp4 opens — none of them would notice a preset that
//! quietly stopped drawing its rings. This does: it renders hand-built
//! `FeatureFrame`s, never audio, so the expected picture depends on nothing but
//! the WGSL, the uniform layout, and the seed.
//!
//! **Software adapter only.** GPU float differences across machines are expected
//! (`AGENTS.md`, determinism), so a golden hash from a hardware adapter would be
//! a hash of that machine. `scripts/quality.d/95-golden-frames-run-on-the-software-adapter.sh`
//! keeps it that way.
//!
//! **Regenerating.** When a preset changes on purpose:
//!
//! ```bash
//! AVZ_UPDATE_GOLDEN=1 cargo test -p avz-core --test golden_frames
//! ```
//!
//! That rewrites `tests/golden/pulse.txt`; commit the new hashes with the shader
//! change and say in the commit message what moved. Never regenerate to make a
//! red test green without looking at why it went red.
//!
//! Needs Mesa's software Vulkan driver: `sudo dnf install mesa-vulkan-drivers`.

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};

use avz_core::analysis::FeatureFrame;
use avz_core::config::Palette;
use avz_core::render::{
    AdapterChoice, BUILT_INS, Globals, Gpu, LinearPalette, Offscreen, PRESETS, PackedParams,
    ParamKind, Preset, Visualizer, palette,
};
use sha2::{Digest, Sha256};

/// Small enough that lavapipe renders a frame in milliseconds, and 256-byte
/// aligned per row so a readback padding bug cannot hide in these hashes.
const WIDTH: u32 = 320;
const HEIGHT: u32 = 180;
const FPS: u32 = 30;

/// Fixed forever: a golden hash is a hash of its seed too.
const GOLDEN_SEED: u64 = 1337;

/// The frames every preset is pinned at: the first, one early, one well into a
/// song. Frame 0 catches a shader that only works once `time` has advanced.
const GOLDEN_FRAMES: [usize; 3] = [0, 10, 100];

/// See `pipeline_render.rs`: one Vulkan device per process, or the loader's
/// debug-utils terminator segfaults when two tests open devices at once.
static ONE_DEVICE_AT_A_TIME: Mutex<()> = Mutex::new(());

fn one_device_at_a_time() -> MutexGuard<'static, ()> {
    ONE_DEVICE_AT_A_TIME
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

fn golden_file(preset: &Preset) -> PathBuf {
    golden_dir().join(format!("{}.txt", preset.name))
}

/// The palette every preset's golden hashes are rendered with.
///
/// `ember` is the default (`Config::default`), so the committed preset hashes
/// are hashes of a zero-config render. The other four built-ins are pinned by
/// `every_built_in_renders_its_golden_frame` instead.
fn ember() -> LinearPalette {
    named("ember")
}

fn named(name: &str) -> LinearPalette {
    palette::resolve(&Palette::Named(name.to_owned()))
        .unwrap_or_else(|err| panic!("`{name}` ships: {err}"))
}

/// The features of golden frame `frame_index`, built by hand.
///
/// Not analyzed from audio: the point of a golden frame is that its input is
/// written down, so a change in the DSP cannot silently rewrite the picture the
/// shader is being held to. The three frames span the shader's inputs — silence,
/// a hit, and a loud sustained passage.
fn synthetic_frame(frame_index: usize) -> FeatureFrame {
    match frame_index {
        // Near silence, with the onset impulse at full: only the flash shows.
        0 => FeatureFrame {
            rms: 0.02,
            rms_env: 0.02,
            onset: 1.0,
            flux: 0.9,
            centroid: 0.0,
            ..FeatureFrame::default()
        },
        // A kick decaying under a bright cymbal: every envelope in play.
        10 => FeatureFrame {
            rms: 0.55,
            rms_env: 0.61,
            bass: 0.90,
            bass_env: 0.72,
            low_mid: 0.40,
            low_mid_env: 0.35,
            mid: 0.25,
            mid_env: 0.30,
            high: 0.80,
            high_env: 0.65,
            air: 0.45,
            air_env: 0.50,
            flux: 0.30,
            onset: 0.25,
            centroid: 0.70,
        },
        // A dense, loud passage: every ring packed, nothing transient.
        _ => FeatureFrame {
            rms: 0.95,
            rms_env: 0.93,
            bass: 0.50,
            bass_env: 0.55,
            low_mid: 0.85,
            low_mid_env: 0.80,
            mid: 1.00,
            mid_env: 0.95,
            high: 0.35,
            high_env: 0.40,
            air: 0.20,
            air_env: 0.22,
            flux: 0.05,
            onset: 0.0,
            centroid: 0.35,
        },
    }
}

/// A preset's parameters at the defaults its schema declares.
///
/// The golden hashes are hashes of a *default* render, which is what makes
/// `param_reaches_declared_uniform_slot` able to assert that setting a parameter
/// back to its default reproduces them.
fn defaults(preset: &Preset) -> PackedParams {
    preset
        .schema()
        .expect("the shipped schema parses")
        .resolve(&toml::Table::new())
        .expect("the shipped defaults pack")
}

/// Render one preset frame on lavapipe and hash the RGBA bytes.
///
/// Opens its own device, so callers must not already hold [`one_device_at_a_time`].
fn render_hash(preset: &Preset, frame_index: usize, seed: u64) -> String {
    render_hash_with(preset, frame_index, seed, defaults(preset))
}

/// [`render_hash`], with the preset's parameters chosen by the caller.
fn render_hash_with(
    preset: &Preset,
    frame_index: usize,
    seed: u64,
    params: PackedParams,
) -> String {
    render_hash_on(preset, frame_index, seed, ember(), params)
}

/// [`render_hash_with`], with the palette chosen by the caller too.
fn render_hash_on(
    preset: &Preset,
    frame_index: usize,
    seed: u64,
    colors: LinearPalette,
    params: PackedParams,
) -> String {
    let _device = one_device_at_a_time();
    let gpu = Gpu::new(AdapterChoice::Software)
        .expect("golden frames need lavapipe: `sudo dnf install mesa-vulkan-drivers`");
    let target = Offscreen::new(&gpu, WIDTH, HEIGHT).expect("a 320x180 frame");
    let visualizer = Visualizer::new(&gpu, preset).expect("the shipped preset compiles");

    let globals = Globals::for_frame(
        frame_index,
        FPS,
        (WIDTH, HEIGHT),
        seed,
        synthetic_frame(frame_index),
        colors,
        params,
    );
    visualizer.draw(&gpu, &target, &globals);
    let pixels = target.read_rgba(&gpu).expect("the frame reads back");

    assert_eq!(pixels.len(), (WIDTH * HEIGHT * 4) as usize);
    hex(&pixels)
}

fn hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// `<key> <sha256>` per line, the way every `tests/golden/*.txt` stores it.
///
/// The key is a frame index for a preset file and a palette name for
/// `palettes.txt`; the format is the same so the regenerate ritual is too.
fn read_golden(path: &PathBuf) -> Vec<(String, String)> {
    let text = fs::read_to_string(path).unwrap_or_else(|err| {
        panic!(
            "{}: {err}. Regenerate with `AVZ_UPDATE_GOLDEN=1 cargo test -p avz-core \
             --test golden_frames`",
            path.display()
        )
    });

    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            let (key, hash) = line
                .split_once(char::is_whitespace)
                .unwrap_or_else(|| panic!("`{line}` is not `<key> <sha256>`"));
            (key.to_owned(), hash.trim().to_owned())
        })
        .collect()
}

fn write_golden(path: &PathBuf, header: &str, hashes: &[(String, String)]) {
    fs::create_dir_all(path.parent().expect("tests/golden")).expect("create tests/golden");

    let mut text = format!(
        "# {header}: sha256 of the RGBA bytes of a\n\
         # {WIDTH}x{HEIGHT} software-adapter render, seed {GOLDEN_SEED}, synthetic features.\n\
         # Regenerate: AVZ_UPDATE_GOLDEN=1 cargo test -p avz-core --test golden_frames\n",
    );
    for (key, hash) in hashes {
        text.push_str(&format!("{key} {hash}\n"));
    }
    fs::write(path, text).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
}

/// The recorded hashes of one preset, keyed by frame index.
fn recorded(preset: &Preset) -> Vec<(usize, String)> {
    read_golden(&golden_file(preset))
        .into_iter()
        .map(|(index, hash)| {
            let index = index
                .parse()
                .unwrap_or_else(|_| panic!("`{index}` is a frame index"));
            (index, hash)
        })
        .collect()
}

fn updating() -> bool {
    std::env::var_os("AVZ_UPDATE_GOLDEN").is_some()
}

/// The harness itself: every shipped preset draws exactly the frames it drew
/// when its hashes were committed. A WGSL edit, a uniform-layout drift, or a
/// changed default palette all land here.
#[test]
fn every_preset_renders_its_golden_frames() {
    for preset in PRESETS {
        let hashes: Vec<(usize, String)> = GOLDEN_FRAMES
            .iter()
            .map(|&index| (index, render_hash(preset, index, GOLDEN_SEED)))
            .collect();

        if updating() {
            let text: Vec<(String, String)> = hashes
                .iter()
                .map(|(index, hash)| (index.to_string(), hash.clone()))
                .collect();
            write_golden(
                &golden_file(preset),
                &format!("Golden frame hashes for the `{}` preset", preset.name),
                &text,
            );
            continue;
        }

        assert_eq!(
            recorded(preset),
            hashes,
            "preset `{}` no longer renders its golden frames. If the change was \
             intended, regenerate with `AVZ_UPDATE_GOLDEN=1 cargo test -p avz-core \
             --test golden_frames` and commit the new hashes with the shader.",
            preset.name,
        );
    }
}

/// The frame every built-in palette is pinned at.
///
/// Frame 10 drives every envelope, so the background (slot 0) and the whole
/// accent ramp (slots 1..4) are on screen at once. A palette whose only
/// difference from another lived in a slot this frame does not read would slip
/// through the distinctness check below.
const PALETTE_FRAME: usize = 10;

fn palette_golden_file() -> PathBuf {
    golden_dir().join("palettes.txt")
}

/// Every built-in renders `pulse` into a different, stable picture.
///
/// Two failures live here, and no other test would see either. A palette that
/// reaches no pixel — resolved, uploaded, and then ignored — makes `--palette`
/// decoration, and every hash below would be the same string. And a change to a
/// built-in's colors, or to the Oklab resample under them, silently rewrites
/// every video anyone ever rendered with that name.
#[test]
fn every_built_in_palette_renders_a_distinct_stable_frame() {
    let preset = Preset::by_name("pulse").expect("pulse ships");

    let hashes: Vec<(String, String)> = BUILT_INS
        .iter()
        .map(|built_in| {
            let hash = render_hash_on(
                preset,
                PALETTE_FRAME,
                GOLDEN_SEED,
                named(built_in.name),
                defaults(preset),
            );
            (built_in.name.to_owned(), hash)
        })
        .collect();

    // Checked before the regenerate branch: `AVZ_UPDATE_GOLDEN=1` must never be
    // able to bless two names that render one picture.
    let distinct: BTreeSet<&str> = hashes.iter().map(|(_, hash)| hash.as_str()).collect();
    assert_eq!(
        distinct.len(),
        BUILT_INS.len(),
        "two built-in palettes render the same frame: {hashes:?}",
    );

    if updating() {
        write_golden(
            &palette_golden_file(),
            &format!("Golden hashes of `pulse` frame {PALETTE_FRAME} under every built-in palette"),
            &hashes,
        );
        return;
    }

    assert_eq!(
        read_golden(&palette_golden_file()),
        hashes,
        "a built-in palette no longer renders the frame its hash was committed \
         from. If the change was intended, regenerate with `AVZ_UPDATE_GOLDEN=1 \
         cargo test -p avz-core --test golden_frames` and say in the commit \
         message which palette moved.",
    );

    // `ember` is the default, so the preset's own golden frame must be the frame
    // `ember` renders. Without this the two files could drift apart and each
    // stay internally consistent.
    let ember = hashes
        .iter()
        .find(|(name, _)| name == "ember")
        .map(|(_, hash)| hash.clone())
        .expect("`ember` ships");
    let from_preset_file = recorded(preset)
        .into_iter()
        .find(|(index, _)| *index == PALETTE_FRAME)
        .map(|(_, hash)| hash)
        .expect("frame 10 is a golden frame");
    assert_eq!(
        ember, from_preset_file,
        "the preset golden frames were rendered with a palette that is not the default",
    );
}

/// An inline palette reaches the pixels, and not by accident of length: two
/// colors are resampled onto five slots, and the frame that comes out is not the
/// frame any built-in renders.
#[test]
fn an_inline_palette_reaches_the_pixels() {
    let preset = Preset::by_name("pulse").expect("pulse ships");
    let inline = palette::resolve(&Palette::Inline(vec![
        "#04070f".parse().expect("a color"),
        "#f2e9d8".parse().expect("a color"),
    ]))
    .expect("two colors resolve");

    let drawn = render_hash_on(preset, PALETTE_FRAME, GOLDEN_SEED, inline, defaults(preset));
    let ember = render_hash_on(
        preset,
        PALETTE_FRAME,
        GOLDEN_SEED,
        named("ember"),
        defaults(preset),
    );

    assert_ne!(
        drawn, ember,
        "an inline palette resolves but never reaches a pixel"
    );
}

/// A value away from the schema's default, whatever the parameter's type.
///
/// Derived rather than written down, so a preset author who adds a parameter
/// gets it covered by `param_reaches_declared_uniform_slot` without touching
/// this file — which is the whole of RFC-001 G3.
fn off_default(kind: &ParamKind) -> toml::Value {
    match kind {
        ParamKind::Float { default, min, max } => {
            let other = if (*default - *max).abs() > f32::EPSILON {
                *max
            } else {
                *min
            };
            toml::Value::Float(f64::from(other))
        }
        ParamKind::Int { default, min, max } => {
            let other = if default != max { *max } else { *min };
            toml::Value::Integer(other)
        }
        ParamKind::Bool { default } => toml::Value::Boolean(!default),
        ParamKind::Enum { default, variants } => {
            let other = variants
                .iter()
                .find(|variant| *variant != default)
                .unwrap_or_else(|| panic!("an enum with one variant tunes nothing"));
            toml::Value::String(other.clone())
        }
        ParamKind::Color { default } => {
            let inverted = format!("#{:02x}{:02x}{:02x}", !default.r, !default.g, !default.b);
            toml::Value::String(inverted)
        }
    }
}

/// Every schema parameter reaches the uniform slot it declares, and the schema's
/// own defaults are what the committed golden hashes were rendered from.
///
/// Two failures this catches, neither of which any other test would notice:
/// a parameter packed into a slot the shader does not read (the knob does
/// nothing), and a schema default drifting away from the constant the shader
/// used before it had parameters (every golden hash silently rewritten).
#[test]
fn param_reaches_declared_uniform_slot() {
    // Frame 10 drives every envelope, the flux, and the onset, so every `pulse`
    // parameter has something to act on.
    const FRAME: usize = 10;

    for preset in PRESETS {
        let schema = preset.schema().expect("the shipped schema parses");

        let baseline = render_hash_with(preset, FRAME, GOLDEN_SEED, defaults(preset));
        let recorded = recorded(preset)
            .into_iter()
            .find(|(index, _)| *index == FRAME)
            .map(|(_, hash)| hash)
            .expect("frame 10 is a golden frame");
        assert_eq!(
            baseline, recorded,
            "preset `{}` renders its golden frames only at its schema defaults",
            preset.name,
        );

        for param in &schema.params {
            let mut overrides = toml::Table::new();
            overrides.insert(param.name.clone(), off_default(&param.kind));
            let packed = schema
                .resolve(&overrides)
                .unwrap_or_else(|err| panic!("`{}` off its default: {err}", param.name));

            assert_ne!(
                render_hash_with(preset, FRAME, GOLDEN_SEED, packed),
                baseline,
                "`{}.{}` packs into params[{}].{} but no pixel depends on it",
                preset.name,
                param.name,
                param.slot.index,
                param.slot.component,
            );
        }
    }
}

/// Determinism, on one machine and one adapter: the same uniform renders the
/// same pixels. A shader that read a wall clock or an unseeded RNG fails here
/// long before anyone compares two machines.
#[test]
fn same_inputs_same_hash_twice() {
    let preset = Preset::by_name("pulse").expect("pulse ships");

    let once = render_hash(preset, 10, GOLDEN_SEED);
    let twice = render_hash(preset, 10, GOLDEN_SEED);

    assert_eq!(
        once, twice,
        "the same frame rendered two different pictures"
    );
}

/// The seed reaches the shader. Without this, `--seed` could be plumbed through
/// every layer and quietly dropped by the last one.
#[test]
fn different_seed_different_hash() {
    let preset = Preset::by_name("pulse").expect("pulse ships");

    // Frame 10 has `high_env` up, so the seeded sparkle grid is on screen.
    let one = render_hash(preset, 10, 1);
    let other = render_hash(preset, 10, 2);

    assert_ne!(
        one, other,
        "the seed does not reach the noise in the shader"
    );
}

/// Two frames of the same song look different. A preset wired to a uniform it
/// never reads would pass every hash test above by rendering one still image.
#[test]
fn a_loud_frame_and_a_quiet_one_are_different_pictures() {
    let preset = Preset::by_name("pulse").expect("pulse ships");

    let quiet = render_hash(preset, 0, GOLDEN_SEED);
    let loud = render_hash(preset, 100, GOLDEN_SEED);

    assert_ne!(
        quiet, loud,
        "pulse renders the same frame however loud it is"
    );
}

/// Every feature `pulse` claims to be driven by actually moves its pixels.
///
/// The M2 acceptance criterion — "pulse visibly distinguishes kick (bass),
/// vocals (mid), cymbals (high)" — as an assertion. Changing one field of the
/// uniform and nothing else must change the frame. A field misplaced in the
/// layout, or dropped from the shader, reads as a still picture here.
#[test]
fn every_feature_pulse_reacts_to_changes_the_frame() {
    let preset = Preset::by_name("pulse").expect("pulse ships");

    // A mid-song frame: `time` is non-zero, so features that only move the
    // animation (the ring drift `low_mid_env` sets) have somewhere to move.
    let baseline = FeatureFrame {
        rms_env: 0.5,
        centroid: 0.5,
        ..FeatureFrame::default()
    };

    /// One feature of the uniform, and the way to turn it up.
    type Driver = (&'static str, fn(&mut FeatureFrame));

    let driven: [Driver; 8] = [
        ("rms_env", |f| f.rms_env = 1.0),
        ("bass_env", |f| f.bass_env = 1.0),
        ("low_mid_env", |f| f.low_mid_env = 1.0),
        ("mid_env", |f| f.mid_env = 1.0),
        ("high_env", |f| f.high_env = 1.0),
        ("air_env", |f| f.air_env = 1.0),
        ("flux", |f| f.flux = 1.0),
        ("onset", |f| f.onset = 1.0),
    ];

    let params = defaults(preset);
    let _device = one_device_at_a_time();
    let gpu = Gpu::new(AdapterChoice::Software).expect("golden frames need lavapipe");
    let target = Offscreen::new(&gpu, WIDTH, HEIGHT).expect("a 320x180 frame");
    let visualizer = Visualizer::new(&gpu, preset).expect("pulse compiles");

    let draw = |features: FeatureFrame| {
        let globals = Globals::for_frame(
            10,
            FPS,
            (WIDTH, HEIGHT),
            GOLDEN_SEED,
            features,
            ember(),
            params,
        );
        visualizer.draw(&gpu, &target, &globals);
        hex(&target.read_rgba(&gpu).expect("the frame reads back"))
    };

    let still = draw(baseline);
    for (name, drive) in driven {
        let mut features = baseline;
        drive(&mut features);
        assert_ne!(
            draw(features),
            still,
            "`{name}` reaches the uniform but never reaches a pixel"
        );
    }

    // The centroid walks the palette, so it must move the hue rather than the
    // geometry. Compare against a *different* centroid, not against zero.
    let warm = draw(FeatureFrame {
        centroid: 1.0,
        ..baseline
    });
    assert_ne!(warm, still, "`centroid` never reaches a pixel");
}

/// The M2 tuning instrument: one PNG per driving feature, in `target/pulse-tuning/`.
///
/// `VISION.md` §9 budgets a manual pass in which `pulse` is looked at, not
/// asserted on — "feels musical" has no assertion, and neither does "the kick
/// reads as a kick". This renders the isolated frames that pass needs, so the
/// ritual is a command rather than a scratch file someone rewrites every time:
///
/// ```bash
/// cargo test -p avz-core --test golden_frames -- --ignored dump_pulse
/// ```
///
/// `#[ignore]`d because it writes files and asserts nothing.
#[test]
#[ignore = "writes PNGs for the manual tuning pass; asserts nothing"]
fn dump_pulse_tuning_frames() {
    let preset = Preset::by_name("pulse").expect("pulse ships");
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/pulse-tuning");
    fs::create_dir_all(&out).expect("create target/pulse-tuning");

    let loud = FeatureFrame {
        rms_env: 0.8,
        centroid: 0.5,
        ..FeatureFrame::default()
    };
    let cases: [(&str, FeatureFrame); 5] = [
        ("00-quiet", FeatureFrame::default()),
        (
            "01-kick-bass",
            FeatureFrame {
                bass_env: 1.0,
                ..loud
            },
        ),
        (
            "02-vocals-mid",
            FeatureFrame {
                mid_env: 1.0,
                ..loud
            },
        ),
        (
            "03-cymbals-high",
            FeatureFrame {
                high_env: 1.0,
                ..loud
            },
        ),
        (
            "04-onset-flash",
            FeatureFrame {
                onset: 1.0,
                bass_env: 1.0,
                ..loud
            },
        ),
    ];

    let _device = one_device_at_a_time();
    let gpu = Gpu::new(AdapterChoice::Software).expect("the tuning pass needs lavapipe");
    let target = Offscreen::new(&gpu, WIDTH, HEIGHT).expect("a 320x180 frame");
    let visualizer = Visualizer::new(&gpu, preset).expect("pulse compiles");

    let params = defaults(preset);
    for (name, features) in cases {
        let globals = Globals::for_frame(
            10,
            FPS,
            (WIDTH, HEIGHT),
            GOLDEN_SEED,
            features,
            ember(),
            params,
        );
        visualizer.draw(&gpu, &target, &globals);
        let pixels = target.read_rgba(&gpu).expect("the frame reads back");

        let path = out.join(format!("{name}.png"));
        image::RgbaImage::from_raw(WIDTH, HEIGHT, pixels)
            .expect("the frame is WIDTH x HEIGHT RGBA")
            .save(&path)
            .unwrap_or_else(|err| panic!("{}: {err}", path.display()));
    }
}
