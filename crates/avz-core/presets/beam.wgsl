// beam — one dusty light shaft from a top corner (issue #50).
//
// A projector shaft: light entering from the top-left or top-right corner,
// widening and dying as it crosses the frame, swaying almost imperceptibly,
// with dust motes adrift inside the light. The mids brighten the lamp, a hit
// lifts it gently, and silence turns the projector off. Everything outside
// the shaft stays transparent.
//
// Determinism: the sway is a closed sinusoid of frame time; the dust is the
// `motes` lattice masked by the shaft (AGENTS.md). Output is linear and
// **premultiplied** (VISION.md §5.3).
//
//   params[0].x  side          params[1].x  sway
//   params[0].y  tilt          params[1].y  dust
//   params[0].z  width         params[1].z  lift
//   params[0].w  reach         params[1].w  brightness

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
    let side_right = i32(g.params[0].x + 0.5) == 1;
    let tilt = g.params[0].y;
    let width = g.params[0].z;
    let reach = g.params[0].w;
    let sway = g.params[1].x;
    let dust = g.params[1].y;
    let lift = g.params[1].z;
    let brightness = g.params[1].w;

    let short = min(g.resolution.x, g.resolution.y);
    // Aspect-true coordinates with the origin at the shaft's corner.
    var p = (position.xy - vec2<f32>(0.0)) / short;
    let extent = g.resolution / short;
    if side_right {
        p.x = extent.x - p.x;
    }

    // The shaft's direction: `tilt` walks it from hugging the top edge (0)
    // to falling straight down (1), swaying by a fraction of a degree-scale
    // sinusoid so the light feels hung, not printed.
    let angle = tilt * 1.35 + sway * 0.05 * sin(g.time * 0.21 * TAU + g.seed);
    let dir = vec2<f32>(cos(angle), sin(angle));
    let ortho = vec2<f32>(-dir.y, dir.x);

    let along = dot(p, dir);
    let across = dot(p, ortho);
    if along < 0.0 {
        return vec4<f32>(0.0);
    }

    // The wedge: narrow at the corner, opening as it travels, dying with
    // distance. The lamp is the mids; a hit lifts it a little.
    let girth = width * (0.25 + along * 0.9);
    let blade = exp(-pow(across / max(girth, 1e-4), 2.0));
    let travel = exp(-along / max(reach, 1e-3));
    let lamp = (0.35 + 0.65 * g.mid_env) * (1.0 + lift * g.onset);

    var light = blade * travel * lamp;

    // Dust adrift in the light: the `motes` lattice, visible only inside the
    // shaft, drifting slowly along it.
    if dust > 0.0 {
        let q = p * 22.0 - dir * g.time * 0.5;
        let cell = floor(q);
        if hash21(cell + g.seed) < 0.3 {
            let at = vec2<f32>(
                hash21(cell + vec2<f32>(g.seed, 2.2)),
                hash21(cell + vec2<f32>(6.1, g.seed)),
            ) * 0.6 + 0.2;
            let d = length(fract(q) - at);
            light += exp(-pow(d / 0.06, 2.0)) * blade * travel * dust * 0.8;
        }
    }

    let tone = accent(clamp(0.55 + g.centroid * 0.25, 0.0, 1.0));
    var color = tone * light * brightness * (0.2 + 0.8 * g.rms_env);
    color = max(color, vec3<f32>(0.0));

    let alpha = clamp(max(color.r, max(color.g, color.b)), 0.0, 1.0);
    return vec4<f32>(color, alpha);
}
