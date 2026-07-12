# Configuration

Everything avz renders is decided by layered configuration with one fixed
precedence, highest first:

1. **CLI flags** — `--fps 60`, `--palette ember`, `--out video.mp4`
2. **`--set` overrides** — `--set visual.intensity=1.4`, `--set bar_count=48`
3. **`--config` file** — a TOML file, documented below
4. **`--sample`'s one default** — a reduced 720p resolution, so excerpts come
   back in seconds; any config file or flag that names a resolution still wins
5. **Preset defaults** — each parameter's schema default (`avz presets <name>`)
6. **Built-in defaults** — what a bare `avz render song.mp3` uses

Start from a complete, documented template — it emits every key below with its
default and a comment, and feeds straight back into `--config`:

```bash
avz config --example > avz.toml
avz render song.mp3 --config avz.toml
```

Unknown keys are rejected, with a "did you mean" suggestion when the spelling
is close — a typo'd key is an error, never a silently ignored no-op.

## Worked examples

```bash
# A config file pins the look; one --set tries a variation on top of it.
avz render song.mp3 --config album.toml --set visual.intensity=1.4

# --set reaches any key by its TOML path...
avz render song.mp3 --set output.fps=60 --set background.darken=0.4

# ...and bare names resolve into the active preset's parameters.
avz render song.mp3 --preset bars --set anchor=top-right --set bar_count=64

# Fast iteration on a chorus: reduced resolution by default (layer 4),
# full-size when you ask for it (layer 2 beats layer 4).
avz render song.mp3 --sample 0:45..1:45
avz render song.mp3 --sample 0:45..1:45 --set output.resolution=1080p
```

`--sample` accepts a bare duration (`--sample 60s`: the first sixty seconds)
or a clock range (`--sample 0:45..1:45`). Durations elsewhere (`in_at`,
`hold`, `fade`) are spelled the same way: `1s`, `0.6s`, `6s`.

## `[output]` — the file avz writes

| Key | Default | What it does |
|---|---|---|
| `resolution` | `"1920x1080"` | Frame size: `WxH`, or a name — `720p`, `1080p`, `4k`. |
| `fps` | `30` | Frames per second. Every analysis window and animation clock derives from it. |
| `codec` | `"x264"` | Video encoder: `x264`, `x265`, or `av1`. The audio is never encoded — the original mp3 stream is copied in regardless. |
| `quality` | `18` | CRF for the chosen codec: lower is better and bigger, 18 is visually lossless for x264, and YouTube-safe. |

```toml
[output]
resolution = "1920x1080"
fps = 30
codec = "x264"
quality = 18
```

## `[visual]` — what draws, and how it moves

| Key | Default | What it does |
|---|---|---|
| `preset` | `"pulse"` | Which visualizer draws the video. `avz presets` lists them; [PRESETS.md](PRESETS.md) documents them. |
| `palette` | `"ember"` | A built-in palette name — `ember`, `glacier`, `verdant`, `mono`, `carpathian` — or an inline list of 2–8 hex colors, resampled onto the five slots a shader reads: `["#1a1a2e", "#e94560"]`. |
| `intensity` | `1.0` | Global motion scale, greater than 0. |
| `smoothing` | `0.35` | Global envelope decay scale, 0 to 1: higher is slower, smoother motion. The single most audible knob — this is what "feels musical" is tuned with. |
| `seed` | `"auto"` | `"auto"` hashes the input file's *name* (not its path), so re-rendering the same song anywhere gives the same video. Any non-negative integer pins it explicitly. |

### `[visual.params]` — the active preset's own parameters

Validated against the preset's schema: names, types, and ranges come from
`avz presets <name>`, and every parameter is documented in
[PRESETS.md](PRESETS.md). For example, `pulse` takes a `bass_drive`:

```toml
[visual]
preset = "pulse"

[visual.params]
bass_drive = 1.2
```

The same keys are reachable as `--set bass_drive=1.2` (bare names resolve into
the active preset) or `--set visual.params.bass_drive=1.2` (fully spelled).

## `[background]` — what sits beneath the visuals

With neither `image` nor `video`, the backdrop is a gradient built from the
palette. The two are mutually exclusive.

