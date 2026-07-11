// ripples — faint rings spreading from every hit (issue #52).
//
// Rain on still water: every hit in the onset history (`needs_onsets`,
// `@binding(4)`) drops a ring at a seeded point, and the ring expands,
// widens, and calms as a closed form of its age — the `particles` rule, worn
// quietly. Overlapping hits lay interference the way real rain does. Nothing
// else is drawn: between hits the frame is still water, which is to say the
// background.
//
// Determinism: a ripple is a pure function of (hit ordinal, age); nothing is
// integrated and no state is kept (AGENTS.md). Output is linear and
// **premultiplied** (VISION.md §5.3).
//
//   params[0].x  speed         params[1].x  scatter
//   params[0].y  calm          params[1].y  tint
//   params[0].z  width         params[1].z  brightness
//   params[0].w  spread

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
// slots sit a thousand seconds old, which ends the loop below.
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
    let calm = max(g.params[0].y, 0.05);
    let width = g.params[0].z;
    let spread = g.params[0].w;
    let scatter = g.params[1].x;
    let tint = g.params[1].y;
    let brightness = g.params[1].z;

    let p = (position.xy - 0.5 * g.resolution) / min(g.resolution.x, g.resolution.y);

    // A ripple older than this has calmed below one percent and so has every
    // slot behind it — including the empty ones.
    let horizon = 4.6 / calm;
    let slots = i32(textureDimensions(onsets).x);

    var light = 0.0;
    var hue = 0.0;
    for (var slot = 0; slot < slots; slot++) {
        let hit = textureLoad(onsets, vec2<i32>(slot, 0), 0).xy;
        let age = g.time - hit.x;
        if age < 0.0 {
            continue;
        }
        if age > horizon {
            break;
        }

        // Where this hit landed: a seeded point in the middle of the water.
        let at = (vec2<f32>(
            hash21(vec2<f32>(hit.y, g.seed)),
            hash21(vec2<f32>(hit.y, g.seed + 17.0)),
        ) - 0.5) * scatter * 1.4;

        // The ring: expanding at `speed`, widening as it `spread`s, calming
        // exponentially; a brief dimple marks the strike itself.
        let radius = age * speed;
        let girth = width * (1.0 + spread * age * 2.0);
        let d = distance(p, at);
        let ring = exp(-pow((d - radius) / max(girth, 1e-4), 2.0));
        let dimple = exp(-age * 7.0) * exp(-pow(d / 0.02, 2.0)) * 0.6;

        let amp = exp(-age * calm);
        light += (ring + dimple) * amp;
        hue += hash21(vec2<f32>(hit.y, g.seed + 31.0)) * (ring + dimple) * amp;
    }

    // Water-colored light: mostly one palette tone, each strike leaning it.
    let lean = hue / max(light, 1e-4);
    let tone = accent(clamp(0.3 + tint * 0.4 * lean + g.centroid * 0.15, 0.0, 1.0));

    var color = tone * light * brightness * (0.3 + 0.7 * g.rms_env);
    color = max(color, vec3<f32>(0.0));

    let alpha = clamp(max(color.r, max(color.g, color.b)), 0.0, 1.0);
    return vec4<f32>(color, alpha);
}
