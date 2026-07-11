// rain — spectral rain: each column falls at the loudness of its own band
// (issue #39).
//
// Falling streaks in columns across the frame, and the frame's horizontal
// axis is the spectrum's log-frequency axis, exactly as in `bars` and
// `ribbons`: each column is lit by its own slice of the 512-bucket spectrum
// (`@binding(3)`, declared with `needs_spectrum`). A loud band rains hard —
// long bright streaks — and a quiet band does not rain at all, so a bass
// drop literally pours down the left of the frame.
//
// Determinism: every drop falls at a *constant* seeded speed — the music
// scales a drop's light, never its position, so a swell cannot teleport the
// rain between frames (AGENTS.md). Spectrum is read with `textureLoad`, not
// sampled, for the same golden-frame reason `ribbons` gives.
//
// Output is linear and **premultiplied** (VISION.md §5.3).
//
//   params[0].x  columns       params[1].x  glow
//   params[0].y  fall          params[1].y  flash
//   params[0].z  streak        params[1].z  vignette
//   params[0].w  layers        params[1].w  brightness

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

// This frame's coarse spectrum: 512 log-spaced buckets, 0..1, normalized.
@group(0) @binding(3) var spectrum: texture_2d<f32>;

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

fn bucket(at: f32, buckets: i32) -> f32 {
    let low = i32(floor(at));
    let t = at - floor(at);
    let a = textureLoad(spectrum, vec2<i32>(clamp(low, 0, buckets - 1), 0), 0).r;
    let b = textureLoad(spectrum, vec2<i32>(clamp(low + 1, 0, buckets - 1), 0), 0).r;
    return mix(a, b, t);
}

// A column's band level: two taps across its slice of the spectrum.
fn column_level(column: f32, columns: f32) -> f32 {
    let buckets = i32(textureDimensions(spectrum).x);
    let span = f32(buckets - 1) / columns;
    let a = bucket((column + 0.3) * span, buckets);
    let b = bucket((column + 0.7) * span, buckets);
    return clamp((a + b) * 0.5, 0.0, 1.0);
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
    let columns = max(g.params[0].x, 1.0);
    let fall = g.params[0].y;
    let streak = g.params[0].z;
    let layers = i32(g.params[0].w + 0.5);
    let glow = g.params[1].x;
    let flash = g.params[1].y;
    let vignette = g.params[1].z;
    let brightness = g.params[1].w;

    let u = position.x / g.resolution.x;
    let v = position.y / g.resolution.y;

    let t = u * columns;
    let column = floor(t);
    let level = column_level(column, columns);

    // The lit thread down the middle of the column.
    let thread = exp(-pow((fract(t) - 0.5) * 3.2, 2.0));

    // The column's hue: its place on the frequency axis, leaned by the
    // centroid — the same walk `bars` makes.
    let tone = accent(clamp(u * 0.8 + g.centroid * 0.2, 0.0, 1.0));

    var light = 0.0;
    for (var layer = 0; layer < 4; layer++) {
        if layer >= layers {
            break;
        }
        let key = hash21(vec2<f32>(column * 4.0 + f32(layer), g.seed));
        let pace = fall * (0.55 + 0.9 * hash21(vec2<f32>(key * 251.0, g.seed + 1.0)));

        // Where this layer's drop head is. Constant speed: the level scales
        // the light below, never this position.
        let head = fract(key * 17.0 + g.time * pace);

        // The trail hangs above the head and fades along `streak`; the head
        // itself is a hot point scaled by `glow`.
        let behind = fract(head - v);
        let tail = exp(-behind / max(streak * (0.25 + 0.75 * level), 0.01));
        let head_light = exp(-behind * 90.0) * glow;

        light += (tail * 0.6 + head_light) * (0.4 + 0.6 * key);
    }

    // A quiet band does not rain: the column's level gates everything, and a
    // hit briefly lights the whole sky of rain.
    var color = tone * light * thread * level * (1.0 + flash * g.onset);

    let p = (position.xy - 0.5 * g.resolution) / min(g.resolution.x, g.resolution.y);
    color *= 1.0 - vignette * smoothstep(0.5, 1.1, length(p));
    color *= brightness * (0.25 + 0.75 * g.rms_env);
    color = max(color, vec3<f32>(0.0));

    let alpha = clamp(max(color.r, max(color.g, color.b)), 0.0, 1.0);
    return vec4<f32>(color, alpha);
}
