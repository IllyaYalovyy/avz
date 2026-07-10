//! The spectrum texture: the other binding a preset may opt into.
//!
//! `VISION.md` §6 lists a "spectrum texture (512×1)" beside the previous-frame
//! texture. A preset opts in with `"needs_spectrum": true` in its schema; the
//! renderer then binds the frame's coarse spectrum at `@binding(3)`, read with
//! `textureLoad` and no sampler.
//!
//! The presets below are built here rather than shipped, for the reason
//! `feedback_texture.rs` gives: that bucket `n` of the uniform's texture reaches
//! column `n` of the frame is a property of the renderer, and it would be
//! invisible through a shader that also draws ribbons.
//!
//! **Software adapter only**, like every other GPU test: `AGENTS.md` expects GPU
//! float differences across machines. Needs Mesa's software Vulkan driver:
//! `sudo dnf install mesa-vulkan-drivers`.

use std::sync::{Mutex, MutexGuard, PoisonError};

use avz_core::analysis::{FeatureFrame, SPECTRUM_BINS};
use avz_core::config::Palette;
use avz_core::render::{
    AdapterChoice, Compositor, Globals, Gpu, Layer, Offscreen, PackedParams, Preset, Visualizer,
    palette,
};

const WIDTH: u32 = 64;
const HEIGHT: u32 = 64;
const FPS: u32 = 30;
const SEED: u64 = 1337;

/// How many spectrum buckets one column of the test frame spans: 512 / 64.
const BUCKETS_PER_COLUMN: usize = SPECTRUM_BINS / WIDTH as usize;

/// See `pipeline_render.rs`: one Vulkan device per process, or the loader's
/// debug-utils terminator segfaults when two tests open devices at once.
static ONE_DEVICE_AT_A_TIME: Mutex<()> = Mutex::new(());

fn one_device_at_a_time() -> MutexGuard<'static, ()> {
    ONE_DEVICE_AT_A_TIME
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// The `VISION.md` §6 uniform contract, the fullscreen triangle, and both
/// optional bindings, so a test shader below is only its fragment stage.
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
@group(0) @binding(3) var spectrum: texture_2d<f32>;
";

/// The shader most tests here use: paint each column of the frame with the
/// bucket that column stands for, so the frame *is* the spectrum row.
const SHOW_SPECTRUM: &str = r"
@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let buckets = i32(textureDimensions(spectrum).x);
    let column = i32(position.x);
    let bucket = column * buckets / i32(g.resolution.x);
    let value = textureLoad(spectrum, vec2<i32>(bucket, 0), 0).r;
    return vec4<f32>(vec3<f32>(value * g.params[0].x), 1.0);
}";

