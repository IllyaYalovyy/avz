// pulse — the minimal, geometric preset (VISION.md §6).
//
// A fullscreen fragment shader over signed-distance geometry: a core disc that
// swells with the kick, concentric rings whose spacing follows the mids, sparkle
// on the highs, a flash on every onset, and a hue that walks the palette with
// the spectral centroid. It is deliberately readable, because it is also the
// instrument the M2 envelope tuning is done on.
//
// Determinism: `time` is `frame_index / fps` and the only clock; every random
// number is a hash of the fragment position, `time`, and `seed` (AGENTS.md).
//
// Output is linear — the layer target is `Rgba8UnormSrgb` and encodes on write.
//
// Output is also **premultiplied** (VISION.md §5.3). `pulse` draws light onto its
// own transparent layer and never paints a background: the RGB it returns is the
// light it emits, and the alpha is how much of the backdrop that light hides.
// Where the preset is dark it is transparent, and the palette gradient beneath
// shows through untouched — which is why every term below is scaled by `rms_env`
// with no floor under it. Silence is not "nearly black" any more; it is nothing.
//
// The `params` slots below are declared in `pulse.json`, which is the only place
// their names, defaults, and ranges live. Every default reproduces the constant
// it replaced, so the golden frames are unchanged by their arrival.
//
//   params[0].x  bass_drive     params[1].x  sparkle_gain
//   params[0].y  ring_count     params[1].y  grain
//   params[0].z  ring_density   params[1].z  glow
//   params[0].w  drift_speed    params[1].w  vignette
//                               params[2].x  flash

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

const TAU: f32 = 6.2831853;

// The fullscreen triangle: three vertices, no vertex buffer, no index buffer.
// It covers the clip cube with one primitive, so no seam runs down the middle.
@vertex
fn vs_main(@builtin(vertex_index) vertex: u32) -> @builtin(position) vec4<f32> {
    let index = i32(vertex);
    let x = f32(index / 2) * 4.0 - 1.0;
    let y = f32(index & 1) * 4.0 - 1.0;
    return vec4<f32>(x, y, 0.0, 1.0);
}

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

// A soft-edged band: 1.0 where `distance` is zero, falling to 0.0 at `width`.
fn band(distance: f32, width: f32) -> f32 {
    return 1.0 - smoothstep(0.0, width, distance);
}

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    // Centered, aspect-corrected, y up. The short edge spans -0.5..0.5, so the
    // geometry below is the same size on any resolution.
    var p = (position.xy - 0.5 * g.resolution) / min(g.resolution.x, g.resolution.y);
    p.y = -p.y;
    let r = length(p);

    // The centroid moves the hue along the palette: bright material reads as the
    // far end of the ramp, dark material as the near end.
    let hue = accent(g.centroid);
    let counter_hue = accent(1.0 - g.centroid);

    // Kick: the core disc swells with the bass envelope and snaps on an onset.
    // `bass_drive` scales the swell; the envelope is already in 0..1, so the
    // clamp only bites once a user drives it past one.
    let bass = clamp(g.bass_env * g.params[0].x, 0.0, 1.0);
    let radius = 0.14 + 0.13 * bass + 0.05 * g.onset;
    let core = band(max(r - radius, 0.0), 0.035 + 0.05 * g.onset);

    // Vocals-ish: the mids set how tightly the rings pack; the low mids push
    // them outward. `time` is the only clock, so the drift is frame-exact.
    let ring_frequency = g.params[0].y + g.params[0].z * g.mid_env;
    let drift = g.time * (0.2 + 0.5 * g.low_mid_env) * g.params[0].w;
    let wave = 0.5 + 0.5 * cos(TAU * (r * ring_frequency - drift));
    let rings = pow(wave, 6.0) * smoothstep(radius, radius + 0.06, r);

    // Cymbals: a sparse grid of cells twinkles, each on its own seeded phase.
    // 96 cells across the short edge reads as shimmer at 1080p rather than as
    // the visible tiling a coarser grid leaves behind.
    let cell = floor(p * 96.0);
    let noise = hash21(cell + vec2<f32>(g.seed * 137.0, g.seed * 71.0));
    let twinkle = 0.5 + 0.5 * cos(TAU * (g.time * (0.7 + noise) + noise));
    let sparkle = g.high_env * step(0.94, noise) * twinkle * g.params[1].x;

    // Shimmer: per-pixel, per-frame grain, small enough to read as texture.
    let grain = hash21(position.xy + vec2<f32>(g.time * 60.0, g.seed * 913.0));

    // Light, starting from none: the backdrop is the compositor's layer, not
    // this one's, so `pulse` adds to a transparent frame rather than to `pal[0]`.
    var color = vec3<f32>(0.0);
    // A hit lands on the beat and not after it: `onset` is 1.0 on exactly the
    // frame the flux peaked (`analysis::onset`), so the flash is the core going
    // brighter and wider on that frame, with no smoothing in front of it.
    color += hue * core * (0.85 + 0.4 * g.onset * g.params[2].x);
    color += counter_hue * rings * 0.55;
    color += vec3<f32>(sparkle);
    color += hue * g.flux * 0.12 * g.params[1].z;
    color += vec3<f32>((grain - 0.5) * g.params[1].y * g.air_env);

    // A vignette keeps the corners out of the way of the geometry — and, now that
    // the frame beneath is a gradient rather than black, opens them onto it.
    color *= 1.0 - g.params[1].w * smoothstep(0.35, 0.95, r);

    // Loudness is the last word: the geometry breathes with the song, and a
    // silent passage fades out entirely, leaving the backdrop alone.
    color *= g.rms_env;
    color = max(color, vec3<f32>(0.0));

    // Coverage is the brightest channel of the light: a saturated highlight hides
    // what is under it, a faint glow veils it, and unlit pixels leave it be. The
    // RGB is already the light `alpha` worth of this layer emits, which is what
    // "premultiplied" means, so it is returned as it stands.
    let alpha = clamp(max(color.r, max(color.g, color.b)), 0.0, 1.0);
    return vec4<f32>(color, alpha);
}
