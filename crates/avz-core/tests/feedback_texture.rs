//! The previous-frame texture: one of the two bindings a preset may opt into.
//!
//! Feedback is "the workhorse of good abstract visuals" (`VISION.md` §6). A
//! preset opts in with `"needs_feedback": true` in its schema; the renderer then
//! binds last frame's pixels at `@binding(1)` and a sampler at `@binding(2)`. The
//! other binding, the spectrum texture, is covered by `spectrum_texture.rs`.
//!
//! The presets below are built here rather than shipped, because the properties
//! under test — frame 0 is black, frame N sees frame N-1, and a preset that did
//! not ask gets nothing — are properties of the renderer and would be invisible
//! through a shader that also draws clouds.
//!
//! **Software adapter only**, like every other GPU test: `AGENTS.md` expects GPU
//! float differences across machines. Needs Mesa's software Vulkan driver:
//! `sudo dnf install mesa-vulkan-drivers`.

use std::sync::{Mutex, MutexGuard, PoisonError};

use avz_core::analysis::FeatureFrame;
use avz_core::config::Palette;
use avz_core::render::{
    AdapterChoice, Compositor, Globals, Gpu, Layer, Offscreen, PARAM_SLOTS, PackedParams, Preset,
    Visualizer, palette,
};

const WIDTH: u32 = 64;
const HEIGHT: u32 = 64;
const FPS: u32 = 30;
const SEED: u64 = 1337;

/// See `pipeline_render.rs`: one Vulkan device per process, or the loader's
/// debug-utils terminator segfaults when two tests open devices at once.
static ONE_DEVICE_AT_A_TIME: Mutex<()> = Mutex::new(());

fn one_device_at_a_time() -> MutexGuard<'static, ()> {
    ONE_DEVICE_AT_A_TIME
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// The `VISION.md` §6 uniform contract and the fullscreen triangle, verbatim, so
/// a test shader below is only its fragment stage.
const PREAMBLE: &str = r"
struct Globals {
    time: f32,
    resolution: vec2<f32>,
    seed: f32,
    rms: f32,
    rms_env: f32,
    bass: f32,
    bass_env: f32,
    low_mid: f32,
    low_mid_env: f32,
    mid: f32,
    mid_env: f32,
    high: f32,
    high_env: f32,
    air: f32,
    air_env: f32,
    flux: f32,
    onset: f32,
    centroid: f32,
    pal: array<vec4<f32>, 5>,
    params: array<vec4<f32>, 8>,
}
@group(0) @binding(0) var<uniform> g: Globals;

@vertex
fn vs_main(@builtin(vertex_index) vertex: u32) -> @builtin(position) vec4<f32> {
    let index = i32(vertex);
    let x = f32(index / 2) * 4.0 - 1.0;
    let y = f32(index & 1) * 4.0 - 1.0;
    return vec4<f32>(x, y, 0.0, 1.0);
}

@group(0) @binding(1) var previous: texture_2d<f32>;
@group(0) @binding(2) var previous_sampler: sampler;
";

/// A preset assembled in the test, from `PREAMBLE` plus one fragment stage.
///
/// `Preset`'s fields are the two embedded files and two strings, so a test can
/// build one the registry never names — which is what lets the renderer's own
/// contract be asserted without a shipped shader in the way.
fn preset(name: &'static str, fragment: &str, needs_feedback: bool) -> Preset {
    let source: &'static str = Box::leak(format!("{PREAMBLE}\n{fragment}").into_boxed_str());
    let schema: &'static str = Box::leak(
        format!(
            r#"{{"needs_feedback": {needs_feedback}, "params": [
                 {{"name":"gain","type":"float","default":0.1,"min":0.0,"max":1.0,
                   "slot":[0,0],"description":"How much each frame adds."}}
               ]}}"#
        )
        .into_boxed_str(),
    );

    Preset {
        name,
        description: "a test preset",
        source,
        schema,
    }
}

fn defaults(preset: &Preset) -> PackedParams {
    preset
        .schema()
        .expect("the test schema parses")
        .resolve(&toml::Table::new())
        .expect("the test defaults pack")
}