/// A preset assembled in the test, from `PREAMBLE` plus one fragment stage.
///
/// `Preset`'s fields are the two embedded files and two strings, so a test can
/// build one the registry never names — which is what lets the renderer's own
/// contract be asserted without a shipped shader in the way.
fn preset(
    name: &'static str,
    fragment: &str,
    needs_spectrum: bool,
    needs_feedback: bool,
) -> Preset {
    let source: &'static str = Box::leak(format!("{PREAMBLE}\n{fragment}").into_boxed_str());
    let schema: &'static str = Box::leak(
        format!(
            r#"{{"needs_spectrum": {needs_spectrum}, "needs_feedback": {needs_feedback},
               "params": [
                 {{"name":"gain","type":"float","default":1.0,"min":0.0,"max":1.0,
                   "slot":[0,0],"description":"How brightly a bucket reads."}}
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

/// A silent spectrum with `bucket` alone at full scale.
fn one_hot(bucket: usize) -> Vec<f32> {
    let mut bins = vec![0.0; SPECTRUM_BINS];
    bins[bucket] = 1.0;
    bins
}

/// Draw one frame of `preset` per row of `spectra`, and read the last one back.
///
/// One visualizer across the whole sequence, exactly as `pipeline::render` does
/// it, so a test can watch the texture change between frames.
///
/// The visualizer layer is composited alone, over no background at all, so what
/// comes back is the premultiplied light the preset wrote and nothing else. The
/// shaders here all return alpha 1.0, so that is their color, unveiled.
fn render_sequence(preset: &Preset, spectra: &[Vec<f32>]) -> Vec<u8> {
    let _device = one_device_at_a_time();
    let gpu = Gpu::new(AdapterChoice::Software)
        .expect("the spectrum tests need lavapipe: `sudo dnf install mesa-vulkan-drivers`");
    let target = Offscreen::new(&gpu, WIDTH, HEIGHT).expect("a 64x64 frame");
    let visual = Layer::new(&gpu, WIDTH, HEIGHT, "visualizer");
    let visualizer = Visualizer::new(&gpu, preset, &visual).expect("the test preset compiles");
    let compositor = Compositor::new(&gpu, &[&visual]).expect("one frame-sized layer");

    let colors = palette::resolve(&Palette::Named("ember".to_owned())).expect("`ember` ships");
    let params = defaults(preset);

    for (index, spectrum) in spectra.iter().enumerate() {
        let globals = Globals::for_frame(
            index,
            FPS,
            (WIDTH, HEIGHT),
            SEED,
            FeatureFrame::default(),
            colors,
            params,
        );
        visualizer.draw(&gpu, &visual, &globals, spectrum);
    }
    compositor.composite(&gpu, &target);

    target.read_rgba(&gpu).expect("the frame reads back")
}

/// The red channel of the top row of the frame, one byte per column.
///
/// Every shader here paints each column one color from top to bottom, so the top
/// row is the frame.
fn columns(frame: &[u8]) -> Vec<u8> {
    frame
        .chunks_exact(4)
        .take(WIDTH as usize)
        .map(|pixel| pixel[0])
        .collect()
}

/// The point of the binding: bucket `n` of the analysis reaches the shader as
/// texel `n`, and nothing smears it into its neighbours. A texture uploaded a
/// row short, in the wrong byte order, or filtered on the way in fails here.
#[test]
fn a_hot_bucket_lights_the_column_that_reads_it_and_no_other() {
    let show = preset("show", SHOW_SPECTRUM, true, false);

    // Column 20 reads bucket 160, which is the one bucket lit.
    const COLUMN: usize = 20;
    let frame = render_sequence(&show, &[one_hot(COLUMN * BUCKETS_PER_COLUMN)]);

    let columns = columns(&frame);
    assert_eq!(
        columns[COLUMN], 255,
        "the hot bucket did not reach column 20"
    );
    for (column, value) in columns.iter().enumerate() {
        if column != COLUMN {
            assert_eq!(
                *value, 0,
                "column {column} lit up from a bucket that is silent",
            );
        }
    }
}

/// The texture is this frame's spectrum, not the render's first one. A preset
/// handed a spectrum once and never again would draw a still ribbon over a
/// moving song.
#[test]
fn the_texture_carries_the_spectrum_of_the_frame_being_drawn() {
    let show = preset("show", SHOW_SPECTRUM, true, false);

    let first = one_hot(8 * BUCKETS_PER_COLUMN);
    let second = one_hot(40 * BUCKETS_PER_COLUMN);

    let alone = columns(&render_sequence(&show, std::slice::from_ref(&second)));
    let after = columns(&render_sequence(&show, &[first, second]));

    assert_eq!(
        alone, after,
        "frame 1 drew frame 0's spectrum: the upload happens once per render",
    );
    assert_eq!(after[40], 255);
    assert_eq!(after[8], 0, "frame 0's bucket is still lit on frame 1");
}

/// Silence is silence: a frame with no spectral energy draws no light, rather
/// than whatever the driver left in the texture's memory.
#[test]
fn a_silent_spectrum_draws_a_black_frame() {
    let show = preset("show", SHOW_SPECTRUM, true, false);

    let frame = render_sequence(&show, &[vec![0.0; SPECTRUM_BINS]]);

    assert!(
        frame.chunks_exact(4).all(|pixel| pixel[..3] == [0, 0, 0]),
        "a silent spectrum drew light",
    );
}

/// The binding is opt-in. A preset that does not declare `needs_spectrum` gets a
/// bind group without it, so a shader that reaches for `@binding(3)` anyway must
/// fail to build rather than read a texture nobody bound.
#[test]
fn a_preset_that_does_not_ask_for_the_spectrum_gets_no_binding() {
    let sneaky = preset("sneaky", SHOW_SPECTRUM, false, false);

    let _device = one_device_at_a_time();
    let gpu = Gpu::new(AdapterChoice::Software).expect("the spectrum tests need lavapipe");
    let visual = Layer::new(&gpu, WIDTH, HEIGHT, "visualizer");

    let err = Visualizer::new(&gpu, &sneaky, &visual)
        .expect_err("`needs_spectrum` is false, so `@binding(3)` is bound to nothing");

    assert!(
        err.to_string().contains("sneaky"),
        "the error must name the preset: {err}",
    );
}

/// The two optional bindings are independent, and their binding numbers do not
/// move when only one of them is asked for. A preset that wanted a spectrum and
/// a trail — `ink` may yet — must find both where the contract says they are.
#[test]
fn a_preset_may_ask_for_the_spectrum_and_the_previous_frame_together() {
    let both = preset(
        "both",
        r"
@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = position.xy / g.resolution;
    let trail = textureSample(previous, previous_sampler, uv).r;
    let buckets = i32(textureDimensions(spectrum).x);
    let bucket = i32(position.x) * buckets / i32(g.resolution.x);
    let value = textureLoad(spectrum, vec2<i32>(bucket, 0), 0).r;
    return vec4<f32>(vec3<f32>(value * g.params[0].x * 0.5 + trail * 0.5), 1.0);
}",
        true,
        true,
    );

    const COLUMN: usize = 30;
    let hot = one_hot(COLUMN * BUCKETS_PER_COLUMN);

    let one = columns(&render_sequence(&both, std::slice::from_ref(&hot)))[COLUMN];
    let two = columns(&render_sequence(&both, &[hot.clone(), hot]))[COLUMN];

    assert!(one > 0, "the spectrum never reached the pixel");
    assert!(
        two > one,
        "the trail never accumulated: {one} then {two}. Asking for the spectrum \
         moved the feedback binding out from under the shader.",
    );
}

/// A preset that asks for neither binding still draws: the plumbing costs the
/// presets that do not use it nothing.
#[test]
fn a_preset_that_asks_for_no_texture_still_draws() {
    let plain = preset(
        "plain",
        r"
@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    return vec4<f32>(vec3<f32>(g.params[0].x), 1.0);
}",
        false,
        false,
    );

    let frame = render_sequence(&plain, &[Vec::new()]);

    assert!(
        frame.chunks_exact(4).all(|pixel| pixel[0] == 255),
        "a preset with no optional bindings failed to draw",
    );
}
