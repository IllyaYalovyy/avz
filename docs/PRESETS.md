# Presets

A preset is the visualizer: a WGSL shader and a parameter schema, embedded in
the binary. `avz presets` lists every one; `avz presets <name>` prints a
preset's parameters, defaults, and ranges — the same tables that appear below.

```bash
avz presets                  # every preset, one line each
avz presets nebula           # nebula's full parameter schema
avz render song.mp3 --preset nebula
```

Set a parameter from the command line with `--set`, or pin it in a config file
under `[visual.params]` (see [CONFIGURATION.md](CONFIGURATION.md) for the
precedence rules):

```bash
avz render song.mp3 --preset nebula --set turbulence=1.8 --set octaves=3
```

```toml
[visual]
preset = "nebula"

[visual.params]
turbulence = 1.8
octaves = 3
```

A `--set` for a parameter the active preset does not declare, or a value
outside its range, fails before anything renders — with a "did you mean"
suggestion when the name is close.

**Conventions that hold across every preset.** Colors come from the palette
(`--palette`, five slots; presets draw with slots 1–4 and leave slot 0 to the
backdrop). Silence leaves the frame to the background: every preset scales
with the song's loudness, so a quiet passage fades the visuals away rather
than freezing them. All motion is deterministic — same file, same config, same
`--seed`, same video. Where a preset has a `vignette`, it darkens the corners
so the visuals read against any backdrop; where it has a `brightness`, that is
the overall output level, applied last.

**Fullscreen or panel.** Most presets fill the frame. Two — `bars` and
`meter` — are *panel* presets: they own one anchored rectangle,
placed by an `anchor` parameter that speaks the text card's nine-grid
vocabulary (`top-left` through `bottom-right`), and leave every pixel outside
it fully transparent, so a `background.image` or `background.video` shows
through untouched.

---

## `pulse`

> minimal, geometric: concentric rings driven by the kick

The first preset built, and the plainest reader of the feature set: the kick
(`bass`) swells a core disc, the mids pack rings around it, the highs twinkle
a sparkle grid over everything, and an onset flashes the core. Good for
checking that a mix *reads* — if `pulse` doesn't move, the problem is the
song, not the preset.

```bash
avz render song.mp3 --preset pulse
avz render song.mp3 --preset pulse --set bass_drive=2.5 --set flash=false
```

| Parameter | Type | Default | Range | What it does |
|---|---|---|---|---|
| `bass_drive` | float | `1` | `0..4` | How hard the kick swells the core disc. |
| `ring_count` | int | `4` | `1..32` | Rings across the frame before the mids pack more in. |
| `ring_density` | float | `14` | `0..32` | How many extra rings a loud midrange packs in. |
| `drift_speed` | float | `1` | `0..4` | How fast the low mids drift the rings outward. |
| `sparkle_gain` | float | `1` | `0..2` | How brightly the highs twinkle in the sparkle grid. |
| `grain` | float | `0.06` | `0..0.5` | How much per-pixel shimmer the air band adds. |
| `glow` | float | `1` | `0..2` | How much spectral flux glows the edge of the core. |
| `vignette` | float | `0.55` | `0..1` | How far the corners of the frame fall off. |
| `flash` | bool | `true` | `true` \| `false` | Whether an onset flashes the core brighter. |

## `nebula`

> organic clouds: an fbm flow field over feedback trails, churned by the bass

Slow-moving noise clouds that drag trails of the previous frame behind them —
the feedback is what makes the motion feel liquid. The bass churns the flow
field, onsets burst from the centre, and the centroid drifts the hue through
the palette. The preset for dark folk and anything that breathes.

```bash
avz render song.mp3 --preset nebula --palette glacier
avz render song.mp3 --preset nebula --set trail_decay=0.94 --set warp=1.2
```

**Performance:** on software rendering, frame time scales with pixel count —
preview with `--sample 10s` before committing to 1080p; `octaves = 2` then
trims about a further 15%

