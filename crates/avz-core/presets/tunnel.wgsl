// tunnel — an endless ring tunnel flown at the speed of the song (issue #34).
//
// The classic bore: rings of light receding to a vanishing point, flown
// through at `speed`. The kick (`bass_env`) swells the walls, the mids stripe
// them, and a hit (`onset`) lights the gates as they pass. `fog` sinks the far
// end of the bore into darkness, so the eye stays on what is arriving. From
// the VISION §12 backlog ("tunnel").
//
// Determinism: the only clock is `g.time` (frame_index / fps), travel is
// `time * speed` — never an integral of the song — and per-ring variation is
// a seeded hash of the ring's index (AGENTS.md). Output is linear and
// **premultiplied** (VISION.md §5.3): silence scales everything to nothing,
// leaving the backdrop alone.
//
//   params[0].x  speed         params[1].x  pulse
//   params[0].y  rings         params[1].y  flash
//   params[0].z  stripes       params[1].z  fog
//   params[0].w  twist         params[1].w  brightness

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

// A seeded hash in 0..1 — the same no-trigonometry construction every preset
// uses, because `sin`-based hashes differ between drivers.
fn hash21(p: vec2<f32>) -> f32 {
    var q = fract(vec3<f32>(p.xyx) * 0.1031);
    q += dot(q, q.yzx + 33.33);
    return fract((q.x + q.y) * q.z);
}

// The palette's accent ramp: pal[1] through pal[4], pal[0] left to the backdrop.
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
    let speed = g.params[0].x;
    let rings = g.params[0].y;
    let stripes = g.params[0].z;
    let twist = g.params[0].w;
    let pulse = g.params[1].x;
    let flash = g.params[1].y;
    let fog = g.params[1].z;
    let brightness = g.params[1].w;

    // Centered, aspect-corrected: the bore's mouth fills the short edge.
    let p = (position.xy - 0.5 * g.resolution) / min(g.resolution.x, g.resolution.y);
    let r = max(length(p), 1e-4);
    let angle = atan2(p.y, p.x);

    // The bore: depth is the reciprocal of the radius, so equal-spaced rings
    // in depth crowd toward the vanishing point on screen. Travel is linear
    // in time; the kick does not move the tunnel, it swells it.
    let depth = 0.35 / r + g.time * speed;
    let ring = depth * rings;
    let ring_id = floor(ring);

    // The gates: a bright line at every ring boundary, each ring tinted its
    // own way by a seeded hash so the bore reads as travel rather than strobe.
    let line = exp(-pow((fract(ring) - 0.5) * 3.4, 2.0));
    let tint = accent(fract(hash21(vec2<f32>(ring_id, g.seed)) * 0.7 + g.centroid * 0.3));

    // The walls: stripes around the circumference, twisted along the bore,
    // brightened by the mids and swollen by the kick.
    var wall = 0.0;
    if stripes > 0.5 {
        wall = 0.5 + 0.5 * cos(angle * stripes + depth * twist * TAU);
        wall = pow(wall, 3.0) * (0.15 + 0.5 * g.mid_env);
    }

    // The kick swell widens the lit band of wall around the viewer's radius;
    // a hit lights every gate at once, decaying with the onset impulse.
    let swell = 1.0 + pulse * g.bass_env * smoothstep(0.6, 0.15, r);
    let gate = line * (0.6 + 0.4 * g.low_mid_env) * (1.0 + flash * g.onset);

    // Fog sinks the vanishing point; the mouth of the bore stays lit.
    let depth_fade = 1.0 - exp(-r * mix(12.0, 2.2, fog));

    var color = (tint * gate + accent(g.centroid) * wall) * swell * depth_fade;

    // Loudness is the last word: silence leaves the backdrop alone.
    color *= brightness * g.rms_env;
    color = max(color, vec3<f32>(0.0));

    let alpha = clamp(max(color.r, max(color.g, color.b)), 0.0, 1.0);
    return vec4<f32>(color, alpha);
}
