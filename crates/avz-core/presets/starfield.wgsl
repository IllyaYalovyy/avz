// starfield — warp stars: loudness is velocity, every hit streaks the sky
// (issue #35).
//
// Star streaks radiating from the center of the frame, in two parallax
// layers. The flight itself is steady — travel is linear in frame time, so
// the field is deterministic — and the *music* is in the streaks: loudness
// (`rms_env`) stretches them into warp lines, a hit (`onset`) stretches and
// brightens them further, and the air band twinkles the stars that are barely
// moving. Silence collapses the streaks back to still points and fades them.
//
// The sky is a log-polar lattice: a star's lane is an angular sector, its
// position a cell along `log(r)`, so streaks crowd toward the vanishing point
// exactly as a tunnel's rings do. Everything about a star — presence, phase,
// tint — is a seeded hash of its (lane, cell), and twinkle uses a
// frame-quantized time so it flickers without wall-clock nondeterminism
// (AGENTS.md).
//
// Output is linear and **premultiplied** (VISION.md §5.3).
//
//   params[0].x  density       params[1].x  twinkle
//   params[0].y  speed         params[1].y  flash
//   params[0].z  warp          params[1].z  tint
//   params[0].w  streak        params[1].w  vignette
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
    let speed = g.params[0].y;
    let warp = g.params[0].z;
    let streak = g.params[0].w;
    let twinkle = g.params[1].x;
    let flash = g.params[1].y;
    let tint = g.params[1].z;
    let vignette = g.params[1].w;
    let brightness = g.params[2].x;

    let p = (position.xy - 0.5 * g.resolution) / min(g.resolution.x, g.resolution.y);
    let r = max(length(p), 1e-4);
    let angle = atan2(p.y, p.x) / TAU + 0.5;

    // How long a streak is right now: the base length, stretched by loudness
    // and by the decaying hit impulse. This is the whole audio mapping.
    let stretch = streak * (0.2 + warp * g.rms_env + flash * g.onset);

    // Twinkle clock: quantized to twelfths of a second of *frame* time, so a
    // star's flicker is reproducible frame for frame.
    let tick = floor(g.time * 12.0);

    var color = vec3<f32>(0.0);
    for (var layer = 0; layer < 2; layer++) {
        let l = f32(layer);
        let sectors = 70.0 + 50.0 * l;

        // The lane (angular sector) and the cell along the flight direction.
        let lane_at = angle * sectors + l * 13.0;
        let lane = floor(lane_at);
        let lane_key = hash21(vec2<f32>(lane, g.seed + l * 101.0));

        let q = log(r) * 5.0 + g.time * speed * (1.0 + 0.4 * l) + lane_key * 19.0;
        let cell = floor(q);
        let star = hash21(vec2<f32>(cell, lane + g.seed));

        // Most cells are empty; `density` decides how many hold a star.
        if star > clamp(density, 0.0, 3.0) * 0.35 {
            continue;
        }

        // The streak: bright at the head, falling off behind it along the
        // flight direction; thin across its lane.
        let behind = fract(q);
        let tail = exp(-behind / max(stretch, 0.02));
        let thin = exp(-pow((fract(lane_at) - 0.5) * 5.0, 2.0));

        // The twinkle: fast seeded flicker on the air band, strongest when
        // the streaks are short — a warp line does not twinkle.
        let flicker = 0.75 + 0.25 * hash21(vec2<f32>(cell * 7.0 + lane, tick));
        let still = 1.0 - clamp(stretch * 2.0, 0.0, 1.0);
        let sparkle = 1.0 + twinkle * g.air_env * still * (flicker - 0.75) * 4.0;

        // A star's own color: mostly starlight, tinted from the palette.
        let hue = accent(fract(star * 5.0 + g.centroid * 0.3));
        let starlight = mix(vec3<f32>(1.0), hue, tint);

        // Far stars (center) are dim; the vanishing point stays open.
        let fade = smoothstep(0.02, 0.3, r);

        color += starlight * tail * thin * sparkle * fade * (0.35 + 0.65 * star / 0.35);
    }

    color *= 1.0 - vignette * smoothstep(0.5, 1.1, r);
    color *= brightness * (0.15 + 0.85 * g.rms_env);
    color = max(color, vec3<f32>(0.0));

    let alpha = clamp(max(color.r, max(color.g, color.b)), 0.0, 1.0);
    return vec4<f32>(color, alpha);
}
