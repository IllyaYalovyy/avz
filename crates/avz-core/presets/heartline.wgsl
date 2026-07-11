// heartline — a quiet line that blips on the beat (issue #53).
//
// One thin trace low in the frame, near-flat, carrying a small EKG blip for
// every hit in the onset history (`needs_onsets`, `@binding(4)`). A blip
// enters at the left the moment its hit lands and travels right, shrinking
// as it goes; spectral flux adds a faint tremor to the baseline, so the line
// is never quite dead while the song plays. Between blips it is just a line
// — the least a visualizer can be and still be alive.
//
// Determinism: a blip's position and size are closed forms of its hit's age
// (the `particles` rule); the tremor rides a frame-quantized clock
// (AGENTS.md). Output is linear and **premultiplied** (VISION.md §5.3).
//
//   params[0].x  line          params[1].x  thickness
//   params[0].y  travel        params[1].y  tremor
//   params[0].z  fade          params[1].z  tint
//   params[0].w  spike         params[1].w  brightness

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
// slots sit a thousand seconds old.
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

// The blip: a sharp rise with a small dip after it, an EKG complex in
// miniature. `s` is the distance ahead of the blip's center, in frame widths.
fn blip(s: f32) -> f32 {
    return exp(-pow(s / 0.010, 2.0)) - 0.5 * exp(-pow((s - 0.022) / 0.014, 2.0));
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
    let line = g.params[0].x;
    let travel = max(g.params[0].y, 0.01);
    let fade = max(g.params[0].z, 0.05);
    let spike = g.params[0].w;
    let thickness = g.params[1].x;
    let tremor = g.params[1].y;
    let tint = g.params[1].z;
    let brightness = g.params[1].w;

    let u = position.x / g.resolution.x;
    let v = position.y / g.resolution.y;

    // The baseline, with a faint flux tremor on a frame-quantized clock.
    let tick = floor(g.time * 30.0);
    let shiver = (hash21(vec2<f32>(floor(u * 90.0), tick + g.seed)) - 0.5)
        * tremor * 0.01 * g.flux;
    var height = 1.0 - line + shiver;

    // Every hit's blip: born at the left edge, travelling right, shrinking
    // with age. A blip that has both left the frame and faded ends the loop
    // early only via the fade horizon — travel alone must not, because a
    // slow traveller can outlive a fast fade.
    let horizon = max(4.6 / fade, 1.2 / travel);
    let slots = i32(textureDimensions(onsets).x);

    var energy = 0.0;
    for (var slot = 0; slot < slots; slot++) {
        let hit = textureLoad(onsets, vec2<i32>(slot, 0), 0).xy;
        let age = g.time - hit.x;
        if age < 0.0 {
            continue;
        }
        if age > horizon {
            break;
        }

        let at = age * travel;
        if at > 1.1 {
            continue;
        }
        let amp = exp(-age * fade);
        height -= spike * amp * blip(u - at);
        energy += amp * exp(-pow((u - at) / 0.05, 2.0));
    }

    // The trace: a hot thin core and a faint glow, brighter where a blip is
    // passing, steady elsewhere.
    let dy = abs(v - height);
    let core = exp(-pow(dy / max(thickness, 1e-4), 2.0));
    let halo = thickness / (dy + thickness) * 0.15;

    let tone = accent(clamp(tint + g.centroid * 0.15, 0.0, 1.0));
    var color = tone * (core + halo) * (0.5 + 0.8 * energy)
        * brightness * (0.25 + 0.75 * g.rms_env);
    color = max(color, vec3<f32>(0.0));

    let alpha = clamp(max(color.r, max(color.g, color.b)), 0.0, 1.0);
    return vec4<f32>(color, alpha);
}
