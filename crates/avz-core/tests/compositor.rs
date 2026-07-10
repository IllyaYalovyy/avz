//! The layer stack: background, visualizer, text, composited premultiplied.
//!
//! `VISION.md` §5.3 stacks three layers into one final pass. The properties
//! under test belong to the compositor rather than to any preset: that the blend
//! is `src + (1 - src.a) * dst` on premultiplied colors, that a layer nobody
//! handed in is not drawn as a black quad, that the bottom of the slice ends up
//! at the bottom of the picture, and that the default backdrop is the vertical
//! palette gradient the shaders no longer paint for themselves.
//!
//! **Software adapter only**, like every other GPU test: GPU float differences
//! across machines are expected everywhere but in a test (`docs/TESTING.md`).
//! Needs Mesa's software Vulkan driver: `sudo dnf install mesa-vulkan-drivers`.

use std::sync::{Mutex, MutexGuard, PoisonError};

use avz_core::analysis::FeatureFrame;
use avz_core::config::Palette;
use avz_core::render::{
    AdapterChoice, Backdrop, Compositor, Globals, Gpu, Layer, LinearPalette, Offscreen, Preset,
    Visualizer, palette,
};

/// 64 × 4 B = 256 B per row, so nothing here can trip over readback padding —
/// `offscreen_readback.rs` owns that risk.
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

fn software_gpu() -> Gpu {
    Gpu::new(AdapterChoice::Software)
        .expect("the compositor tests need lavapipe: `sudo dnf install mesa-vulkan-drivers`")
}

fn ember() -> LinearPalette {
    palette::resolve(&Palette::Named("ember".to_owned())).expect("`ember` ships")
}

/// The sRGB opto-electronic transfer function: light in, the byte the frame
/// stores out. The renderer does this in the driver; the expectations below do
/// it here, so a wrong blend cannot be blessed by a wrong expectation.
fn linear_to_srgb(light: f32) -> u8 {
    let encoded = if light <= 0.003_130_8 {
        light * 12.92
    } else {
        1.055 * light.powf(1.0 / 2.4) - 0.055
    };
    (encoded * 255.0).round().clamp(0.0, 255.0) as u8
}

/// The premultiplied-alpha `over` operator, on linear light.
///
/// Written out rather than imported: this is the reference the GPU is held to.
fn over(src: [f32; 4], dst: [f32; 4]) -> [f32; 4] {
    std::array::from_fn(|channel| src[channel] + (1.0 - src[3]) * dst[channel])
}

/// `over` as the four bytes the frame stores. Alpha is stored linearly even in
/// an sRGB texture, so only the color channels are encoded.
fn expected_bytes(src: [f32; 4], dst: [f32; 4]) -> [u8; 4] {
    let blended = over(src, dst);
    [
        linear_to_srgb(blended[0]),
        linear_to_srgb(blended[1]),
        linear_to_srgb(blended[2]),
        (blended[3] * 255.0).round().clamp(0.0, 255.0) as u8,
    ]
}

/// An 8-bit encode of one blend, then a decode by the reader, costs at most a
/// byte each way. A wrong blend factor misses by tens.
const TOLERANCE: i32 = 2;

fn assert_close(actual: [u8; 4], expected: [u8; 4], what: &str) {
    for channel in 0..4 {
        let drift = i32::from(actual[channel]) - i32::from(expected[channel]);
        assert!(
            drift.abs() <= TOLERANCE,
            "{what}: composited {actual:?}, expected about {expected:?}",
        );
    }
}

/// The top-left pixel of a frame. Every layer below is one flat color, so one
/// pixel is the frame.
fn first_pixel(frame: &[u8]) -> [u8; 4] {
    [frame[0], frame[1], frame[2], frame[3]]
}

fn pixel_at(frame: &[u8], x: u32, y: u32) -> [u8; 4] {
    let at = ((y * WIDTH + x) * 4) as usize;
    [frame[at], frame[at + 1], frame[at + 2], frame[at + 3]]
}

/// Composite `layers`, bottom first, and read the frame back.
fn composite(gpu: &Gpu, layers: &[&Layer]) -> Vec<u8> {
    let target = Offscreen::new(gpu, WIDTH, HEIGHT).expect("a 64x64 frame");
    let compositor = Compositor::new(gpu, layers).expect("the layers are all frame-sized");

    assert_eq!(
        compositor.layers(),
        layers.len(),
        "the compositor draws one quad per layer it was handed",
    );

    compositor.composite(gpu, &target);
    target.read_rgba(gpu).expect("the frame reads back")
}

fn filled(gpu: &Gpu, label: &str, premultiplied: [f32; 4]) -> Layer {
    let layer = Layer::new(gpu, WIDTH, HEIGHT, label);
    layer.clear(gpu, premultiplied);
    layer
}

