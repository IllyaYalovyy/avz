// nebula — organic clouds over a feedback trail (VISION.md §6).
//
// A domain-warped fbm flow field lit through the palette, composited over an
// advected, decayed copy of the previous frame. The trail is what makes the
// clouds move like clouds rather than boil in place: each frame drags the last
// one a little way along the flow before adding new light to it.
//
// This is the preset that forces the previous-frame texture (RFC-001 Step 17).
// It asks for it with `"needs_feedback": true` in `nebula.json`; the renderer
// then binds last frame's pixels at `@binding(1)` and a sampler at `@binding(2)`,
// black on the first frame of a render.
//
// Audio mapping (VISION.md §6): `bass_env` churns the flow — faster and more
// turbulent under a kick; `rms_env` is the overall brightness, so a silent
// passage fades to the trail alone; `onset` injects a burst from the centre;
// `centroid` walks the hue along the palette.
//
// Determinism: `time` is `frame_index / fps` and the only clock; every random
// number is a hash of a lattice cell and `seed` (AGENTS.md). No `sin`-based
// hash: those differ between drivers and golden frames would drift with them.
//
// Trails are per-render state. A `--sample 1:00..1:10` therefore starts its
// trails from black at 1:00 rather than inheriting the ten minutes before it;
// the excerpt converges within a second and looks like the full render after
// that.
//
// Output is linear — the render target is `Rgba8UnormSrgb` and encodes on write,
// and the feedback texture decodes on sample, so the blend below is in light.
//
// The `params` slots below are declared in `nebula.json`, which is the only place
// their names, defaults, and ranges live.
//
//   params[0].x  flow_scale       params[1].x  flow_speed
//   params[0].y  turbulence       params[1].y  octaves
//   params[0].z  trail_decay      params[1].z  warp
//   params[0].w  burst_strength   params[1].w  vignette
//                                 params[2].x  brightness

// The uniform contract every preset receives. The Rust side that fills it is
// `render/globals.rs`; the layout it encodes is documented there.
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

// Last frame, as the renderer left it. Present because `nebula.json` declares
// `needs_feedback`; a preset that does not is handed neither of these.
@group(0) @binding(1) var previous: texture_2d<f32>;
@group(0) @binding(2) var previous_sampler: sampler;

/// The most octaves `octaves` may ask for. Mirrors the `max` in `nebula.json`:
/// WGSL needs a bound it can see to unroll, and a runaway uniform must not
/// become a runaway loop.
const MAX_OCTAVES: u32 = 6u;

// The fullscreen triangle: three vertices, no vertex buffer, no index buffer.
// It covers the clip cube with one primitive, so no seam runs down the middle.
@vertex
fn vs_main(@builtin(vertex_index) vertex: u32) -> @builtin(position) vec4<f32> {
    let index = i32(vertex);
    let x = f32(index / 2) * 4.0 - 1.0;
    let y = f32(index & 1) * 4.0 - 1.0;
    return vec4<f32>(x, y, 0.0, 1.0);
}

// A seeded hash in 0..1, the same one `pulse` uses. Two dimensions in, one out,
// no trigonometry.
fn hash21(p: vec2<f32>) -> f32 {
    var q = fract(vec3<f32>(p.xyx) * 0.1031);
    q += dot(q, q.yzx + 33.33);
    return fract((q.x + q.y) * q.z);
}

// Value noise: the four lattice corners around `p`, smoothstepped between.
fn value_noise(p: vec2<f32>) -> f32 {
    let cell = floor(p);
    let offset = fract(p);
    let weight = offset * offset * (3.0 - 2.0 * offset);

    let a = hash21(cell);
    let b = hash21(cell + vec2<f32>(1.0, 0.0));
    let c = hash21(cell + vec2<f32>(0.0, 1.0));
    let d = hash21(cell + vec2<f32>(1.0, 1.0));

    return mix(mix(a, b, weight.x), mix(c, d, weight.x), weight.y);
}

// Fractional Brownian motion: octaves of value noise, each half the amplitude
// and a little over twice the frequency of the last. The irrational-ish 2.02 and
// the offset per octave keep the lattices from lining up into a visible grid.
//
// Normalized by the amplitudes actually summed, so changing `octaves` changes the
// detail rather than the brightness.
fn fbm(point: vec2<f32>, octaves: u32) -> f32 {
    var p = point;
    var amplitude = 0.5;
    var sum = 0.0;
    var total = 0.0;

    for (var octave = 0u; octave < MAX_OCTAVES; octave += 1u) {
        if octave >= octaves {
            break;
        }
        sum += amplitude * value_noise(p);
        total += amplitude;
        p = p * 2.02 + vec2<f32>(11.7, 3.1);
        amplitude *= 0.5;
    }

    return sum / max(total, 1e-4);
}

