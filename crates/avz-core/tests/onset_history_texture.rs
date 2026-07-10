//! The onset-history texture: the third binding a preset may opt into.
//!
//! `VISION.md` §6 lists it beside the previous-frame and spectrum textures. A
//! preset opts in with `"needs_onsets": true` in its schema; the renderer then
//! binds the last [`ONSET_SLOTS`] hits at or before the frame being drawn to
//! `@binding(4)`, newest first, each slot a `(birth, ordinal)` pair read with
//! `textureLoad` and no sampler.
//!
//! The presets below are built here rather than shipped, for the reason
//! `spectrum_texture.rs` gives: that slot `n` of the history reaches column `n`
//! of the frame is a property of the *renderer*, and it would be invisible
//! through a shader that also draws particles.
//!
//! **Software adapter only**, like every other GPU test: `AGENTS.md` expects GPU
//! float differences across machines. Needs Mesa's software Vulkan driver:
//! `sudo dnf install mesa-vulkan-drivers`.

use std::sync::{Mutex, MutexGuard, PoisonError};

use avz_core::analysis::{EMPTY_HISTORY, FeatureFrame, NO_ONSET, NO_ORDINAL, ONSET_SLOTS};
use avz_core::config::Palette;
use avz_core::render::{
    AdapterChoice, Compositor, Globals, Gpu, Layer, Offscreen, PackedParams, Preset, Visualizer,
    palette,
};

/// One column per slot, so a column of the frame *is* a slot of the history.
const WIDTH: u32 = ONSET_SLOTS as u32;
const HEIGHT: u32 = 8;
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

/// The `VISION.md` §6 uniform contract, the fullscreen triangle, and all three
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
@group(0) @binding(4) var onsets: texture_2d<f32>;
";

/// Paint column `n` with slot `n`'s birth time in red and its ordinal in green,
/// each scaled by a known constant, so the frame *is* the history row.
///
/// The scale keeps both inside the `0..1` a `Rgba8UnormSrgb` target stores. An
/// unfilled slot's `NO_ONSET` is negative and clamps to zero on write, which the
/// tests below read as "no hit here".
const SHOW_ONSETS: &str = r"
@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let slot = i32(position.x);
    let hit = textureLoad(onsets, vec2<i32>(slot, 0), 0).xy;
    return vec4<f32>(hit.x * g.params[0].x, hit.y * g.params[0].x, 0.0, 1.0);
}";