| Parameter | Type | Default | Range | What it does |
|---|---|---|---|---|
| `flow_scale` | float | `2.4` | `0.5..8` | How many cloud cells span the short edge of the frame. |
| `turbulence` | float | `1` | `0..3` | How hard the bass churns the flow field. |
| `trail_decay` | float | `0.88` | `0..0.98` | How much of the previous frame survives into this one. |
| `burst_strength` | float | `1` | `0..3` | How brightly an onset bursts from the centre. |
| `flow_speed` | float | `0.35` | `0..2` | How fast the clouds drift when nothing is playing. |
| `octaves` | int | `4` | `1..6` | Layers of noise: detail, at a cost on software rendering. |
| `warp` | float | `0.75` | `0..2` | How far the flow field drags the clouds into wisps. |
| `vignette` | float | `0.45` | `0..1` | How far the corners of the frame fall off. |
| `brightness` | float | `1` | `0..2` | How much light the loudness of the song puts into the clouds. |

## `ribbons`

> classic and reactive: a stack of ribbons displaced by the song's own spectrum

The frame's horizontal axis *is* the spectrum's log-frequency axis, bass at
the left. Each ribbon reads its own slice: where a band is loud the ribbon
swells and lights, where it is quiet it thins away to nothing. The most
literal of the fullscreen presets — you can watch the vocal enter.

```bash
avz render song.mp3 --preset ribbons
avz render song.mp3 --preset ribbons --set ribbon_count=3 --set amplitude=0.8
```

**Performance:** every ribbon reads the spectrum once per pixel, so frame time
scales with `ribbon_count` — on software rendering keep it at 5 or below, and
preview with `--sample 10s` before committing to 1080p

| Parameter | Type | Default | Range | What it does |
|---|---|---|---|---|
| `ribbon_count` | int | `5` | `1..12` | How many ribbons are stacked across the frame. |
| `amplitude` | float | `0.45` | `0..1.5` | How far a loud band displaces the ribbon that reads it. |
| `thickness` | float | `0.014` | `0.001..0.08` | How wide a ribbon's lit core is, as a fraction of the short edge. |
| `glow` | float | `0.8` | `0..3` | How far the halo around each ribbon reaches into the frame. |
| `spread` | float | `0.6` | `0..1` | How far apart the ribbons rest before the music moves them. |
| `drift_speed` | float | `0.7` | `0..4` | How fast the ribbons travel when nothing is playing. |
| `blur` | float | `2` | `0..8` | How many spectrum buckets are averaged into each ribbon sample. |
| `vignette` | float | `0.45` | `0..1` | How far the corners of the frame fall off. |
| `brightness` | float | `1` | `0..2` | How much light the loudness of the song puts into the ribbons. |

## `particles`

> energetic: every hit throws a burst of sparks the highs make twinkle

Every detected onset throws a burst of particles that fly, fall, and burn out;
the highs make whatever is still in the air twinkle. The preset that makes
percussion visible — a fill reads as a volley. Dense electronic material may
want a smaller `burst_size` and a shorter `lifetime` so the frame doesn't
saturate.

```bash
avz render song.mp3 --preset particles
avz render song.mp3 --preset particles --set gravity=-0.4 --set lifetime=3
```

**Performance:** every live burst reads `burst_size` particles at every pixel
it covers, so frame time scales with `burst_size` times the number of bursts
still in the air (`lifetime`) — on software rendering keep `burst_size` at 48
or below, or shorten `lifetime`, and preview with `--sample 10s` before
committing to 1080p

| Parameter | Type | Default | Range | What it does |
|---|---|---|---|---|
| `burst_size` | int | `40` | `1..256` | How many particles a single hit throws. |
| `lifetime` | float | `1.6` | `0.1..6` | How long a particle burns, in seconds, before it goes out. |
| `speed` | float | `0.9` | `0..4` | How hard a hit throws its particles. |
| `gravity` | float | `0.25` | `-2..2` | How fast a burst falls; negative makes it rise. |
| `drag` | float | `1.4` | `0.05..6` | How quickly a particle loses the speed it was thrown with. |
| `size` | float | `0.011` | `0.001..0.06` | How wide a particle's lit core is, as a fraction of the short edge. |
| `glow` | float | `0.7` | `0..3` | How far the halo around each particle reaches into the frame. |
| `sparkle` | float | `1` | `0..2` | How brightly the highs twinkle the particles still in the air. |
| `brightness` | float | `1` | `0..2` | How much light the loudness of the song puts into the bursts. |
| `vignette` | float | `0.45` | `0..1` | How far the corners of the frame fall off. |
| `spread` | float | `0.18` | `0..1` | How far a burst is thrown from the middle of the frame. |

## `kaleido`

> symmetric and hypnotic: a mirrored fold that turns while the hue walks the palette

