// halo — a soft glow breathing in a chosen corner (issue #44).
//
// The first of the subtle presets: one radial glow anchored on the nine-grid,
// its radius breathing with the loudness, its edge wobbled by slow noise so
// it reads as light rather than as a stamped disc. A hit swells it gently —
// no flash. Everything else in the frame stays fully transparent, so the
// halo is an accent over a background image or video, not a visual of its
// own. Silence fades it to nothing.
//
// Determinism: frame time is the only clock; the edge wobble is value noise
// on the shared no-trigonometry hash (AGENTS.md). Output is linear and
// **premultiplied** (VISION.md §5.3).
//
//   params[0].x  anchor        params[1].x  wobble
//   params[0].y  size          params[1].y  swell
//   params[0].z  breathe       params[1].z  margin
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
    let anchor = i32(g.params[0].x + 0.5);
    let size = g.params[0].y;
    let breathe = g.params[0].z;
    let softness = g.params[0].w;
    let wobble = g.params[1].x;
    let swell = g.params[1].y;
    let margin = g.params[1].z;
    let brightness = g.params[1].w;

    // The anchor point on the nine-grid, inset by `margin` from the edges.
    let column = f32(anchor % 3);
    let row = f32(anchor / 3);
    let at = vec2<f32>(
        mix(margin, 1.0 - margin, column * 0.5),
        mix(margin, 1.0 - margin, row * 0.5),
    );

    // Distance in aspect-true units, so the halo is round on any frame.
    let short = min(g.resolution.x, g.resolution.y);
    let uv = position.xy / g.resolution;
    let delta = (uv - at) * g.resolution / short;
    let d = length(delta);

    // The radius breathes with the loudness and swells softly on a hit; the
    // edge is wobbled by slow noise walking around the rim.
    var radius = size * (0.75 + breathe * 0.35 * g.rms_env + swell * 0.2 * g.onset);
    let rim = vnoise(delta / max(d, 1e-4) * 2.3 + vec2<f32>(g.time * 0.11, g.seed));
    radius *= 1.0 + wobble * 0.18 * (rim - 0.5);

    // A soft profile: `softness` trades a lantern edge for a long feather.
    let profile = exp(-pow(d / max(radius, 1e-4), mix(3.2, 1.4, softness)));

    // Warm center, cooler feather, leaning with the centroid.
    let tone = mix(
        accent(0.65 + g.centroid * 0.2),
        accent(0.2 + g.centroid * 0.2),
        clamp(d / max(radius, 1e-4), 0.0, 1.0),
    );

    var color = tone * profile * brightness * (0.25 + 0.75 * g.rms_env);
    color = max(color, vec3<f32>(0.0));

    let alpha = clamp(max(color.r, max(color.g, color.b)), 0.0, 1.0);
    return vec4<f32>(color, alpha);
}
