//! The effects stage, observed in pixels (RFC-002, issue #54).
//!
//! The matrix arithmetic is unit-tested in `render::effects`; these tests
//! prove the *pass* — that the shader really samples where the UV matrix
//! says and really multiplies through the color matrix, on lavapipe, through
//! the same `Compositor → EffectsPass → Offscreen` path the pipeline runs.
//!
//! The scene is the default palette backdrop: a vertical gradient, which is
//! enough structure to see zoom and rotation without any preset involved.

use std::sync::{Mutex, MutexGuard, PoisonError};

use avz_core::analysis::FeatureFrame;
use avz_core::config::{Effects, Palette};
use avz_core::render::{
    AdapterChoice, Backdrop, ClipTime, Compositor, EffectsPass, Gpu, Layer, Offscreen, palette,
};

const WIDTH: u32 = 320;
const HEIGHT: u32 = 180;

/// A clip with no length: `fade_gain` reads it as "nothing to fade", which is
/// what every test that is not about the fade wants.
const NO_FADE: ClipTime = ClipTime {
    elapsed: 0.0,
    duration: 0.0,
};

/// One Vulkan device at a time, as `pipeline_render.rs` serializes.
static DEVICE: Mutex<()> = Mutex::new(());

fn one_device_at_a_time() -> MutexGuard<'static, ()> {
    DEVICE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Render the default gradient backdrop, through the effects pass when one is
/// given, at frame time 0 and with no fade.
fn rendered(effects: Option<&Effects>) -> Vec<u8> {
    rendered_at(effects, 0.0, NO_FADE)
}

/// The same, at a chosen song time and clip position.
fn rendered_at(effects: Option<&Effects>, time: f32, clip: ClipTime) -> Vec<u8> {
    let gpu = Gpu::new(AdapterChoice::Software)
        .expect("effects tests need lavapipe: `sudo dnf install mesa-vulkan-drivers`");

    let colors = palette::resolve(&Palette::Named("ember".to_owned())).expect("`ember` ships");
    let backdrop = Backdrop::default().layer(&gpu, WIDTH, HEIGHT, colors);
    let compositor = Compositor::new(&gpu, &[&backdrop]).expect("one layer composites");
    let target = Offscreen::new(&gpu, WIDTH, HEIGHT).expect("a 320x180 frame");

    match effects {
        None => compositor.composite(&gpu, &target),
        Some(effects) => {
            let flat = Layer::new(&gpu, WIDTH, HEIGHT, "flattened");
            let pass = EffectsPass::new(&gpu, &flat).expect("the effects pass builds");
            compositor.composite_into(&gpu, &flat);
            pass.apply(&gpu, &target, effects, &FeatureFrame::default(), time, clip);
        }
    }

    target.read_rgba(&gpu).expect("the frame reads back")
}

fn pixel(frame: &[u8], x: u32, y: u32) -> [u8; 4] {
    let at = ((y * WIDTH + x) * 4) as usize;
    [frame[at], frame[at + 1], frame[at + 2], frame[at + 3]]
}