| Key | Default | What it does |
|---|---|---|
| `image` | *(none)* | A still image (PNG or JPEG), also reachable as `--bg art/forest.png`. |
| `video` | *(none)* | A looped, muted video. ffmpeg loops, scales, and frame-rate-converts it; it always starts at its first frame, so `--sample` moves the song and never the loop. |
| `fit` | `"cover"` | How the source is fitted to the frame: `cover` (fill, crop overflow), `contain` (letterbox), or `stretch`. |
| `blur` | `0.0` | Gaussian blur of the background, as a standard deviation in output pixels — free for an image, per-frame for a video. |
| `darken` | `0.0` | How much of the background's light to take away, 0 to 1, so the visuals read on top. |

```toml
[background]
image = "art/forest.png"     # or: video = "loops/smoke.mp4"
fit = "cover"
blur = 6.0
darken = 0.35
```

The panel presets (`bars`, `meter`) are made for this: they draw in one
anchored rectangle and leave the rest of the frame to the background.

## `[text]` — the title card

Title and artist come from the file's ID3 tags; `title` and `artist` here (or
`--title` / `--artist`) override them, and `--no-text` or `enabled = false`
removes the card. Missing tags warn and skip the card — they never fail the
render.

| Key | Default | What it does |
|---|---|---|
| `enabled` | `true` | Whether the card is drawn at all. |
| `position` | `"bottom-left"` | The nine-grid: `top-left` through `bottom-right`, or `center`. |
| `in_at` | `"1s"` | When the card starts fading in. |
| `hold` | `"6s"` | How long it stays fully visible. |
| `fade` | `"0.6s"` | How long the fade in and the fade out each take. |
| `font` | `"auto"` | `"auto"` is the bundled OFL font; a path uses your own. |
| `size` | `0.05` | Title height, as a fraction of the frame's height. |
| `margin` | `0.06` | The card's distance from the frame's edges, as a fraction of the short edge. |
| `title` | *(from ID3)* | Overrides the tag. |
| `artist` | *(from ID3)* | Overrides the tag. |

```toml
[text]
enabled = true
position = "bottom-left"
in_at = "1s"
hold = "6s"
fade = "0.6s"
```

## `[effects]` — transforming the finished picture

A post pass over the whole composited frame — background, visualizer, and
text together — applied geometry first (zoom and rotation about the center),
then color, in linear light (RFC-002). Every default is the identity: an
absent `[effects]` section costs nothing and changes nothing, byte for byte.
The fringe a zoom-out or rotation exposes clamps to the edge pixel, which
reads as camera movement rather than black bars.

| Key | Default | What it does |
|---|---|---|
| `zoom` | `1.0` | Magnification about the frame's center, `0.5`–`3`. |
| `pulse` | `0.0` | How much the kick swells the zoom, up to `0.5`; try `0.06` for a breathing picture. |
| `spin` | `0.0` | Rotation in turns per second, `-2`–`2`; negative turns the other way. |
| `sway` | `0.0` | How far the bass tilts the picture, in turns, up to `±0.25`; keep it small. |
| `hue` | `0.0` | Hue rotation in turns; `0.5` swaps the palette's warm and cool ends. |
| `hue_drift` | `0.0` | Hue rotation speed, in turns per second, `-2`–`2`. |
| `saturation` | `1.0` | `0` is gray, `1` neutral, up to `3`. |
| `contrast` | `1.0` | Pivots at mid-gray, `0.2`–`3`. |
| `brightness` | `1.0` | Plain gain, `0`–`3`. |
| `flash` | `0.0` | How much a hit lifts the brightness, up to `2`; try `0.15`. |

```toml
[effects]
zoom = 1.05        # a touch tighter than shot
pulse = 0.06       # the kick breathes the picture
spin = 0.02        # one slow turn every fifty seconds
saturation = 1.2
flash = 0.15       # every hit is a pulse of light
```

```bash
# The same knobs from the command line, combined freely:
avz render song.mp3 --preset nebula --set effects.pulse=0.08 --set effects.hue_drift=0.05
```

## Reproducibility

A config file checked into an album repo *is* the render: same mp3, same
config, same seed, same avz version — same video, byte-comparable modulo
encoder nondeterminism. That contract is per version: a new avz release may
change what a preset draws (the CHANGELOG calls those out as breaking), so pin
the version alongside the config when it matters.

```bash
for f in album/*.mp3; do avz render "$f" --config album.toml; done
```

---

*This reference is held to the code by `crates/avz-core/tests/docs_reference.rs`:
every section and key `avz config --example` emits must appear here, so a new
config key without documentation fails the test suite.*
