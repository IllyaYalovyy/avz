// kaleido — a mirrored fold and a walking hue (VISION.md §6).
//
// The frame is cut into `segments` wedges around its centre and every wedge is
// made a reflection of its neighbour, so whatever is drawn inside one is drawn
// symmetrically in all of them: petals, rings, and the grain of the glass they
// are cut from. The fold turns, the rings travel outward, and the palette walks
// under both. Symmetric, hypnotic.
//
// This is the preset RFC-001 NG1 said would need no new binding, and it does not:
// the fold is a function of the fragment's own polar coordinates and the uniform
// every preset receives. `kaleido` is three files in `presets/` and one registry
// row — G3 holding for the fifth preset, with no optional texture declared.
//
// Audio mapping (VISION.md §6): `bass_env` pumps the fold toward the viewer on a
// kick; `low_mid_env` leans on the spin; `mid_env` grinds the grain into the
// glass; `high_env` lights the facet edges; `air_env` shimmers the fine texture;
// `flux` lifts the whole frame; `onset` flares it from the centre; `centroid`
// and `time` together walk the hue along the palette; `rms_env` is the overall
// brightness, so a silent passage leaves the backdrop alone.
//
// Determinism (AGENTS.md): `time` is `frame_index / fps`, and it reaches the
// picture through exactly three knobs — `spin`, `drift`, and `hue_cycle`. Set all
// three to zero and this shader is a pure function of its features, which is what
// `the_only_clocks_kaleido_reads_are_the_three_knobs_that_name_one` asserts.
// Every random number is a hash of a noise lattice cell and `seed`; no `sin`-based
// hash, because those differ between drivers and golden frames would drift with
// them.
//
// The grain is sampled in the *folded* coordinates rather than at the fragment's
// own position. Per-pixel grain — `pulse`'s shimmer — would break the symmetry
// the whole preset is for, and break it invisibly: the frame would still look
// folded, and `a_mirrored_fold_reflects_the_frame_across_its_axis` is what would
// notice.
//
// Output is linear — the layer target is `Rgba8UnormSrgb` and encodes on write.
//
// Output is also **premultiplied** (VISION.md §5.3): the RGB is the light this
// layer emits and the alpha is how much of the backdrop that light hides. The
// dark facets between the shards are transparent, not black, so the palette
// gradient reads through the glass.
//
// The `params` slots below are declared in `kaleido.json`, which is the only
// place their names, defaults, and ranges live.
//
//   params[0].x  segments      params[1].x  ring_count    params[2].x  detail
//   params[0].y  spin          params[1].y  drift         params[2].y  flash
//   params[0].z  hue_cycle     params[1].z  petals        params[2].z  vignette
//   params[0].w  zoom          params[1].w  shard         params[2].w  brightness
//                                                         params[3].x  mirror

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

const TAU: f32 = 6.2831853;

// The fullscreen triangle: three vertices, no vertex buffer, no index buffer.
// It covers the clip cube with one primitive, so no seam runs down the middle.
@vertex
fn vs_main(@builtin(vertex_index) vertex: u32) -> @builtin(position) vec4<f32> {
    let index = i32(vertex);
    let x = f32(index / 2) * 4.0 - 1.0;
    let y = f32(index & 1) * 4.0 - 1.0;
    return vec4<f32>(x, y, 0.0, 1.0);
}

