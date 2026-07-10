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
pub const PRESETS: &[Preset] = &[
    Preset {
        name: "pulse",
        description: "minimal, geometric: concentric rings driven by the kick",
        source: include_str!("pulse.wgsl"),
        schema: include_str!("pulse.json"),
    },
    Preset {
        name: "nebula",
        description: "organic clouds: an fbm flow field over feedback trails, churned by the bass",
        source: include_str!("nebula.wgsl"),
        schema: include_str!("nebula.json"),
    },
    Preset {
        name: "ribbons",
        description: "classic and reactive: a stack of ribbons displaced by the song's own spectrum",
        source: include_str!("ribbons.wgsl"),
        schema: include_str!("ribbons.json"),
    },
    Preset {
        name: "particles",
        description: "energetic: every hit throws a burst of sparks the highs make twinkle",
        source: include_str!("particles.wgsl"),
        schema: include_str!("particles.json"),
    },
    Preset {
        name: "kaleido",
        description: "symmetric and hypnotic: a mirrored fold that turns while the hue walks the palette",
        source: include_str!("kaleido.wgsl"),
        schema: include_str!("kaleido.json"),
    },
    Preset {
        name: "ink",
        description: "slow and brooding: a reaction-diffusion marble the loudness of the song grows",
        source: include_str!("ink.wgsl"),
        schema: include_str!("ink.json"),
    },
];