A kaleidoscope: the frame folded into `segments` wedges, rings and petals
turning inside it, the kick pulling the whole fold toward the viewer. Runs on
nothing but the uniform — no textures — so it is equally cheap everywhere.
Hypnotic on steady material; on erratic material, lower `flash`.

```bash
avz render song.mp3 --preset kaleido --palette carpathian
avz render song.mp3 --preset kaleido --set segments=10 --set spin=-0.08
```

| Parameter | Type | Default | Range | What it does |
|---|---|---|---|---|
| `segments` | int | `6` | `3..24` | How many wedges the fold cuts the frame into. |
| `spin` | float | `0.04` | `-1..1` | How fast the fold turns, in turns per second; negative turns it the other way. |
| `hue_cycle` | float | `0.05` | `0..1` | How fast the hue walks the palette, in sweeps per second. |
| `zoom` | float | `0.5` | `0..1` | How hard a kick pulls the fold toward the viewer. |
| `ring_count` | float | `4.5` | `0..24` | How many rings are stacked between the centre of the fold and its edge. |
| `drift` | float | `0.3` | `-2..2` | How fast the rings travel outward; negative draws them inward. |
| `petals` | float | `3` | `0.5..12` | How many petals fill each wedge of the fold. |
| `shard` | float | `3.5` | `1..16` | How sharply the petals and rings cut into shards of glass. |
| `detail` | float | `2` | `0..8` | How fine the grain the mids grind into the glass is. |
| `flash` | float | `0.8` | `0..3` | How brightly an onset flares out of the middle of the fold. |
| `vignette` | float | `0.45` | `0..1` | How far the corners of the frame fall off. |
| `brightness` | float | `1` | `0..2` | How much light the loudness of the song puts into the glass. |
| `mirror` | bool | `true` | `true` \| `false` | Whether each wedge is a mirror image of its neighbours, or a rotated copy. |

## `ink`

> slow and brooding: a reaction-diffusion marble the loudness of the song grows

A reaction-diffusion system grown on the previous frame: the song's loudness
feeds ink into water, the bass stirs it, onsets drop blots into the middle,
and the pattern crawls. The slowest preset by temperament — built for long,
quiet material where `nebula` would still be too busy.

```bash
avz render song.mp3 --preset ink --palette mono
avz render song.mp3 --preset ink --set growth=1.4 --set swirl=1.2
```

**Performance:** frame time scales with pixel count, not with `steps` — the
nine texture samples of the previous frame and the readback dominate, so on
software rendering eight reaction sub-steps cost under 10% more than one, not
eight times more. Turn `steps` down to slow the ink down, not to speed the
render up; preview with `--sample 10s` before committing to 1080p, which is a
little over twice the work of 720p

| Parameter | Type | Default | Range | What it does |
|---|---|---|---|---|
| `diffusion` | float | `0.28` | `0..1` | How far the ink bleeds into the water around it each frame. |
| `growth` | float | `0.9` | `0..4` | How hard the ink makes more of itself. Scaled by the loudness of the song. |
| `dissolve` | float | `0.04` | `0..0.2` | How fast the ink gives up. Above about a quarter of the growth rate it wins, and a passage clears the frame. |
| `crowd` | float | `1.7` | `0.6..2` | How much water a neighbour's ink uses up. Above 1.0 a dense blob starves, stops filling, and hollows out. |
| `steps` | int | `4` | `1..8` | Reaction sub-steps per frame: how far the ink gets in a thirtieth of a second, and how sharp its fronts are. |
| `seed_rate` | float | `0.045` | `0..0.2` | How much new ink the song feeds into the water between hits. |
| `detail` | float | `3.4` | `0.5..8` | How many blooms of new ink span the short edge of the frame. |
| `swirl` | float | `0.6` | `0..2` | How hard the bass stirs the water the ink is drifting in. |
| `flash` | float | `1` | `0..2` | How much ink an onset drops into the middle of the frame. |
| `hue_cycle` | float | `0.03` | `0..0.5` | How fast the palette walks under the ink, in cycles per second. |
| `vignette` | float | `0.35` | `0..1` | How far from the walls of the dish the ink stops being able to grow. |
| `brightness` | float | `1` | `0..1.5` | How much light the ink gives back. It can never emit more than it covers. |

## `bars`

> a spectrum analyzer in one corner: anchored bars over whatever is behind them