/// The three cases the `over` operator has to get right, against pixels computed
/// by hand rather than by the GPU: an opaque layer replaces what is under it, a
/// half-covering one lets half of it through, and a transparent one is invisible.
///
/// A non-premultiplied blend (`SrcAlpha`, `OneMinusSrcAlpha`) passes the first
/// and third and fails the second by a factor of two on the color channels,
/// which is exactly the bug this exists to catch.
#[test]
fn premultiply_blend_math_matches_reference() {
    const BACKGROUND: [f32; 4] = [0.25, 0.25, 0.25, 1.0];

    let _device = one_device_at_a_time();
    let gpu = software_gpu();
    let background = filled(&gpu, "background", BACKGROUND);

    // Premultiplied, so the color channels are already scaled by alpha: half-
    // covering white is `[0.5, 0.5, 0.5, 0.5]`, not `[1.0, 1.0, 1.0, 0.5]`.
    let cases: [(&str, [f32; 4]); 3] = [
        ("opaque red over grey", [1.0, 0.0, 0.0, 1.0]),
        ("half-covering white over grey", [0.5, 0.5, 0.5, 0.5]),
        ("transparent over grey", [0.0, 0.0, 0.0, 0.0]),
    ];

    for (what, top) in cases {
        let layer = filled(&gpu, what, top);
        let frame = composite(&gpu, &[&background, &layer]);

        assert_close(first_pixel(&frame), expected_bytes(top, BACKGROUND), what);
    }
}

/// Bottom of the slice, bottom of the picture.
#[test]
fn layers_composite_bottom_to_top() {
    let _device = one_device_at_a_time();
    let gpu = software_gpu();

    let red = filled(&gpu, "red", [1.0, 0.0, 0.0, 1.0]);
    let green = filled(&gpu, "green", [0.0, 1.0, 0.0, 1.0]);

    assert_eq!(
        first_pixel(&composite(&gpu, &[&red, &green])),
        [0, 255, 0, 255],
        "the last layer is the top one",
    );
    assert_eq!(
        first_pixel(&composite(&gpu, &[&green, &red])),
        [255, 0, 0, 255],
        "swapping the slice swaps the picture",
    );
}

/// A layer nobody handed in costs nothing: no bind group, no draw, and above all
/// no opaque black quad standing in for it.
///
/// The probe is the alpha channel. A half-covering white layer over *nothing*
/// composites to alpha 128; the same layer over a black opaque stand-in
/// composites to alpha 255 with the very same color channels, so only alpha can
/// tell the two apart.
#[test]
fn absent_layers_skip_render_passes() {
    let _device = one_device_at_a_time();
    let gpu = software_gpu();

    let half = filled(&gpu, "half", [0.5, 0.5, 0.5, 0.5]);

    let alone = composite(&gpu, &[&half]);
    assert_close(
        first_pixel(&alone),
        expected_bytes([0.5, 0.5, 0.5, 0.5], [0.0, 0.0, 0.0, 0.0]),
        "a lone layer composites over nothing, not over a black quad",
    );

    // And the empty stack draws nothing at all: the target is cleared and left.
    let empty = composite(&gpu, &[]);
    assert!(
        empty.chunks_exact(4).all(|pixel| pixel == [0, 0, 0, 0]),
        "an empty layer stack drew something",
    );
}

/// Every layer covers the whole frame, and a stack of mismatched layers is a
/// renderer bug worth a message rather than a sheared picture.
#[test]
fn a_layer_that_is_not_frame_sized_is_rejected() {
    let _device = one_device_at_a_time();
    let gpu = software_gpu();

    let frame = Layer::new(&gpu, WIDTH, HEIGHT, "frame");
    let smaller = Layer::new(&gpu, WIDTH / 2, HEIGHT, "smaller");

    let err = Compositor::new(&gpu, &[&frame, &smaller])
        .expect_err("two layer sizes cannot both be the frame");

    assert!(
        err.to_string().contains("smaller"),
        "the error must name the layer: {err}",
    );
}

