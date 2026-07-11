// fireflies — a few wandering lights that flare on hits (issue #47).
//
// A handful of flies wandering the frame on slow, closed-form paths — each
// one a sum of two incommensurate oscillations with seeded frequencies and
// phases, so the path is a pure function of frame time and never integrates
// (AGENTS.md). Each fly blinks on its own slow cycle; on a hit, a seeded,
// second-by-second alternating subset flares gently. The mids lift the swarm
// into view, and silence puts it out.
//
// Subtle by design: a dozen faint points over a background, not a particle
// system. Output is linear and **premultiplied** (VISION.md §5.3).
//
//   params[0].x  flies         params[1].x  blink
//   params[0].y  size          params[1].y  flare
//   params[0].z  wander        params[1].z  tint
//   params[0].w  pace          params[1].w  brightness

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
    let flies = i32(g.params[0].x + 0.5);
    let size = g.params[0].y;
    let wander = g.params[0].z;
    let pace = g.params[0].w;
    let blink = g.params[1].x;
    let flare = g.params[1].y;
    let tint = g.params[1].z;
    let brightness = g.params[1].w;

    let p = (position.xy - 0.5 * g.resolution) / min(g.resolution.x, g.resolution.y);
    let t = g.time;

    // Which seeded half of the swarm this second's hit would flare.
    let epoch = floor(t);

    var color = vec3<f32>(0.0);
    for (var k = 0; k < 24; k++) {
        if k >= flies {
            break;
        }
        let fk = f32(k);
        let a = hash21(vec2<f32>(fk, g.seed));
        let b = hash21(vec2<f32>(fk, g.seed + 11.0));
        let c = hash21(vec2<f32>(fk, g.seed + 23.0));

        // The wander: two incommensurate oscillations per axis, slow and
        // seeded, spread over the middle of the frame.
        let w1 = pace * (0.5 + a) * TAU;
        let w2 = pace * (0.9 + b) * TAU * 0.37;
        let home = (vec2<f32>(a, b) - 0.5) * wander * 1.2;
        let at = home
            + vec2<f32>(
                sin(w1 * t + a * TAU) + 0.5 * sin(w2 * t + c * TAU),
                cos(w1 * 0.83 * t + b * TAU) + 0.5 * cos(w2 * 1.13 * t + a * TAU),
            ) * wander * 0.25;

        // The blink: a soft seeded cycle, mostly off — fireflies are shy.
        let cycle = 0.5 + 0.5 * sin(t * blink * (0.6 + 0.8 * c) * TAU + c * TAU);
        var lamp = smoothstep(0.55, 0.95, cycle);

        // The flare: this second's seeded half of the swarm answers a hit,
        // softly, on top of whatever its blink was doing.
        if hash21(vec2<f32>(fk, epoch + g.seed)) > 0.5 {
            lamp += flare * g.onset * 0.6;
        }

        let d = distance(p, at);
        let body = exp(-pow(d / max(size, 1e-5), 2.0));
        let halo = size / (d + size) * 0.12;

        let lamp_color = mix(vec3<f32>(1.0, 0.95, 0.75), accent(fract(a + g.centroid * 0.3)), tint);
        color += lamp_color * (body + halo) * lamp * (0.5 + 0.5 * a);
    }

    // The mids lift the swarm; silence puts it out.
    color *= brightness * (0.3 + 0.7 * g.mid_env) * g.rms_env;
    color = max(color, vec3<f32>(0.0));

    let alpha = clamp(max(color.r, max(color.g, color.b)), 0.0, 1.0);
    return vec4<f32>(color, alpha);
}
