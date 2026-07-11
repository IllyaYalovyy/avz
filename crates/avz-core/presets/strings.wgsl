// strings — harp strings across the frame, plucked by the hits and left to
// ring down (issue #43).
//
// Vertical strings at even spacing, each with its own seeded pitch. Every hit
// in the onset history (`needs_onsets`, `@binding(4)`) plucks a seeded subset
// of them, and a plucked string vibrates in its fundamental — a damped sine
// of its *age since the hit*, so the motion is a closed form of (hit, string)
// exactly as `particles` derives its bursts (AGENTS.md). Several overlapping
// hits superpose, the way real strings do. The bass leans the whole set, the
// centroid walks the palette across the course, and a string glows brightest
// while it still rings.
//
// Output is linear and **premultiplied** (VISION.md §5.3); silence stills
// and fades the strings to the backdrop.
//
//   params[0].x  strings       params[1].x  pluck
//   params[0].y  tone          params[1].y  thickness
//   params[0].z  damping       params[1].z  glow
//   params[0].w  amplitude     params[1].w  vignette
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

// The song's recent hits, newest first: (birth seconds, ordinal); empty
// slots sit at -1000, which ends the loop below by age.
@group(0) @binding(4) var onsets: texture_2d<f32>;

const TAU: f32 = 6.2831853;
const PI: f32 = 3.14159265;

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

@vertex
fn vs_main(@builtin(vertex_index) vertex: u32) -> @builtin(position) vec4<f32> {
    let index = i32(vertex);
    let x = f32(index / 2) * 4.0 - 1.0;
    let y = f32(index & 1) * 4.0 - 1.0;
    return vec4<f32>(x, y, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let count = max(g.params[0].x, 1.0);
    let tone = g.params[0].y;
    let damping = max(g.params[0].z, 0.05);
    let amplitude = g.params[0].w;
    let pluck = g.params[1].x;
    let thickness = g.params[1].y;
    let glow = g.params[1].z;
    let vignette = g.params[1].w;
    let brightness = g.params[2].x;

    let u = position.x / g.resolution.x;
    let v = position.y / g.resolution.y;

    // Which string this pixel watches, and that string's own voice.
    let s = floor(u * count);
    let rest = (s + 0.5) / count;
    let voice = hash21(vec2<f32>(s, g.seed));
    let pitch = tone * (1.5 + 2.5 * voice);

    // The fundamental: pinned at both ends, widest mid-string.
    let mode = sin(PI * v);

    // A pluck rings until its envelope is inaudible; that horizon also ends
    // the slot loop, since slots descend in birth time and empty slots sit a
    // thousand seconds old.
    let horizon = 5.0 / damping;
    let slots = i32(textureDimensions(onsets).x);

    var swing = 0.0;
    var ring = 0.0;
    for (var slot = 0; slot < slots; slot++) {
        let hit = textureLoad(onsets, vec2<i32>(slot, 0), 0).xy;
        let age = g.time - hit.x;
        if age < 0.0 {
            continue;
        }
        if age > horizon {
            break;
        }
        // Does this hit pluck this string? A seeded deal per (hit, string).
        if hash21(vec2<f32>(hit.y * 13.7 + s, g.seed + 5.0)) > pluck {
            continue;
        }

        let envelope = exp(-age * damping);
        swing += envelope * sin(TAU * pitch * age + voice * TAU);
        ring += envelope;
    }

    // The string, displaced by its superposed plucks; the bass leans the
    // whole course gently even between hits.
    let lean = 0.25 * g.bass_env * mode / count;
    let center = rest + swing * amplitude * 0.45 * mode / count + lean;

    let d = abs(u - center) * count;
    let core = exp(-pow(d / max(thickness, 1e-3), 2.0));
    let halo = glow * thickness / (d + thickness) * 0.3;

    // A ringing string burns; an idle one is a faint filament.
    let energy = 0.18 + clamp(ring, 0.0, 1.5);
    let tint = accent(fract(s / count + g.centroid * 0.3));

    var color = tint * (core + halo) * energy * (1.0 + 0.4 * g.onset);

    let p = (position.xy - 0.5 * g.resolution) / min(g.resolution.x, g.resolution.y);
    color *= 1.0 - vignette * smoothstep(0.5, 1.1, length(p));
    color *= brightness * g.rms_env;
    color = max(color, vec3<f32>(0.0));

    let alpha = clamp(max(color.r, max(color.g, color.b)), 0.0, 1.0);
    return vec4<f32>(color, alpha);
}
