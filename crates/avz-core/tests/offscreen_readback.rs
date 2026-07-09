//! Offscreen rendering against a real Vulkan device.
//!
//! Everything here runs on the software adapter (lavapipe), which is what makes
//! the assertions stable across machines: GPU float differences are expected and
//! tolerated everywhere else, but never in a test (`docs/TESTING.md`).
//!
//! These tests need Mesa's software Vulkan driver. On Fedora:
//! `sudo dnf install mesa-vulkan-drivers`.

use avz_core::render::{AdapterChoice, AdapterKind, Gpu, Offscreen};

/// Opaque red, in linear space. Exactly 0.0 and 1.0, so the linear → sRGB
/// encode of the clear value is lossless and the expected bytes are unambiguous.
const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
const RED_BYTES: [u8; 4] = [255, 0, 0, 255];

const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
const BLACK_BYTES: [u8; 4] = [0, 0, 0, 255];

fn software_gpu() -> Gpu {
    Gpu::new(AdapterChoice::Software).expect(
        "lavapipe must be installed for the render tests: `sudo dnf install mesa-vulkan-drivers`",
    )
}

#[test]
fn the_software_adapter_is_a_cpu_adapter_and_needs_no_warning() {
    let gpu = software_gpu();

    assert_eq!(gpu.kind(), AdapterKind::Software);
    assert!(
        !gpu.fell_back_to_software(),
        "`--adapter software` asked for this and must not be warned about it"
    );
}

/// `--adapter gpu` on a machine with a GPU must not hand back lavapipe. On a
/// GPU-less machine it must fail rather than render slowly — either way, it
/// never returns a software adapter.
#[test]
fn asking_for_gpu_never_yields_a_software_adapter() {
    match Gpu::new(AdapterChoice::Gpu) {
        Ok(gpu) => assert_eq!(gpu.kind(), AdapterKind::Hardware),
        Err(err) => {
            let msg = err.to_string();
            assert!(
                msg.contains("--adapter software"),
                "a GPU-less host must be told how to render anyway: {msg}"
            );
        }
    }
}

/// `auto` always finds something: a GPU, or lavapipe with the fallback flagged.
#[test]
fn auto_always_finds_an_adapter_and_flags_a_software_fallback() {
    let gpu = Gpu::new(AdapterChoice::Auto).expect("auto renders on any Vulkan host");

    assert_eq!(
        gpu.fell_back_to_software(),
        gpu.kind() == AdapterKind::Software,
        "the warning must fire exactly when auto lands on software"
    );
}

/// UT-003, on a host where the only Vulkan adapter is lavapipe.
///
/// The assertions above hold on a GPU too, so they cannot prove the fallback
/// ever runs. `scripts/quality.d/70-gpu-less-host-falls-back-to-lavapipe.sh`
/// restricts Vulkan to the lavapipe ICD and sets this variable, which turns the
/// "either way is fine" checks into "the fallback must have happened".
#[test]
fn a_gpu_less_host_falls_back_to_software_and_says_so() {
    if std::env::var_os("AVZ_TEST_EXPECT_NO_GPU").is_none() {
        return;
    }

    let gpu = Gpu::new(AdapterChoice::Auto).expect("auto renders even with no GPU");
    assert_eq!(gpu.kind(), AdapterKind::Software);
    assert!(
        gpu.fell_back_to_software(),
        "an auto render on lavapipe must be warnable"
    );

    let err = Gpu::new(AdapterChoice::Gpu).expect_err("`--adapter gpu` has no GPU to use");
    let msg = err.to_string();
    assert!(msg.contains("no hardware GPU adapter found"), "{msg}");
    assert!(msg.contains("--adapter software"), "{msg}");
}

/// The risk this whole module exists for. 300 px × 4 B = 1200 B per row, which
/// wgpu pads to 1280 — so a readback that ignores the padding produces a frame
/// skewed by 80 bytes per row, which looks like a sheared image, not a crash.
#[test]
fn readback_handles_non_multiple_of_256_row_widths() {
    let gpu = software_gpu();
    let frame = Offscreen::new(&gpu, 300, 7).expect("a 300x7 frame fits any adapter");

    frame.clear(&gpu, RED);
    let pixels = frame.read_rgba(&gpu).expect("the frame reads back");

    assert_eq!(
        pixels.len(),
        300 * 7 * 4,
        "no padding survives the readback"
    );
    for (index, pixel) in pixels.chunks_exact(4).enumerate() {
        assert_eq!(
            pixel,
            RED_BYTES,
            "pixel {index} at row {} column {} is not the clear color",
            index / 300,
            index % 300,
        );
    }
}

#[test]
fn an_aligned_frame_width_reads_back_unchanged() {
    let gpu = software_gpu();
    let frame = Offscreen::new(&gpu, 320, 180).expect("320x180 is the CI render size");

    frame.clear(&gpu, RED);
    let pixels = frame.read_rgba(&gpu).expect("the frame reads back");

    assert_eq!(pixels.len(), 320 * 180 * 4);
    assert!(pixels.chunks_exact(4).all(|pixel| pixel == RED_BYTES));
}

/// Rendering a song means reading thousands of frames through one target. The
/// buffer must be remappable, and each read must reflect the newest clear.
#[test]
fn a_target_can_be_rendered_and_read_back_repeatedly() {
    let gpu = software_gpu();
    let frame = Offscreen::new(&gpu, 300, 2).expect("a 300x2 frame fits any adapter");
    let mut pixels = Vec::new();

    for (color, expected) in [(RED, RED_BYTES), (BLACK, BLACK_BYTES), (RED, RED_BYTES)] {
        frame.clear(&gpu, color);
        frame
            .read_rgba_into(&gpu, &mut pixels)
            .expect("the frame reads back");

        assert_eq!(pixels.len(), 300 * 2 * 4);
        assert!(
            pixels.chunks_exact(4).all(|pixel| pixel == expected),
            "the readback shows a stale frame"
        );
    }
}

#[test]
fn a_zero_sized_frame_is_a_render_error_not_a_driver_panic() {
    let gpu = software_gpu();

    let err = Offscreen::new(&gpu, 1920, 0).expect_err("a zero-height frame has no rows");

    assert!(err.to_string().contains("height"), "{err}");
}

/// The device's own limit, not an arbitrary constant, decides what is too big —
/// and the message names the adapter that refused.
#[test]
fn a_frame_larger_than_the_adapter_allows_names_the_adapter() {
    let gpu = software_gpu();
    let max = gpu.device().limits().max_texture_dimension_2d;

    let err = Offscreen::new(&gpu, max + 1, 16).expect_err("beyond the adapter's limit");

    let msg = err.to_string();
    assert!(msg.contains(gpu.adapter_name()), "{msg}");
    assert!(msg.contains(&max.to_string()), "{msg}");
}
