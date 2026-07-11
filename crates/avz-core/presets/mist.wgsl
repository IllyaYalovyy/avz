// mist — low fog drifting along the bottom (issue #49).
//
// Layered fog hugging the bottom of the frame, drifting sideways on the
// wind. The low mids thicken it — a warm arrangement fills the ground with
// fog, a thin one lets it thin to wisps — the bass leans its surface gently,
// and the top edge feathers into nothing. No hits, no sparkle: weather, not
// a visualizer. Silence lets it settle out entirely.
//
// Determinism: value-noise fog drifted linearly by frame time; thickness is
// the envelope value itself, never accumulated (AGENTS.md). Output is linear
// and **premultiplied** (VISION.md §5.3).
//
//   params[0].x  height        params[1].x  layers
//   params[0].y  wind          params[1].y  pale
//   params[0].z  billow        params[1].z  sway
//   params[0].w  thicken       params[1].w  brightness

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
    return 0.55 * vnoise(p) + 0.3 * vnoise(p * 2.09 + 7.3) + 0.15 * vnoise(p * 4.17 + 3.9);
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
    let wind = g.params[0].y;
    let billow = g.params[0].z;
    let thicken = g.params[0].w;
    let layers = i32(g.params[1].x + 0.5);
    let pale = g.params[1].y;
    let sway = g.params[1].z;
    let brightness = g.params[1].w;

    let aspect = g.resolution.x / g.resolution.y;
    let u = position.x / g.resolution.x;
    let lift = 1.0 - position.y / g.resolution.y;

    // The fog bank's ceiling right now: the bass leans it, gently.
    let bank = height * (0.85 + sway * 0.25 * g.bass_env);
    if lift > bank * 1.5 {
        return vec4<f32>(0.0);
    }

    // How much fog the song is making: the low mids are the fog machine.
    let supply = 0.35 + thicken * 0.65 * g.low_mid_env;

    var fog = 0.0;
    for (var k = 0; k < 3; k++) {
        if k >= layers {
            break;
        }
        let fk = f32(k);
        // Each layer drifts at its own pace, the nearer one faster.
        let body = fbm(vec2<f32>(
            u * billow * 3.5 * aspect - g.time * wind * (0.6 + 0.5 * fk) + fk * 31.0 + g.seed * 7.0,
            lift * billow * 5.0 + fk * 17.0,
        ));

        // Denser at the ground, feathered at the ceiling.
        let settle = 1.0 - smoothstep(0.0, bank * (1.0 + 0.3 * fk), lift);
        fog += body * settle * (0.6 - 0.15 * fk);
    }
    fog = clamp(fog * supply, 0.0, 1.0);

    // Fog is pale: the palette's low end whitened by `pale`.
    let tone = mix(accent(0.2 + g.centroid * 0.15), vec3<f32>(0.85), pale);

    var color = tone * fog * brightness * (0.25 + 0.75 * g.rms_env);
    color = max(color, vec3<f32>(0.0));

    let alpha = clamp(max(color.r, max(color.g, color.b)), 0.0, 1.0);
    return vec4<f32>(color, alpha);
}
