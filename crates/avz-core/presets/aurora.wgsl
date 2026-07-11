// aurora — curtains of light swayed by the bass, shimmered by the air
// (issue #37).
//
// Layered aurora curtains hanging across the upper frame: each curtain is a
// noise-drawn rim with light draping downward from it, striated vertically
// like the real thing. The bass (`bass_env`) deepens the sway of the
// curtains, the air band (`air_env`) shimmers their striations on a
// frame-quantized clock, the centroid walks the hue, and a hit (`onset`)
// breathes light into the whole sky. Where `nebula` is clouds, this is
// curtains: structure hangs from a rim instead of churning.
//
// Determinism: value noise built on the shared no-trigonometry hash, drifted
// linearly by frame time; sway *amplitude* is modulated by the bass rather
// than integrated, so frame N never depends on frame N-1 (AGENTS.md).
// Output is linear and **premultiplied** (VISION.md §5.3); silence fades the
// sky to the backdrop.
//
//   params[0].x  curtains      params[1].x  shimmer
//   params[0].y  scale         params[1].y  drape
//   params[0].z  drift         params[1].z  vignette
//   params[0].w  sway          params[1].w  brightness

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

// Value noise: bilinear hash interpolation with a smooth fade.
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

// Three octaves: enough for a rim that meanders at two scales.
fn fbm(p: vec2<f32>) -> f32 {
    return 0.5 * vnoise(p) + 0.3 * vnoise(p * 2.13 + 7.7) + 0.2 * vnoise(p * 4.31 + 13.1);
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
    let curtains = i32(g.params[0].x + 0.5);
    let scale = g.params[0].y;
    let drift = g.params[0].z;
    let sway = g.params[0].w;
    let shimmer = g.params[1].x;
    let drape = g.params[1].y;
    let vignette = g.params[1].z;
    let brightness = g.params[1].w;

    // Centered, aspect-corrected, y up.
    var p = (position.xy - 0.5 * g.resolution) / min(g.resolution.x, g.resolution.y);
    p.y = -p.y;

    // The bass deepens the sway; a hit breathes light into the sky.
    let swing = 0.08 * (0.4 + sway * g.bass_env);
    let breathe = 0.75 + 0.25 * g.mid_env + 0.35 * g.onset;

    var color = vec3<f32>(0.0);
    for (var k = 0; k < 4; k++) {
        if k >= curtains {
            break;
        }
        let fk = f32(k);
        let lane = fk * 37.0 + g.seed * 7.0;

        // The rim: a noise curve meandering across the upper frame, each
        // curtain hanging a little lower than the one behind it.
        let base = 0.30 - fk * 0.11;
        let rim = base
            + (fbm(vec2<f32>(p.x * scale + g.time * drift * (1.0 + 0.3 * fk), lane)) - 0.5) * swing * 6.0;

        // Above the rim: nothing. Below: light draping downward and dying,
        // brightest just under the rim.
        let below = rim - p.y;
        if below < 0.0 {
            continue;
        }
        let veil = exp(-below / max(drape * 0.4, 0.02)) * smoothstep(0.0, 0.02, below);

        // Vertical striations — the curtain's folds — shimmered by the air
        // band on a frame-quantized clock.
        let tick = floor(g.time * 9.0);
        let fold = 0.55 + 0.45 * vnoise(vec2<f32>(p.x * scale * 9.0 + lane, fk * 5.0));
        let wink = 0.8 + 0.2 * hash21(vec2<f32>(floor(p.x * scale * 9.0) + lane, tick));
        let striae = fold * mix(1.0, wink, clamp(shimmer * g.air_env * 1.6, 0.0, 1.0));

        // Green-to-violet in spirit: the palette walks with height and the
        // centroid, deeper curtains cooler.
        let hue = accent(clamp(0.15 + below * 1.8 + g.centroid * 0.25 + fk * 0.12, 0.0, 1.0));
        color += hue * veil * striae * (1.0 - fk * 0.18);
    }

    color *= breathe;
    color *= 1.0 - vignette * smoothstep(0.5, 1.1, length(p));
    color *= brightness * g.rms_env;
    color = max(color, vec3<f32>(0.0));

    let alpha = clamp(max(color.r, max(color.g, color.b)), 0.0, 1.0);
    return vec4<f32>(color, alpha);
}
