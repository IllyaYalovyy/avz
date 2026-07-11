// veil — the frame's edges breathe with the song (issue #48).
//
// An inverse vignette: soft light reaching in from the chosen edges, its
// reach breathing with the loudness, textured by faint slow noise so it reads
// as gauze rather than gradient. A hit lifts it slightly; silence lets the
// frame go dark. The middle of the frame stays transparent — this is a
// picture frame of light around whatever is behind it.
//
// Determinism: frame time drifts the gauze noise linearly; the breathing is
// the envelope value itself, never accumulated (AGENTS.md). Output is linear
// and **premultiplied** (VISION.md §5.3).
//
//   params[0].x  sides         params[1].x  texture
//   params[0].y  reach         params[1].y  lift
//   params[0].z  breathe       params[1].z  hue
//   params[0].w  softness      params[1].w  brightness

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
    let sides = i32(g.params[0].x + 0.5);
    let reach = g.params[0].y;
    let breathe = g.params[0].z;
    let softness = g.params[0].w;
    let texture_amt = g.params[1].x;
    let lift = g.params[1].y;
    let hue = g.params[1].z;
    let brightness = g.params[1].w;

    let uv = position.xy / g.resolution;

    // How far in the veil reaches right now.
    let span = reach * (0.55 + breathe * 0.45 * g.rms_env + lift * 0.25 * g.onset);

    // Each edge's glow, kept or dropped by `sides`:
    // 0 all, 1 top, 2 bottom, 3 horizontal (left+right), 4 vertical (top+bottom).
    let power = mix(2.6, 1.2, softness);
    let from_edge = vec4<f32>(uv.y, 1.0 - uv.y, uv.x, 1.0 - uv.x); // top, bottom, left, right
    let keep = array<vec4<f32>, 5>(
        vec4<f32>(1.0, 1.0, 1.0, 1.0),
        vec4<f32>(1.0, 0.0, 0.0, 0.0),
        vec4<f32>(0.0, 1.0, 0.0, 0.0),
        vec4<f32>(0.0, 0.0, 1.0, 1.0),
        vec4<f32>(1.0, 1.0, 0.0, 0.0),
    );
    let mask = keep[clamp(sides, 0, 4)];

    var glow = 0.0;
    for (var e = 0; e < 4; e++) {
        if mask[e] < 0.5 {
            continue;
        }
        glow = max(glow, exp(-pow(from_edge[e] / max(span, 1e-4), power)));
    }

    // The gauze: slow, coarse noise drifting along the frame, faint by
    // default — enough to keep the veil from reading as a clean gradient.
    let weave = vnoise(uv * vec2<f32>(5.0, 3.5) + vec2<f32>(g.time * 0.07, g.seed));
    glow *= 1.0 - texture_amt * 0.5 * weave;

    let tone = accent(clamp(0.25 + hue * 0.4 + g.centroid * 0.25, 0.0, 1.0));
    var color = tone * glow * brightness * (0.2 + 0.8 * g.rms_env);
    color = max(color, vec3<f32>(0.0));

    let alpha = clamp(max(color.r, max(color.g, color.b)), 0.0, 1.0);
    return vec4<f32>(color, alpha);
}
