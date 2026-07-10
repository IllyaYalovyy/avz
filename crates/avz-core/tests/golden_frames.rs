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
//! That rewrites `tests/golden/<preset>.txt`; commit the new hashes with the
//! shader change and say in the commit message what moved. Never regenerate to
//! make a red test green without looking at why it went red.
//!
//! **The hashes are of composited frames.** Since RFC-001 Step 18 a preset draws
//! premultiplied light into its own layer and the compositor stacks it over the
//! palette backdrop (`VISION.md` §5.3), so what reaches the encoder — and what is
//! hashed here — is the whole layer stack, exactly as `pipeline::render` builds
//! it. A preset that went transparent would show up as the backdrop alone.
//!
//! **Feedback presets are warmed up.** A preset that declares `needs_feedback` is
//! drawn from frame 0 up to the golden frame, because its picture is a function
//! of every frame before it. See [`render_hash_on`].
//!
//! Needs Mesa's software Vulkan driver: `sudo dnf install mesa-vulkan-drivers`.

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};

use avz_core::analysis::{EMPTY_HISTORY, FeatureFrame, ONSET_SLOTS, SPECTRUM_BINS};
use avz_core::config::{self, Palette};
use avz_core::render::{
    AdapterChoice, BUILT_INS, Backdrop, Card, CardText, Compositor, Globals, Gpu, Layer,
    LinearPalette, Offscreen, PRESETS, PackedParams, ParamKind, Preset, TextCard, Visualizer,
    palette,
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
///
/// Every other index is that loud sustained passage, which is what a feedback
/// preset's warm-up frames are drawn from: an onset at frame 0, then a hundred
/// frames of dense material for its trail to accumulate out of.
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

/// The coarse spectrum of golden frame `frame_index`, built by hand.
///
/// Written down rather than analyzed, for the reason [`synthetic_frame`] is: the
/// point of a golden frame is that its input lives in this file, so a change in
/// the DSP cannot silently rewrite the picture a preset is held to.
///
/// The shape is a bass hump under a harmonic comb and a formant that walks with
/// the frame — spectral structure at three scales, so a preset that averages
/// buckets together, reads them at the wrong offset, or ignores them entirely
/// renders a different frame. Its overall level follows the frame's `rms_env`,
/// so the silent golden frame has a silent spectrum too.
fn synthetic_spectrum(frame_index: usize) -> Vec<f32> {
    let level = synthetic_frame(frame_index).rms_env;
    let walk = (frame_index % 20) as f32 / 20.0;

    (0..SPECTRUM_BINS)
        .map(|bucket| {
            let x = bucket as f32 / (SPECTRUM_BINS - 1) as f32;
            let hump = (-(x / 0.18).powi(2)).exp();
            let formant = (-(((x - (0.35 + 0.25 * walk)) / 0.06).powi(2))).exp();
            let comb = 0.5 + 0.5 * (std::f32::consts::TAU * x * 40.0).cos();
            let tilt = 1.0 - 0.8 * x;

            (level * (0.7 * hump + 0.5 * formant) * tilt * (0.55 + 0.45 * comb)).clamp(0.0, 1.0)
        })
        .collect()
}

/// A spectrum with no energy in it, for the frames drawn from silent features.
fn silent_spectrum() -> Vec<f32> {
    vec![0.0; SPECTRUM_BINS]
}

/// How often the golden song is struck: every twelfth frame, from the first.
///
/// 0.4 s apart at 30 fps — a brisk backbeat, and slower than the 1.6 s a
/// `particles` burst lives, so a golden frame well into the song has several
/// bursts on it at different ages rather than one.
const GOLDEN_ONSET_EVERY: usize = 12;

/// The recent hits of golden frame `frame_index`, built by hand.
///
/// Written down rather than detected, for the reason [`synthetic_frame`] is: a
/// change in the onset detector must not silently rewrite the picture a burst
/// preset is held to. The rule is closed-form in `frame_index`, so a preset drawn
/// at frame 100 alone sees exactly the window it would have seen had frames
/// 0..99 been drawn first.
///
/// Frame 0 is a hit, which is what makes `synthetic_frame(0)`'s full onset
/// impulse and this agree with one another.
fn synthetic_onsets(frame_index: usize) -> Vec<f32> {
    let hits: Vec<usize> = (0..=frame_index).step_by(GOLDEN_ONSET_EVERY).collect();

    let mut history = EMPTY_HISTORY;
    let ordinals = hits.len().saturating_sub(ONSET_SLOTS)..hits.len();
    for (slot, ordinal) in history.chunks_exact_mut(2).zip(ordinals.rev()) {
        // The same `frame_index / fps` divide `Globals::for_frame` makes, so a
        // shader subtracting a birth from `time` subtracts two exact siblings.
        slot[0] = (hits[ordinal] as f64 / f64::from(FPS)) as f32;
        slot[1] = ordinal as f32;
    }
    history.to_vec()
}

/// A history with no hits in it, for the frames drawn from silent features.
fn silent_onsets() -> Vec<f32> {
    EMPTY_HISTORY.to_vec()
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

/// Whether `preset` samples the previous frame, and so must be warmed up.
fn needs_feedback(preset: &Preset) -> bool {
    preset
        .schema()
        .expect("the shipped schema parses")
        .needs_feedback
}

/// Whether `preset` samples the frame's coarse spectrum.
fn needs_spectrum(preset: &Preset) -> bool {
    preset
        .schema()
        .expect("the shipped schema parses")
        .needs_spectrum
}

/// The layer stack of one render: a backdrop, a visualizer layer over it, and the
/// compositor that flattens them into the frame that is read back.
///
/// The same stack `pipeline::render` builds, at test resolution. `backdrop` is
/// `None` for the tests that want the preset's own premultiplied light with
/// nothing under it.
struct Stage {
    target: Offscreen,
    visual: Layer,
    visualizer: Visualizer,
    compositor: Compositor,
}

impl Stage {
    fn new(gpu: &Gpu, preset: &Preset, colors: LinearPalette, backdrop: Option<Backdrop>) -> Self {
        let target = Offscreen::new(gpu, WIDTH, HEIGHT).expect("a 320x180 frame");
        let visual = Layer::new(gpu, WIDTH, HEIGHT, "visualizer");
        let visualizer = Visualizer::new(gpu, preset, &visual).expect("the preset compiles");

        let background = backdrop.map(|style| style.layer(gpu, WIDTH, HEIGHT, colors));
        let layers: Vec<&Layer> = background.iter().chain([&visual]).collect();
        let compositor = Compositor::new(gpu, &layers).expect("frame-sized layers");

        Self {
            target,
            visual,
            visualizer,
            compositor,
        }
    }

    fn draw(&self, gpu: &Gpu, globals: &Globals, spectrum: &[f32], onsets: &[f32]) {
        self.visualizer
            .draw(gpu, &self.visual, globals, spectrum, onsets);
    }

    /// Composite the stack and read the frame back, tightly packed.
    fn read(&self, gpu: &Gpu) -> Vec<u8> {
        self.compositor.composite(gpu, &self.target);
        self.target.read_rgba(gpu).expect("the frame reads back")
    }
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
///
/// The preset is composited over the default backdrop, which is the stack a
/// zero-config `avz render` builds.
///
/// A preset that samples the previous frame is drawn from frame 0 up to
/// `frame_index`, exactly as `pipeline::render` would draw it, and only the last
/// frame is composited and hashed. Hashing a feedback preset's frame 100 in
/// isolation would pin a picture with no trails in it — which is to say, none of
/// what the preset is for. A preset without feedback draws the one frame asked
/// for.
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
    let stage = Stage::new(&gpu, preset, colors, Some(Backdrop::default()));

    let first = if needs_feedback(preset) {
        0
    } else {
        frame_index
    };
    for index in first..=frame_index {
        let globals = Globals::for_frame(
            index,
            FPS,
            (WIDTH, HEIGHT),
            seed,
            synthetic_frame(index),
            colors,
            params,
        );
        stage.draw(
            &gpu,
            &globals,
            &synthetic_spectrum(index),
            &synthetic_onsets(index),
        );
    }
    let pixels = stage.read(&gpu);

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
        "# {header}.\n\
         # sha256 of the RGBA bytes of a {WIDTH}x{HEIGHT} software-adapter render.\n\
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
                &format!(
                    "Golden frame hashes for the `{}` preset, seed {GOLDEN_SEED}, \
                     synthetic features",
                    preset.name,
                ),
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
            &format!(
                "Golden hashes of `pulse` frame {PALETTE_FRAME}, seed {GOLDEN_SEED}, under \
                 every built-in palette"
            ),
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
    let stage = Stage::new(&gpu, preset, ember(), Some(Backdrop::default()));

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
        stage.draw(
            &gpu,
            &globals,
            &synthetic_spectrum(10),
            &synthetic_onsets(10),
        );
        hex(&stage.read(&gpu))
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

/// Every preset draws a layer the backdrop can be seen through.
///
/// The premultiplied contract (`VISION.md` §5.3) is one line at the bottom of a
/// shader, and it is the easiest line for the next preset author to get wrong:
/// `return vec4<f32>(color, 1.0)` compiles, renders, hashes, and looks fine on
/// its own — while covering the background layer with an opaque rectangle in
/// every render anyone makes with it. Nothing else in this file would notice,
/// because a golden hash blesses whatever it is shown.
///
/// So: on a silent frame, with no backdrop under it, a preset's own layer must
/// have somewhere it did not fully cover. An opaque shader has no such pixel.
#[test]
fn every_preset_draws_a_layer_the_backdrop_shows_through() {
    for preset in PRESETS {
        let _device = one_device_at_a_time();
        let gpu = Gpu::new(AdapterChoice::Software).expect("golden frames need lavapipe");
        let stage = Stage::new(&gpu, preset, ember(), None);

        // Silence: nothing to draw, so nothing to hide the backdrop with.
        let globals = Globals::for_frame(
            10,
            FPS,
            (WIDTH, HEIGHT),
            GOLDEN_SEED,
            FeatureFrame::default(),
            ember(),
            defaults(preset),
        );
        stage.draw(&gpu, &globals, &silent_spectrum(), &silent_onsets());
        let pixels = stage.read(&gpu);

        let opaque = pixels.chunks_exact(4).filter(|px| px[3] == 255).count();
        assert_eq!(
            opaque,
            0,
            "preset `{}` covers the frame on a silent frame: {opaque} of {} pixels \
             are opaque. Its fragment shader returns a hardcoded alpha instead of \
             the coverage of the light it drew, and no background layer will ever \
             be visible under it.",
            preset.name,
            (WIDTH * HEIGHT) as usize,
        );
    }
}

/// `nebula` reads the previous frame, and reads it into the picture.
///
/// The plumbing is proven in `feedback_texture.rs` against a shader that does
/// nothing else. This proves the shipped preset is wired to it: frame 30 of a
/// render that began at frame 0 carries thirty frames of trail, and frame 30 of
/// a render that began at frame 30 carries none. Nothing else in this file would
/// notice a `nebula` that dropped `textureSample` — its golden hashes would
/// simply be blessed without trails.
#[test]
fn nebula_frames_depend_on_the_frames_before_them() {
    const FRAME: usize = 30;

    let nebula = Preset::by_name("nebula").expect("nebula ships");
    assert!(needs_feedback(nebula), "nebula asks for the previous frame");

    let warm = render_hash(nebula, FRAME, GOLDEN_SEED);

    // The same frame, rendered cold: one draw, black feedback beneath it.
    let cold = {
        let _device = one_device_at_a_time();
        let gpu = Gpu::new(AdapterChoice::Software).expect("golden frames need lavapipe");
        let stage = Stage::new(&gpu, nebula, ember(), Some(Backdrop::default()));
        let globals = Globals::for_frame(
            FRAME,
            FPS,
            (WIDTH, HEIGHT),
            GOLDEN_SEED,
            synthetic_frame(FRAME),
            ember(),
            defaults(nebula),
        );
        stage.draw(
            &gpu,
            &globals,
            &synthetic_spectrum(FRAME),
            &synthetic_onsets(FRAME),
        );
        hex(&stage.read(&gpu))
    };

    assert_ne!(
        warm, cold,
        "nebula renders frame {FRAME} the same warm or cold: the trail reaches no pixel",
    );
}

/// `--sample 3s --adapter software` must produce "stable, non-black, evolving
/// output" (RFC-001 Step 17). Stable and evolving are the hashes above; that it
/// is *lit* is this, and a screen-blended trail saturating to white would fail it
/// as surely as a shader that drew nothing.
///
/// The visualizer layer is composited with **no backdrop under it**: with the
/// palette gradient beneath, every pixel of every frame would be lit and this
/// test would pass on a `nebula` that had stopped drawing entirely.
#[test]
fn nebula_renders_a_lit_frame_that_is_neither_black_nor_blown_out() {
    let nebula = Preset::by_name("nebula").expect("nebula ships");

    let _device = one_device_at_a_time();
    let gpu = Gpu::new(AdapterChoice::Software).expect("golden frames need lavapipe");
    let stage = Stage::new(&gpu, nebula, ember(), None);

    // Ninety frames is the three seconds `--sample 3s` renders at 30 fps: long
    // enough for the trail to reach whatever level it settles at.
    for index in 0..90 {
        let globals = Globals::for_frame(
            index,
            FPS,
            (WIDTH, HEIGHT),
            GOLDEN_SEED,
            synthetic_frame(index),
            ember(),
            defaults(nebula),
        );
        stage.draw(
            &gpu,
            &globals,
            &synthetic_spectrum(index),
            &synthetic_onsets(index),
        );
    }
    let pixels = stage.read(&gpu);

    let lit = pixels
        .chunks_exact(4)
        .filter(|px| px[..3] != [0, 0, 0])
        .count();
    let white = pixels
        .chunks_exact(4)
        .filter(|px| px[..3] == [255, 255, 255])
        .count();
    let total = (WIDTH * HEIGHT) as usize;

    assert!(
        lit * 2 > total,
        "after 90 frames only {lit} of {total} pixels are lit: nebula renders black",
    );
    assert!(
        white * 4 < total,
        "after 90 frames {white} of {total} pixels are pure white: the trail blew out",
    );
}

/// A spectrum with `buckets` at full scale and everything else silent.
fn hot(buckets: std::ops::Range<usize>) -> Vec<f32> {
    let mut spectrum = silent_spectrum();
    spectrum[buckets].fill(1.0);
    spectrum
}

/// One frame of `ribbons` over no backdrop, drawn from a loud frame's features
/// and the spectrum given. What comes back is the preset's own light.
fn ribbon_light(spectrum: &[f32]) -> Vec<u8> {
    const FRAME: usize = 100;

    let ribbons = Preset::by_name("ribbons").expect("ribbons ships");

    let _device = one_device_at_a_time();
    let gpu = Gpu::new(AdapterChoice::Software).expect("golden frames need lavapipe");
    let stage = Stage::new(&gpu, ribbons, ember(), None);
    let globals = Globals::for_frame(
        FRAME,
        FPS,
        (WIDTH, HEIGHT),
        GOLDEN_SEED,
        synthetic_frame(FRAME),
        ember(),
        defaults(ribbons),
    );
    stage.draw(&gpu, &globals, spectrum, &synthetic_onsets(FRAME));

    stage.read(&gpu)
}

/// How many pixels of the left and right thirds of `frame` carry any light.
fn lit_thirds(frame: &[u8]) -> (usize, usize) {
    let third = WIDTH as usize / 3;
    let mut left = 0;
    let mut right = 0;

    for (index, pixel) in frame.chunks_exact(4).enumerate() {
        if pixel[..3] == [0, 0, 0] {
            continue;
        }
        match index % WIDTH as usize {
            column if column < third => left += 1,
            column if column >= WIDTH as usize - third => right += 1,
            _ => {}
        }
    }

    (left, right)
}

/// `ribbons` reads the spectrum texture, and reads it *positionally*: the width
/// of the frame is the frequency axis.
///
/// The plumbing is proven in `spectrum_texture.rs` against a shader that does
/// nothing else. This proves the shipped preset is wired to it, and wired the
/// right way round. Nothing else in this file would notice a `ribbons` that
/// dropped its `textureLoad` and drew flat lines — its golden hashes would
/// simply be blessed without the spectrum in them.
#[test]
fn ribbons_draws_its_light_where_the_spectrum_has_energy() {
    let ribbons = Preset::by_name("ribbons").expect("ribbons ships");
    assert!(needs_spectrum(ribbons), "ribbons asks for the spectrum");

    let quarter = SPECTRUM_BINS / 4;
    let (bass_left, bass_right) = lit_thirds(&ribbon_light(&hot(0..quarter)));
    let (air_left, air_right) = lit_thirds(&ribbon_light(&hot(3 * quarter..SPECTRUM_BINS)));

    assert!(
        bass_left > 10 * bass_right.max(1),
        "energy in the lowest buckets lit {bass_left} pixels on the left and \
         {bass_right} on the right: the frequency axis is not the frame's width",
    );
    assert!(
        air_right > 10 * air_left.max(1),
        "energy in the highest buckets lit {air_left} pixels on the left and \
         {air_right} on the right: the frequency axis runs backwards",
    );
}

/// A silent spectrum under a loud frame draws nothing. The ribbon *is* the
/// spectrum: a preset that drew its lanes whatever the texture said would have
/// passed every hash above, and would ignore the music.
#[test]
fn ribbons_draws_nothing_where_the_spectrum_is_silent() {
    let frame = ribbon_light(&silent_spectrum());

    assert!(
        frame.chunks_exact(4).all(|pixel| pixel[..3] == [0, 0, 0]),
        "ribbons drew light from a spectrum with no energy in it",
    );
}

/// `--sample 3s --adapter software` must produce stable, non-black, evolving
/// output, as for `nebula`: that the frame is *lit* without blowing out.
#[test]
fn ribbons_renders_a_lit_frame_that_is_neither_black_nor_blown_out() {
    let frame = ribbon_light(&synthetic_spectrum(100));

    let lit = frame
        .chunks_exact(4)
        .filter(|pixel| pixel[..3] != [0, 0, 0])
        .count();
    let white = frame
        .chunks_exact(4)
        .filter(|pixel| pixel[..3] == [255, 255, 255])
        .count();
    let total = (WIDTH * HEIGHT) as usize;

    assert!(
        lit * 10 > total,
        "only {lit} of {total} pixels are lit: ribbons renders black",
    );
    assert!(
        white * 4 < total,
        "{white} of {total} pixels are pure white: the ribbons blew out",
    );
}

/// Whether `preset` re-simulates from the song's recent hits.
fn needs_onsets(preset: &Preset) -> bool {
    preset
        .schema()
        .expect("the shipped schema parses")
        .needs_onsets
}

/// One frame of `particles` over no backdrop, drawn at `frame_index` from the
/// history given. What comes back is the preset's own light.
///
/// Loud, sustained features, so the picture is the bursts and not the loudness
/// envelope: `synthetic_frame` reserves those for frames past 10.
fn particle_light(frame_index: usize, onsets: &[f32]) -> Vec<u8> {
    let particles = Preset::by_name("particles").expect("particles ships");

    let _device = one_device_at_a_time();
    let gpu = Gpu::new(AdapterChoice::Software).expect("golden frames need lavapipe");
    let stage = Stage::new(&gpu, particles, ember(), None);
    let globals = Globals::for_frame(
        frame_index,
        FPS,
        (WIDTH, HEIGHT),
        GOLDEN_SEED,
        synthetic_frame(100),
        ember(),
        defaults(particles),
    );
    stage.draw(&gpu, &globals, &silent_spectrum(), onsets);

    stage.read(&gpu)
}

/// [`particle_light`], with some of `particles`' parameters moved off their
/// defaults.
fn particle_light_with(frame_index: usize, onsets: &[f32], overrides: &[(&str, f64)]) -> Vec<u8> {
    let particles = Preset::by_name("particles").expect("particles ships");

    let mut table = toml::Table::new();
    for (name, value) in overrides {
        table.insert((*name).to_owned(), toml::Value::Float(*value));
    }
    let params = particles
        .schema()
        .expect("the shipped schema parses")
        .resolve(&table)
        .expect("the overrides are in range");

    let _device = one_device_at_a_time();
    let gpu = Gpu::new(AdapterChoice::Software).expect("golden frames need lavapipe");
    let stage = Stage::new(&gpu, particles, ember(), None);
    let globals = Globals::for_frame(
        frame_index,
        FPS,
        (WIDTH, HEIGHT),
        GOLDEN_SEED,
        synthetic_frame(100),
        ember(),
        params,
    );
    stage.draw(&gpu, &globals, &silent_spectrum(), onsets);

    stage.read(&gpu)
}

/// A history holding one hit, born `age` seconds before frame `frame_index`.
fn one_hit(frame_index: usize, age: f32) -> Vec<f32> {
    let time = (frame_index as f64 / f64::from(FPS)) as f32;

    let mut history = EMPTY_HISTORY;
    history[0] = time - age;
    history[1] = 0.0;
    history.to_vec()
}

/// How far the lit pixels of `frame` reach from its center, and how many of them
/// there are. Distances are in the shader's own units: the short edge is 1.0.
fn lit_spread(frame: &[u8]) -> (f32, usize) {
    let (width, height) = (WIDTH as f32, HEIGHT as f32);
    let short = width.min(height);

    let mut farthest: f32 = 0.0;
    let mut lit = 0;

    for (index, pixel) in frame.chunks_exact(4).enumerate() {
        if pixel[..3] == [0, 0, 0] {
            continue;
        }
        lit += 1;
        let x = (index % WIDTH as usize) as f32 - 0.5 * width;
        let y = (index / WIDTH as usize) as f32 - 0.5 * height;
        farthest = farthest.max((x * x + y * y).sqrt() / short);
    }

    (farthest, lit)
}

/// `particles` reads the onset history, and reads it as *time*: a burst thrown
/// by a hit expands away from where it was thrown as the hit recedes.
///
/// The plumbing is proven in `onset_history_texture.rs` against a shader that
/// does nothing else. This proves the shipped preset is wired to it, and wired
/// the right way round. Nothing else in this file would notice a `particles`
/// that dropped its `textureLoad` and drew a still cloud — its golden hashes
/// would simply be blessed without the music in them.
#[test]
fn particles_throws_a_burst_that_expands_as_the_hit_that_threw_it_recedes() {
    let particles = Preset::by_name("particles").expect("particles ships");
    assert!(needs_onsets(particles), "particles asks for the hits");

    const FRAME: usize = 60;
    let (fresh, _) = lit_spread(&particle_light(FRAME, &one_hit(FRAME, 0.0)));
    let (young, _) = lit_spread(&particle_light(FRAME, &one_hit(FRAME, 0.25)));
    let (older, _) = lit_spread(&particle_light(FRAME, &one_hit(FRAME, 0.9)));

    assert!(
        young > fresh,
        "a burst 0.25 s old reaches {young:.3} from the center and a brand new one \
         {fresh:.3}: the particles are not moving",
    );
    assert!(
        older > young,
        "a burst 0.9 s old reaches {older:.3} and a 0.25 s one {young:.3}: the \
         burst stops expanding",
    );
}

/// A burst older than `lifetime` has gone out, and a frame no hit has reached
/// yet was never lit. Both are the same assertion about the sentinel: a shader
/// that read an unfilled slot as a hit at time zero would open every render with
/// an explosion the song never played.
#[test]
fn particles_draws_nothing_without_a_live_hit() {
    const FRAME: usize = 60;

    let lifetime = match defaults(Preset::by_name("particles").expect("particles ships"))[0][1] {
        seconds if seconds > 0.0 => seconds,
        other => panic!("`lifetime` defaults to {other}"),
    };

    let empty = particle_light(FRAME, &silent_onsets());
    assert!(
        empty.chunks_exact(4).all(|pixel| pixel[..3] == [0, 0, 0]),
        "particles drew light on a frame no hit has reached yet",
    );

    let burnt = particle_light(FRAME, &one_hit(FRAME, lifetime + 0.1));
    assert!(
        burnt.chunks_exact(4).all(|pixel| pixel[..3] == [0, 0, 0]),
        "a burst older than its {lifetime} s lifetime is still burning",
    );

    // And the live burst the two are being compared against does draw, so the
    // assertions above are not passing on a preset that draws nothing at all.
    let (_, lit) = lit_spread(&particle_light(FRAME, &one_hit(FRAME, 0.3)));
    assert!(lit > 0, "particles never draws anything");
}

/// The brightest pixel of `frame` in each of `buckets` rings out to `limit`,
/// measured from the center in the shader's units: the short edge is 1.0.
fn brightest_by_radius(frame: &[u8], buckets: usize, limit: f32) -> Vec<u8> {
    let (width, height) = (WIDTH as f32, HEIGHT as f32);
    let short = width.min(height);

    let mut profile = vec![0u8; buckets];
    for (index, pixel) in frame.chunks_exact(4).enumerate() {
        let x = (index % WIDTH as usize) as f32 - 0.5 * width;
        let y = (index / WIDTH as usize) as f32 - 0.5 * height;
        let radius = (x * x + y * y).sqrt() / short;
        if radius >= limit {
            continue;
        }

        let ring = (radius / limit * buckets as f32) as usize;
        let light = pixel[..3].iter().copied().max().unwrap_or(0);
        profile[ring.min(buckets - 1)] = profile[ring.min(buckets - 1)].max(light);
    }

    profile
}

/// The burst cull is an optimization, and an optimization may not change the
/// picture.
///
/// `particles` skips a whole burst when the pixel it is shading lies outside the
/// shell between the burst's slowest and fastest particle, widened by how far a
/// particle's halo reaches. Widened by too little, the cull shaves the halo off
/// both faces of the shell — a burst with a hard edge and a hollow middle, and
/// every other test here still passes: it expands, it fades, it twinkles, and
/// its golden hashes are blessed with the clipping baked in.
///
/// The halo falls to zero smoothly, so the picture must too. A ring of pixels
/// that is dark while the ring beside it is bright is a cut, not a falloff.
/// `spread` and `gravity` are turned off so the burst sits on the frame's center
/// and its rings are the shader's own falloff and nothing else.
#[test]
fn the_burst_cull_takes_no_light_off_the_burst_it_skips() {
    const FRAME: usize = 60;
    const BUCKETS: usize = 48;
    const LIMIT: f32 = 0.5;

    /// A ring this bright cannot sit next to a ring with no light at all.
    const BRIGHT: u8 = 40;

    let frame = particle_light_with(
        FRAME,
        &one_hit(FRAME, 0.4),
        &[("spread", 0.0), ("gravity", 0.0)],
    );
    let profile = brightest_by_radius(&frame, BUCKETS, LIMIT);

    assert!(
        profile.iter().any(|&light| light > 200),
        "no ring of the burst is lit at all: {profile:?}",
    );

    for (ring, pair) in profile.windows(2).enumerate() {
        let [inner, outer] = [pair[0], pair[1]];
        assert!(
            !(inner == 0 && outer > BRIGHT || inner > BRIGHT && outer == 0),
            "ring {ring} reads {inner} and ring {} reads {outer}: the burst has a \
             hard edge where its halo should fade. The cull's `reach` is narrower \
             than a particle's glow, so it is skipping bursts that still light \
             this pixel.\nprofile: {profile:?}",
            ring + 1,
        );
    }
}

/// A burst at age zero has thrown nothing anywhere yet: every particle is still
/// at the origin, and what the frame shows is the disc their overlapping glows
/// make. A cull that only admits pixels *on* the burst's shell would leave that
/// disc a single pixel wide, and the expansion test above would not notice —
/// a one-pixel burst still expands.
#[test]
fn a_burst_lights_a_disc_on_the_frame_it_is_thrown() {
    const FRAME: usize = 60;

    let (_, lit) = lit_spread(&particle_light(FRAME, &one_hit(FRAME, 0.0)));

    assert!(
        lit > 100,
        "a brand new burst lit {lit} pixels: its particles are all at the origin, \
         and their glow should cover a disc around it",
    );
}

/// Where the light of `frame` falls farther than `radius` from the center, in
/// the shader's own units: the short edge is 1.0.
fn light_beyond(frame: &[u8], radius: f32) -> Vec<[u8; 3]> {
    let (width, height) = (WIDTH as f32, HEIGHT as f32);
    let short = width.min(height);

    frame
        .chunks_exact(4)
        .enumerate()
        .filter(|(index, _)| {
            let x = (index % WIDTH as usize) as f32 - 0.5 * width;
            let y = (index / WIDTH as usize) as f32 - 0.5 * height;
            (x * x + y * y).sqrt() / short > radius
        })
        .map(|(_, pixel)| [pixel[0], pixel[1], pixel[2]])
        .collect()
}

/// A burst is bound to the *hit* that threw it, not to the slot the hit happens
/// to occupy this frame.
///
/// A slot is a place in a sliding window: the 0.3 s-old burst below sits in slot
/// 0 until a new hit lands, and in slot 1 afterwards. Every particle's direction,
/// speed, and color is a seeded hash, and a `particles` that hashed the slot
/// would tear all of them across the frame on every kick — a shimmer nobody would
/// read as a bug, and one no golden hash would catch, because both frames would
/// be blessed. Hashing the hit's ordinal is what makes a burst hold still.
///
/// The new hit is at age zero, so its own light is a blob at the origin: outside
/// `RADIUS` nothing of it can reach, and the older burst's light there must be
/// the same to the byte.
#[test]
fn a_burst_is_hashed_from_the_hit_that_threw_it_and_not_from_its_slot() {
    const FRAME: usize = 60;

    /// Past the reach of a brand-new burst — its origin jitter plus its halo —
    /// and well inside the 0.3 s-old burst that both frames share.
    const RADIUS: f32 = 0.15;

    let time = (FRAME as f64 / f64::from(FPS)) as f32;
    let older = (time - 0.3, 5.0);

    let mut alone = EMPTY_HISTORY;
    alone[0] = older.0;
    alone[1] = older.1;

    // The same hit, one slot back, with a fresh one in front of it.
    let mut after = EMPTY_HISTORY;
    after[0] = time;
    after[1] = 6.0;
    after[2] = older.0;
    after[3] = older.1;

    let before = particle_light(FRAME, &alone);
    let shifted = particle_light(FRAME, &after);

    let lit = light_beyond(&before, RADIUS)
        .iter()
        .filter(|pixel| **pixel != [0, 0, 0])
        .count();
    assert!(lit > 0, "the older burst reaches nothing beyond {RADIUS}");

    assert_eq!(
        light_beyond(&before, RADIUS),
        light_beyond(&shifted, RADIUS),
        "a new hit moved the burst that was already in the air: its particles are \
         hashed from the slot it sits in rather than from the hit that threw it",
    );
}

/// The whole reason the hits are re-simulated rather than integrated: frame `N`
/// is a pure function of frame `N`'s inputs. A `particles` that carried state
/// between draws would render frame 100 differently depending on whether frames
/// 0..99 came first — and `--sample 1:00..1:03` would render a different video
/// than the same seconds of a full render.
#[test]
fn particles_renders_a_frame_the_same_whether_or_not_the_frames_before_it_were_drawn() {
    let particles = Preset::by_name("particles").expect("particles ships");
    const FRAME: usize = 100;

    let _device = one_device_at_a_time();
    let gpu = Gpu::new(AdapterChoice::Software).expect("golden frames need lavapipe");
    let stage = Stage::new(&gpu, particles, ember(), Some(Backdrop::default()));

    let draw = |index: usize| {
        let globals = Globals::for_frame(
            index,
            FPS,
            (WIDTH, HEIGHT),
            GOLDEN_SEED,
            synthetic_frame(index),
            ember(),
            defaults(particles),
        );
        stage.draw(&gpu, &globals, &silent_spectrum(), &synthetic_onsets(index));
    };

    draw(FRAME);
    let cold = hex(&stage.read(&gpu));

    for index in 0..=FRAME {
        draw(index);
    }
    let warm = hex(&stage.read(&gpu));

    assert_eq!(
        cold, warm,
        "particles renders frame {FRAME} differently once the frames before it \
         have been drawn: something is being carried between draws",
    );
}

/// The highs twinkle the particles still in the air (`VISION.md` §6). Nothing
/// else here would notice a `sparkle` wired to a feature the preset does not
/// claim to react to.
#[test]
fn the_highs_twinkle_the_particles_particles_still_has_in_the_air() {
    let particles = Preset::by_name("particles").expect("particles ships");
    const FRAME: usize = 60;

    let _device = one_device_at_a_time();
    let gpu = Gpu::new(AdapterChoice::Software).expect("golden frames need lavapipe");
    let stage = Stage::new(&gpu, particles, ember(), None);

    let draw = |high_env: f32| {
        let features = FeatureFrame {
            rms_env: 0.9,
            high_env,
            centroid: 0.4,
            ..FeatureFrame::default()
        };
        let globals = Globals::for_frame(
            FRAME,
            FPS,
            (WIDTH, HEIGHT),
            GOLDEN_SEED,
            features,
            ember(),
            defaults(particles),
        );
        stage.draw(&gpu, &globals, &silent_spectrum(), &one_hit(FRAME, 0.3));
        hex(&stage.read(&gpu))
    };

    assert_ne!(
        draw(0.0),
        draw(1.0),
        "a cymbal-heavy frame and a dull one twinkle the same burst identically",
    );
}

/// `--sample 3s --adapter software` must produce stable, non-black, evolving
/// output, as for `nebula` and `ribbons`: that the frame is *lit* without
/// blowing out. A burst is sparse by nature, so the bar for "lit" is a hundredth
/// of the frame rather than the tenth those two are held to.
#[test]
fn particles_renders_a_lit_frame_that_is_neither_black_nor_blown_out() {
    const FRAME: usize = 100;
    let frame = particle_light(FRAME, &synthetic_onsets(FRAME));

    let lit = frame
        .chunks_exact(4)
        .filter(|pixel| pixel[..3] != [0, 0, 0])
        .count();
    let white = frame
        .chunks_exact(4)
        .filter(|pixel| pixel[..3] == [255, 255, 255])
        .count();
    let total = (WIDTH * HEIGHT) as usize;

    assert!(
        lit * 100 > total,
        "only {lit} of {total} pixels are lit: particles renders black",
    );
    assert!(
        white * 4 < total,
        "{white} of {total} pixels are pure white: the bursts blew out",
    );
}

/// A loud, mid-heavy passage with nothing transient in it: enough light in every
/// band `kaleido` reads that its fold is on screen, and no onset flare washing
/// the geometry out.
fn kaleido_features() -> FeatureFrame {
    FeatureFrame {
        rms_env: 0.90,
        bass_env: 0.40,
        low_mid_env: 0.50,
        mid_env: 0.60,
        high_env: 0.50,
        air_env: 0.30,
        flux: 0.10,
        centroid: 0.40,
        ..FeatureFrame::default()
    }
}

/// One frame of `kaleido` over no backdrop, drawn at `frame_index` from
/// `features`, with some of its parameters moved off their defaults.
///
/// No backdrop: what comes back is the preset's own premultiplied light, so a
/// test may read "unlit" off a pixel rather than off the palette gradient the
/// compositor would have put under it.
fn kaleido_light(
    frame_index: usize,
    features: FeatureFrame,
    overrides: &[(&str, toml::Value)],
) -> Vec<u8> {
    let kaleido = Preset::by_name("kaleido").expect("kaleido ships");

    let mut table = toml::Table::new();
    for (name, value) in overrides {
        table.insert((*name).to_owned(), value.clone());
    }
    let params = kaleido
        .schema()
        .expect("the shipped schema parses")
        .resolve(&table)
        .expect("the overrides are in range");

    let _device = one_device_at_a_time();
    let gpu = Gpu::new(AdapterChoice::Software).expect("golden frames need lavapipe");
    let stage = Stage::new(&gpu, kaleido, ember(), None);
    let globals = Globals::for_frame(
        frame_index,
        FPS,
        (WIDTH, HEIGHT),
        GOLDEN_SEED,
        features,
        ember(),
        params,
    );
    stage.draw(&gpu, &globals, &silent_spectrum(), &silent_onsets());

    stage.read(&gpu)
}

/// `kaleido` is the preset RFC-001 NG1 said would need no new binding: a fold
/// over the frame it draws itself, and nothing else. Its whole diff is three
/// files in `presets/`, which is G3 (`scripts/quality.d/96-*`) holding for a
/// fifth time — but only for as long as its schema asks for nothing.
///
/// A `needs_feedback` that crept in here would cost a full-resolution texture
/// and a per-frame copy on every render, and no other test would say so.
#[test]
fn kaleido_folds_the_uniform_alone_and_asks_the_renderer_for_no_texture() {
    let kaleido = Preset::by_name("kaleido").expect("kaleido ships");
    let schema = kaleido.schema().expect("the shipped schema parses");

    assert!(!schema.needs_feedback, "kaleido reads no previous frame");
    assert!(!schema.needs_spectrum, "kaleido reads no spectrum");
    assert!(!schema.needs_onsets, "kaleido reads no onset history");
}

/// The luminance of `frame` around a circle of `radius` from its center, at
/// `samples` angles, counter-clockwise from the positive x-axis.
///
/// Bilinear rather than nearest-pixel: the frames below are compared against
/// *rotations* of themselves, and a rotation lands between pixel centers. Reading
/// the nearest one would leave a quantization floor an order of magnitude above
/// the symmetry being measured. `radius` is in the shader's own units, where the
/// short edge is 1.0.
fn ring_profile(frame: &[u8], radius: f32, samples: usize) -> Vec<f32> {
    let (width, height) = (WIDTH as f32, HEIGHT as f32);
    let short = width.min(height);

    let light = |x: usize, y: usize| -> f32 {
        let pixel = &frame[4 * (y * WIDTH as usize + x)..];
        f32::from(pixel[0]) + f32::from(pixel[1]) + f32::from(pixel[2])
    };

    (0..samples)
        .map(|sample| {
            let angle = std::f32::consts::TAU * sample as f32 / samples as f32;
            // The shader's `p.y` runs up and the frame's rows run down.
            let x = 0.5 * width + radius * short * angle.cos() - 0.5;
            let y = 0.5 * height - radius * short * angle.sin() - 0.5;

            let (x0, y0) = (x.floor(), y.floor());
            let (fx, fy) = (x - x0, y - y0);
            let (x0, y0) = (x0 as usize, y0 as usize);

            let top = light(x0, y0) * (1.0 - fx) + light(x0 + 1, y0) * fx;
            let bottom = light(x0, y0 + 1) * (1.0 - fx) + light(x0 + 1, y0 + 1) * fx;
            top * (1.0 - fy) + bottom * fy
        })
        .collect()
}

/// How far `profile` is from itself turned by `shift` samples.
fn turned_by(profile: &[f32], shift: usize) -> f32 {
    let sum: f32 = (0..profile.len())
        .map(|i| (profile[i] - profile[(i + shift) % profile.len()]).abs())
        .sum();
    sum / profile.len() as f32
}

/// `kaleido` folds the frame into the number of wedges its schema declares.
///
/// The whole preset is that fold, and nothing else in this file would notice its
/// absence: a shader that drew the same petals and rings without folding them
/// would have its golden hashes blessed, would react to every feature, and would
/// leave the backdrop showing through. So the frame is compared against *itself,
/// turned by one wedge* — the symmetry a fold into `segments` wedges has by
/// construction, whatever else the shader draws.
///
/// Turned by half a wedge it must not match, or the assertion above would pass on
/// a shader that drew concentric rings and no angular structure at all.
#[test]
fn kaleido_folds_the_frame_into_the_wedges_it_declares() {
    /// A ring well inside the vignette, out where the petals have room.
    const RADIUS: f32 = 0.35;
    /// Divisible by both `SEGMENTS` and twice `SEGMENTS`, so a wedge and half a
    /// wedge are both whole numbers of samples.
    const SAMPLES: usize = 360;
    const SEGMENTS: i64 = 6;

    let frame = kaleido_light(
        100,
        kaleido_features(),
        &[("segments", toml::Value::Integer(SEGMENTS))],
    );
    let profile = ring_profile(&frame, RADIUS, SAMPLES);

    let wedge = SAMPLES / SEGMENTS as usize;
    let at_wedge = turned_by(&profile, wedge);
    let at_half_wedge = turned_by(&profile, wedge / 2);

    assert!(
        at_half_wedge > 1.0,
        "the frame is the same half a wedge round as it is here (by {at_half_wedge:.3} \
         of 765): kaleido drew no angular structure to fold",
    );
    assert!(
        at_wedge * 4.0 < at_half_wedge,
        "turned by one wedge the frame differs by {at_wedge:.3} and turned by half a \
         wedge by {at_half_wedge:.3}: kaleido is not folding its frame into \
         {SEGMENTS} wedges",
    );
}

/// Where `frame` differs from itself reflected top to bottom, by more than the
/// last bit of an 8-bit channel.
///
/// The reflection is exact in the shader's coordinates: a 180-row frame puts its
/// center on a row boundary, so the pixel centers of rows `j` and `179 - j` sit
/// at `+d` and `-d` from it. Only the fold decides whether the light there does.
fn rows_not_mirrored(frame: &[u8]) -> usize {
    let (width, height) = (WIDTH as usize, HEIGHT as usize);

    (0..height / 2)
        .flat_map(|row| (0..width).map(move |column| (row, column)))
        .filter(|&(row, column)| {
            let top = &frame[4 * (row * width + column)..][..3];
            let bottom = &frame[4 * ((height - 1 - row) * width + column)..][..3];
            top.iter().zip(bottom).any(|(a, b)| a.abs_diff(*b) > 1)
        })
        .count()
}

/// `mirror` is what makes a kaleidoscope a kaleidoscope: alternate wedges are
/// reflections of their neighbours rather than copies, so the fold has an axis
/// and the frame is symmetric across it.
///
/// With `spin = 0` that axis is the frame's own horizontal, and the picture must
/// be its own reflection. Turned off, the wedges are rotated copies and the
/// reflection is gone. A `params[3].x` the shader packed but never branched on
/// would render one picture for both, and `param_reaches_declared_uniform_slot`
/// would still pass — a bool changes *a* pixel through any use at all.
#[test]
fn a_mirrored_fold_reflects_the_frame_across_its_axis() {
    let still = [("spin", toml::Value::Float(0.0))];

    let mirrored = kaleido_light(
        100,
        kaleido_features(),
        &[still[0].clone(), ("mirror", toml::Value::Boolean(true))],
    );
    let rotated = kaleido_light(
        100,
        kaleido_features(),
        &[still[0].clone(), ("mirror", toml::Value::Boolean(false))],
    );

    assert_eq!(
        rows_not_mirrored(&mirrored),
        0,
        "a mirrored fold with no spin on it is not symmetric about the frame's \
         horizontal: its wedges are not reflections of one another",
    );

    let asymmetric = rows_not_mirrored(&rotated);
    assert!(
        asymmetric > 1_000,
        "with `mirror` off only {asymmetric} pixels break the reflection: the fold \
         mirrors its wedges whatever the parameter says",
    );
}

/// The only clocks in `kaleido` are the three knobs that name one: the fold's
/// `spin`, the rings' `drift`, and the palette's `hue_cycle`.
///
/// Turn all three off and the frame is a function of its features alone, so the
/// same features render the same picture three seconds apart. This is `AGENTS.md`
/// determinism stated where it can actually be checked: a `sin(time)` wobble
/// somewhere in the shader is invisible in a golden hash — which pins one frame —
/// and invisible in `param_reaches_declared_uniform_slot`, which never moves
/// `time`.
#[test]
fn the_only_clocks_kaleido_reads_are_the_three_knobs_that_name_one() {
    let stopped = [
        ("spin", toml::Value::Float(0.0)),
        ("drift", toml::Value::Float(0.0)),
        ("hue_cycle", toml::Value::Float(0.0)),
    ];

    let early = kaleido_light(10, kaleido_features(), &stopped);
    let late = kaleido_light(100, kaleido_features(), &stopped);

    assert_eq!(
        hex(&early),
        hex(&late),
        "with its spin, its drift, and its hue cycle all at zero, kaleido renders \
         frame 10 and frame 100 differently: something else in the shader reads \
         the clock",
    );
}

/// The hue cycles (`VISION.md` §6): the palette walks on under a passage whose
/// features never move, which is the half of "hypnotic" that is not the fold.
///
/// Everything else that reads `time` is turned off, so what changes between the
/// two frames is the hue and nothing else. Without this, `hue_cycle` could be
/// wired to `centroid` alone and still pass every other test here.
#[test]
fn the_hue_cycles_with_time_under_features_that_do_not_move() {
    let cycling = [
        ("spin", toml::Value::Float(0.0)),
        ("drift", toml::Value::Float(0.0)),
        ("hue_cycle", toml::Value::Float(0.5)),
    ];

    let early = kaleido_light(10, kaleido_features(), &cycling);
    let late = kaleido_light(100, kaleido_features(), &cycling);

    assert_ne!(
        hex(&early),
        hex(&late),
        "three seconds apart, under identical features, kaleido renders the same \
         colors: the hue does not cycle with time",
    );
}

/// `--sample 3s --adapter software` must produce stable, non-black, evolving
/// output, as for every preset before it: that the frame is *lit* without blowing
/// out. A fold fills the frame, so the bar for "lit" is the half of it the
/// vignette leaves alone.
#[test]
fn kaleido_renders_a_lit_frame_that_is_neither_black_nor_blown_out() {
    let frame = kaleido_light(100, kaleido_features(), &[]);

    let lit = frame
        .chunks_exact(4)
        .filter(|pixel| pixel[..3] != [0, 0, 0])
        .count();
    let white = frame
        .chunks_exact(4)
        .filter(|pixel| pixel[..3] == [255, 255, 255])
        .count();
    let total = (WIDTH * HEIGHT) as usize;

    assert!(
        lit * 2 > total,
        "only {lit} of {total} pixels are lit: kaleido renders black",
    );
    assert!(
        white * 4 < total,
        "{white} of {total} pixels are pure white: the fold blew out",
    );
}

/// A loud, sustained passage: the material `ink` grows out of. `rms_env` is the
/// growth rate (`VISION.md` §6), so this is the field being fed.
fn loud() -> FeatureFrame {
    FeatureFrame {
        rms_env: 0.90,
        bass_env: 0.35,
        low_mid_env: 0.55,
        mid_env: 0.45,
        high_env: 0.40,
        air_env: 0.25,
        flux: 0.10,
        centroid: 0.35,
        ..FeatureFrame::default()
    }
}

/// The same passage played quietly. Only `rms_env` moves, so anything the two
/// render differently is something `rms_env` drives and nothing else does.
fn quiet() -> FeatureFrame {
    FeatureFrame {
        rms_env: 0.05,
        ..loud()
    }
}

/// A hit on frame 0 — the drop of ink the field grows from — and `sustain` for
/// every frame after it.
///
/// `onset` is 1.0 on exactly the frame the flux peaked (`analysis::onset`), so the
/// drop happens once and never repeats. Everything below therefore watches one
/// drop of ink live out its life, which is the only way to see a reaction.
fn a_drop_then(sustain: FeatureFrame) -> impl Fn(usize) -> FeatureFrame {
    move |index| {
        if index == 0 {
            FeatureFrame {
                onset: 1.0,
                ..sustain
            }
        } else {
            sustain
        }
    }
}

/// `ink` drawn from frame 0 through frame `frames`, and read back over no
/// backdrop.
///
/// A feedback preset has no other kind of frame: what comes back is the whole
/// history, which is the point. No backdrop, so a test may read "no ink here" off
/// a pixel's alpha rather than off the palette gradient the compositor would have
/// put under it.
fn ink_field(
    frames: usize,
    seed: u64,
    features: impl Fn(usize) -> FeatureFrame,
    overrides: &[(&str, toml::Value)],
) -> Vec<u8> {
    let ink = Preset::by_name("ink").expect("ink ships");

    let mut table = toml::Table::new();
    for (name, value) in overrides {
        table.insert((*name).to_owned(), value.clone());
    }
    let params = ink
        .schema()
        .expect("the shipped schema parses")
        .resolve(&table)
        .expect("the overrides are in range");

    let _device = one_device_at_a_time();
    let gpu = Gpu::new(AdapterChoice::Software).expect("golden frames need lavapipe");
    let stage = Stage::new(&gpu, ink, ember(), None);

    for index in 0..=frames {
        let globals = Globals::for_frame(
            index,
            FPS,
            (WIDTH, HEIGHT),
            seed,
            features(index),
            ember(),
            params,
        );
        stage.draw(&gpu, &globals, &silent_spectrum(), &silent_onsets());
    }

    stage.read(&gpu)
}

/// How many pixels of `frame` carry *grown* ink, past the density the reaction
/// only reaches by running away with itself.
fn grown(frame: &[u8]) -> usize {
    frame.chunks_exact(4).filter(|pixel| pixel[3] > 128).count()
}

/// How much of the backdrop the ink hides where it is thickest, out of 255.
///
/// [`grown`] cannot see a field that is thinning uniformly: the plateau a dense
/// blob settles on sits just above its threshold, so the first frame of decay
/// carries every pixel across at once and the count falls off a cliff. The peak
/// density falls smoothly, and is what "there is still ink here" means.
fn thickest(frame: &[u8]) -> u8 {
    frame
        .chunks_exact(4)
        .map(|pixel| pixel[3])
        .max()
        .expect("the frame has pixels")
}

/// How far the ink reaches from the centre of `frame`, in the shader's own units
/// where the short edge spans 1.0.
fn ink_reach(frame: &[u8]) -> f32 {
    let (width, height) = (WIDTH as usize, HEIGHT as usize);
    let short = WIDTH.min(HEIGHT) as f32;

    frame
        .chunks_exact(4)
        .enumerate()
        .filter(|(_, pixel)| pixel[3] > 2)
        .map(|(index, _)| {
            let x = (index % width) as f32 + 0.5 - 0.5 * WIDTH as f32;
            let y = (index / width) as f32 + 0.5 - 0.5 * height as f32;
            (x * x + y * y).sqrt() / short
        })
        .fold(0.0, f32::max)
}

/// How far from the centre of the frame the onset drops its ink, from `ink.wgsl`.
/// Nothing but diffusion can carry the field past it.
const DROP_RADIUS: f32 = 0.34;

/// `ink` grows out of the frame before it and asks the renderer for nothing else.
///
/// RFC-001 NG1 predicted this: a reaction-diffusion reads the previous frame, and
/// the previous frame already exists. So the feedback texture must be declared —
/// a shipped `ink` that forgot to would compile, render a memoryless boil, and
/// have its golden hashes blessed — and the other two bindings must not be, since
/// each costs a texture and an upload on every frame of every render.
///
/// `ink` also carries a `perf_hint`, because `steps` is the knob a user reading
/// "reaction sub-steps per frame" will reach for first, and it is the wrong one.
/// Measured on lavapipe at 720p, eight sub-steps cost under 10% more than one —
/// the nine texture samples and the readback dominate, and the reaction loop is
/// cheap ALU. So the hint has to name `steps` in order to *disclaim* it, and point
/// at `--sample` and the resolution, which are what actually pay. `docs/RELEASE.md`
/// requires the numbers in a hint to be re-measured rather than re-read; whether
/// the hint is *true* is that checklist's job, and only its shape is asserted here.
#[test]
fn ink_grows_from_the_previous_frame_and_asks_for_no_other_texture() {
    let ink = Preset::by_name("ink").expect("ink ships");
    let schema = ink.schema().expect("the shipped schema parses");

    assert!(schema.needs_feedback, "ink grows out of the previous frame");
    assert!(!schema.needs_spectrum, "ink reads no spectrum");
    assert!(!schema.needs_onsets, "ink reads no onset history");

    let hint = schema
        .perf_hint
        .expect("`steps` invites a user to tune the wrong knob; the hint says so");
    assert!(
        hint.contains("steps"),
        "the perf hint does not mention `steps`, which is the knob a reader of this \
         schema will reach for and the one that buys the least: {hint}",
    );
    assert!(
        hint.contains("--sample"),
        "the perf hint sends the user to no lever that actually pays: {hint}",
    );
}

/// The loudness of the song is the growth rate (`VISION.md` §6): the one sentence
/// the brief writes about how `ink` should move.
///
/// Two renders whose features differ in `rms_env` and in nothing else. Under the
/// loud one the drop takes hold and the reaction runs away with itself; under the
/// quiet one the same drop of ink, fed the same blooms and stirred the same way,
/// never reaches the density it needs to survive its own dissolving.
///
/// A golden hash cannot see this — `rms_env` moving the picture *at all* would
/// satisfy it — and neither can `param_reaches_declared_uniform_slot`, which never
/// moves a feature.
#[test]
fn the_loudness_of_the_song_is_the_growth_rate() {
    const FRAMES: usize = 30;

    let played = ink_field(FRAMES, GOLDEN_SEED, a_drop_then(loud()), &[]);
    let whispered = ink_field(FRAMES, GOLDEN_SEED, a_drop_then(quiet()), &[]);

    assert!(
        grown(&played) > 5_000,
        "after {FRAMES} loud frames only {} pixels carry grown ink: the drop never \
         took hold",
        grown(&played),
    );
    assert_eq!(
        grown(&whispered),
        0,
        "the same drop, under a passage quiet in nothing but its `rms_env`, grew \
         {} pixels of dense ink: the loudness is not the growth rate",
        grown(&whispered),
    );
}

/// And when the song stops, the ink lets the backdrop back.
///
/// Sixty loud frames grow a dense field; sixty silent ones must dissolve it. This
/// is what makes `ink` a preset and not a stain: the alpha it writes is the
/// backdrop's coverage (`VISION.md` §5.3), so a field that never dissolved would
/// leave a silent outro behind a sheet of ink.
///
/// It is also the sharpest feedback test in this file, because of the control it
/// renders. `never_played` is fed *the same silent features on the same frame* as
/// `a_frame_later` — it differs only in the sixty frames before it, which it spent
/// in silence rather than in music. A memoryless shader cannot tell the two apart
/// and must draw them alike. `ink` draws a dense field and clear water: the
/// picture at frame N is a function of the frames before it and not of frame N's
/// features. No golden hash of a single frame would notice the difference.
#[test]
fn the_ink_dissolves_back_to_the_backdrop_when_the_song_stops() {
    const GROWN_FOR: usize = 60;

    let playing = |index: usize| a_drop_then(loud())(index);
    let then_silence = |index: usize| {
        if index <= GROWN_FOR {
            playing(index)
        } else {
            FeatureFrame::default()
        }
    };

    let at_the_last_note = ink_field(GROWN_FOR, GOLDEN_SEED, playing, &[]);
    let a_frame_later = ink_field(GROWN_FOR + 1, GOLDEN_SEED, then_silence, &[]);
    let never_played = ink_field(GROWN_FOR + 1, GOLDEN_SEED, |_| FeatureFrame::default(), &[]);
    let two_seconds_later = ink_field(GROWN_FOR + 60, GOLDEN_SEED, then_silence, &[]);

    assert!(
        grown(&at_the_last_note) > 5_000,
        "sixty loud frames grew only {} pixels of dense ink",
        grown(&at_the_last_note),
    );

    // The field decays fast enough that the dense-pixel *count* falls off a cliff
    // on this frame — the plateau sits just over `grown`'s threshold. What must
    // survive the first silent frame is the ink itself, and what says so is how
    // much backdrop it still hides.
    assert!(
        thickest(&a_frame_later) > 100,
        "one silent frame took the ink down to {}/255 at its thickest: the field is \
         not carried from the frame before",
        thickest(&a_frame_later),
    );
    assert!(
        thickest(&never_played) < 10,
        "sixty-one silent frames grew ink {}/255 thick on their own: the control is \
         not clear water, so `a_frame_later` proves nothing about memory",
        thickest(&never_played),
    );

    assert!(
        thickest(&two_seconds_later) < 20,
        "two seconds into the silence the ink still covers {}/255 of the backdrop at \
         its thickest: it does not dissolve",
        thickest(&two_seconds_later),
    );
}

/// Diffusion is the only way the ink can leave the drop that threw it.
///
/// The onset drops its ink inside `r < DROP_RADIUS` and nothing else in the shader
/// puts any outside — the blooms are turned off here, and so is the stirring. So
/// the field can only cross that circle by bleeding across it, one texel per
/// frame, which is the diffusion half of a reaction-diffusion.
///
/// Nothing else in this file would notice its absence. A shader that read only its
/// own texel of the previous frame would still grow, still react to every feature,
/// still dissolve into silence, and still have its golden hashes blessed — it
/// would simply never spread, and `ink` would be a preset about a circle.
#[test]
fn diffusion_is_the_only_way_the_ink_leaves_the_drop_that_threw_it() {
    const FRAMES: usize = 60;

    let still_water = [
        ("swirl", toml::Value::Float(0.0)),
        ("seed_rate", toml::Value::Float(0.0)),
    ];
    let mut sealed = still_water.to_vec();
    sealed.push(("diffusion", toml::Value::Float(0.0)));

    let bleeding = ink_reach(&ink_field(
        FRAMES,
        GOLDEN_SEED,
        a_drop_then(loud()),
        &still_water,
    ));
    let sealed = ink_reach(&ink_field(
        FRAMES,
        GOLDEN_SEED,
        a_drop_then(loud()),
        &sealed,
    ));

    assert!(
        sealed < DROP_RADIUS,
        "with `diffusion = 0` the ink reached {sealed:.3} from the centre, past the \
         {DROP_RADIUS} the drop covers: something other than diffusion moves the field",
    );
    assert!(
        bleeding > DROP_RADIUS,
        "after {FRAMES} frames the ink has reached {bleeding:.3} from the centre and \
         the drop covers {DROP_RADIUS}: the field never left the disc it was dropped \
         in, so the shader reads no neighbour of the previous frame",
    );
}

/// The reaction sub-steps are what advance the reaction, and the issue asked for
/// them by name: "a couple of feedback iterations per output frame for the RD
/// look".
///
/// One Euler step of the reaction per frame barely moves the field; the shipped
/// four get it further in the same thirty frames — more of the frame grown dense,
/// and a peak density one step never reaches, because the growth term is stepped
/// four times against one frame's worth of dissolving.
///
/// The measure is density, not spread. More sub-steps grow *less* area, not more:
/// the reaction is bistable, so every extra step also eats the thin ink that sits
/// below its threshold, sharpening fronts rather than pushing them outward. Only
/// diffusion spreads the field, which is
/// `diffusion_is_the_only_way_the_ink_leaves_the_drop_that_threw_it`.
///
/// Against the shipped `steps = 4` rather than the maximum 8, and not because 8 is
/// slow: past the point where the fronts meet, more steps grow *fewer* dense
/// pixels, because `crowd` starves a blob's interior and hollows it out. That
/// hollowing is the reaction-diffusion look, so the monotone claim is the one
/// below and not "more steps, more ink, forever".
///
/// `param_reaches_declared_uniform_slot` proves only that `steps` changes *a*
/// pixel, which a `steps` wired to the hue would also do.
#[test]
fn more_reaction_sub_steps_advance_the_reaction_further_in_the_same_frames() {
    const FRAMES: usize = 30;

    let once = ink_field(
        FRAMES,
        GOLDEN_SEED,
        a_drop_then(loud()),
        &[("steps", toml::Value::Integer(1))],
    );
    let four_times = ink_field(
        FRAMES,
        GOLDEN_SEED,
        a_drop_then(loud()),
        &[("steps", toml::Value::Integer(4))],
    );

    assert!(
        grown(&four_times) > grown(&once) * 6 / 5,
        "four reaction sub-steps a frame grew {} pixels of dense ink and one grew \
         {}: the sub-steps do not advance the reaction",
        grown(&four_times),
        grown(&once),
    );
    assert!(
        thickest(&four_times) > thickest(&once),
        "one sub-step a frame reaches {}/255 at its thickest and four reach {}: the \
         extra steps buy no density, so they are not stepping the growth term",
        thickest(&once),
        thickest(&four_times),
    );
}

/// The field at frame `N` is a function of `(seed, features[0..N])` and nothing
/// else — which is what the issue asks of the reaction-diffusion state, and what
/// `AGENTS.md` asks of every preset.
///
/// Two renders of the same frames agree exactly, so no wall clock and no unseeded
/// randomness reaches the field through a hundred rounds of feedback. Two seeds
/// disagree, so `--seed` reaches the blooms the ink grows from rather than being
/// plumbed through every layer and dropped by the last one.
#[test]
fn ink_is_reproducible_from_its_seed_and_its_frames() {
    const FRAMES: usize = 40;

    let once = ink_field(FRAMES, GOLDEN_SEED, a_drop_then(loud()), &[]);
    let twice = ink_field(FRAMES, GOLDEN_SEED, a_drop_then(loud()), &[]);
    let elsewhere = ink_field(FRAMES, GOLDEN_SEED + 1, a_drop_then(loud()), &[]);

    assert_eq!(
        hex(&once),
        hex(&twice),
        "{FRAMES} frames of feedback rendered two different fields from the same \
         seed and the same features",
    );
    assert_ne!(
        hex(&once),
        hex(&elsewhere),
        "two seeds grow the same field: the seed does not reach the blooms",
    );
}

/// `--sample 3s --adapter software` must produce stable, non-black, evolving
/// output, as for every preset before it: that the frame is *lit* without blowing
/// out. `ink` premultiplies its light by its own coverage, so it cannot blow out
/// by construction — this is what says the construction is the one that shipped.
#[test]
fn ink_renders_a_lit_frame_that_is_neither_black_nor_blown_out() {
    let frame = ink_field(100, GOLDEN_SEED, a_drop_then(loud()), &[]);

    let lit = frame
        .chunks_exact(4)
        .filter(|pixel| pixel[..3] != [0, 0, 0])
        .count();
    let white = frame
        .chunks_exact(4)
        .filter(|pixel| pixel[..3] == [255, 255, 255])
        .count();
    let total = (WIDTH * HEIGHT) as usize;

    assert!(
        lit * 4 > total,
        "after 100 frames only {lit} of {total} pixels are lit: ink renders black",
    );
    assert!(
        white * 4 < total,
        "{white} of {total} pixels are pure white: the field blew out",
    );
}

/// The two moments of the card's envelope that are worth pinning: halfway up the
/// fade, and fully up during the hold.
///
/// With the default `[text]` — `in_at = 1.0s`, `fade = 0.6s` — frame 39 is 1.3 s
/// (the middle of the fade in) and frame 48 is 1.6 s (the first fully opaque
/// frame). At 30 fps, both are exact.
const CARD_FRAMES: [usize; 2] = [39, 48];

fn card_golden_file() -> PathBuf {
    golden_dir().join("text-card.txt")
}

/// The card as the golden frames set it: large enough that a 320x180 frame
/// carries readable ink, and words with an ascender, a descender, and a space.
fn card_config() -> config::Text {
    config::Text {
        size: 0.16,
        ..config::Config::default().text
    }
}

fn card_words() -> CardText {
    CardText {
        title: Some("Sine Tones".to_owned()),
        artist: Some("avz test fixture".to_owned()),
    }
}

/// The card composited over the palette gradient on `frame_index`, hashed.
///
/// No visualizer: the card is what is being pinned, and `pulse` drawing over the
/// same pixels would hide a card that stopped rendering.
fn card_hash(frame_index: usize) -> String {
    let _device = one_device_at_a_time();
    let gpu = Gpu::new(AdapterChoice::Software).expect("golden frames need lavapipe");

    let target = Offscreen::new(&gpu, WIDTH, HEIGHT).expect("a 320x180 frame");
    let background = Backdrop::default().layer(&gpu, WIDTH, HEIGHT, ember());
    let layer = Layer::new(&gpu, WIDTH, HEIGHT, "text card");
    let compositor = Compositor::new(&gpu, &[&background, &layer]).expect("two 320x180 layers");

    let card = Card::prepare(&card_config(), &card_words(), (WIDTH, HEIGHT))
        .expect("the bundled font reads")
        .expect("latin words leave ink");
    let text = TextCard::new(&gpu, &card, ember()).expect("the card's pass builds");

    text.draw(&gpu, &layer, frame_index, FPS);
    compositor.composite(&gpu, &target);

    hex(&target.read_rgba(&gpu).expect("the frame reads back"))
}

/// The text card draws the same two frames it drew when its hashes were
/// committed: the same glyphs, in the same place, at the same two opacities.
///
/// This is the only test that would notice a `cosmic-text` upgrade that reshaped
/// the words, a font swap, a nine-grid anchor that moved, or an envelope whose
/// midpoint drifted. The unit tests hold the geometry and the envelope to their
/// arithmetic; only a hash holds the *picture* to what a human once looked at.
#[test]
fn the_text_card_renders_its_golden_frames() {
    let hashes: Vec<(String, String)> = CARD_FRAMES
        .iter()
        .map(|&index| (index.to_string(), card_hash(index)))
        .collect();

    // Checked before the regenerate branch: a card that ignored `frame_index`
    // would draw one picture twice, and `AVZ_UPDATE_GOLDEN=1` must never bless it.
    assert_ne!(
        hashes[0].1, hashes[1].1,
        "the card renders the middle of its fade and its full opacity alike: \
         the envelope never reaches a pixel",
    );

    if updating() {
        write_golden(
            &card_golden_file(),
            "Golden hashes of the text card, set in the bundled font, over the `ember` \
             gradient",
            &hashes,
        );
        return;
    }

    assert_eq!(
        read_golden(&card_golden_file()),
        hashes,
        "the text card no longer renders the frames its hashes were committed \
         from. If the change was intended, regenerate with `AVZ_UPDATE_GOLDEN=1 \
         cargo test -p avz-core --test golden_frames` and say in the commit \
         message what moved.",
    );
}

/// Before `in_at` there is no card, and the frame is the backdrop alone.
///
/// The card layer is always composited when a card exists, so a shader that
/// ignored the opacity would paint the type over every frame of the song.
#[test]
fn the_text_card_is_invisible_before_it_fades_in() {
    let _device = one_device_at_a_time();
    let gpu = Gpu::new(AdapterChoice::Software).expect("golden frames need lavapipe");

    let target = Offscreen::new(&gpu, WIDTH, HEIGHT).expect("a 320x180 frame");
    let background = Backdrop::default().layer(&gpu, WIDTH, HEIGHT, ember());
    let layer = Layer::new(&gpu, WIDTH, HEIGHT, "text card");
    let compositor = Compositor::new(&gpu, &[&background, &layer]).expect("two 320x180 layers");
    let bare = Compositor::new(&gpu, &[&background]).expect("one 320x180 layer");

    let card = Card::prepare(&card_config(), &card_words(), (WIDTH, HEIGHT))
        .expect("the bundled font reads")
        .expect("latin words leave ink");
    let text = TextCard::new(&gpu, &card, ember()).expect("the card's pass builds");

    // Frame 0 is `0.0s`, and the default card is asked for at `1.0s`.
    text.draw(&gpu, &layer, 0, FPS);
    compositor.composite(&gpu, &target);
    let before = target.read_rgba(&gpu).expect("the frame reads back");

    bare.composite(&gpu, &target);
    let backdrop = target.read_rgba(&gpu).expect("the frame reads back");

    assert_eq!(
        before, backdrop,
        "the card is drawn a second before it is asked for",
    );
}

/// The card, at 960x540, across its whole envelope, in `target/text-card/`.
///
/// A hash says the card did not change; it does not say the card is well set.
/// Leading, margins, and the rise are typography, and typography is looked at:
///
/// ```bash
/// cargo test -p avz-core --test golden_frames -- --ignored dump_text_card
/// ```
///
/// `#[ignore]`d because it writes files and asserts nothing.
#[test]
#[ignore = "writes PNGs for the manual typography pass; asserts nothing"]
fn dump_text_card_frames() {
    const WIDE: u32 = 960;
    const TALL: u32 = 540;
    /// The whole envelope of the default `[text]`, at 30 fps: before it is asked
    /// for (`1.0s`), halfway up the fade, the first fully opaque frame (`1.6s`),
    /// the middle of the hold, halfway down the fade out, and the first frame
    /// after the card is gone (`8.2s`).
    const KEPT: [usize; 6] = [25, 39, 48, 120, 237, 246];

    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/text-card");
    fs::create_dir_all(&out).expect("create target/text-card");

    let _device = one_device_at_a_time();
    let gpu = Gpu::new(AdapterChoice::Software).expect("the typography pass needs lavapipe");
    let target = Offscreen::new(&gpu, WIDE, TALL).expect("a 960x540 frame");
    let background = Backdrop::default().layer(&gpu, WIDE, TALL, ember());
    let layer = Layer::new(&gpu, WIDE, TALL, "text card");
    let compositor = Compositor::new(&gpu, &[&background, &layer]).expect("two 960x540 layers");

    let card = Card::prepare(&config::Config::default().text, &card_words(), (WIDE, TALL))
        .expect("the bundled font reads")
        .expect("latin words leave ink");
    let text = TextCard::new(&gpu, &card, ember()).expect("the card's pass builds");

    for index in KEPT {
        text.draw(&gpu, &layer, index, FPS);
        compositor.composite(&gpu, &target);
        let pixels = target.read_rgba(&gpu).expect("the frame reads back");

        let path = out.join(format!("{index:03}.png"));
        image::RgbaImage::from_raw(WIDE, TALL, pixels)
            .expect("the frame is WIDE x TALL RGBA")
            .save(&path)
            .unwrap_or_else(|err| panic!("{}: {err}", path.display()));
    }
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
    let stage = Stage::new(&gpu, preset, ember(), Some(Backdrop::default()));

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
        stage.draw(
            &gpu,
            &globals,
            &synthetic_spectrum(10),
            &synthetic_onsets(10),
        );
        let pixels = stage.read(&gpu);

        let path = out.join(format!("{name}.png"));
        image::RgbaImage::from_raw(WIDTH, HEIGHT, pixels)
            .expect("the frame is WIDTH x HEIGHT RGBA")
            .save(&path)
            .unwrap_or_else(|err| panic!("{}: {err}", path.display()));
    }
}

/// The same instrument for `nebula`, in `target/nebula-tuning/`.
///
/// A trail preset cannot be looked at one frame at a time — its whole character
/// is what a hundred frames leave behind — so this dumps a *sequence*, at
/// 960x540 and the frame indices where the trail has and has not converged:
///
/// ```bash
/// cargo test -p avz-core --test golden_frames -- --ignored dump_nebula
/// ```
///
/// The features are a slow bass swell with a hit every second, which is what the
/// dark-folk material `nebula` is tuned against sounds like to the analyzer.
///
/// `#[ignore]`d because it writes files and asserts nothing.
#[test]
#[ignore = "writes PNGs for the manual tuning pass; asserts nothing"]
fn dump_nebula_tuning_frames() {
    const WIDE: u32 = 960;
    const TALL: u32 = 540;
    const KEPT: [usize; 6] = [0, 1, 5, 30, 60, 120];

    let preset = Preset::by_name("nebula").expect("nebula ships");
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/nebula-tuning");
    fs::create_dir_all(&out).expect("create target/nebula-tuning");

    let _device = one_device_at_a_time();
    let gpu = Gpu::new(AdapterChoice::Software).expect("the tuning pass needs lavapipe");
    let target = Offscreen::new(&gpu, WIDE, TALL).expect("a 960x540 frame");
    let background = Backdrop::default().layer(&gpu, WIDE, TALL, ember());
    let visual = Layer::new(&gpu, WIDE, TALL, "visualizer");
    let visualizer = Visualizer::new(&gpu, preset, &visual).expect("nebula compiles");
    let compositor = Compositor::new(&gpu, &[&background, &visual]).expect("two 960x540 layers");

    let params = defaults(preset);
    for index in 0..=KEPT[KEPT.len() - 1] {
        let seconds = index as f32 / FPS as f32;
        // A bass swell that breathes once every four seconds, a hit on the beat.
        let swell = 0.5 + 0.5 * (seconds * 0.5).sin();
        let features = FeatureFrame {
            rms_env: 0.35 + 0.45 * swell,
            bass_env: swell,
            centroid: 0.25 + 0.4 * swell,
            onset: f32::from(index % FPS as usize == 0),
            ..FeatureFrame::default()
        };

        let globals = Globals::for_frame(
            index,
            FPS,
            (WIDE, TALL),
            GOLDEN_SEED,
            features,
            ember(),
            params,
        );
        visualizer.draw(
            &gpu,
            &visual,
            &globals,
            &synthetic_spectrum(index),
            &synthetic_onsets(index),
        );

        if !KEPT.contains(&index) {
            continue;
        }
        compositor.composite(&gpu, &target);
        let pixels = target.read_rgba(&gpu).expect("the frame reads back");
        let path = out.join(format!("{index:03}.png"));
        image::RgbaImage::from_raw(WIDE, TALL, pixels)
            .expect("the frame is WIDE x TALL RGBA")
            .save(&path)
            .unwrap_or_else(|err| panic!("{}: {err}", path.display()));
    }
}

/// The same instrument for `ribbons`, in `target/ribbons-tuning/`.
///
/// ```bash
/// cargo test -p avz-core --test golden_frames -- --ignored dump_ribbons
/// ```
///
/// A hash says the ribbons did not change; it does not say they read as ribbons.
/// The spectrum is a moving formant over a bass hump — a voice over a kick — so
/// what the frames show is whether the stack tracks the music across the
/// frequency axis rather than merely wobbling.
///
/// `#[ignore]`d because it writes files and asserts nothing.
#[test]
#[ignore = "writes PNGs for the manual tuning pass; asserts nothing"]
fn dump_ribbons_tuning_frames() {
    const WIDE: u32 = 960;
    const TALL: u32 = 540;
    const KEPT: [usize; 5] = [0, 10, 25, 60, 100];

    let preset = Preset::by_name("ribbons").expect("ribbons ships");
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/ribbons-tuning");
    fs::create_dir_all(&out).expect("create target/ribbons-tuning");

    let _device = one_device_at_a_time();
    let gpu = Gpu::new(AdapterChoice::Software).expect("the tuning pass needs lavapipe");
    let target = Offscreen::new(&gpu, WIDE, TALL).expect("a 960x540 frame");
    let background = Backdrop::default().layer(&gpu, WIDE, TALL, ember());
    let visual = Layer::new(&gpu, WIDE, TALL, "visualizer");
    let visualizer = Visualizer::new(&gpu, preset, &visual).expect("ribbons compiles");
    let compositor = Compositor::new(&gpu, &[&background, &visual]).expect("two 960x540 layers");

    let params = defaults(preset);
    for index in KEPT {
        let globals = Globals::for_frame(
            index,
            FPS,
            (WIDE, TALL),
            GOLDEN_SEED,
            synthetic_frame(index),
            ember(),
            params,
        );
        visualizer.draw(
            &gpu,
            &visual,
            &globals,
            &synthetic_spectrum(index),
            &synthetic_onsets(index),
        );

        compositor.composite(&gpu, &target);
        let pixels = target.read_rgba(&gpu).expect("the frame reads back");
        let path = out.join(format!("{index:03}.png"));
        image::RgbaImage::from_raw(WIDE, TALL, pixels)
            .expect("the frame is WIDE x TALL RGBA")
            .save(&path)
            .unwrap_or_else(|err| panic!("{}: {err}", path.display()));
    }
}

/// The same instrument for `particles`, in `target/particles-tuning/`.
///
/// ```bash
/// cargo test -p avz-core --test golden_frames -- --ignored dump_particles
/// ```
///
/// A hash says the bursts did not change; it does not say they read as bursts.
/// A burst preset also cannot be judged from one frame — what it is *for* is the
/// arc between the hit and the last spark going out — so this dumps the frames
/// around and after two hits half a second apart: the throw, the spread, the
/// fall, and the frame after the second hit lands on top of the first burst.
///
/// The features are a dense passage with a hit on frames 0 and 15, so `high_env`
/// is up and the twinkle is on screen.
///
/// `#[ignore]`d because it writes files and asserts nothing.
#[test]
#[ignore = "writes PNGs for the manual tuning pass; asserts nothing"]
fn dump_particles_tuning_frames() {
    const WIDE: u32 = 960;
    const TALL: u32 = 540;
    const HITS: [usize; 2] = [0, 15];
    const KEPT: [usize; 6] = [0, 4, 12, 15, 24, 44];

    let preset = Preset::by_name("particles").expect("particles ships");
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/particles-tuning");
    fs::create_dir_all(&out).expect("create target/particles-tuning");

    let _device = one_device_at_a_time();
    let gpu = Gpu::new(AdapterChoice::Software).expect("the tuning pass needs lavapipe");
    let target = Offscreen::new(&gpu, WIDE, TALL).expect("a 960x540 frame");
    let background = Backdrop::default().layer(&gpu, WIDE, TALL, ember());
    let visual = Layer::new(&gpu, WIDE, TALL, "visualizer");
    let visualizer = Visualizer::new(&gpu, preset, &visual).expect("particles compiles");
    let compositor = Compositor::new(&gpu, &[&background, &visual]).expect("two 960x540 layers");

    // The window the shipped preset sees: the hits at or before this frame,
    // newest first, exactly as `FeatureTimeline::onset_history` builds it.
    let history = |index: usize| {
        let mut row = EMPTY_HISTORY;
        let landed: Vec<usize> = HITS.iter().copied().filter(|&hit| hit <= index).collect();
        for (slot, ordinal) in row.chunks_exact_mut(2).zip((0..landed.len()).rev()) {
            slot[0] = (landed[ordinal] as f64 / f64::from(FPS)) as f32;
            slot[1] = ordinal as f32;
        }
        row.to_vec()
    };

    let params = defaults(preset);
    for index in KEPT {
        let features = FeatureFrame {
            rms_env: 0.85,
            high_env: 0.7,
            centroid: 0.45,
            flux: 0.2,
            onset: f32::from(HITS.contains(&index)),
            ..FeatureFrame::default()
        };

        let globals = Globals::for_frame(
            index,
            FPS,
            (WIDE, TALL),
            GOLDEN_SEED,
            features,
            ember(),
            params,
        );
        visualizer.draw(&gpu, &visual, &globals, &silent_spectrum(), &history(index));

        compositor.composite(&gpu, &target);
        let pixels = target.read_rgba(&gpu).expect("the frame reads back");
        let path = out.join(format!("{index:03}.png"));
        image::RgbaImage::from_raw(WIDE, TALL, pixels)
            .expect("the frame is WIDE x TALL RGBA")
            .save(&path)
            .unwrap_or_else(|err| panic!("{}: {err}", path.display()));
    }
}

/// The same instrument for `kaleido`, in `target/kaleido-tuning/`.
///
/// ```bash
/// cargo test -p avz-core --test golden_frames -- --ignored dump_kaleido
/// ```
///
/// A hash says the fold did not change; it does not say the fold is hypnotic.
/// What is being looked at is whether the symmetry reads as glass rather than as
/// a wallpaper tile — whether the spin is slow enough to follow and the hue walks
/// the palette without banding at the wrap. So the frames are seconds apart, over
/// a swell with a hit in it.
///
/// `#[ignore]`d because it writes files and asserts nothing.
#[test]
#[ignore = "writes PNGs for the manual tuning pass; asserts nothing"]
fn dump_kaleido_tuning_frames() {
    const WIDE: u32 = 960;
    const TALL: u32 = 540;
    const KEPT: [usize; 6] = [0, 15, 30, 90, 150, 240];

    let preset = Preset::by_name("kaleido").expect("kaleido ships");
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/kaleido-tuning");
    fs::create_dir_all(&out).expect("create target/kaleido-tuning");

    let _device = one_device_at_a_time();
    let gpu = Gpu::new(AdapterChoice::Software).expect("the tuning pass needs lavapipe");
    let target = Offscreen::new(&gpu, WIDE, TALL).expect("a 960x540 frame");
    let background = Backdrop::default().layer(&gpu, WIDE, TALL, ember());
    let visual = Layer::new(&gpu, WIDE, TALL, "visualizer");
    let visualizer = Visualizer::new(&gpu, preset, &visual).expect("kaleido compiles");
    let compositor = Compositor::new(&gpu, &[&background, &visual]).expect("two 960x540 layers");

    let params = defaults(preset);
    for index in KEPT {
        let seconds = index as f32 / FPS as f32;
        // A swell that breathes once every eight seconds, a hit on the beat.
        let swell = 0.5 + 0.5 * (seconds * 0.8).sin();
        let features = FeatureFrame {
            rms_env: 0.45 + 0.45 * swell,
            bass_env: swell,
            low_mid_env: 0.3 + 0.4 * swell,
            mid_env: 0.7 - 0.3 * swell,
            high_env: 0.5 * swell,
            air_env: 0.3,
            flux: 0.15,
            onset: f32::from(index % FPS as usize == 0),
            centroid: 0.25 + 0.4 * swell,
            ..FeatureFrame::default()
        };

        let globals = Globals::for_frame(
            index,
            FPS,
            (WIDE, TALL),
            GOLDEN_SEED,
            features,
            ember(),
            params,
        );
        visualizer.draw(
            &gpu,
            &visual,
            &globals,
            &silent_spectrum(),
            &silent_onsets(),
        );

        compositor.composite(&gpu, &target);
        let pixels = target.read_rgba(&gpu).expect("the frame reads back");
        let path = out.join(format!("{index:03}.png"));
        image::RgbaImage::from_raw(WIDE, TALL, pixels)
            .expect("the frame is WIDE x TALL RGBA")
            .save(&path)
            .unwrap_or_else(|err| panic!("{}: {err}", path.display()));
    }
}

/// The same instrument for `ink`, in `target/ink-tuning/`.
///
/// ```bash
/// cargo test -p avz-core --test golden_frames -- --ignored dump_ink
/// ```
///
/// A hash says the field did not change; it does not say the field is ink. What
/// is being looked at is whether the marble reads as something growing in water —
/// whether the fronts have an edge, whether the interior holds a pattern instead
/// of filling, and whether a quiet bar gives the backdrop back. So the frames
/// span a swell, and the last of them are drawn after the song has dropped away.
///
/// Every frame from 0 is drawn, because a feedback preset has no other kind of
/// frame; only the listed ones are written out.
///
/// `#[ignore]`d because it writes files and asserts nothing.
#[test]
#[ignore = "writes PNGs for the manual tuning pass; asserts nothing"]
fn dump_ink_tuning_frames() {
    const WIDE: u32 = 960;
    const TALL: u32 = 540;
    const LAST: usize = 300;
    const KEPT: [usize; 7] = [1, 15, 60, 120, 210, 260, 300];

    let preset = Preset::by_name("ink").expect("ink ships");
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/ink-tuning");
    fs::create_dir_all(&out).expect("create target/ink-tuning");

    let _device = one_device_at_a_time();
    let gpu = Gpu::new(AdapterChoice::Software).expect("the tuning pass needs lavapipe");
    let target = Offscreen::new(&gpu, WIDE, TALL).expect("a 960x540 frame");
    let background = Backdrop::default().layer(&gpu, WIDE, TALL, ember());
    let visual = Layer::new(&gpu, WIDE, TALL, "visualizer");
    let visualizer = Visualizer::new(&gpu, preset, &visual).expect("ink compiles");
    let compositor = Compositor::new(&gpu, &[&background, &visual]).expect("two 960x540 layers");

    let params = defaults(preset);
    for index in 0..=LAST {
        let seconds = index as f32 / FPS as f32;
        // A swell that breathes once every eight seconds, a hit on the beat — and
        // then, from seven seconds in, the song drops away and the ink dissolves.
        let swell = 0.5 + 0.5 * (seconds * 0.8).sin();
        let playing = f32::from(seconds < 7.0);
        let features = FeatureFrame {
            rms_env: (0.35 + 0.55 * swell) * playing,
            bass_env: swell * playing,
            low_mid_env: (0.3 + 0.4 * swell) * playing,
            mid_env: (0.7 - 0.3 * swell) * playing,
            high_env: 0.5 * swell * playing,
            air_env: 0.3 * playing,
            flux: 0.15 * playing,
            onset: f32::from(index % FPS as usize == 0) * playing,
            centroid: 0.25 + 0.4 * swell,
            ..FeatureFrame::default()
        };

        let globals = Globals::for_frame(
            index,
            FPS,
            (WIDE, TALL),
            GOLDEN_SEED,
            features,
            ember(),
            params,
        );
        visualizer.draw(
            &gpu,
            &visual,
            &globals,
            &silent_spectrum(),
            &silent_onsets(),
        );

        if !KEPT.contains(&index) {
            continue;
        }
        compositor.composite(&gpu, &target);
        let pixels = target.read_rgba(&gpu).expect("the frame reads back");
        let path = out.join(format!("{index:03}.png"));
        image::RgbaImage::from_raw(WIDE, TALL, pixels)
            .expect("the frame is WIDE x TALL RGBA")
            .save(&path)
            .unwrap_or_else(|err| panic!("{}: {err}", path.display()));
    }
}

/// The panel presets (#31, #32): a visualization that lives in one anchored
/// rectangle and leaves the rest of the frame to the backdrop.
const PANEL_PRESETS: &[&str] = &["bars"];

/// The pixel rectangle a panel preset's defaults claim, from the same
/// arithmetic the shader does: fractions of the frame for the size, a fraction
/// of the short edge for the margin, anchored bottom-left.
fn default_panel_rect(width: f32, height: f32, margin: f32) -> (u32, u32, u32, u32) {
    let panel_w = width * WIDTH as f32;
    let panel_h = height * HEIGHT as f32;
    let m = margin * WIDTH.min(HEIGHT) as f32;

    let x0 = m;
    let y1 = HEIGHT as f32 - m;
    (
        x0 as u32,
        (y1 - panel_h) as u32,
        (x0 + panel_w) as u32,
        y1 as u32,
    )
}

/// A panel preset owns its rectangle and nothing else (#31, #32): on a loud
/// frame, every pixel outside the panel is *exactly* the pixel a silent render
/// leaves — which is what lets a background image or video show through
/// untouched — and the panel itself is visibly lit.
#[test]
fn a_panel_preset_lights_only_its_panel() {
    for name in PANEL_PRESETS {
        let preset = PRESETS
            .iter()
            .find(|preset| preset.name == *name)
            .unwrap_or_else(|| panic!("`{name}` ships"));

        let _device = one_device_at_a_time();
        let gpu = Gpu::new(AdapterChoice::Software)
            .expect("panel tests need lavapipe: `sudo dnf install mesa-vulkan-drivers`");
        let stage = Stage::new(&gpu, preset, ember(), Some(Backdrop::default()));
        let params = defaults(preset);

        // Silence first: zero features and an empty spectrum must leave the
        // backdrop alone everywhere, panel included.
        let silent = Globals::for_frame(
            10,
            FPS,
            (WIDTH, HEIGHT),
            GOLDEN_SEED,
            FeatureFrame::default(),
            ember(),
            params,
        );
        stage.draw(&gpu, &silent, &silent_spectrum(), &silent_onsets());
        let backdrop_only = stage.read(&gpu);

        // Then the loud golden frame 10, a kick under a cymbal.
        let loud = Globals::for_frame(
            10,
            FPS,
            (WIDTH, HEIGHT),
            GOLDEN_SEED,
            synthetic_frame(10),
            ember(),
            params,
        );
        stage.draw(&gpu, &loud, &synthetic_spectrum(10), &synthetic_onsets(10));
        let lit = stage.read(&gpu);

        // The schema's own defaults decide where the panel is. Two pixels of
        // slack absorb the panel's edge falling between texels.
        let schema = preset.schema().expect("the shipped schema parses");
        let default_of = |param: &str| -> f32 {
            let found = schema
                .params
                .iter()
                .find(|p| p.name == param)
                .unwrap_or_else(|| panic!("`{name}` declares `{param}`"));
            match found.kind {
                ParamKind::Float { default, .. } => default,
                ref other => panic!("`{param}` is {}, expected float", other.type_name()),
            }
        };
        let (x0, y0, x1, y1) = default_panel_rect(
            default_of("width"),
            default_of("height"),
            default_of("margin"),
        );
        const SLACK: u32 = 2;

        let mut inside_changed = 0u32;
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                let at = ((y * WIDTH + x) * 4) as usize;
                let (was, now) = (&backdrop_only[at..at + 4], &lit[at..at + 4]);

                let outside = x + SLACK < x0 || x > x1 + SLACK || y + SLACK < y0 || y > y1 + SLACK;
                if outside {
                    assert_eq!(
                        was, now,
                        "`{name}` painted outside its panel at ({x}, {y}): \
                         rect is ({x0}, {y0})..({x1}, {y1})"
                    );
                } else if was != now {
                    inside_changed += 1;
                }
            }
        }

        let panel_area = (x1 - x0) * (y1 - y0);
        assert!(
            inside_changed * 10 >= panel_area,
            "`{name}` barely lit its own panel on a loud frame: \
             {inside_changed} of {panel_area} pixels changed"
        );
    }
}
