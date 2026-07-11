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
    Preset {
        name: "bars",
        description: "a spectrum analyzer in one corner: anchored bars over whatever is behind them",
        source: include_str!("bars.wgsl"),
        schema: include_str!("bars.json"),
    },
    Preset {
        name: "meter",
        description: "a VU meter in one spot: the loudness as an anchored ladder of LEDs",
        source: include_str!("meter.wgsl"),
        schema: include_str!("meter.json"),
    },
    Preset {
        name: "tunnel",
        description: "an endless ring tunnel flown at the speed of the song, every hit a lit gate",
        source: include_str!("tunnel.wgsl"),
        schema: include_str!("tunnel.json"),
    },
    Preset {
        name: "starfield",
        description: "a warp-speed starfield: loudness is velocity, and every hit streaks the sky",
        source: include_str!("starfield.wgsl"),
        schema: include_str!("starfield.json"),
    },
    Preset {
        name: "horizon",
        description: "a synthwave sunset: a scanlined sun over a perspective grid the kick pulses",
        source: include_str!("horizon.wgsl"),
        schema: include_str!("horizon.json"),
    },
    Preset {
        name: "aurora",
        description: "curtains of aurora light swaying with the bass, shimmered by the air band",
        source: include_str!("aurora.wgsl"),
        schema: include_str!("aurora.json"),
    },
    Preset {
        name: "scope",
        description: "an oscilloscope figure: a lissajous curve the bands bend and the beat brightens",
        source: include_str!("scope.wgsl"),
        schema: include_str!("scope.json"),
    },
    Preset {
        name: "rain",
        description: "spectral rain: each column falls at the loudness of its own band",
        source: include_str!("rain.wgsl"),
        schema: include_str!("rain.json"),
    },
    Preset {
        name: "tiles",
        description: "an equalizer wall of tiles: the spectrum lit floor to ceiling across the frame",
        source: include_str!("tiles.wgsl"),
        schema: include_str!("tiles.json"),
    },
    Preset {
        name: "orbits",
        description: "band planets on trails: five bodies orbiting, each swollen by its own band",
        source: include_str!("orbits.wgsl"),
        schema: include_str!("orbits.json"),
    },
    Preset {
        name: "stained",
        description: "stained glass lit from behind by the bands, re-leaded on every hit",
        source: include_str!("stained.wgsl"),
        schema: include_str!("stained.json"),
    },
    Preset {
        name: "strings",
        description: "harp strings across the frame, plucked by the hits and left to ring down",
        source: include_str!("strings.wgsl"),
        schema: include_str!("strings.json"),
    },
    Preset {
        name: "halo",
        description: "a soft glow breathing in a chosen corner, an accent over whatever is behind it",
        source: include_str!("halo.wgsl"),
        schema: include_str!("halo.json"),
    },
    Preset {
        name: "embers",
        description: "soft flames along the bottom edge, stoked by the kick, shedding embers",
        source: include_str!("embers.wgsl"),
        schema: include_str!("embers.json"),
    },
    Preset {
        name: "motes",
        description: "drifting dust, barely lit: the quietest preset, an atmosphere over a background",
        source: include_str!("motes.wgsl"),
        schema: include_str!("motes.json"),
    },
    Preset {
        name: "fireflies",
        description: "a few wandering lights that blink to themselves and flare gently on the hits",
        source: include_str!("fireflies.wgsl"),
        schema: include_str!("fireflies.json"),
    },
    Preset {
        name: "veil",
        description: "an inverse vignette: the frame's edges breathe light with the song",
        source: include_str!("veil.wgsl"),
        schema: include_str!("veil.json"),
    },
];
