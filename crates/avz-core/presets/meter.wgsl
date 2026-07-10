// meter — a volume meter that lives in one spot (issue #32).
//
// The second panel preset, sibling of `bars`: an anchored rectangle owns the
// visualization, every pixel outside it is fully transparent, and a
// background image or video shows through untouched.
// `a_panel_preset_lights_only_its_panel` holds it to that.
//
// Inside the panel: a VU-style level meter. `rms_env` — the enveloped
// loudness, so the needle moves musically rather than twitching — fills the
// meter from its floor; `segments` breaks the fill into LEDs, or 0 leaves it
// continuous. The palette walks along the meter's length, so a palette that
// runs cool-to-hot reads exactly like the classic green-amber-red ladder.
// The topmost lit sliver flashes with `onset`, which makes the hits legible
// even in a tiny panel.
//
// Unlike `bars`, a faint `track` marks the unlit remainder, because a meter's
// scale is information: silence should read as "a meter at zero", not as "no
// meter". The track is drawn from the same parameters on every frame, so it
// changes nothing outside the panel and nothing between frames — the panel
// test's silent render carries it too, on both sides of the comparison.
//
// Placement is the same schema vocabulary as `bars`: `anchor` is the text
// card's nine-grid (`config::Position` order, packed as the variant index),
// `margin` a fraction of the short edge. The sides are the meter's own:
// `length` runs along the orientation, `thickness` — a fraction of the short
// edge, so a meter is the same sliver on any aspect — across it.
//
// Determinism: no `time`, no hash — the uniform is the whole input, and one
// frame's picture is a pure function of that frame (AGENTS.md).
//
// Output is linear, and **premultiplied** (VISION.md §5.3).
//
//   params[0].x  anchor        params[1].x  margin
//   params[0].y  orientation   params[1].y  segments
//   params[0].z  length        params[1].z  track
//   params[0].w  thickness     params[1].w  brightness

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

// The palette's accent ramp, as `bars` and `ribbons` walk it: pal[1] through
// pal[4], leaving pal[0] to the backdrop.
fn accent(t: f32) -> vec3<f32> {
    let x = clamp(t, 0.0, 1.0) * 3.0;
    let stop = min(u32(x), 2u);
    return mix(g.pal[stop + 1u].rgb, g.pal[stop + 2u].rgb, x - f32(stop));
}

// Where the panel's top-left corner sits, from the anchor's nine-grid index —
// the same arithmetic as `bars`, and `config::Position::ALL` order.
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
    let vertical = i32(g.params[0].y + 0.5) == 0;
    let length = g.params[0].z;
    let short = min(g.resolution.x, g.resolution.y);
    let thickness = g.params[0].w * short;
    let margin = g.params[1].x * short;
    let segments = g.params[1].y;
    let track = g.params[1].z;
    let brightness = g.params[1].w;

    // The panel: `length` along the orientation, `thickness` across it.
    var panel = vec2<f32>(thickness, length * g.resolution.y);
    if !vertical {
        panel = vec2<f32>(length * g.resolution.x, thickness);
    }

    // The hard clip at the panel's edge, before anything is computed: every
    // pixel outside it belongs to the backdrop, whatever the song is doing.
    let origin = panel_origin(anchor, panel, margin);
    let local = position.xy - origin;
    if local.x < 0.0 || local.x > panel.x || local.y < 0.0 || local.y > panel.y {
        return vec4<f32>(0.0);
    }

    // `rise` runs from the meter's floor to its ceiling: upward when it
    // stands, rightward when it lies down.
    var rise = 1.0 - local.y / panel.y;
    var across = local.x / panel.x;
    if !vertical {
        rise = local.x / panel.x;
        across = local.y / panel.y;
    }

    let level = clamp(g.rms_env, 0.0, 1.0);

    // Lit or track. Segmented, a pixel belongs to the LED whose center decides
    // for the whole segment — a level mid-segment lights it fully or not at
    // all, which is what makes LEDs read as LEDs — and the space between
    // segments stays transparent. Continuous, a soft edge about a pixel and a
    // half wide keeps the tip from shimmering.
    var lit = 0.0;
    var gap = false;
    if segments >= 1.0 {
        let s = rise * segments;
        let inside = fract(s);
        gap = inside < 0.12 || inside > 0.88;
        lit = select(0.0, 1.0, (floor(s) + 0.5) / segments <= level);
    } else {
        let edge = 1.5 / max(select(panel.y, panel.x, !vertical), 1.0);
        lit = smoothstep(-edge, edge, level - rise);
    }
    if gap {
        return vec4<f32>(0.0);
    }

    // The topmost lit sliver is the needle: brighter than the body, and
    // flashing with the onset impulse so hits read even in a tiny panel.
    let tip = smoothstep(0.12, 0.0, abs(level - rise)) * lit;

    // The palette walks the meter's length; a cool-to-hot palette reads as the
    // classic green-amber-red ladder. A slight fade across the thickness keeps
    // the sliver from looking like flat paint.
    let tone = accent(rise);
    let shade = 0.85 + 0.3 * (1.0 - abs(across - 0.5) * 2.0);

    var color = tone * shade * (lit * brightness + tip * (0.6 + 0.8 * g.onset));
    color += tone * track * (1.0 - lit) * 0.5;
    color = max(color, vec3<f32>(0.0));

    // Premultiplied, as the compositor expects.
    let alpha = clamp(max(color.r, max(color.g, color.b)), 0.0, 1.0);
    return vec4<f32>(color, alpha);
}