/// A preset assembled in the test, from `PREAMBLE` plus one fragment stage.
fn preset(name: &'static str, fragment: &str, needs_onsets: bool, needs_feedback: bool) -> Preset {
    let source: &'static str = Box::leak(format!("{PREAMBLE}\n{fragment}").into_boxed_str());
    let schema: &'static str = Box::leak(
        format!(
            r#"{{"needs_onsets": {needs_onsets}, "needs_feedback": {needs_feedback},
               "params": [
                 {{"name":"scale","type":"float","default":0.1,"min":0.0,"max":1.0,
                   "slot":[0,0],"description":"How a slot's value maps onto 0..1."}}
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

/// A history with `hits` in it, newest first, each `(birth, ordinal)`.
fn history(hits: &[(f32, f32)]) -> Vec<f32> {
    let mut row = EMPTY_HISTORY;
    for (slot, &(birth, ordinal)) in hits.iter().enumerate() {
        row[slot * 2] = birth;
        row[slot * 2 + 1] = ordinal;
    }
    row.to_vec()
}

/// Draw one frame of `preset` per row of `histories`, and read the last one back.
///
/// One visualizer across the whole sequence, exactly as `pipeline::render` does
/// it, so a test can watch the texture change between frames. The visualizer
/// layer is composited alone, over no background, so what comes back is the
/// premultiplied light the preset wrote and nothing else.
fn render_sequence(preset: &Preset, histories: &[Vec<f32>]) -> Vec<u8> {
    let _device = one_device_at_a_time();
    let gpu = Gpu::new(AdapterChoice::Software)
        .expect("the onset tests need lavapipe: `sudo dnf install mesa-vulkan-drivers`");
    let target = Offscreen::new(&gpu, WIDTH, HEIGHT).expect("a 64x8 frame");
    let visual = Layer::new(&gpu, WIDTH, HEIGHT, "visualizer");
    let visualizer = Visualizer::new(&gpu, preset, &visual).expect("the test preset compiles");
    let compositor = Compositor::new(&gpu, &[&visual]).expect("one frame-sized layer");

    let colors = palette::resolve(&Palette::Named("ember".to_owned())).expect("`ember` ships");
    let params = defaults(preset);

    for (index, row) in histories.iter().enumerate() {
        let globals = Globals::for_frame(
            index,
            FPS,
            (WIDTH, HEIGHT),
            SEED,
            FeatureFrame::default(),
            colors,
            params,
        );
        visualizer.draw(&gpu, &visual, &globals, &[], row);
    }
    compositor.composite(&gpu, &target);

    target.read_rgba(&gpu).expect("the frame reads back")
}

/// The red and green of the top row of the frame, one pair per column.
///
/// Every shader here paints each column one color from top to bottom, so the top
/// row is the frame.
fn columns(frame: &[u8]) -> Vec<(u8, u8)> {
    frame
        .chunks_exact(4)
        .take(WIDTH as usize)
        .map(|pixel| (pixel[0], pixel[1]))
        .collect()
}

/// The point of the binding: slot `n` of the history reaches the shader as texel
/// `n`, both channels intact and in the right order. A texture uploaded with the
/// channels swapped, a row short, or in the wrong byte order fails here.
#[test]
fn every_slot_reaches_the_texel_that_reads_it_with_both_channels_intact() {
    let show = preset("show", SHOW_ONSETS, true, false);

    // `scale` is 0.1, so a birth of 5.0 s reads 0.5 and an ordinal of 3 reads
    // 0.3 — values an 8-bit sRGB target holds apart from their neighbours.
    let hits = [(5.0, 3.0), (2.0, 2.0), (1.0, 1.0)];
    let frame = render_sequence(&show, &[history(&hits)]);
    let columns = columns(&frame);

    // sRGB encodes on write, so the bytes are not `value * 255`. What the test
    // needs is that the three slots are distinct, ordered, and non-zero, and
    // that every slot behind them is the sentinel's clamped zero.
    for (slot, (birth, ordinal)) in hits.iter().enumerate() {
        let (red, green) = columns[slot];
        assert!(red > 0, "slot {slot} lost its birth of {birth}");
        assert!(green > 0, "slot {slot} lost its ordinal of {ordinal}");
    }
    assert!(
        columns[0].0 > columns[1].0 && columns[1].0 > columns[2].0,
        "the births did not arrive in slot order: {:?}",
        &columns[..3],
    );
    assert!(
        columns[0].1 > columns[1].1 && columns[1].1 > columns[2].1,
        "the ordinals did not arrive in slot order: {:?}",
        &columns[..3],
    );

    for (slot, &(red, green)) in columns.iter().enumerate().skip(hits.len()) {
        assert_eq!(
            (red, green),
            (0, 0),
            "slot {slot} holds a hit nobody played"
        );
    }
}

/// A slot no hit has reached yet reads the sentinel, not zero. A zeroed slot
/// would claim a hit at time zero, and every burst preset would open its render
/// with an explosion the song never played.
#[test]
fn an_unfilled_slot_reads_the_sentinel_and_not_a_hit_at_time_zero() {
    let sentinel = preset(
        "sentinel",
        // Paint red where slot 0's birth is the sentinel, green where it is not.
        r"
@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let hit = textureLoad(onsets, vec2<i32>(0, 0), 0).xy;
    let empty = hit.x < -1.0 && hit.y < 0.0;
    return vec4<f32>(select(0.0, 1.0, empty), select(1.0, 0.0, empty), 0.0, 1.0);
}",
        true,
        false,
    );

    let frame = render_sequence(&sentinel, &[history(&[])]);

    assert_eq!(
        columns(&frame)[0],
        (255, 0),
        "an empty history did not read `NO_ONSET` and `NO_ORDINAL` \
         ({NO_ONSET}, {NO_ORDINAL})",
    );
}

/// The texture holds this frame's window, not the render's first one. A preset
/// handed the history once and never again would keep re-drawing one burst.
#[test]
fn the_texture_carries_the_history_of_the_frame_being_drawn() {
    let show = preset("show", SHOW_ONSETS, true, false);

    let first = history(&[(1.0, 0.0)]);
    let second = history(&[(6.0, 1.0), (1.0, 0.0)]);

    let alone = columns(&render_sequence(&show, std::slice::from_ref(&second)));
    let after = columns(&render_sequence(&show, &[first, second]));

    assert_eq!(
        alone, after,
        "frame 1 drew frame 0's history: the upload happens once per render",
    );
    assert!(after[1].0 > 0, "the older hit did not slide into slot 1");
}

/// The binding is opt-in. A preset that does not declare `needs_onsets` gets a
/// bind group without it, so a shader that reaches for `@binding(4)` anyway must
/// fail to build rather than read a texture nobody bound.
#[test]
fn a_preset_that_does_not_ask_for_the_onsets_gets_no_binding() {
    let sneaky = preset("sneaky", SHOW_ONSETS, false, false);

    let _device = one_device_at_a_time();
    let gpu = Gpu::new(AdapterChoice::Software).expect("the onset tests need lavapipe");
    let visual = Layer::new(&gpu, WIDTH, HEIGHT, "visualizer");

    let err = Visualizer::new(&gpu, &sneaky, &visual)
        .expect_err("`needs_onsets` is false, so `@binding(4)` is bound to nothing");

    assert!(
        err.to_string().contains("sneaky"),
        "the error must name the preset: {err}",
    );
}

/// The three optional bindings are independent, and their binding numbers do not
/// move when only some of them are asked for. A preset that wanted the hits and
/// a trail must find both where the contract says they are.
#[test]
fn a_preset_may_ask_for_the_onsets_and_the_previous_frame_together() {
    let both = preset(
        "both",
        r"
@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = position.xy / g.resolution;
    let trail = textureSample(previous, previous_sampler, uv).r;
    let hit = textureLoad(onsets, vec2<i32>(0, 0), 0).x;
    return vec4<f32>(hit * g.params[0].x * 0.5 + trail * 0.5, 0.0, 0.0, 1.0);
}",
        true,
        true,
    );

    let hit = history(&[(4.0, 0.0)]);

    let one = columns(&render_sequence(&both, std::slice::from_ref(&hit)))[0].0;
    let two = columns(&render_sequence(&both, &[hit.clone(), hit]))[0].0;

    assert!(one > 0, "the hit never reached the pixel");
    assert!(
        two > one,
        "the trail never accumulated: {one} then {two}. Asking for the onsets \
         moved the feedback binding out from under the shader.",
    );
}
