// bars — a spectrum analyzer that lives in one corner (issue #31).
//
// The first *panel* preset: a visualization that owns an anchored rectangle
// and leaves every other pixel fully transparent, so a background image or
// video (`background.image` / `background.video`) shows through untouched.
// Everything outside the panel returns alpha 0 before any spectrum is read —
// `a_panel_preset_lights_only_its_panel` holds the shader to exactly that.
//
// Inside the panel: classic analyzer bars. The panel's horizontal axis is the
// spectrum's log-frequency axis (the same 512-bucket texture `ribbons` reads,
// bass at the left), each bar averaging its own slice of it. Bars grow upward
// from the panel's floor; a `glow` halo rises from each bar's tip into the
// unlit part of its column, and never past the panel's edge.
//
// The panel is placed by schema parameters, not by code: `anchor` is the text
// card's nine-grid vocabulary (`config::Position` order — the enum packs as
// its variant index), `width`/`height` are fractions of the frame, `margin` a
// fraction of the short edge. The golden panel test recomputes this same
// arithmetic from the schema defaults, so the schema, this shader, and the
// test cannot drift apart silently.
//
// Audio mapping: the spectrum *is* the motion — no envelope scales the panel,
// because a bar's height already carries the loudness of its band, and a
// silent spectrum draws nothing at all. `centroid` leans the palette walk so
// bright passages tint toward the palette's hot end.
//
// Determinism: nothing here reads `time` or any hash — the spectrum texture
// and the parameters are the whole input, so one frame's picture is a pure
// function of that frame (AGENTS.md).
//
// Output is linear, and **premultiplied** (VISION.md §5.3): RGB is the light
// this layer emits, alpha is how much of the backdrop it hides.
//
//   params[0].x  anchor        params[1].x  bar_count
//   params[0].y  width         params[1].y  gap
//   params[0].z  height        params[1].z  glow
//   params[0].w  margin        params[1].w  brightness

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

// This frame's coarse spectrum: 512 log-spaced buckets from 20 Hz to 16 kHz,
// each in 0..1 after the global normalization (`analysis::spectrum`).
@group(0) @binding(3) var spectrum: texture_2d<f32>;

// The palette's accent ramp, as `ribbons` walks it: pal[1] through pal[4],
// leaving pal[0] to the backdrop so the panel never dissolves into it.
fn accent(t: f32) -> vec3<f32> {
    let x = clamp(t, 0.0, 1.0) * 3.0;
    let stop = min(u32(x), 2u);
    return mix(g.pal[stop + 1u].rgb, g.pal[stop + 2u].rgb, x - f32(stop));
}

// One bucket of the spectrum, interpolated between the two texels `at` falls
// between. `at` is in buckets, and reads off either end clamp to the edge.
fn bucket(at: f32, buckets: i32) -> f32 {
    let low = i32(floor(at));
    let t = at - floor(at);
    let a = textureLoad(spectrum, vec2<i32>(clamp(low, 0, buckets - 1), 0), 0).r;
    let b = textureLoad(spectrum, vec2<i32>(clamp(low + 1, 0, buckets - 1), 0), 0).r;
    return mix(a, b, t);
}

// A bar's level: its slice of the spectrum, averaged over four evenly spaced
// taps. Four fixed taps rather than a loop over the slice: `bar_count` is a
// user's knob, and a loop whose length the user sets would let one parameter
// multiply the frame time (the reasoning `ribbons` applies to `blur`).
fn bar_level(bar: f32, count: f32) -> f32 {
    let buckets = i32(textureDimensions(spectrum).x);
    let span = f32(buckets - 1) / count;
    let start = bar * span;

    var sum = 0.0;
    for (var tap = 0; tap < 4; tap++) {
        let at = start + span * (f32(tap) + 0.5) / 4.0;
        sum += bucket(at, buckets);
    }
    return clamp(sum / 4.0, 0.0, 1.0);
}

// Where the panel's top-left corner sits, from the anchor's nine-grid index:
// column = index % 3 (left, center, right), row = index / 3 (top, center,
// bottom) — `config::Position::ALL` order, which the schema's variant list
// repeats verbatim.
fn panel_origin(anchor: i32, panel: vec2<f32>, margin: f32) -> vec2<f32> {
    let column = anchor % 3;
    let row = anchor / 3;

    var origin = vec2<f32>(0.0);
    switch column {
        case 0: { origin.x = margin; }
        case 1: { origin.x = (g.resolution.x - panel.x) * 0.5; }
        default: { origin.x = g.resolution.x - margin - panel.x; }
    }
    switch row {
        case 0: { origin.y = margin; }
        case 1: { origin.y = (g.resolution.y - panel.y) * 0.5; }
        default: { origin.y = g.resolution.y - margin - panel.y; }
    }
    return origin;
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
    let anchor = i32(g.params[0].x + 0.5);
    let panel = vec2<f32>(
        g.params[0].y * g.resolution.x,
        g.params[0].z * g.resolution.y,
    );
    let margin = g.params[0].w * min(g.resolution.x, g.resolution.y);
    let bar_count = max(g.params[1].x, 1.0);
    let gap = g.params[1].y;
    let glow = g.params[1].z;
    let brightness = g.params[1].w;

    // The panel's claim, and a hard clip at its edge: one branch, and every
    // pixel outside it belongs to the backdrop, whatever the song is doing.
    let origin = panel_origin(anchor, panel, margin);
    let local = position.xy - origin;
    if local.x < 0.0 || local.x > panel.x || local.y < 0.0 || local.y > panel.y {
        return vec4<f32>(0.0);
    }

    // Panel coordinates: `u` across the frequency axis, `rise` up from the
    // panel's floor, both 0..1.
    let u = local.x / panel.x;
    let rise = 1.0 - local.y / panel.y;

    // Which bar this pixel belongs to, and where in the bar's pitch it falls.
    // Half the `gap` is shaved from each side, so bars stay centered.
    let t = u * bar_count;
    let bar = floor(t);
    let pitch = t - bar;
    let half_gap = gap * 0.5;
    if pitch < half_gap || pitch > 1.0 - half_gap {
        return vec4<f32>(0.0);
    }

    let level = bar_level(bar, bar_count);

    // Lit below the bar's tip, with a soft edge about a pixel and a half tall
    // so the tip does not shimmer between frames; a halo above it, scaled by
    // `glow` and dying off within the column.
    let edge = 1.5 / max(panel.y, 1.0);
    let lit = smoothstep(-edge, edge, level - rise);
    let overshoot = max(rise - level, 0.0);
    let halo = glow * level * exp(-overshoot * 14.0) * (1.0 - lit);

    // The frequency axis walks the palette, and the centroid leans the walk
    // toward the hot end when the song brightens. The lit body warms slightly
    // toward its tip so tall bars read as burning, not as flat paint.
    let tone = accent(u * 0.8 + g.centroid * 0.2);
    let body = tone * lit * (0.75 + 0.35 * smoothstep(0.0, 1.0, rise / max(level, 1e-3)));

    var color = (body + tone * halo) * brightness;
    color = max(color, vec3<f32>(0.0));

    // Premultiplied, as the compositor expects: coverage is the brightest
    // channel of the light this pixel emits.
    let alpha = clamp(max(color.r, max(color.g, color.b)), 0.0, 1.0);
    return vec4<f32>(color, alpha);
}
