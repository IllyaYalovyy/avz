// ribbons — the spectrum, drawn (VISION.md §6).
//
// A stack of ribbons across the frame, each one displaced by the song's own
// spectrum: the horizontal axis of the frame *is* the log-frequency axis of the
// spectrum texture, bass at the left edge and air at the right. Where a band is
// loud its ribbon swells, thickens, and lights; where it is quiet the ribbon
// thins away to nothing and the backdrop shows through.
//
// This is the preset that forces the spectrum texture (RFC-001 NG1, issue #24).
// It asks for it with `"needs_spectrum": true` in `ribbons.json`; the renderer
// then binds this frame's 512-bucket row at `@binding(3)`. It is read with
// `textureLoad` and interpolated here rather than sampled: hardware filtering of
// a float texture is a driver-dependent rounding, and golden frames would drift
// with it (AGENTS.md, determinism).
//
// Audio mapping (VISION.md §6): the spectrum texture places the light along the
// frequency axis; `rms_env` is the overall brightness, so a silent passage fades
// out entirely; `bass_env` sways the whole stack; `onset` flashes it; `flux`
// lifts the glow; `centroid` walks the hue along the palette.
//
// Determinism: `time` is `frame_index / fps` and the only clock; each ribbon's
// frequency window and phase are a hash of its index and `seed`. No `sin`-based
// hash: those differ between drivers.
//
// Output is linear — the layer target is `Rgba8UnormSrgb` and encodes on write.
//
// Output is also **premultiplied** (VISION.md §5.3): the RGB is the light this
// layer emits, and the alpha is how much of the backdrop that light hides. Every
// term is scaled by `rms_env` with no floor under it, so silence is nothing at
// all rather than a dark rectangle over the background.
//
// The `params` slots below are declared in `ribbons.json`, which is the only
// place their names, defaults, and ranges live.
//
//   params[0].x  ribbon_count     params[1].x  spread
//   params[0].y  amplitude        params[1].y  drift_speed
//   params[0].z  thickness        params[1].z  blur
//   params[0].w  glow             params[1].w  vignette
//                                 params[2].x  brightness

// The uniform contract every preset receives. The Rust side that fills it is
// `render/globals.rs`; the layout it encodes is documented there.
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

// This frame's coarse spectrum: 512 log-spaced buckets from 20 Hz to 16 kHz,
// each in 0..1 after the global normalization (`analysis::spectrum`).
@group(0) @binding(3) var spectrum: texture_2d<f32>;

const TAU: f32 = 6.2831853;

// A seeded hash in 0..1. Two dimensions in, one out, no trigonometry: `sin`
// hashes differ between drivers, and golden frames would drift with them.
fn hash21(p: vec2<f32>) -> f32 {
    var q = fract(vec3<f32>(p.xyx) * 0.1031);
    q += dot(q, q.yzx + 33.33);
    return fract((q.x + q.y) * q.z);
}

// The accent ramp: `pal[1]` through `pal[4]`, walked by `t` in 0..1.
//
// `pal[0]` is the background and stays out of it, so a palette's darkest color
// never becomes a highlight.
fn accent(t: f32) -> vec3<f32> {
    let x = clamp(t, 0.0, 1.0) * 3.0;
    let stop = min(u32(x), 2u);
    return mix(g.pal[stop + 1u].rgb, g.pal[stop + 2u].rgb, x - f32(stop));
}

// One bucket of the spectrum, interpolated between the two texels `at` falls
// between. `at` is in buckets, and reads off either end clamp to the edge.
fn bucket(at: f32, buckets: i32) -> f32 {
    let low = i32(floor(at));
    let t = at - floor(at);
    let a = textureLoad(spectrum, vec2<i32>(clamp(low, 0, buckets - 1), 0), 0).r;
    let b = textureLoad(spectrum, vec2<i32>(clamp(low + 1, 0, buckets - 1), 0), 0).r;
    return mix(a, b, t);
}

