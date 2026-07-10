// particles — a burst of light on every hit (VISION.md §6).
//
// The song's kicks and snares throw particles out of the middle of the frame.
// Each one flies, slows against the air, falls, dims, and goes out; the highs
// make the ones still burning twinkle. Where nothing has been struck for a
// while, the frame is empty and the backdrop shows through.
//
// This is the preset that forces the onset-history binding (RFC-001 NG1, issue
// #25). It asks for it with `"needs_onsets": true` in `particles.json`; the
// renderer then binds the last 64 hits at or before this frame at `@binding(4)`,
// newest first, each slot the hit's birth time in seconds and its ordinal among
// the song's hits.
//
// Determinism — the whole design (VISION.md §6, AGENTS.md).
//
// A fragment shader sees one frame and carries nothing between draws, so a
// particle that was spawned a second ago has to be *re-derived* here rather than
// remembered. Every particle is therefore a closed form of `(hit, index)`: the
// hit gives it a birth time, the index and a seeded hash give it a direction and
// a speed, and `age = time - birth` gives it everything else. Nothing is
// integrated frame by frame, nothing is stored in a texture between frames, and
// frame `N` never depends on how the driver rounded frame `N-1`. Skip to frame
// 4000 of a song and it draws what a render that passed through frames 0..3999
// would have drawn.
//
// The hashes key on the hit's **ordinal**, never on its slot. A slot is a place
// in a sliding window: slot 3 names a different hit the moment a new one lands,
// and a particle hashed on it would tear across the frame on every kick.
//
// Audio mapping (VISION.md §6): the hits spawn the bursts; `high_env` twinkles
// the particles still in the air; `onset` and `flux` flare the whole frame on
// the beat; `rms_env` is the overall brightness; `centroid` walks the hue along
// the palette.
//
// Output is linear — the layer target is `Rgba8UnormSrgb` and encodes on write.
//
// Output is also **premultiplied** (VISION.md §5.3): the RGB is the light this
// layer emits, and the alpha is how much of the backdrop that light hides. A
// frame with no live burst on it emits nothing, so it hides nothing.
//
// The `params` slots below are declared in `particles.json`, which is the only
// place their names, defaults, and ranges live.
//
//   params[0].x  burst_size       params[1].x  drag
//   params[0].y  lifetime         params[1].y  size
//   params[0].z  speed            params[1].z  glow
//   params[0].w  gravity          params[1].w  sparkle
//                                 params[2].x  brightness
//                                 params[2].y  vignette
//                                 params[2].z  spread

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

// The song's recent hits, newest first: `.x` is the birth time in seconds and
// `.y` the hit's ordinal. An unfilled slot reads a birth a thousand seconds
// before the song began, so the lifetime test below rejects it with no separate
// emptiness flag (`analysis::onset`, `NO_ONSET`).
@group(0) @binding(4) var onsets: texture_2d<f32>;

const TAU: f32 = 6.2831853;

// The slowest particle of a burst, as a fraction of the fastest. A burst whose
// particles all flew at one speed would be an expanding ring rather than a spray.
const SPEED_FLOOR: f32 = 0.25;

// How far out a particle's halo is still worth drawing, in particle radii.
//
// The halo below falls off as `1 / (1 + d²/r²)`, which never quite reaches zero,
// so something has to say where it stops. Six radii is where it has fallen to a
// thirty-seventh of its peak. The exact value it has *there* is subtracted from
// every sample of it, so the halo reaches zero at the cut rather than stepping
// off it, and this constant costs no visible seam.
const GLOW_REACH: f32 = 6.0;