// The accent ramp: `pal[1]` through `pal[4]`, walked by `t` in 0..1.
//
// `pal[0]` is the background and stays out of it, so a palette's darkest color
// never becomes a highlight.
fn accent(t: f32) -> vec3<f32> {
    let x = clamp(t, 0.0, 1.0) * 3.0;
    let stop = min(u32(x), 2u);
    return mix(g.pal[stop + 1u].rgb, g.pal[stop + 2u].rgb, x - f32(stop));
}

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    // Two frames: `uv` in 0..1 with y down, for sampling the previous frame, and
    // `p` centered and aspect-corrected with y up, for the geometry.
    let uv = position.xy / g.resolution;
    var p = (position.xy - 0.5 * g.resolution) / min(g.resolution.x, g.resolution.y);
    p.y = -p.y;
    let r = length(p);

    let flow_scale = g.params[0].x;
    let turbulence = g.params[0].y;
    let trail_decay = g.params[0].z;
    let burst_strength = g.params[0].w;
    let flow_speed = g.params[1].x;
    let octaves = u32(clamp(g.params[1].y, 1.0, f32(MAX_OCTAVES)));
    let warp = g.params[1].z;
    let vignette = g.params[1].w;
    let brightness = g.params[2].x;

    // The seed slides the whole noise lattice, so two seeds are two nebulae
    // rather than the same one shifted by a texel.
    let origin = vec2<f32>(g.seed * 137.0, g.seed * 71.0);

    // The kick: under a bass swell the field churns faster and warps harder.
    let churn = 1.0 + turbulence * g.bass_env;
    let t = g.time * flow_speed * churn;

    // Domain warp: an fbm offset applied to the coordinates of a second fbm.
    // One noise lookup would give clouds; warping the lookup gives them wisps.
    let q = p * flow_scale + origin;
    let flow = vec2<f32>(
        fbm(q + vec2<f32>(0.0, t), octaves),
        fbm(q + vec2<f32>(5.2, 1.3 - t), octaves),
    ) - 0.5;
    let warped = q + warp * churn * flow;
    let density = fbm(warped + vec2<f32>(1.7 * t, -0.9 * t), octaves);

    // The centroid moves the hue along the palette: bright material reads as the
    // far end of the ramp, dark material as the near end.
    let hue = accent(g.centroid);
    let counter_hue = accent(1.0 - g.centroid);

    // Loudness is the brightness: a silent passage stops feeding the clouds and
    // the trail below fades them out on its own.
    let cloud = smoothstep(0.36, 0.88, density) * (0.25 + 0.75 * g.rms_env) * brightness;

    // The light this frame would emit with no history behind it.
    var emission = g.pal[0].rgb * (0.10 + 0.30 * density);
    emission += hue * cloud;
    // A vignette keeps the corners dark. The trail is an average of emissions, so
    // vignetting the emission vignettes the trail with it.
    emission *= 1.0 - vignette * smoothstep(0.30, 0.95, r);
    emission = clamp(emission, vec3<f32>(0.0), vec3<f32>(1.0));

    // Advect the previous frame along the flow, drifting it slightly inward.
    // `uv` runs y down while `flow` was built y up, so the y component is negated
    // on the way back.
    let drift = vec2<f32>(flow.y, -flow.x) * 0.012 * churn;
    let previous_uv = 0.5 + (uv - 0.5) * 0.997 + drift * vec2<f32>(1.0, -1.0);
    let trail = textureSample(previous, previous_sampler, previous_uv).rgb;

    // An exponential average of the emission, sampled where the flow carried it:
    // the picture lags the clouds by `trail_decay` and smears along their motion,
    // which is what reads as a wisp. Averaging rather than adding is what keeps a
    // hundred frames of overlapping cloud from unioning into white and losing the
    // palette — the steady state of this blend is the emission itself.
    //
    // The cost is that a render fades in from black over the first few frames,
    // frame 0 having no history. A `--sample` excerpt does the same.
    var color = mix(emission, trail, trail_decay);

    // A hit lands on the beat and not after it: `onset` is 1.0 on exactly the
    // frame the flux peaked (`analysis::onset`). The burst is added on top of the
    // average rather than into it, so it flashes at full strength on that frame,
    // then enters the history and rides outward on the flow as it decays.
    let burst = g.onset * burst_strength * exp(-6.0 * r) * (0.35 + 0.65 * density);
    color += counter_hue * burst;

    return vec4<f32>(clamp(color, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
