// tiles — an equalizer wall of tiles: the spectrum lit floor to ceiling
// (issue #40).
//
// A chunky LED wall filling the whole frame: columns are bands of the
// 512-bucket spectrum (`@binding(3)`, `needs_spectrum`), rows light from the
// floor up to each column's level, the topmost lit tile in every column
// burning hotter — a wall-sized cousin of the `bars` panel and the `meter`'s
// LEDs. Unlit tiles keep a faint ghost so the wall reads as a wall, a hit
// (`onset`) brightens every lit face, and the centroid leans the palette walk
// up the rows.
//
// Determinism: no clock at all — the spectrum texture and the parameters are
// the whole input, and one frame's wall is a pure function of that frame
// (AGENTS.md). Spectrum is read with `textureLoad`, never sampled.
//
// Output is linear and **premultiplied** (VISION.md §5.3); a silent spectrum
// lights nothing but the ghost, which itself fades with the song.
//
//   params[0].x  columns       params[1].x  peak
//   params[0].y  rows          params[1].y  flash
//   params[0].z  bevel         params[1].z  vignette
//   params[0].w  ghost         params[1].w  brightness

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

@group(0) @binding(3) var spectrum: texture_2d<f32>;

fn accent(t: f32) -> vec3<f32> {
    let x = clamp(t, 0.0, 1.0) * 3.0;
    let stop = min(u32(x), 2u);
    return mix(g.pal[stop + 1u].rgb, g.pal[stop + 2u].rgb, x - f32(stop));
}

fn bucket(at: f32, buckets: i32) -> f32 {
    let low = i32(floor(at));
    let t = at - floor(at);
    let a = textureLoad(spectrum, vec2<i32>(clamp(low, 0, buckets - 1), 0), 0).r;
    let b = textureLoad(spectrum, vec2<i32>(clamp(low + 1, 0, buckets - 1), 0), 0).r;
    return mix(a, b, t);
}

// A column's level: four evenly spaced taps over its slice, as `bars` reads.
fn column_level(column: f32, columns: f32) -> f32 {
    let buckets = i32(textureDimensions(spectrum).x);
    let span = f32(buckets - 1) / columns;
    let start = column * span;

    var sum = 0.0;
    for (var tap = 0; tap < 4; tap++) {
        sum += bucket(start + span * (f32(tap) + 0.5) / 4.0, buckets);
    }
    return clamp(sum / 4.0, 0.0, 1.0);
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
    let columns = max(g.params[0].x, 1.0);
    let rows = max(g.params[0].y, 1.0);
    let bevel = g.params[0].z;
    let ghost = g.params[0].w;
    let peak = g.params[1].x;
    let flash = g.params[1].y;
    let vignette = g.params[1].z;
    let brightness = g.params[1].w;

    let u = position.x / g.resolution.x;
    // Rows count from the floor: row 0 is the bottom of the frame.
    let rise = 1.0 - position.y / g.resolution.y;

    let tu = u * columns;
    let tv = rise * rows;
    let column = floor(tu);
    let row = floor(tv);

    // The tile's face: a beveled rectangle, dark grout between neighbours.
    let fu = fract(tu);
    let fv = fract(tv);
    let edge = max(abs(fu - 0.5), abs(fv - 0.5)) * 2.0;
    let face = smoothstep(1.0 - bevel, 1.0 - bevel * 1.8 - 0.04, edge);

    let level = column_level(column, columns);

    // Lit up to the column's level; the topmost lit tile burns hotter, the
    // way the `meter`'s needle does.
    let step_level = (row + 0.5) / rows;
    let lit = select(0.0, 1.0, step_level <= level);
    let is_top = select(0.0, 1.0, abs((row + 0.5) / rows - level) < 1.0 / rows);
    let hot = peak * is_top * lit;

    // The palette climbs the rows — a cool-to-hot palette reads floor-green
    // to ceiling-red — leaned by the centroid.
    let tone = accent(clamp(step_level * 0.85 + g.centroid * 0.15, 0.0, 1.0));

    var color = tone * face
        * (lit * (0.85 + 0.35 * hot) * (1.0 + flash * g.onset) + ghost * (1.0 - lit));

    let p = (position.xy - 0.5 * g.resolution) / min(g.resolution.x, g.resolution.y);
    color *= 1.0 - vignette * smoothstep(0.5, 1.1, length(p));
    color *= brightness * (0.2 + 0.8 * g.rms_env);
    color = max(color, vec3<f32>(0.0));

    let alpha = clamp(max(color.r, max(color.g, color.b)), 0.0, 1.0);
    return vec4<f32>(color, alpha);
}
