// horizon — a synthwave sunset: a scanlined sun over a perspective grid
// (issue #36).
//
// The terrain-flyover corner of the VISION §12 backlog, taken as the genre
// classic instead of a raymarch: a striped sun sitting on the horizon, a
// perspective floor grid scrolling toward the viewer, sparse stars above.
// The kick (`bass_env`) pulses the grid lines and swells the sun, a hit
// (`onset`) flares the horizon line itself, the air band twinkles the stars,
// and the centroid leans the palette walk.
//
// Determinism: the scroll is linear in frame time, the stars and their
// twinkle are seeded hashes on a frame-quantized clock, nothing integrates
// (AGENTS.md). Output is linear and **premultiplied** (VISION.md §5.3);
// silence fades the scene to the backdrop.
//
//   params[0].x  sun_size      params[1].x  pulse
//   params[0].y  scanlines     params[1].y  flare
//   params[0].z  grid          params[1].z  stars
//   params[0].w  speed         params[1].w  vignette
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
    let sun_size = g.params[0].x;
    let scanlines = g.params[0].y;
    let grid = g.params[0].z;
    let speed = g.params[0].w;
    let pulse = g.params[1].x;
    let flare = g.params[1].y;
    let stars = g.params[1].z;
    let vignette = g.params[1].w;
    let brightness = g.params[2].x;

    // Centered, aspect-corrected, y up. The horizon sits a little below the
    // middle, which is where album art wants its sky.
    var p = (position.xy - 0.5 * g.resolution) / min(g.resolution.x, g.resolution.y);
    p.y = -p.y;
    let horizon = -0.08;

    var color = vec3<f32>(0.0);

    if p.y < horizon {
        // The floor: a perspective grid. Depth is the reciprocal of the drop
        // below the horizon; lines scroll toward the viewer at `speed`.
        let drop = horizon - p.y;
        let z = 1.0 / max(drop, 1e-3);
        let gx = p.x * z * grid * 0.35;
        let gz = z * grid * 0.35 + g.time * speed * grid * 0.35;

        // A line where either coordinate is near an integer, thinning with
        // distance so the mesh converges cleanly, pulsing with the kick.
        let wx = abs(fract(gx) - 0.5);
        let wz = abs(fract(gz) - 0.5);
        let thin = clamp(drop * 5.0, 0.15, 1.0);
        let line = exp(-pow(wx / (0.045 * thin), 2.0)) + exp(-pow(wz / (0.045 * thin), 2.0));

        let near_fade = smoothstep(0.0, 0.06, drop);
        let beat = 0.55 + 0.45 * g.low_mid_env + pulse * 0.6 * g.bass_env;
        color += accent(0.15 + g.centroid * 0.3) * line * beat * near_fade;
    } else {
        // The sky: a scanlined sun swelling gently with the loudness, its
        // stripes thickening toward its base, palette-lit bottom to top.
        let radius = sun_size * (1.0 + 0.12 * g.rms_env);
        let center = vec2<f32>(0.0, horizon + radius * 0.75);
        let d = length(p - center);

        let inside = smoothstep(radius, radius - 0.01, d);
        let band = (p.y - (center.y - radius)) / max(radius * 2.0, 1e-3);
        var stripe = 1.0;
        if scanlines > 0.5 {
            let cut = 0.5 + 0.5 * cos(band * scanlines * 6.2831853);
            stripe = smoothstep(0.15 + 0.5 * (1.0 - band), 0.6, cut);
        }
        let sun = inside * stripe;
        let glowring = exp(-pow(max(d - radius, 0.0) * 9.0, 1.5)) * 0.5;

        color += accent(clamp(band * 0.8 + 0.1, 0.0, 1.0)) * (sun + glowring)
            * (0.75 + 0.25 * g.mid_env);

        // Sparse stars on a hash lattice, twinkling with the air band on a
        // frame-quantized clock, hidden near the sun.
        let cell = floor(p * 34.0 + vec2<f32>(g.seed * 13.0, 0.0));
        let has = hash21(cell + g.seed);
        if has > 0.93 {
            let sp = fract(p * 34.0 + vec2<f32>(g.seed * 13.0, 0.0)) - 0.5;
            let tick = floor(g.time * 10.0);
            let wink = 0.5 + 0.5 * hash21(cell + tick);
            let dot_light = exp(-dot(sp, sp) * 90.0);
            color += vec3<f32>(0.9) * dot_light * stars
                * (0.2 + 0.8 * g.air_env * wink) * smoothstep(radius * 1.4, radius * 2.2, d);
        }
    }

    // The horizon line itself: always faintly lit, flaring on the beat.
    let seam = exp(-pow((p.y - horizon) * 60.0, 2.0));
    color += accent(0.6 + 0.2 * g.centroid) * seam * (0.35 + flare * g.onset);

    color *= 1.0 - vignette * smoothstep(0.45, 1.05, length(p));
    color *= brightness * g.rms_env;
    color = max(color, vec3<f32>(0.0));

    let alpha = clamp(max(color.r, max(color.g, color.b)), 0.0, 1.0);
    return vec4<f32>(color, alpha);
}
