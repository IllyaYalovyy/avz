// ink — a reaction-diffusion field grown on the previous frame (VISION.md §6).
//
// Ink is dropped into still water, spreads, feeds on the clean water around it,
// starves where the water is already black, and dissolves everywhere else. What
// is left is a slow, brooding marble that never repeats. `rms_env` is the growth
// rate: the louder the passage, the harder the ink invades, and a silence
// dissolves it back to the backdrop.
//
// This is the last preset RFC-001 NG1 deferred, and — as that non-goal predicted —
// it needs no new binding. A reaction-diffusion reads the previous frame, and the
// previous frame already exists: `"needs_feedback": true` in `ink.json` binds it
// at `@binding(1)`, transparent black before frame 0 — clear water, since the
// field this shader grows *is* that texture's alpha. `ink` is three files in
// `presets/` and one registry row, which is G3 holding for the sixth and last
// preset.
//
// **The field lives in the alpha channel.** The layer a preset draws is
// premultiplied (VISION.md §5.3): the RGB is the light it emits and the alpha is
// how much of the backdrop that light hides. For ink those are not two facts but
// one — the alpha *is* the ink's density, and the light is what that density
// looks like under the palette. So the state this shader carries from frame to
// frame is exactly `previous.a`, and the RGB is recomputed from it every frame
// rather than fed back. Two consequences worth knowing: a palette change repaints
// the ink instead of smearing the old colors into it, and the state is quantized
// once per frame to the 8 bits of a linear alpha channel (sRGB encodes the color
// channels, never alpha) rather than once per reaction step.
//
// **The model.** One species, which is Gray-Scott with its solvent eliminated:
//
//     u = 1 - crowd * blur(v)        the clean water left in the neighbourhood
//     v' = v + diffusion * (blur(v) - v),  then `steps` of
//     v' = v + growth * v*v*u - dissolve * v
//
// `v*v*u` is the autocatalysis — ink makes ink, but only where there is water to
// make it out of — and `-dissolve * v` is the ink giving up. `crowd` above 1 is
// what keeps the frame from filling: a pixel whose neighbourhood is already dense
// has *negative* water, stops growing, and dissolves, so a blob hollows out and
// its front eats outward. That is the whole reaction-diffusion look, and it is
// why the field settles into a marble rather than a black sheet.
//
// **Why `steps` is a reaction sub-step and not a render pass.** The issue asked
// for "a couple of feedback iterations per output frame". One Euler step of the
// reaction at 30 fps evolves the field almost not at all; several give the fronts
// their speed and their sharpness (a bistable front travels at about the square
// root of the reaction rate). The diffusion, though, cannot be iterated here: it
// is a convex mix toward a 3×3 blur, and mixing toward a *frozen* blur twice only
// gets closer to that same blur. Iterating it for real would mean drawing the
// preset `steps` times per frame — a change to the render contract, a full-frame
// copy per iteration, and the 8-bit state quantized `steps` times instead of
// once. So the diffusion takes one step per frame, at the lattice's own stability
// limit, and the reaction — which is local, stiff, and where the pattern comes
// from — takes `steps` of them. Recorded in RFC-001 NG1.
//
// **The lattice is the pixel grid.** The field is one value per texel, so the
// diffusion stencil reaches exactly one texel and no further: a wider cell leaves
// wavelengths between a texel and a cell that the stencil cannot see and the
// bistable reaction grows into a moiré hatch. That makes the marble the only
// length in this shader that is measured in pixels rather than in fractions of
// the short edge — a 1080p render draws a finer one, and takes six times as many
// frames to grow it across the frame, as a `--sample` preview at 320x180 does.
// Everything else — the drop, the blooms, the stirring, the dish — is a fraction
// of the short edge and previews faithfully.
//
// Audio mapping (VISION.md §6): `rms_env` is the growth rate, and the only thing
// that decides whether the ink lives or dissolves; `bass_env` stirs the water, so
// the low end drags the field along a flow; `low_mid_env` feeds new ink in;
// `mid_env` twists the flow; `high_env` lights the fronts where the ink is eating
// outward; `air_env` shimmers the wet surface; `flux` lifts the whole field;
// `onset` drops ink into the middle of the frame; `centroid` and `time` together
// walk the hue along the palette.
//
// Determinism (AGENTS.md): `time` is `frame_index / fps` and the only clock, and
// every random number is a hash of a noise lattice cell and `seed` — no
// `sin`-based hash, because those differ between drivers. The field at frame `N`
// is a function of `(seed, features[0..N])` and nothing else, which is what the
// issue asks for and what `ink_is_reproducible_from_its_seed_and_its_frames`
// asserts. Like every feedback preset, `--sample 1:00..1:10` starts its field
// from clear water at 1:00 rather than inheriting the ten minutes before it.
//
// Output is linear — the layer target is `Rgba8UnormSrgb` and encodes on write,
// and the feedback texture decodes on sample.
//
// The `params` slots below are declared in `ink.json`, which is the only place
// their names, defaults, and ranges live.
//
//   params[0].x  diffusion     params[1].x  steps       params[2].x  flash
//   params[0].y  growth        params[1].y  seed_rate   params[2].y  hue_cycle
//   params[0].z  dissolve      params[1].z  detail      params[2].z  vignette
//   params[0].w  crowd         params[1].w  swirl       params[2].w  brightness

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