/// sRGB byte to linear light, the same curve the shader samples through.
fn linear(byte: u8) -> f32 {
    let c = byte as f32 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// RFC-002 G3: the identity config routed *through the pass* reproduces the
/// plain composite byte for byte — resampling at an identity UV matrix and
/// multiplying by an identity color matrix must lose nothing.
#[test]
fn the_identity_config_is_byte_identical_through_the_pass() {
    let _device = one_device_at_a_time();
    let plain = rendered(None);
    let through = rendered(Some(&Effects::default()));
    assert_eq!(plain, through, "the identity pass repainted the frame");
}

/// Brightness doubles the light of every pixel, in linear terms, up to
/// clamping and a byte of rounding on each side of the sRGB round trip.
#[test]
fn brightness_doubles_the_linear_light() {
    let _device = one_device_at_a_time();
    let plain = rendered(None);
    let effects = Effects {
        brightness: 2.0,
        ..Effects::default()
    };
    let brightened = rendered(Some(&effects));

    for (x, y) in [
        (WIDTH / 2, HEIGHT / 4),
        (WIDTH / 4, HEIGHT / 2),
        (10, HEIGHT - 10),
    ] {
        let before = pixel(&plain, x, y);
        let after = pixel(&brightened, x, y);
        for channel in 0..3 {
            let expected = (linear(before[channel]) * 2.0).min(1.0);
            let got = linear(after[channel]);
            assert!(
                (expected - got).abs() < 0.02,
                "({x},{y}) channel {channel}: expected ~{expected}, got {got}"
            );
        }
    }
}

/// A quarter turn carries the backdrop's vertical gradient onto the
/// horizontal axis: rows become uniform, columns become the gradient.
#[test]
fn a_quarter_turn_turns_the_gradient_sideways() {
    let _device = one_device_at_a_time();
    let effects = Effects {
        spin: 0.25,
        ..Effects::default()
    };
    // time = 0 would be no turn at all; the pass is applied at time 1s.
    let gpu_frame = rendered_at(Some(&effects), 1.0, NO_FADE);

    // A rotation in aspect-true units is a true rotation in *pixels*: a step
    // along the row samples the source a step along its column. So after a
    // quarter turn the gradient runs along the rows, and the columns — which
    // sample across the source's uniform rows — agree with themselves.
    let mid = HEIGHT / 2;
    let up = pixel(&gpu_frame, WIDTH / 2, mid - 30);
    let down = pixel(&gpu_frame, WIDTH / 2, mid + 30);
    let column_spread: i32 = (0..3).map(|c| (up[c] as i32 - down[c] as i32).abs()).sum();

    let left = pixel(&gpu_frame, WIDTH / 2 - 30, mid);
    let right = pixel(&gpu_frame, WIDTH / 2 + 30, mid);
    let row_spread: i32 = (0..3)
        .map(|c| (left[c] as i32 - right[c] as i32).abs())
        .sum();

    assert!(
        column_spread <= 3,
        "after a quarter turn a column should be uniform: differs by {column_spread}"
    );
    assert!(
        row_spread > column_spread + 4,
        "the gradient should now run along the row: row {row_spread}, column {column_spread}"
    );
}

/// Zoom leaves the center pixel where it was and pulls the picture's halfway
/// points inward: the pixel a quarter from the center now shows what half
/// out used to.
#[test]
fn zoom_magnifies_about_the_center() {
    let _device = one_device_at_a_time();
    let plain = rendered(None);
    let effects = Effects {
        zoom: 2.0,
        ..Effects::default()
    };
    let zoomed = rendered(Some(&effects));

    let cx = WIDTH / 2;
    let cy = HEIGHT / 2;

    let center_before = pixel(&plain, cx, cy);
    let center_after = pixel(&zoomed, cx, cy);
    for channel in 0..3 {
        assert!(
            (center_before[channel] as i32 - center_after[channel] as i32).abs() <= 2,
            "the center must not move under zoom: {center_before:?} vs {center_after:?}"
        );
    }

    // A point 40 rows above center now shows what 20 rows above center did.
    let sampled = pixel(&zoomed, cx, cy - 40);
    let source = pixel(&plain, cx, cy - 20);
    for channel in 0..3 {
        assert!(
            (sampled[channel] as i32 - source[channel] as i32).abs() <= 3,
            "zoom 2 should map -40 rows onto -20: {sampled:?} vs {source:?}"
        );
    }
}

/// Saturation zero grays the warm backdrop: the three channels of every
/// interior pixel converge.
#[test]
fn zero_saturation_grays_the_picture() {
    let _device = one_device_at_a_time();
    let effects = Effects {
        saturation: 0.0,
        ..Effects::default()
    };
    let gray = rendered(Some(&effects));

    for (x, y) in [(WIDTH / 2, HEIGHT / 4), (WIDTH / 3, (HEIGHT * 3) / 4)] {
        let [r, g, b, _] = pixel(&gray, x, y);
        let spread = [r, g, b].iter().copied().max().unwrap() as i32
            - [r, g, b].iter().copied().min().unwrap() as i32;
        assert!(spread <= 2, "({x},{y}) should be gray, got r{r} g{g} b{b}");
    }
}

/// The clip fade, in pixels: black at the clip's first frame, black again at its
/// last, and — once the fade is up — the very bytes an unfaded render writes.
///
/// The last assertion is the one that matters. A fade that only *approximately*
/// restores the picture would mean every render carrying a fade in was subtly
/// re-graded from end to end, and this pins that it is not: past the ramp the
/// gain is exactly 1, and exactly 1 is a no-op.
#[test]
fn a_fade_takes_the_clip_from_black_and_back_to_black() {
    let _device = one_device_at_a_time();
    let effects = Effects {
        fade_in: "2s".parse().expect("a duration"),
        fade_out: "2s".parse().expect("a duration"),
        ..Effects::default()
    };
    let clip_at = |elapsed: f32| ClipTime {
        elapsed,
        duration: 10.0,
    };

    for (elapsed, edge) in [(0.0, "first"), (10.0, "last")] {
        let frame = rendered_at(Some(&effects), 0.0, clip_at(elapsed));
        for (x, y) in [(WIDTH / 2, HEIGHT / 4), (WIDTH / 3, (HEIGHT * 3) / 4)] {
            let [r, g, b, _] = pixel(&frame, x, y);
            assert_eq!(
                [r, g, b],
                [0, 0, 0],
                "the clip's {edge} frame should be black, got r{r} g{g} b{b}"
            );
        }
    }

    // Halfway up the ramp: dimmed, but not out.
    let half = rendered_at(Some(&effects), 0.0, clip_at(1.0));
    let full = rendered(Some(&effects));
    for (x, y) in [(WIDTH / 2, HEIGHT / 4), (WIDTH / 3, (HEIGHT * 3) / 4)] {
        for channel in 0..3 {
            let dimmed = linear(pixel(&half, x, y)[channel]);
            let lit = linear(pixel(&full, x, y)[channel]);
            assert!(
                (dimmed - lit * 0.5).abs() < 0.01,
                "half the fade should be half the light: {dimmed} vs {lit}"
            );
        }
    }

    // Between the two fades the gain is exactly 1, so the pass writes exactly
    // what it writes with no fade configured at all.
    let held = rendered_at(Some(&effects), 0.0, clip_at(5.0));
    let plain = rendered(Some(&Effects::default()));
    assert_eq!(
        held, plain,
        "past the fade in, the picture is byte-identical to an unfaded render"
    );
}