The first panel preset: a classic spectrum analyzer that owns one anchored
rectangle and leaves everything outside it to the background. Bars divide the
same 512-bucket spectrum `ribbons` reads, bass at the left; a `glow` halo
rises from each bar's tip and dies inside the panel. Made for sitting over a
`background.image` or `background.video`:

```bash
avz render song.mp3 --preset bars --bg art/cover.jpg
avz render song.mp3 --preset bars --set anchor=bottom-center \
    --set width=0.9 --set height=0.15 --set bar_count=64
```

**Performance:** the panel is a fraction of the frame and each pixel reads the
spectrum four times, so `bars` is among the cheapest presets at any
`bar_count` — on software rendering the resolution is what costs, and
`--sample` previews at 720p for exactly that reason

| Parameter | Type | Default | Range | What it does |
|---|---|---|---|---|
| `anchor` | enum | `bottom-left` | `top-left` \| `top-center` \| `top-right` \| `center-left` \| `center` \| `center-right` \| `bottom-left` \| `bottom-center` \| `bottom-right` | Which of the nine grid positions the panel sits in, the text card's vocabulary. |
| `width` | float | `0.38` | `0.05..1` | The panel's width, as a fraction of the frame's width. |
| `height` | float | `0.24` | `0.05..1` | The panel's height, as a fraction of the frame's height. |
| `margin` | float | `0.04` | `0..0.4` | The gap between the panel and the frame's edges, as a fraction of the short edge. |
| `bar_count` | int | `32` | `4..96` | How many bars divide the spectrum, bass at the left edge and air at the right. |
| `gap` | float | `0.35` | `0..0.9` | How much of each bar's pitch is empty space between neighbours. |
| `glow` | float | `0.6` | `0..3` | How brightly a bar's tip halos upward into the unlit part of its column. |
| `brightness` | float | `1` | `0..2` | How much light the panel puts out overall. |

## `meter`

> a VU meter in one spot: the loudness as an anchored ladder of LEDs

The second panel preset: the song's enveloped loudness fills a ladder of LED
`segments` — or a continuous bar at `segments = 0` — standing or lying down by
`orientation`. The palette walks the meter's length, so a cool-to-hot palette
reads as the classic green-amber-red; the topmost lit sliver flashes on the
beat. A faint `track` keeps the unlit scale visible, because a meter at zero
is information.

```bash
avz render song.mp3 --preset meter --set background.video=loops/tape.mp4
avz render song.mp3 --preset meter --set anchor=bottom-center \
    --set orientation=horizontal --set length=0.8
```

**Performance:** the panel is a sliver of the frame and reads nothing but the
uniform, so `meter` is the cheapest preset avz ships — if a render is slow,
the resolution is what costs, not this

| Parameter | Type | Default | Range | What it does |
|---|---|---|---|---|
| `anchor` | enum | `bottom-right` | `top-left` \| `top-center` \| `top-right` \| `center-left` \| `center` \| `center-right` \| `bottom-left` \| `bottom-center` \| `bottom-right` | Which of the nine grid positions the meter sits in, the text card's vocabulary. |
| `orientation` | enum | `vertical` | `vertical` \| `horizontal` | Whether the meter stands and fills upward, or lies down and fills to the right. |
| `length` | float | `0.45` | `0.1..1` | The meter's long side, as a fraction of the frame edge it runs along. |
| `thickness` | float | `0.05` | `0.01..0.3` | The meter's short side, as a fraction of the frame's short edge. |
| `margin` | float | `0.04` | `0..0.4` | The gap between the meter and the frame's edges, as a fraction of the short edge. |
| `segments` | int | `24` | `0..64` | How many LED segments divide the meter; 0 is a continuous, unbroken bar. |
| `track` | float | `0.12` | `0..1` | How visible the unlit remainder of the meter is, so quiet passages still show the scale. |
| `brightness` | float | `1` | `0..2` | How much light the lit segments put out. |

## `tunnel`

> an endless ring tunnel flown at the speed of the song, every hit a lit gate

The classic bore, from the VISION backlog: rings of light receding to a
vanishing point, flown through at a steady `speed`. The kick swells the walls
around you, the mids stripe them, and every hit lights the gates as they pass.
`fog` sinks the far end into darkness so the eye stays on what is arriving.

```bash
avz render song.mp3 --preset tunnel
avz render song.mp3 --preset tunnel --set speed=2 --set twist=-1.5 --set fog=0.8
```