// The spectrum at `u` in 0..1 across the frame, averaged over three taps `blur`
// buckets apart.
//
// Three taps rather than a running average over `blur` buckets: this runs once
// per ribbon per pixel, and a loop whose length a user sets would make
// `ribbon_count` and `blur` multiply into the frame time. `blur` of zero
// collapses the taps onto one another, which is a plain interpolated read.
fn spectrum_at(u: f32, blur: f32) -> f32 {
    let buckets = i32(textureDimensions(spectrum).x);
    let at = clamp(u, 0.0, 1.0) * f32(buckets - 1);
    let step = max(blur, 0.0);

    let left = bucket(at - step, buckets);
    let here = bucket(at, buckets);
    let right = bucket(at + step, buckets);

    return (left + here + right) / 3.0;
}

// The fullscreen triangle: three vertices, no vertex buffer, no index buffer.
@vertex
fn vs_main(@builtin(vertex_index) vertex: u32) -> @builtin(position) vec4<f32> {
    let index = i32(vertex);
    let x = f32(index / 2) * 4.0 - 1.0;
    let y = f32(index & 1) * 4.0 - 1.0;
    return vec4<f32>(x, y, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    // Centered, aspect-corrected, y up. The short edge spans -0.5..0.5, so a
    // ribbon is the same thickness on any resolution.
    var p = (position.xy - 0.5 * g.resolution) / min(g.resolution.x, g.resolution.y);
    p.y = -p.y;

    let count = i32(g.params[0].x);
    let amplitude = g.params[0].y;
    let thickness = g.params[0].z;
    let glow = g.params[0].w;
    let spread = g.params[1].x;
    let drift_speed = g.params[1].y;
    let blur = g.params[1].z;
    let vignette = g.params[1].w;
    let brightness = g.params[2].x;

    // The frame's width is the spectrum's frequency axis, left to right.
    let u = position.x / g.resolution.x;

    var color = vec3<f32>(0.0);
    for (var index = 0; index < count; index = index + 1) {
        // Where this ribbon sits in the stack, in 0..1 from top to bottom.
        let lane = (f32(index) + 0.5) / f32(count);

        // Each ribbon reads the spectrum through its own window, offset a little
        // along the frequency axis, so two ribbons never trace the same curve.
        let noise = hash21(vec2<f32>(f32(index) + 1.0, g.seed * 331.0));
        let level = spectrum_at(u + (noise - 0.5) * 0.05, blur);

        // The rest height of the ribbon, the travelling wave about it, and the
        // sway the kick gives the whole stack.
        let rest = (lane - 0.5) * spread * 0.9;
        let phase = noise * TAU + g.time * drift_speed * (0.5 + noise);
        let wave = sin(TAU * u * (1.5 + 2.0 * noise) + phase);
        let sway = 0.04 * g.bass_env * sin(phase);
        let curve = rest + amplitude * level * wave * 0.35 + sway;

        // A soft core with a halo around it, both scaled by the loudness of the
        // band this column reads: the ribbon *is* the spectrum, so where the
        // spectrum is silent there is no ribbon to see.
        let distance = abs(p.y - curve);
        let width = thickness * (0.35 + 1.2 * level);
        let core = exp(-(distance * distance) / max(width * width, 1e-6));
        let halo = glow * width / (distance + width);
        let energy = (core + halo * 0.25) * level;

        // The centroid walks the hue along the palette; the ribbon's place in
        // the stack spreads the colors apart.
        color += accent(lane * 0.6 + g.centroid * 0.4) * energy;
    }

    // A hit lands on the beat and not after it: `onset` is 1.0 on exactly the
    // frame the flux peaked (`analysis::onset`), so the flash is every ribbon
    // going brighter on that frame, with no smoothing in front of it.
    color *= brightness * (0.85 + 0.45 * g.onset + 0.2 * g.flux);

    // A vignette keeps the corners out of the way and opens them onto the
    // backdrop the compositor draws beneath.
    color *= 1.0 - vignette * smoothstep(0.35, 0.95, length(p));

    // Loudness is the last word: the ribbons breathe with the song, and a silent
    // passage fades out entirely, leaving the backdrop alone.
    color *= g.rms_env;
    color = max(color, vec3<f32>(0.0));

    // Coverage is the brightest channel of the light: a saturated highlight hides
    // what is under it, a faint glow veils it, and unlit pixels leave it be. The
    // RGB is already the light `alpha` worth of this layer emits, which is what
    // "premultiplied" means, so it is returned as it stands.
    let alpha = clamp(max(color.r, max(color.g, color.b)), 0.0, 1.0);
    return vec4<f32>(color, alpha);
}