// A seeded hash in 0..1, the same one `pulse` and `nebula` use. Two dimensions
// in, one out, no trigonometry.
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
// rather than snapping from `pal[4]` to `pal[1]` once a second. A hue that
// cycles has to close, and the ramp does not.
fn cycle(phase: f32) -> vec3<f32> {
    return accent(abs(2.0 * fract(phase) - 1.0));
}

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    // Centered, aspect-corrected, y up. The short edge spans -0.5..0.5, so the
    // fold is the same size on any resolution.
    var p = (position.xy - 0.5 * g.resolution) / min(g.resolution.x, g.resolution.y);
    p.y = -p.y;
    let r = length(p);

    let segments = max(g.params[0].x, 3.0);
    let spin = g.params[0].y;
    let hue_cycle = g.params[0].z;
    let zoom = g.params[0].w;
    let ring_count = g.params[1].x;
    let drift = g.params[1].y;
    let petals = g.params[1].z;
    let shard = g.params[1].w;
    let detail = g.params[2].x;
    let flash = g.params[2].y;
    let vignette = g.params[2].z;
    let brightness = g.params[2].w;
    let mirror = g.params[3].x > 0.5;

    // The fold. `angle` is wrapped into one wedge, and — when `mirror` is on —
    // reflected about the wedge's middle, which is what makes each wedge the
    // mirror image of the two beside it. Everything below is a function of `r`
    // and this folded angle alone, so the frame comes out symmetric under a turn
    // of one wedge whatever the shader draws inside it.
    //
    // `spin` is one of the three knobs that read the clock; the low mids lean on
    // it, so the fold turns harder through a thick passage.
    let wedge = TAU / segments;
    let turn = g.time * spin * TAU * (1.0 + 0.6 * g.low_mid_env);
    var angle = atan2(p.y, p.x) + turn;
    angle -= wedge * floor(angle / wedge);
    if mirror {
        angle = abs(angle - 0.5 * wedge);
    }

    // The kick pulls the fold toward the viewer: the radius the geometry is drawn
    // against shrinks, so the rings and petals swell out of the centre.
    let pump = max(1.0 - 0.45 * zoom * g.bass_env, 0.2);
    let radius = r / pump;

    // Back to a point, now in the folded frame. The grain is sampled here rather
    // than at `p`, so it is cut into the glass and folded with it.
    let folded = vec2<f32>(cos(angle), sin(angle)) * radius;
    let origin = vec2<f32>(g.seed * 137.0, g.seed * 71.0);
    let grain = value_noise(folded * (2.0 + 3.0 * detail) + origin);
    let fine = value_noise(folded * (9.0 + 7.0 * detail) + origin.yx);

    // The mids grind the grain in; with nothing in them the glass is nearly clear.
    let glass = mix(0.5, grain, 0.25 + 0.75 * g.mid_env);

    // Petals across the wedge and rings out from the centre. `drift` is the
    // second knob that reads the clock.
    let petal = 0.5 + 0.5 * cos(TAU * petals * angle / wedge);
    let ring = 0.5 + 0.5 * cos(TAU * (radius * ring_count - g.time * drift));

    // A facet is where a petal crosses a ring; `shard` sharpens both into cut
    // glass rather than a soft plaid, and the highs light the edges of the cut.
    let facet = petal * ring;
    let shine = pow(facet, shard);
    let edge = pow(facet, shard * 2.0) * g.high_env * 0.8;

    // The hue walks the palette with the centroid, with the radius, and with
    // `time` — the third and last knob that reads the clock, and the one that
    // makes a still fold hypnotic rather than a wallpaper tile.
    let phase = g.centroid + g.time * hue_cycle + 0.25 * radius;
    let hue = cycle(phase);
    let counter = cycle(phase + 0.5);

    // Light, starting from none: the backdrop is the compositor's layer, not this
    // one's, so the dark facets stay transparent and the gradient reads through.
    var color = hue * shine * (0.35 + 0.65 * glass);
    color += counter * edge;
    color += vec3<f32>(fine * g.air_env * 0.15);
    color += hue * g.flux * 0.10;

    // A hit lands on the beat and not after it: `onset` is 1.0 on exactly the
    // frame the flux peaked (`analysis::onset`), so the flare is the middle of
    // the fold going bright on that frame, with no smoothing in front of it.
    color += counter * g.onset * flash * exp(-3.0 * radius);

    // A vignette keeps the corners out of the way of the fold — and, the frame
    // beneath being a gradient rather than black, opens them onto it.
    color *= 1.0 - vignette * smoothstep(0.35, 0.95, r);

    // Loudness is the last word: the glass breathes with the song, and a silent
    // passage fades out entirely, leaving the backdrop alone.
    color *= brightness * g.rms_env;
    color = clamp(color, vec3<f32>(0.0), vec3<f32>(1.0));

    // Coverage is the brightest channel of the light, as in `pulse`: a saturated
    // shard hides what is under it, a faint one veils it, and the dark glass
    // between them leaves it be.
    let alpha = clamp(max(color.r, max(color.g, color.b)), 0.0, 1.0);
    return vec4<f32>(color, alpha);
}