**Performance:** one pass over the uniform and no textures, so frame time
scales with pixel count alone — on software rendering preview with
`--sample 10s` before committing to 1080p

| Parameter | Type | Default | Range | What it does |
|---|---|---|---|---|
| `speed` | float | `1` | `0..4` | How fast the tunnel is flown, in bores per second. |
| `rings` | float | `10` | `2..24` | How many ring gates light the bore at any depth. |
| `stripes` | int | `12` | `0..48` | How many stripes run around the walls; 0 leaves the walls dark. |
| `twist` | float | `0.6` | `-3..3` | How hard the wall stripes spiral along the bore; negative spirals the other way. |
| `pulse` | float | `1` | `0..3` | How hard the kick swells the walls around the viewer. |
| `flash` | float | `1` | `0..3` | How brightly a hit lights the gates as they pass. |
| `fog` | float | `0.55` | `0..1` | How deep the far end of the bore sinks into darkness. |
| `brightness` | float | `1` | `0..2` | How much light the loudness of the song puts into the bore. |

## `starfield`

> a warp-speed starfield: loudness is velocity, and every hit streaks the sky

Two parallax layers of stars radiating from the center. The flight is steady;
the music is in the streaks — loudness stretches the stars into warp lines, a
hit stretches and brightens them further, and the air band twinkles whatever
is barely moving. Silence collapses the sky back to still, faint points.

```bash
avz render song.mp3 --preset starfield
avz render song.mp3 --preset starfield --set warp=2 --set tint=0.7 --set density=1.8
```

**Performance:** two lattice layers over the uniform and no textures, so frame
time scales with pixel count alone — on software rendering preview with
`--sample 10s` before committing to 1080p

| Parameter | Type | Default | Range | What it does |
|---|---|---|---|---|
| `density` | float | `1` | `0.2..3` | How crowded the sky is. |
| `speed` | float | `0.6` | `0..3` | How fast the field flies past when nothing is playing. |
| `warp` | float | `1` | `0..3` | How far the loudness of the song stretches the stars into warp lines. |
| `streak` | float | `0.35` | `0.05..1` | The base streak length the warp multiplies. |
| `twinkle` | float | `1` | `0..2` | How hard the air band flickers the stars that are barely moving. |
| `flash` | float | `1` | `0..3` | How far a hit stretches and brightens every streak. |
| `tint` | float | `0.35` | `0..1` | How much of the palette colors the starlight; 0 is plain white stars. |
| `vignette` | float | `0.3` | `0..1` | How far the corners of the frame fall off. |
| `brightness` | float | `1` | `0..2` | How much light the loudness of the song puts into the sky. |

## `horizon`

> a synthwave sunset: a scanlined sun over a perspective grid the kick pulses

The genre classic: a striped sun on the horizon, a perspective floor grid
scrolling toward the viewer, sparse stars twinkling with the air band. The
kick pulses the grid and swells the sun, and every hit flares the horizon
line itself. The backlog's "terrain flyover", taken as neon instead of rock.

```bash
avz render song.mp3 --preset horizon --palette ember
avz render song.mp3 --preset horizon --set grid=10 --set scanlines=20 --set speed=1.6
```

**Performance:** one pass over the uniform and no textures, so frame time
scales with pixel count alone — on software rendering preview with
`--sample 10s` before committing to 1080p

| Parameter | Type | Default | Range | What it does |
|---|---|---|---|---|
| `sun_size` | float | `0.28` | `0.05..0.6` | The sun's radius, as a fraction of the frame's short edge. |
| `scanlines` | int | `14` | `0..40` | How many stripes cut the sun; 0 leaves it whole. |
| `grid` | float | `6` | `2..16` | How dense the floor grid is. |
| `speed` | float | `0.8` | `0..4` | How fast the floor scrolls toward the viewer. |
| `pulse` | float | `1` | `0..3` | How hard the kick pulses the grid lines. |
| `flare` | float | `1` | `0..3` | How brightly a hit flares the horizon line. |
| `stars` | float | `1` | `0..2` | How bright the sky's stars are; the air band twinkles them. |
| `vignette` | float | `0.35` | `0..1` | How far the corners of the frame fall off. |
| `brightness` | float | `1` | `0..2` | How much light the loudness of the song puts into the scene. |

---

*This reference is held to the code by `crates/avz-core/tests/docs_reference.rs`:
every preset, parameter, default, and performance note above is checked against
the embedded schemas, so a stale table fails the test suite.*