/// Render `frames` frames of `preset` in sequence and return the last one's RGBA.
///
/// One visualizer across the whole sequence, exactly as `pipeline::render` does
/// it: the feedback texture is per-render state, so a fresh visualizer starts
/// from black again.
///
/// The visualizer layer is composited alone, over no background at all, so what
/// comes back is the premultiplied light the preset wrote and nothing else. The
/// shaders below all return alpha 1.0, so that is their color, unveiled.
fn render_sequence(preset: &Preset, frames: usize) -> Vec<u8> {
    let _device = one_device_at_a_time();
    let gpu = Gpu::new(AdapterChoice::Software)
        .expect("the feedback tests need lavapipe: `sudo dnf install mesa-vulkan-drivers`");
    let target = Offscreen::new(&gpu, WIDTH, HEIGHT).expect("a 64x64 frame");
    let visual = Layer::new(&gpu, WIDTH, HEIGHT, "visualizer");
    let visualizer = Visualizer::new(&gpu, preset, &visual).expect("the test preset compiles");
    let compositor = Compositor::new(&gpu, &[&visual]).expect("one frame-sized layer");

    let colors = palette::resolve(&Palette::Named("ember".to_owned())).expect("`ember` ships");
    let params = defaults(preset);

    for index in 0..frames {
        let globals = Globals::for_frame(
            index,
            FPS,
            (WIDTH, HEIGHT),
            SEED,
            FeatureFrame::default(),
            colors,
            params,
        );
        visualizer.draw(&gpu, &visual, &globals, &[]);
    }
    compositor.composite(&gpu, &target);

    target.read_rgba(&gpu).expect("the frame reads back")
}

/// The red channel of the top-left pixel: every test shader here paints the
/// whole frame one color, so one pixel is the frame.
fn red(frame: &[u8]) -> u8 {
    frame[0]
}

/// A shader that shows the feedback texture and nothing else. On the first
/// frame there is no previous frame, and `VISION.md` §6's contract is that what
/// it samples is black rather than whatever the driver left in memory.
#[test]
fn the_feedback_texture_is_black_on_the_first_frame() {
    let mirror = preset(
        "mirror",
        r"
@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = position.xy / g.resolution;
    let seen = textureSample(previous, previous_sampler, uv).rgb;
    // `params[0].x` keeps the schema honest; it contributes nothing.
    return vec4<f32>(seen + vec3<f32>(g.params[0].x * 0.0), 1.0);
}",
        true,
    );

    let frame = render_sequence(&mirror, 1);

    assert!(
        frame.chunks_exact(4).all(|px| px[..3] == [0, 0, 0]),
        "frame 0 sampled something other than black from the feedback texture",
    );
}

/// Frame N samples frame N-1. A shader that adds a constant every frame must
/// therefore climb, and keep climbing: that is the whole of a trail effect.
#[test]
fn the_feedback_texture_carries_the_previous_frame() {
    let accumulate = preset(
        "accumulate",
        r"
@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = position.xy / g.resolution;
    let seen = textureSample(previous, previous_sampler, uv).rgb;
    return vec4<f32>(seen + vec3<f32>(g.params[0].x), 1.0);
}",
        true,
    );

    let one = red(&render_sequence(&accumulate, 1));
    let two = red(&render_sequence(&accumulate, 2));
    let three = red(&render_sequence(&accumulate, 3));

    assert!(one > 0, "frame 1 drew nothing at all");
    assert!(
        one < two && two < three,
        "the feedback texture never advances: frames read {one}, {two}, {three}",
    );
}

/// The binding is opt-in. A preset that does not declare `needs_feedback` gets a
/// bind group with the uniform alone, so a shader that reaches for `@binding(1)`
/// anyway must fail to build rather than sample a texture nobody bound.
#[test]
fn a_preset_that_does_not_ask_for_feedback_gets_no_binding() {
    let sneaky = preset(
        "sneaky",
        r"
@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = position.xy / g.resolution;
    return vec4<f32>(textureSample(previous, previous_sampler, uv).rgb + g.params[0].x, 1.0);
}",
        false,
    );

    let _device = one_device_at_a_time();
    let gpu = Gpu::new(AdapterChoice::Software).expect("the feedback tests need lavapipe");
    let visual = Layer::new(&gpu, WIDTH, HEIGHT, "visualizer");

    let err = Visualizer::new(&gpu, &sneaky, &visual)
        .expect_err("`needs_feedback` is false, so `@binding(1)` is bound to nothing");

    assert!(
        err.to_string().contains("sneaky"),
        "the error must name the preset: {err}",
    );
}

/// A preset that neither declares the flag nor reaches for the binding still
/// draws — the plumbing costs the presets that do not use it nothing.
#[test]
fn a_preset_without_feedback_still_renders() {
    let plain = preset(
        "plain",
        r"
@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    return vec4<f32>(vec3<f32>(g.params[0].x), 1.0);
}",
        false,
    );

    let frame = render_sequence(&plain, 2);

    assert!(red(&frame) > 0, "a preset with no feedback drew nothing");
}

/// `PackedParams` is what a schema resolves to, and the test presets above lean
/// on its shape. A change to `PARAM_SLOTS` that this file did not notice would
/// otherwise show up as a confusing shader failure.
#[test]
fn the_test_presets_pack_into_the_uniform_avz_ships() {
    let plain = preset("plain", "", false);

    assert_eq!(defaults(&plain).len(), PARAM_SLOTS);
    assert!((defaults(&plain)[0][0] - 0.1).abs() < f32::EPSILON);
}