// Last frame's layer, as this shader left it. Present because `ink.json` declares
// `needs_feedback`. Only the alpha channel is read: that is the field.
@group(0) @binding(1) var previous: texture_2d<f32>;
@group(0) @binding(2) var previous_sampler: sampler;

/// The most reaction sub-steps `steps` may ask for. Mirrors the `max` in
/// `ink.json`: WGSL needs a bound it can see to unroll, and a runaway uniform must
/// not become a runaway loop.
const MAX_STEPS: u32 = 8u;

// The fullscreen triangle: three vertices, no vertex buffer, no index buffer.
// It covers the clip cube with one primitive, so no seam runs down the middle.
@vertex
fn vs_main(@builtin(vertex_index) vertex: u32) -> @builtin(position) vec4<f32> {
    let index = i32(vertex);
    let x = f32(index / 2) * 4.0 - 1.0;
    let y = f32(index & 1) * 4.0 - 1.0;
    return vec4<f32>(x, y, 0.0, 1.0);
}

// A seeded hash in 0..1, the same one `pulse`, `nebula`, and `kaleido` use. Two
// dimensions in, one out, no trigonometry.
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

// The accent ramp: `pal[1]` through `pal[4]`, walked by `t` in 0..1.
//
// `pal[0]` is the background and stays out of it, so a palette's darkest color
// never becomes a highlight.
fn accent(t: f32) -> vec3<f32> {
    let x = clamp(t, 0.0, 1.0) * 3.0;
    let stop = min(u32(x), 2u);
    return mix(g.pal[stop + 1u].rgb, g.pal[stop + 2u].rgb, x - f32(stop));
}

// The accent ramp made cyclic: `phase` walks up the palette and back down it
// rather than snapping from `pal[4]` to `pal[1]` once a second, as `kaleido` does.
fn cycle(phase: f32) -> vec3<f32> {
    return accent(abs(2.0 * fract(phase) - 1.0));
}

// The field at `uv`, which is the alpha of the previous frame and nothing else.
fn field(uv: vec2<f32>) -> f32 {
    return textureSample(previous, previous_sampler, uv).a;
}


