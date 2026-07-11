// orbits — band planets on trails: five bodies orbiting, each swollen by its
// own band (issue #41).
//
// Five bodies circle the center, one per band — bass innermost, air outermost
// — each orbit swelling with its band's envelope, each body brightening as
// its band speaks. The previous frame (`needs_feedback`, `@binding(1)`) is
// pulled back, faded by `trail`, and gently curled, so every body drags a
// comet tail that bends behind it. A hit flashes every body at once; a small
// sun of the song's own loudness burns in the middle.
//
// Determinism: each body's angle is `time * rate` plus a seeded phase —
// nothing integrates outside the feedback texture, which is the one place
// state may live (the nebula/ink rule). Output is linear and
// **premultiplied** (VISION.md §5.3); silence starves both the bodies and
// their trails.
//
//   params[0].x  scale         params[1].x  trail
//   params[0].y  speed         params[1].y  curl
//   params[0].z  swell         params[1].z  flash
//   params[0].w  body_size     params[1].w  brightness

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

// Last frame's pixels: black on frame 0, trails ever after.
@group(0) @binding(1) var previous: texture_2d<f32>;
@group(0) @binding(2) var previous_sampler: sampler;

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

// The five bands, innermost orbit to outermost.
fn band_env(k: i32) -> f32 {
    switch k {
        case 0: { return g.bass_env; }
        case 1: { return g.low_mid_env; }
        case 2: { return g.mid_env; }
        case 3: { return g.high_env; }
        default: { return g.air_env; }
    }
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
    let scale = g.params[0].x;
    let speed = g.params[0].y;
    let swell = g.params[0].z;
    let body_size = g.params[0].w;
    let trail = g.params[1].x;
    let curl = g.params[1].y;
    let flash = g.params[1].z;
    let brightness = g.params[1].w;

    let short = min(g.resolution.x, g.resolution.y);
    let p = (position.xy - 0.5 * g.resolution) / short;

    // The history, pulled from a slightly rotated past so trails bend behind
    // the orbits instead of smearing straight.
    let turn = curl * 0.02;
    let rotated = vec2<f32>(
        p.x * cos(turn) - p.y * sin(turn),
        p.x * sin(turn) + p.y * cos(turn),
    );
    let previous_uv = (rotated * short + 0.5 * g.resolution) / g.resolution;
    var color = textureSample(previous, previous_sampler, previous_uv).rgb * trail;

    // The sun: the loudness itself, small and steady in the middle.
    let core = exp(-dot(p, p) / (0.0016 + 0.002 * g.rms_env));
    color += accent(g.centroid) * core * (0.4 + 0.6 * g.rms_env);

    // The five planets.
    for (var k = 0; k < 5; k++) {
        let fk = f32(k);
        let env = band_env(k);

        // Alternating directions keep the system from reading as one wheel.
        let direction = select(1.0, -1.0, (k & 1) == 1);
        let rate = speed * (1.25 - 0.17 * fk) * direction;
        let angle = g.time * rate * TAU + hash21(vec2<f32>(fk, g.seed)) * TAU;

        // The orbit: farther out per band, swollen by its own envelope.
        let radius = scale * (0.10 + 0.065 * fk) * (1.0 + swell * 0.35 * env);
        let at = vec2<f32>(cos(angle), sin(angle)) * radius;

        let d = distance(p, at);
        let size = body_size * (0.7 + 0.5 * env);
        let body = exp(-pow(d / max(size, 1e-4), 2.0));
        let halo = size / (d + size) * 0.25;

        color += accent(fk / 4.0) * (body + halo)
            * (0.25 + 0.75 * env) * (1.0 + flash * g.onset);
    }

    // Silence starves the light — the recycled trail included, so a quiet
    // passage empties the sky rather than freezing it.
    color *= brightness * (0.15 + 0.85 * g.rms_env);
    color = clamp(color, vec3<f32>(0.0), vec3<f32>(4.0));

    let alpha = clamp(max(color.r, max(color.g, color.b)), 0.0, 1.0);
    return vec4<f32>(color, alpha);
}
