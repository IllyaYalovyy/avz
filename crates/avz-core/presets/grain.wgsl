// grain — film grain and a slow light leak (issue #51).
//
// A texture overlay, not a picture: faint per-pixel grain re-rolled on a
// cine cadence, and one warm light leak wandering the frame on a closed-form
// path, breathing with the loudness. The air band shimmers the grain a
// little harder. Put it over a background image or video and the still
// picture starts to feel like footage.
//
// Determinism: the grain re-rolls on `floor(time * cadence)` — a
// frame-quantized clock, so every re-render sputters identically — and the
// leak's path is a closed sinusoid of frame time (AGENTS.md). Output is
// linear and **premultiplied** (VISION.md §5.3); the grain's coverage is
// deliberately tiny.
//
//   params[0].x  grain         params[1].x  leak_size
//   params[0].y  cadence       params[1].y  drift
//   params[0].z  shimmer       params[1].z  warmth
//   params[0].w  leak          params[1].w  brightness

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

fn hash21(p: vec2<f32>) -> f32 {
    var q = fract(vec3<f32>(p.xyx) * 0.1031);
    q += dot(q, q.yzx + 33.33);
    return fract((q.x + q.y) * q.z);
}

fn accent(t: f32) -> vec3<f32> {
    let x = clamp(t, 0.0, 1.0) * 3.0;
    let stop = min(u32(x), 2u);
    return mix(g.pal[stop + 1u].rgb, g.pal[stop + 2u].rgb, x - f32(stop));
}

@vertex
fn vs_main(@builtin(vertex_index) vertex: u32) -> @builtin(position) vec4<f32> {
    let index = i32(vertex);
    let x = f32(index / 2) * 4.0 - 1.0;
    let y = f32(index & 1) * 4.0 - 1.0;
    return vec4<f32>(x, y, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let grain_amt = g.params[0].x;
    let cadence = max(g.params[0].y, 1.0);
    let shimmer = g.params[0].z;
    let leak = g.params[0].w;
    let leak_size = g.params[1].x;
    let drift = g.params[1].y;
    let warmth = g.params[1].z;
    let brightness = g.params[1].w;

    let p = (position.xy - 0.5 * g.resolution) / min(g.resolution.x, g.resolution.y);

    // The grain: one hash per pixel per cadence tick. `shimmer` lets the air
    // band roughen it, the way tape hiss rides a bright mix.
    let tick = floor(g.time * cadence);
    let n = hash21(position.xy + vec2<f32>(tick * 13.7, g.seed + tick));
    let sputter = abs(n - 0.5) * 2.0;
    let dust = sputter * grain_amt * (0.6 + shimmer * g.air_env);

    // The leak: one warm blob wandering a slow closed path, breathing with
    // the loudness, its color from the palette's chosen warmth.
    let at = vec2<f32>(
        0.42 * sin(g.time * drift * TAU + g.seed * TAU),
        0.30 * cos(g.time * drift * 0.61 * TAU + g.seed * 9.0),
    );
    let d = distance(p, at);
    let bloom = exp(-pow(d / max(leak_size, 1e-3), 2.0))
        * leak * (0.25 + 0.5 * g.rms_env);

    var color = vec3<f32>(0.9) * dust + accent(warmth) * bloom;
    color *= brightness * (0.3 + 0.7 * g.rms_env);
    color = max(color, vec3<f32>(0.0));

    let alpha = clamp(max(color.r, max(color.g, color.b)), 0.0, 1.0);
    return vec4<f32>(color, alpha);
}
