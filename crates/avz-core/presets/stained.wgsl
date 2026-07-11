// stained — stained glass lit from behind by the bands, re-leaded on every
// hit (issue #42).
//
// A voronoi mosaic of glass panes behind dark lead seams. Each pane is dealt
// one of the five bands by its own hash and glows as that band speaks, so a
// verse lights one constellation of panes and the chorus another. The song's
// hits re-arrange the window: the *ordinal* of the newest hit — read from the
// onset history (`needs_onsets`, `@binding(4)`) — seeds the jitter of every
// pane, so each hit re-leads the glass into a new mosaic and the onset
// impulse flares it while it settles.
//
// Determinism: the generation counter is data from the analysis pass, not
// state accumulated here — skip to frame 4000 and the glass is whatever the
// hits up to that frame made it (the `particles` closed-form rule).
//
// Output is linear and **premultiplied** (VISION.md §5.3); silence dims the
// window to the backdrop.
//
//   params[0].x  cells         params[1].x  glow
//   params[0].y  jitter        params[1].y  flash
//   params[0].z  lead          params[1].z  vignette
//   params[0].w  reshatter     params[1].w  brightness

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

// The song's recent hits, newest first: x is the hit's birth time in seconds,
// y its ordinal among the song's hits; empty slots sit at -1000.
@group(0) @binding(4) var onsets: texture_2d<f32>;

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
    let cells = max(g.params[0].x, 1.0);
    let jitter = g.params[0].y;
    let lead = g.params[0].z;
    let reshatter = g.params[0].w > 0.5;
    let glow = g.params[1].x;
    let flash = g.params[1].y;
    let vignette = g.params[1].z;
    let brightness = g.params[1].w;

    // The window's generation: how many hits the song has landed, straight
    // from the analysis — the newest hit's ordinal plus one, so the very
    // first hit already re-leads the opening window (generation 0).
    var generation = 0.0;
    let newest = textureLoad(onsets, vec2<i32>(0, 0), 0).xy;
    if reshatter && newest.x >= 0.0 && newest.x <= g.time {
        generation = newest.y + 1.0;
    }

    var p = (position.xy - 0.5 * g.resolution) / min(g.resolution.x, g.resolution.y);
    let q = p * cells;
    let home = floor(q);

    // Voronoi over the 3x3 neighbourhood: nearest pane and second nearest,
    // whose difference is the lead seam. The generation salts every feature
    // point, which is what re-leads the window on a hit.
    var f1 = 1e9;
    var f2 = 1e9;
    var pane = vec2<f32>(0.0);
    for (var dy = -1; dy <= 1; dy++) {
        for (var dx = -1; dx <= 1; dx++) {
            let cell = home + vec2<f32>(f32(dx), f32(dy));
            let salt = g.seed + generation * 0.618034;
            let feature = cell
                + vec2<f32>(
                    hash21(cell + vec2<f32>(salt, 1.3)),
                    hash21(cell + vec2<f32>(2.7, salt)),
                ) * jitter;
            let d = distance(q, feature);
            if d < f1 {
                f2 = f1;
                f1 = d;
                pane = feature;
            } else if d < f2 {
                f2 = d;
            }
        }
    }

    // The pane's deal: which band lights it, and which tone it is cut from.
    let deal = hash21(pane * 7.13 + g.seed);
    let band = i32(deal * 4.999);
    let tone = accent(fract(deal * 3.7 + g.centroid * 0.25));

    // Glass: lit by its band, thicker (darker) toward the pane's center seam,
    // flaring when the window has just been re-leaded.
    let thickness = 0.75 + 0.25 * smoothstep(0.0, 0.5, f1);
    let light = band_env(band) * glow * (1.0 + flash * g.onset);

    // Lead: dark seams where the two nearest panes meet.
    let seam = smoothstep(0.0, max(lead, 1e-3), f2 - f1);

    var color = tone * light * thickness * seam;

    color *= 1.0 - vignette * smoothstep(0.5, 1.1, length(p));
    color *= brightness * (0.2 + 0.8 * g.rms_env);
    color = max(color, vec3<f32>(0.0));

    let alpha = clamp(max(color.r, max(color.g, color.b)), 0.0, 1.0);
    return vec4<f32>(color, alpha);
}
