// Every preset avz ships, in the order `avz presets` lists them.
//
// This file is `include!`d by `src/render/preset.rs`, which is what lets a new
// preset be *only* files in this directory (RFC-001 G3). Adding one:
//
//   1. drop `<name>.wgsl` and `<name>.json` beside this file,
//   2. add one row below,
//   3. commit its golden hashes:
//      AVZ_UPDATE_GOLDEN=1 cargo test -p avz-core --test golden_frames
//
// Nothing outside `presets/` moves. `scripts/quality.d/96-a-preset-is-only-files-in-presets.sh`
// is what keeps that true.
//
// The `include_str!` paths are relative to *this* file, not to the module that
// includes it.

/// Every preset avz ships, in the order `avz presets` lists them.
pub const PRESETS: &[Preset] = &[Preset {
    name: "pulse",
    description: "minimal, geometric: concentric rings driven by the kick",
    source: include_str!("pulse.wgsl"),
    schema: include_str!("pulse.json"),
}];
