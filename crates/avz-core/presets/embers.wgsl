// embers — soft flames along the bottom edge (issue #45).
//
// A low fire band hugging the bottom of the frame: noise flames that lick a
// little higher when the kick lands, and sparse embers that detach and drift
// up, twinkling with the air band. Deliberately quiet — a hearth under the
// picture, not a wall of fire. Everything above the band is transparent, and
// silence banks the fire down to nothing.
//
// Determinism: the flame field is value noise scrolled linearly by frame
// time; the kick raises the *reach* of the flames, never their history.
// Embers are hash-cell dots with frame-quantized twinkle (AGENTS.md).
// Output is linear and **premultiplied** (VISION.md §5.3).
//
//   params[0].x  height        params[1].x  flicker
//   params[0].y  stoke         params[1].y  sparks
//   params[0].z  billow        params[1].z  warmth
//   params[0].w  rise          params[1].w  brightness

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

fn hash21(p: vec2<f32>) -> f32 {
    var q = fract(vec3<f32>(p.xyx) * 0.1031);
    q += dot(q, q.yzx + 33.33);
    return fract((q.x + q.y) * q.z);
}

fn vnoise(p: vec2<f32>) -> f32 {
    let cell = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = hash21(cell);
    let b = hash21(cell + vec2<f32>(1.0, 0.0));
    let c = hash21(cell + vec2<f32>(0.0, 1.0));
    let d = hash21(cell + vec2<f32>(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

fn fbm(p: vec2<f32>) -> f32 {
    return 0.55 * vnoise(p) + 0.3 * vnoise(p * 2.17 + 5.2) + 0.15 * vnoise(p * 4.41 + 11.7);
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
    let height = g.params[0].x;
    let stoke = g.params[0].y;
    let billow = g.params[0].z;
    let rise = g.params[0].w;
    let flicker = g.params[1].x;
    let sparks = g.params[1].y;
    let warmth = g.params[1].z;
    let brightness = g.params[1].w;

    let aspect = g.resolution.x / g.resolution.y;
    let u = position.x / g.resolution.x;
    // `lift` is height above the bottom edge, 0..1 of the frame.
    let lift = 1.0 - position.y / g.resolution.y;

    // How high the fire reaches right now: the kick stokes it.
    let reach = height * (0.8 + stoke * 0.4 * g.bass_env);

    var color = vec3<f32>(0.0);

    if lift < reach * 1.4 {
        // The flame body: noise rising through the band, hotter at the foot.
        let n = fbm(vec2<f32>(
            u * billow * 6.0 * aspect + g.seed * 9.0,
            lift * billow * 9.0 - g.time * rise * 2.2,
        ));
        let lick = fbm(vec2<f32>(u * billow * 14.0 * aspect, g.time * rise * 3.1 + g.seed));

        let foot = clamp(1.0 - lift / max(reach, 1e-3), 0.0, 1.0);
        var heat = pow(foot, 1.6) * (0.35 + 0.9 * n) * (0.8 + flicker * 0.4 * (lick - 0.5));
        heat = clamp(heat, 0.0, 1.0);

        // Palette heat ramp: `warmth` decides how far up the accent the
        // hottest flame reaches, the centroid leaning it slightly.
        color += accent(clamp(heat * warmth + g.centroid * 0.1, 0.0, 1.0)) * heat;
    }

    // The embers: sparse hash cells drifting upward, twinkling with the air
    // band, gone before mid-frame.
    let cell_at = vec2<f32>(u * 26.0 * aspect, lift * 26.0 - g.time * 0.9);
    let cell = floor(cell_at);
    if hash21(cell + g.seed) > 1.0 - sparks * 0.06 {
        let sp = fract(cell_at) - 0.5;
        let tick = floor(g.time * 8.0);
        let wink = 0.5 + 0.5 * hash21(cell + tick);
        let fade = smoothstep(0.5, 0.05, lift);
        color += accent(0.8) * exp(-dot(sp, sp) * 40.0) * fade
            * (0.25 + 0.75 * g.air_env * wink) * 0.8;
    }

    // A banked fire: silence takes it down to coals.
    color *= brightness * (0.2 + 0.8 * g.rms_env);
    color = max(color, vec3<f32>(0.0));

    let alpha = clamp(max(color.r, max(color.g, color.b)), 0.0, 1.0);
    return vec4<f32>(color, alpha);
}