// A seeded hash in 0..1. Two dimensions in, one out, no trigonometry: `sin`
// hashes differ between drivers, and golden frames would drift with them.
fn hash21(p: vec2<f32>) -> f32 {
    var q = fract(vec3<f32>(p.xyx) * 0.1031);
    q += dot(q, q.yzx + 33.33);
    return fract((q.x + q.y) * q.z);
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

// How far a particle of unit launch speed has travelled `age` seconds after it
// was thrown, against a drag proportional to its velocity.
//
// `v(t) = v₀·exp(-k·t)` integrates to `x(t) = v₀·(1 - exp(-k·t)) / k`, which is
// the closed form this preset is built on: no step, no state, no accumulation.
fn travel(age: f32, drag: f32) -> f32 {
    let k = max(drag, 1e-3);
    return (1.0 - exp(-k * age)) / k;
}

// The fullscreen triangle: three vertices, no vertex buffer, no index buffer.
@vertex
fn vs_main(@builtin(vertex_index) vertex: u32) -> @builtin(position) vec4<f32> {
    let index = i32(vertex);
    let x = f32(index / 2) * 4.0 - 1.0;
    let y = f32(index & 1) * 4.0 - 1.0;
    return vec4<f32>(x, y, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    // Centered, aspect-corrected, y up. The short edge spans -0.5..0.5, so a
    // particle is the same size on any resolution.
    var p = (position.xy - 0.5 * g.resolution) / min(g.resolution.x, g.resolution.y);
    p.y = -p.y;

    let burst_size = i32(g.params[0].x);
    let lifetime = max(g.params[0].y, 1e-3);
    let speed = g.params[0].z;
    let gravity = g.params[0].w;
    let drag = g.params[1].x;
    let size = g.params[1].y;
    let glow = g.params[1].z;
    let sparkle = g.params[1].w;
    let brightness = g.params[2].x;
    let vignette = g.params[2].y;
    let spread = g.params[2].z;

    let slots = i32(textureDimensions(onsets).x);

    // The farthest a particle's light reaches from the particle itself. A radius
    // shrinks with age and never exceeds `size`, so this bounds every burst.
    let reach = size * GLOW_REACH;

    var color = vec3<f32>(0.0);
    for (var slot = 0; slot < slots; slot = slot + 1) {
        let hit = textureLoad(onsets, vec2<i32>(slot, 0), 0).xy;
        let birth = hit.x;
        let ordinal = hit.y;
        let age = g.time - birth;

        // A hit the analysis placed after this frame cannot have thrown anything
        // yet — it never happens through `onset_history`, but a preset must not
        // draw a negative age if it ever did.
        if (age < 0.0) {
            continue;
        }
        // Slots descend in birth time, so once one burst has burnt out, so has
        // every slot behind it — including the empty ones, whose thousand-second
        // age is what ends the loop on a song's opening frames.
        if (age > lifetime) {
            break;
        }

        // One number that names this hit and no other, small enough that the
        // hashes below keep their precision late in a long song.
        let key = fract(ordinal * 0.618034 + g.seed);

        // Where the burst was thrown from, and where its unmoving particle would
        // be now: every particle carries the same fall, so the fall is the
        // burst's, not the particle's. That is what makes the cull below exact.
        let jitter = vec2<f32>(
            hash21(vec2<f32>(key * 191.0 + 3.0, 7.0)),
            hash21(vec2<f32>(key * 271.0 + 5.0, 11.0)),
        );
        let origin = (jitter - 0.5) * spread * 0.6;
        let center = origin + vec2<f32>(0.0, -0.5 * gravity * age * age);

        // The burst is a spherical shell between its slowest and fastest
        // particle. A pixel outside that shell is in none of this burst's
        // particles, and skipping it here is what keeps the preset affordable:
        // a frame with six live bursts on it evaluates one or two of them per
        // pixel, not six.
        let flight = travel(age, drag);
        let radius = length(p - center);
        if (radius + reach < speed * SPEED_FLOOR * flight
            || radius - reach > speed * flight) {
            continue;
        }

        // The particle dims and shrinks as it ages, and is gone at `lifetime`.
        let fade = 1.0 - age / lifetime;
        let rad = max(size * (0.4 + 0.6 * fade), 1e-5);
        let cut = rad * GLOW_REACH;

        for (var index = 0; index < burst_size; index = index + 1) {
            let spin = hash21(vec2<f32>(f32(index) + 1.0, key * 337.0 + 13.0));
            let dash = hash21(vec2<f32>(f32(index) + 97.0, key * 419.0 + 17.0));
            let tint = hash21(vec2<f32>(f32(index) + 193.0, key * 523.0 + 19.0));

            // Stratified around the circle rather than hashed onto it: forty
            // independent angles leave gaps and clumps, and a burst wants neither.
            let angle = (f32(index) + spin) / f32(burst_size) * TAU;
            let launch = speed * (SPEED_FLOOR + (1.0 - SPEED_FLOOR) * dash);
            let at = center + vec2<f32>(cos(angle), sin(angle)) * launch * flight;

            let offset = p - at;
            let square = dot(offset, offset);
            if (square > cut * cut) {
                continue;
            }

            // A bright core inside a halo. The halo has its own cut-off value
            // subtracted off it, so it reaches zero exactly where the test above
            // stops looking for it — `glow` scales what is left, and so cannot
            // move the edge the cull was sized against.
            let rr = rad * rad;
            let core = exp(-square / rr);
            let halo = glow * max(rr / (square + rr) - 1.0 / (1.0 + GLOW_REACH * GLOW_REACH), 0.0);

            // The highs are what make a spark glitter rather than glide. Each
            // particle twinkles on its own hashed period, so a burst shimmers
            // instead of blinking as one.
            let twinkle = 1.0 + sparkle * g.high_env
                * sin(g.time * (6.0 + 14.0 * tint) + tint * TAU);
            let energy = (core + halo * 0.35) * fade * fade * max(twinkle, 0.0);

            color += accent(tint * 0.35 + g.centroid * 0.5 + fade * 0.15) * energy;
        }
    }

    // A hit lands on the beat and not after it: `onset` is 1.0 on exactly the
    // frame the flux peaked (`analysis::onset`), so the flare is the whole frame
    // going brighter on that frame, with no smoothing in front of it.
    color *= brightness * (0.85 + 0.5 * g.onset + 0.25 * g.flux);

    // Loudness breathes the frame, but not all the way down: a burst thrown on
    // the last beat of a phrase must still be in the air through the silence
    // after it.
    color *= 0.45 + 0.55 * g.rms_env;

    // A vignette keeps the corners out of the way and opens them onto the
    // backdrop the compositor draws beneath.
    color *= 1.0 - vignette * smoothstep(0.12, 0.75, length(p));
    color = max(color, vec3<f32>(0.0));

    // Coverage is the brightest channel of the light: a saturated spark hides
    // what is under it, a faint halo veils it, and unlit pixels leave it be. The
    // RGB is already the light `alpha` worth of this layer emits, which is what
    // "premultiplied" means, so it is returned as it stands.
    let alpha = clamp(max(color.r, max(color.g, color.b)), 0.0, 1.0);
    return vec4<f32>(color, alpha);
}
