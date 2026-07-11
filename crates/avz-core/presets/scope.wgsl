// scope — an oscilloscope figure: a lissajous curve the bands bend
// (issue #38).
//
// The lissajous/oscilloscope corner of the VISION §12 backlog. One luminous
// curve traced on a dark scope: `x = sin(a·t + φ)`, `y = sin(b·t)`, with
// `b = a + 1` so the figure is the classic closed knot. The bands bend it —
// the kick stretches its horizontal axis, the mids its vertical, the highs
// ripple a fine harmonic along it — the phase turns slowly so the knot
// tumbles, and a hit brightens the beam. Silence lets the beam fade out.
//
// The curve is drawn as distance to `points` line segments sampled along one
// period; frame time is the only clock and there is no hash at all, so the
// figure is exactly reproducible (AGENTS.md). Trigonometry here is geometry,
// not hashing — the banned `sin` is the *hash* construction, which drivers
// disagree on when abused for randomness.
//
// Output is linear and **premultiplied** (VISION.md §5.3).
//
//   params[0].x  complexity    params[1].x  spin
//   params[0].y  points        params[1].y  wobble
//   params[0].z  thickness     params[1].z  flash
//   params[0].w  glow          params[1].w  vignette
//                              params[2].x  brightness

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

fn accent(t: f32) -> vec3<f32> {
    let x = clamp(t, 0.0, 1.0) * 3.0;
    let stop = min(u32(x), 2u);
    return mix(g.pal[stop + 1u].rgb, g.pal[stop + 2u].rgb, x - f32(stop));
}

// Where the beam is at parameter `t`, in centered aspect coordinates.
fn beam(t: f32, order: f32, phase: f32, wobble: f32) -> vec2<f32> {
    // The kick stretches x, the mids stretch y, the highs ripple a fine
    // third-harmonic wobble along both.
    let ax = 0.36 * (0.55 + 0.45 * g.bass_env);
    let ay = 0.36 * (0.55 + 0.45 * g.mid_env);
    let ripple = wobble * 0.05 * g.high_env * sin(t * order * 3.0 + phase * 2.0);

    return vec2<f32>(
        (ax + ripple) * sin(order * t + phase),
        (ay - ripple) * sin((order + 1.0) * t),
    );
}

// Distance from `p` to the segment `a`-`b`.
fn seg(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let ab = b - a;
    let h = clamp(dot(p - a, ab) / max(dot(ab, ab), 1e-8), 0.0, 1.0);
    return length(p - a - ab * h);
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
    let order = max(g.params[0].x, 1.0);
    let points = i32(g.params[0].y + 0.5);
    let thickness = g.params[0].z;
    let glow = g.params[0].w;
    let spin = g.params[1].x;
    let wobble = g.params[1].y;
    let flash = g.params[1].z;
    let vignette = g.params[1].w;
    let brightness = g.params[2].x;

    var p = (position.xy - 0.5 * g.resolution) / min(g.resolution.x, g.resolution.y);
    p.y = -p.y;

    let phase = g.time * spin * TAU;

    // The nearest approach of the beam to this pixel, over one closed period
    // sampled as `points` segments.
    var nearest = 1e9;
    var nearest_t = 0.0;
    var prev = beam(0.0, order, phase, wobble);
    for (var i = 1; i <= points; i++) {
        let t = f32(i) / f32(points) * TAU;
        let next = beam(t, order, phase, wobble);
        let d = seg(p, prev, next);
        if d < nearest {
            nearest = d;
            nearest_t = t;
        }
        prev = next;
    }

    // A scope beam: a hot core and a phosphor halo, brightening on the beat.
    let core = exp(-pow(nearest / max(thickness, 1e-4), 2.0));
    let halo = glow * thickness / (nearest + thickness) * 0.35;
    let energy = (core + halo) * (1.0 + flash * g.onset);

    // The hue walks along the trace and with the centroid, like a beam whose
    // phosphor never quite settles.
    let color_at = fract(nearest_t / TAU + g.centroid * 0.4);
    var color = accent(color_at) * energy;

    color *= 1.0 - vignette * smoothstep(0.45, 1.05, length(p));
    color *= brightness * g.rms_env;
    color = max(color, vec3<f32>(0.0));

    let alpha = clamp(max(color.r, max(color.g, color.b)), 0.0, 1.0);
    return vec4<f32>(color, alpha);
}