@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    // Two frames: `uv` in 0..1 with y down, for sampling the previous frame, and
    // `p` centered and aspect-corrected with y up, for the geometry. The short
    // edge of `p` spans -0.5..0.5, so every length below is resolution-free.
    let uv = position.xy / g.resolution;
    var p = (position.xy - 0.5 * g.resolution) / min(g.resolution.x, g.resolution.y);
    p.y = -p.y;
    let r = length(p);

    let diffusion = g.params[0].x;
    let growth = g.params[0].y;
    let dissolve = g.params[0].z;
    let crowd = g.params[0].w;
    let steps = u32(clamp(g.params[1].x, 1.0, f32(MAX_STEPS)));
    let seed_rate = g.params[1].y;
    let detail = g.params[1].z;
    let swirl = g.params[1].w;
    let flash = g.params[2].x;
    let hue_cycle = g.params[2].y;
    let vignette = g.params[2].z;
    let brightness = g.params[2].w;

    // The seed slides the whole noise lattice, so two seeds are two marbles
    // rather than the same one shifted by a texel.
    let origin = vec2<f32>(g.seed * 137.0, g.seed * 71.0);

    // The dish the ink is in: it cannot grow near the walls. The vignette is a
    // starved rim rather than a darkened one, because the alpha this shader
    // writes is the field itself — dimming the corners' light without taking
    // their ink would leave them hiding the backdrop behind black.
    let dish = 1.0 - vignette * smoothstep(0.35, 0.95, r);

    // The lattice the ink grows on is the frame's own pixel grid, one cell to a
    // texel: the field is stored one value per texel, and a cell wider than that
    // leaves wavelengths the stencil below cannot see for the bistable reaction to
    // grow into a moiré hatch.
    let texel = 1.0 / g.resolution;

    // The water is stirred: a curl of value noise, turning slowly on its own,
    // leaned on by the bass and twisted by the mids. Two cells is as far as the
    // field may travel in a frame, or it tears.
    let stir = p * 1.6 + origin.yx + vec2<f32>(0.0, g.time * 0.05);
    let flow = vec2<f32>(
        value_noise(stir) - 0.5,
        value_noise(stir + vec2<f32>(7.3, 2.1)) - 0.5,
    );
    let twist = 0.5 + 0.5 * g.mid_env;
    let curl = vec2<f32>(flow.y, -flow.x) * twist + flow * (1.0 - twist);
    let advect = curl * swirl * (0.25 + 0.75 * g.bass_env) * 2.0 * texel;
    // `advect` was built with y up; `uv` runs y down.
    let source = uv + advect * vec2<f32>(1.0, -1.0);

    // The 3x3 blur the diffusion mixes toward, and the neighbourhood density the
    // reaction eats out of. The weights are the binomial tent — 4, 2, 1 over 16 —
    // and they sum to 1, so `blur - here` is a discrete Laplacian and
    // `mix(here, blur, diffusion)` is a convex mix, stable for any `diffusion` in
    // 0..1 and, at `diffusion = 0`, exactly no diffusion at all.
    //
    // The tent rather than the flat eight-neighbour mean, because it has a *zero*
    // at both Nyquist frequencies. The reaction below is bistable — it amplifies
    // whatever it is given, and what it is given at the finest scale is the
    // difference between two adjacent texels, the last bit of an 8-bit alpha
    // rounded two ways. Against a kernel with any gain left at Nyquist, the field
    // grows a hatch of pixel noise over everything; against this one, the mode is
    // gone before the reaction ever sees it.
    //
    // The activator is a texel and the inhibitor is its neighbourhood: a short
    // range against a long one, which is what makes a blob hollow out from the
    // middle as its front eats outward, rather than fill.
    let dx = vec2<f32>(texel.x, 0.0);
    let dy = vec2<f32>(0.0, texel.y);
    let here = field(source);
    let orthogonal = field(source - dy) + field(source - dx)
        + field(source + dx) + field(source + dy);
    let diagonal = field(source - dx - dy) + field(source + dx - dy)
        + field(source - dx + dy) + field(source + dx + dy);
    let blur = 0.25 * here + 0.125 * orthogonal + 0.0625 * diagonal;

    // Loudness is the growth rate (VISION.md §6). The floor is what lets a hit in
    // near-silence land at all; it is far under the rate the ink needs to survive
    // its own dissolving, so a quiet passage still clears the frame.
    let rate = growth * (0.02 + 0.98 * g.rms_env) * dish;

    // New ink: a sparse field of blooms the low mids feed, drifting so the marble
    // never settles, and a drop in the middle of the frame on every hit. Both are
    // starved at the rim with everything else.
    let bloom = smoothstep(
        0.62,
        0.92,
        value_noise(p * detail + origin + vec2<f32>(0.017, -0.011) * g.time),
    );
    let feed = seed_rate * bloom * (0.10 + 0.90 * g.low_mid_env) * dish;
    // `onset` is 1.0 on exactly the frame the flux peaked (`analysis::onset`), so
    // the drop lands on the beat and not after it. Bounded inside `r < 0.34`: the
    // ink leaves that disc by diffusing, which is the only way it can. The
    // falloff is written as `1 - smoothstep` rather than as a smoothstep with its
    // edges reversed, which WGSL leaves indeterminate.
    let splash = 1.0 - smoothstep(0.0, 0.34, r);
    let drop = g.onset * flash * splash * (0.35 + 0.65 * bloom) * dish;

    // Diffuse once — a convex mix toward the blur, which is as far as one frame of
    // a 3x3 lattice can carry the field — then react `steps` times.
    var v = clamp(mix(here, blur, diffusion) + feed + drop, 0.0, 1.0);
    let water = 1.0 - crowd * blur;
    for (var step = 0u; step < MAX_STEPS; step += 1u) {
        if step >= steps {
            break;
        }
        v = clamp(v + rate * v * v * water - dissolve * v, 0.0, 1.0);
    }

    // The front: where this frame's ink and its neighbourhood disagree is where it
    // is eating outward, and that edge is what the highs light up.
    let front = clamp(abs(v - blur) * 5.0, 0.0, 1.0);

    // The hue walks the palette with the centroid, with the ink's own density, and
    // with `time`, which is what keeps a held chord from freezing the color.
    let phase = g.centroid + g.time * hue_cycle + 0.30 * v;
    let deep = g.pal[0].rgb;
    let wet = cycle(phase);
    let burn = cycle(phase + 0.5);

    // Light, as a function of the density and nothing carried over: thin ink is
    // wet and colored, thick ink is the palette's own darkness, and the fronts
    // between them burn.
    var light = mix(deep, wet * 0.85, smoothstep(0.10, 0.55, v));
    light = mix(light, deep, smoothstep(0.55, 0.95, v));
    light += burn * front * (0.20 + 0.80 * g.high_env);
    light += vec3<f32>(value_noise(p * 26.0 + origin) * g.air_env * 0.14);
    light += wet * g.flux * 0.12;
    light = clamp(light * brightness, vec3<f32>(0.0), vec3<f32>(1.0));

    // Premultiplied by the coverage, which is the ink itself: the RGB can never
    // exceed the alpha, however bright `brightness` makes the light, so `ink`
    // cannot blow the frame out and cannot hide the backdrop behind black.
    return vec4<f32>(light * v, v);
}