/// The default backdrop is a vertical gradient between palette slots 0 and 1,
/// which is what "the shader clears to black" turned into (`VISION.md` §5.3).
#[test]
fn the_default_backdrop_is_a_vertical_gradient_across_palette_slots_zero_and_one() {
    let _device = one_device_at_a_time();
    let gpu = software_gpu();
    let colors = ember();

    let gradient = Backdrop::Gradient.layer(&gpu, WIDTH, HEIGHT, colors);
    let frame = composite(&gpu, &[&gradient]);

    let top = pixel_at(&frame, 0, 0);
    let bottom = pixel_at(&frame, 0, HEIGHT - 1);
    let middle = pixel_at(&frame, 0, HEIGHT / 2);

    assert_close(top, srgb_of(colors[0]), "the top row is palette slot 0");
    assert_close(
        bottom,
        srgb_of(colors[1]),
        "the bottom row is palette slot 1",
    );
    assert!(
        (top[0] < middle[0] && middle[0] < bottom[0])
            || (top[0] > middle[0] && middle[0] > bottom[0]),
        "the gradient is not monotone: {top:?} .. {middle:?} .. {bottom:?}",
    );

    // Every column is the same: the gradient runs down the frame, not across it.
    for y in 0..HEIGHT {
        assert_eq!(
            pixel_at(&frame, 0, y),
            pixel_at(&frame, WIDTH - 1, y),
            "row {y} is not flat: the gradient runs the wrong way",
        );
    }
}

/// The other backdrop `VISION.md` §5.3 names: one flat color, palette slot 0.
#[test]
fn a_solid_backdrop_is_palette_slot_zero_everywhere() {
    let _device = one_device_at_a_time();
    let gpu = software_gpu();
    let colors = ember();

    let solid = Backdrop::Solid.layer(&gpu, WIDTH, HEIGHT, colors);
    let frame = composite(&gpu, &[&solid]);

    let expected = srgb_of(colors[0]);
    for pixel in frame.chunks_exact(4) {
        assert_close([pixel[0], pixel[1], pixel[2], pixel[3]], expected, "solid");
    }
}

/// A linear-space palette color, as the bytes an opaque frame stores it in.
fn srgb_of(color: [f32; 4]) -> [u8; 4] {
    [
        linear_to_srgb(color[0]),
        linear_to_srgb(color[1]),
        linear_to_srgb(color[2]),
        255,
    ]
}

/// The whole point of the premultiplied visualizer layer: where the preset draws
/// no light it draws no alpha, and the backdrop comes through untouched.
///
/// `pulse` scales its every term by `rms_env`, so a silent frame is a frame it
/// contributes nothing to. Byte-exact, not approximate: `src + (1 - 0) * dst` is
/// the destination, unmodified.
#[test]
fn visualizer_alpha_zero_shows_background_exactly() {
    let preset = Preset::by_name("pulse").expect("pulse ships");
    let params = preset
        .schema()
        .expect("the shipped schema parses")
        .resolve(&toml::Table::new())
        .expect("the shipped defaults pack");

    let _device = one_device_at_a_time();
    let gpu = software_gpu();
    let colors = ember();

    let backdrop = Backdrop::Gradient.layer(&gpu, WIDTH, HEIGHT, colors);
    let visual = Layer::new(&gpu, WIDTH, HEIGHT, "visualizer");
    let visualizer = Visualizer::new(&gpu, preset, &visual).expect("pulse compiles");

    // Silence: every envelope, the flux, and the onset are zero.
    let globals = Globals::for_frame(
        30,
        FPS,
        (WIDTH, HEIGHT),
        SEED,
        FeatureFrame::default(),
        colors,
        params,
    );
    visualizer.draw(&gpu, &visual, &globals, &[]);

    assert_eq!(
        composite(&gpu, &[&backdrop, &visual]),
        composite(&gpu, &[&backdrop]),
        "a silent `pulse` frame is not transparent: it veils the backdrop",
    );
}

/// And when it does draw, it draws: the same stack on a loud frame is not the
/// backdrop. Without this the test above would pass on a visualizer layer that
/// never rendered at all.
#[test]
fn a_loud_visualizer_frame_covers_the_background() {
    let preset = Preset::by_name("pulse").expect("pulse ships");
    let params = preset
        .schema()
        .expect("the shipped schema parses")
        .resolve(&toml::Table::new())
        .expect("the shipped defaults pack");

    let _device = one_device_at_a_time();
    let gpu = software_gpu();
    let colors = ember();

    let backdrop = Backdrop::Gradient.layer(&gpu, WIDTH, HEIGHT, colors);
    let visual = Layer::new(&gpu, WIDTH, HEIGHT, "visualizer");
    let visualizer = Visualizer::new(&gpu, preset, &visual).expect("pulse compiles");

    let globals = Globals::for_frame(
        30,
        FPS,
        (WIDTH, HEIGHT),
        SEED,
        FeatureFrame {
            rms_env: 1.0,
            bass_env: 1.0,
            mid_env: 1.0,
            centroid: 0.5,
            ..FeatureFrame::default()
        },
        colors,
        params,
    );
    visualizer.draw(&gpu, &visual, &globals, &[]);

    assert_ne!(
        composite(&gpu, &[&backdrop, &visual]),
        composite(&gpu, &[&backdrop]),
        "a loud `pulse` frame reached no pixel",
    );
}
