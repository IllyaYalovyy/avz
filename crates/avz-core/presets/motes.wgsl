// motes — drifting dust, barely lit (issue #46).
//
// Sparse dust motes in two depth layers, drifting slowly on one seeded
// heading, the way dust crosses a shaft of afternoon light. The mids lift
// them gently into view, the air band twinkles them on a frame-quantized
// clock, and hits do nothing at all — this is the quietest preset avz ships,
// an atmosphere over a background rather than a visual.
//
// Determinism: drift is linear in frame time along a fixed heading; every
// mote is a hash of its lattice cell (AGENTS.md). Output is linear and
// **premultiplied** (VISION.md §5.3); silence settles the dust to nothing.
//
//   params[0].x  density       params[1].x  twinkle
//   params[0].y  size          params[1].y  tint
//   params[0].z  drift         params[1].z  glow
//   params[0].w  heading       params[1].w  brightness

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
    let density = g.params[0].x;
    let size = g.params[0].y;
    let drift = g.params[0].z;
    let heading = g.params[0].w;
    let twinkle = g.params[1].x;
    let tint = g.params[1].y;
    let glow = g.params[1].z;
    let brightness = g.params[1].w;

    let p = (position.xy - 0.5 * g.resolution) / min(g.resolution.x, g.resolution.y);
    let dir = vec2<f32>(cos(heading * TAU), sin(heading * TAU));
    let tick = floor(g.time * 7.0);

    var color = vec3<f32>(0.0);
    for (var layer = 0; layer < 2; layer++) {
        let l = f32(layer);
        // The far layer is finer, slower, and dimmer.
        let cells = (11.0 + 9.0 * l) / max(density, 1e-3);
        let q = p * cells - dir * g.time * drift * (1.0 - 0.4 * l) * cells + l * 53.0;
        let cell = floor(q);

        let deal = hash21(cell + g.seed);
        if deal > 0.32 {
            continue;
        }

        // The mote: jittered off its cell center, a soft speck with a faint
        // halo, twinkling with the air band.
        let at = vec2<f32>(
            hash21(cell + vec2<f32>(g.seed, 3.1)),
            hash21(cell + vec2<f32>(7.9, g.seed)),
        ) * 0.6 + 0.2;
        let d = length(fract(q) - at) / cells;

        let radius = size * (0.6 + deal) * (1.0 - 0.35 * l);
        let speck = exp(-pow(d / max(radius, 1e-5), 2.0));
        let halo = glow * radius / (d + radius) * 0.15;

        let wink = 0.7 + 0.3 * hash21(cell + tick);
        let sparkle = mix(1.0, wink, clamp(twinkle * g.air_env * 1.5, 0.0, 1.0));

        let dust = mix(vec3<f32>(1.0), accent(fract(deal * 9.0 + g.centroid * 0.3)), tint);
        color += dust * (speck + halo) * sparkle * (1.0 - 0.4 * l);
    }

    // The mids lift the dust into view; silence settles it.
    color *= brightness * (0.2 + 0.5 * g.mid_env + 0.3 * g.rms_env) * g.rms_env;
    color = max(color, vec3<f32>(0.0));

    let alpha = clamp(max(color.r, max(color.g, color.b)), 0.0, 1.0);
    return vec4<f32>(color, alpha);
}
