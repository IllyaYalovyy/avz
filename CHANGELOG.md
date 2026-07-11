# Changelog

All notable changes to avz are recorded here. Versions follow
[semantic versioning](https://semver.org/).

Renders are reproducible per version, not across versions: a change to a shader,
to the envelope defaults, or to normalization gives an unchanged input and config
a different video. Those changes are called out under **Breaking changes** even
when no API moved, because a config checked into an album repo is an API.

## [Unreleased]

### Added

- **`halo`** (#44). First of the subtle presets: a soft glow breathing in a
  chosen corner, rim stirred by noise, swelling gently on hits. An accent for
  backgrounds, not a visual of its own.

- **`strings`** (#43). Harp strings with seeded pitches: every hit plucks a
  seeded subset, plucks superpose and ring down as closed forms of their age —
  the particles rule on one dimension. A fill reads as a strum.

- **`stained`** (#42). A voronoi stained-glass window: each pane glows with
  its dealt band, and the newest hit's ordinal — read from the onset history —
  re-leads the whole mosaic, so every hit is a new window.

- **`orbits`** (#41). Five band-planets circling a sun of the song's
  loudness, comet tails curling behind them through the feedback texture;
  every hit flashes the system.

- **`tiles`** (#40). An equalizer wall filling the frame: columns are bands,
  rows light to each column's level, the top tile burns hotter, hits brighten
  the wall. No clock at all — a pure function of the frame's spectrum.

- **`rain`** (#39). Spectral rain: columns are bands, a loud band rains long
  bright streaks and a quiet one stays dry. Drops fall at constant seeded
  speeds — the music scales their light, never their position.

- **`scope`** (#38). An oscilloscope beam tracing a lissajous knot: the kick
  and mids stretch its axes, the highs ripple a harmonic along it, hits
  brighten the phosphor. The backlog's lissajous, drawn with segment
  distances and no hash at all.

- **`aurora`** (#37). Curtains of light hanging across the upper frame: the
  bass deepens their sway, the air band shimmers the folds, hits breathe
  light into the sky. Where `nebula` churns, `aurora` hangs.

- **`horizon`** (#36). A synthwave sunset: scanlined sun, perspective floor
  grid scrolling toward the viewer, air-twinkled stars; the kick pulses the
  grid, hits flare the horizon line. The backlog's terrain flyover, as neon.

- **`starfield`** (#35). A warp-speed starfield in two parallax layers:
  loudness stretches the stars into warp lines, hits stretch them further, the
  air band twinkles whatever is barely moving, and silence collapses the sky
  to still points.

- **`tunnel`** (#34). An endless ring tunnel flown at the speed of the song:
  the kick swells the walls, the mids stripe them, every hit lights the gates,
  and `fog` sinks the vanishing point. From the VISION §12 backlog.

- **The reference docs** (#33). `docs/PRESETS.md` documents every preset and
  every parameter — description, example commands, the full table of types,
  defaults, ranges, and performance notes — and `docs/CONFIGURATION.md`
  documents every config section and key with the precedence rules and worked
  examples. Neither can rot: `docs_reference.rs` checks both against the
  embedded registry, schemas, and `config --example` generator, so an
  undocumented preset, parameter, or key fails the suite.

- **`meter`, the second panel preset** (#32). A VU-style level meter: the
  enveloped loudness fills an anchored ladder of LED `segments` (or a
  continuous bar at `segments = 0`), standing or lying down by `orientation`,
  the palette walking its length so a cool-to-hot palette reads as the classic
  green-amber-red. The topmost lit sliver flashes on the beat. Unlike `bars`,
  a faint `track` keeps the unlit scale visible, because a meter at zero is
  information; the track never leaks outside the panel. The cheapest preset
  avz ships — no binding beyond the uniform.

- **`bars`, the first panel preset** (#31). A spectrum analyzer that lives in
  one anchored rectangle — `anchor` speaks the text card's nine-grid
  vocabulary, `width`/`height`/`margin` size the panel — and leaves every
  pixel outside it fully transparent, so a `background.image` or
  `background.video` shows through untouched. Bars divide the same 512-bucket
  spectrum `ribbons` reads, bass at the left; a `glow` halo rises from each
  tip and dies inside the panel. First shipped consumer of the schema's
  `enum` parameter kind. `a_panel_preset_lights_only_its_panel` holds every
  panel preset to "exactly the backdrop outside the rectangle."

- **The codec matrix.** `--codec x265` and `--codec av1` encode, alongside the
  `x264` that already did. `--quality` stays one number and reaches all three as
  `-crf`, `-pix_fmt yuv420p` still holds across the matrix, and the audio is
  still the original mp3 stream copied in. The last deferral of RFC-001 NG3,
  which closes that non-goal.

  The knobs differ where the encoders differ, and nowhere else. x264 and x265
  take `-preset slow` — offline rendering already costs minutes of GPU time.
  SVT-AV1's preset is a number rather than a word, and its default of 10 is tuned
  for encoding faster than realtime, which avz never has to do; `-preset 6` is
  the same bargain `slow` strikes. x265 is told `-x265-params log-level=error`,
  because libx265 writes its banner to stderr under any ffmpeg log level and avz
  keeps that stream clear for the last words of a failure. HEVC is tagged
  `-tag:v hvc1`: the same bitstream the mp4 muxer would have written as `hev1`,
  named the way QuickTime and Safari insist on finding it.

  **A binary that answers `-version` is not a binary that has the encoder.**
  Fedora's stock `ffmpeg-free` is built without `libx264` and `libx265`, so avz
  now asks ffmpeg what it can encode — `ffmpeg -encoders`, parsed — before it
  decodes the song. A codec this ffmpeg cannot encode exits 2 with the encoder
  named and the RPM Fusion install line, in the first millisecond rather than
  after an hour of software rendering. It is the user's configuration that is
  refused, not the encoder that had a bad day, so a batch loop stops rather than
  retries (`VISION.md` §8).

  AV1 means SVT-AV1. ffmpeg also offers libaom and rav1e; naming one encoder per
  codec is what keeps `--quality` a single scale and the argv a single shape, and
  a quality hook now fails the build if a second encoder name appears anywhere
  outside `encode/encoder.rs`.

- **Looped background video.** `background.video` composites a muted video
  beneath the visuals, looped for as long as the song lasts. It takes the same
  `fit`, `blur`, and `darken` as `background.image`, and is still mutually
  exclusive with it. The last deferral of RFC-001 NG2, which closes that
  non-goal.

  A second ffmpeg does the work `VISION.md` §5.3 says it should: `-stream_loop -1`
  loops the source, a `scale`/`crop`/`pad` chain fits it, and `-r` converts its
  frame rate — so avz reads exactly `width × height × 4` bytes per frame and
  uploads them. Any format ffmpeg decodes, at any resolution and any rate. `-an`
  means the video's own audio is never decoded, so it can never reach the mux.

  The frame's alpha is binary by construction: the filter chain flattens the
  source's alpha *before* it resamples, and only a `contain` letterbox
  reintroduces transparency, as fully transparent bars. Premultiplied and straight
  alpha agree there, so the bytes ffmpeg wrote are the bytes the layer stores — no
  per-frame premultiply — and the compositor draws the palette backdrop through
  the bars exactly as it does under a `contain` image.

  `blur` and `darken` still happen in light rather than in encoded bytes, but a
  video pays for them once per *frame* where an image paid once per render. So the
  default costs nothing at all, a `darken` alone is a 256-entry lookup table built
  once, and only a `blur` takes the full trip through linear f32.

  The decoder runs on its own thread behind a bounded queue and is read with a
  timeout, which is `VISION.md` §11's own mitigation for the risk it names there:
  a wedged decode thread ends the render with a message naming the video, never
  with a render that hangs, and a decoder that outruns a software render blocks
  after two frames instead of buffering the whole loop into memory.

  **The loop always starts at its first frame**, `--sample` included. It has a
  clock of its own and no timestamp in the song, so `--sample 1:00..1:03` previews
  that minute's visuals over the *opening* three seconds of the loop rather than
  over the seconds a full render would have reached by then. Determinism is
  untouched: the same inputs and the same config still produce the same video.

- **`ink` preset.** Ink is dropped into still water on every onset, spreads, feeds
  on the clean water around it, starves where the water is already black, and
  dissolves everywhere else. What is left is a slow, brooding marble that never
  repeats. `rms_env` is the growth rate: a loud passage makes the ink invade, and
  a silence dissolves it back to the backdrop. The last preset deferred by RFC-001
  NG1, which closes that non-goal — and, as it predicted, it needed no new binding.
  A reaction-diffusion reads the previous frame, and `needs_feedback` already
  bound it. Three files in `presets/` and one registry row.

  The field lives in the **alpha channel**, which for a premultiplied layer
  (`VISION.md` §5.3) is not a trick but an identity: the alpha *is* the ink's
  density, and the RGB is what that density looks like under the palette. So a
  palette change repaints the ink instead of smearing old colors into it, and
  `ink` cannot blow the frame out — it can never emit more light than it covers.
  The model is Gray-Scott with its solvent eliminated; `crowd` above 1.0 is what
  keeps the frame from filling, since a pixel whose neighbourhood is already dense
  starves, stops growing, and hollows out while its front eats outward.

  `steps` is a *reaction* sub-step, not a render pass. The reaction is local and
  stiff and takes `steps` Euler steps inside the one fragment shader; the
  diffusion takes exactly one, at the lattice's stability limit, because mixing
  twice toward a frozen 3×3 blur only gets closer to that same blur. Iterating it
  for real would mean drawing the preset `steps` times a frame — a change to the
  render contract. Recorded in RFC-001 NG1.

- **`kaleido` preset.** The frame is cut into wedges around its centre and every
  wedge is made a reflection of its neighbour, so the petals, rings, and grain
  drawn inside one are drawn symmetrically in all of them. The fold turns, the
  rings travel outward, and the palette walks under both. The third preset
  deferred by RFC-001 NG1 to land, and the first to need nothing from the
  renderer: no feedback texture, no spectrum, no onset history — a fold is a
  function of the fragment's own polar coordinates and the uniform every preset
  receives. Three files in `presets/` and one registry row, which is G3 holding
  with no core change at all behind it.

  `time` reaches the picture through exactly three knobs — `spin`, `drift`, and
  `hue_cycle` — and a test sets all three to zero and demands the same features
  render the same frame three seconds apart, which is where a stray `sin(time)`
  would otherwise hide. The grain is sampled in the *folded* coordinates rather
  than at the fragment's own position: per-pixel grain would break the symmetry
  the whole preset is for, and break it invisibly.
- **`particles` preset.** Every hit throws a burst of sparks out of the middle of
  the frame; they fly, slow against the air, fall, dim, and go out, and the highs
  make the ones still burning twinkle. The second preset deferred by RFC-001 NG1
  to land.

  Every particle is a closed form of `(hit, index)` rather than a simulation
  stepped forward: the hit gives it a birth, a seeded hash gives it a direction
  and a speed, and `age = time - birth` gives it the rest. Nothing is integrated
  between frames, so frame `N` is a pure function of frame `N`'s inputs — skip to
  frame 4000 of a song and it draws what a render that passed through frames
  `0..3999` would have drawn. Particles are drawn in the fragment stage against
  the same fullscreen triangle every preset uses, not as vertex-pulled point
  sprites: that keeps the one code path (`AGENTS.md`, rendering), and a per-burst
  cull against the shell its particles occupy is what pays for it. The schema
  carries a `perf_hint` for software rendering.
- **Onset-history texture binding.** A preset opts in with `"needs_onsets": true`
  in its schema and reads the last 64 hits at or before the frame being drawn as
  a `64×1` `Rg32Float` texture at `@binding(4)`, newest first, each slot the hit's
  birth time in seconds and its ordinal among the song's hits — with `textureLoad`
  and no sampler, for the reason the spectrum binding gives. Unfilled slots hold a
  birth a thousand seconds before the song began, so a preset's own lifetime test
  rejects them and no emptiness flag is needed.

  The uniform's `onset` is one number about the frame being drawn, which is
  everything a flash needs and nothing at all to a particle spawned a second ago
  and still in the air. Generic, not `particles`-specific, and independent of the
  other two bindings: a preset may ask for any subset. This is the binding #24
  said would not be needed; RFC-001 NG1 records why it was, and why `kaleido` and
  `ink` should need no fourth.
- **`ribbons` preset.** A stack of ribbons across the frame, each displaced,
  thickened, and lit by the song's own spectrum: the width of the frame is the
  log-frequency axis, bass at the left edge and air at the right. The first
  preset deferred by RFC-001 NG1 to land.
- **Spectrum texture binding.** Analysis now emits a coarse 512-bucket,
  log-spaced spectrum per video frame (`VISION.md` §5.1), normalized over the
  whole song like every other feature. A preset opts in with
  `"needs_spectrum": true` in its schema and reads the frame's row as a `512×1`
  `R32Float` texture at `@binding(3)`, with `textureLoad` and no sampler — a
  float sampler is a wgpu feature lavapipe does not always carry, and hardware
  filtering rounds differently per driver. Generic, not `ribbons`-specific, and
  independent of `needs_feedback`: a preset may ask for either, both, or
  neither. This is the last generic binding RFC-001 planned.

### Changed

- **A `background.video` that does not exist now exits 3, not 2.** It used to be
  refused as a configuration error, because avz could draw no video at all. It is
  now a file the user named, exactly like `--bg`, and a batch loop can tell "this
  song's loop is missing" from "my config is wrong".
- **`encode::video_encoder` no longer returns a `Result`.** Every codec now names
  an encoder, so the fallible question moved to `encode::ensure_encoder`, which
  asks the ffmpeg binary rather than a table. `encode::encoders` is new and lists
  what a preflighted ffmpeg was built with.

### Fixed

- **The feedback texture cleared to opaque black before the first frame.** The
  previous-frame history is a premultiplied layer, and before frame 0 there is no
  layer, so its coverage is zero — but `Feedback::new` cleared it to
  `wgpu::Color::BLACK`, whose alpha is 1, while every other surface in the
  renderer already cleared to transparent black. `nebula` averages the trail's
  alpha into its own coverage, so every `nebula` render (and every `--sample`
  excerpt of one) opened by hiding the background layer behind a sheet of opaque
  black that faded down over the first frames, rather than fading the clouds up
  out of the backdrop. Found while writing `ink`, which carries its state in the
  alpha channel and would have started every render saturated with ink it never
  drew.

### Breaking changes

- **`nebula` renders differently near the start of a render.** The feedback fix
  above changes the first frames of any `nebula` render — the opening no longer
  fades down from black — and its golden hashes moved at frames 0, 10, and 100.
  Same input and same config, different video, as the preamble warns. No config
  key changed.

## [0.1.0] - unreleased

The first usable release. `avz render song.mp3` decodes an mp3, extracts its
features, draws them on the GPU, and pipes the frames into ffmpeg — one command,
no configuration, and the original audio muxed in untouched.

### Added

- **`avz render`** — the whole pipeline: symphonia decode → FFT feature timeline
  → wgpu offscreen render → ffmpeg encode. Output is `<song-stem>.mp4` next to
  the input unless `--out` says otherwise, and nothing appears at that path until
  ffmpeg exits cleanly.
- **Audio features** driven by the music rather than the clock: RMS, five band
  energies (`bass`, `low_mid`, `mid`, `high`, `air`), spectral flux, spectral
  centroid, and onsets from an adaptive `median + k·MAD` threshold with an
  absolute noise floor. Every energy feature also arrives smoothed through an
  attack/decay envelope, pinned in time rather than in frames, so a hit swells
  and fades over the same milliseconds at 24, 30, and 60 fps.
- **Two presets.** `pulse` — concentric rings driven by the kick. `nebula` —
  an fbm flow field over feedback trails, churned by the bass. Both are selected
  with `--preset` and configured with typed, validated parameters:
  `avz presets <name>` prints the schema, `--set` and `[visual.params]` set it.
- **Palettes.** Five built-ins (`ember`, `glacier`, `verdant`, `mono`,
  `carpathian`) or two to eight inline hex colors, resampled in Oklab onto the
  five slots a shader reads.
- **Layers.** A static background image with `cover` / `contain` / `stretch`
  fitting, gaussian blur, and darkening — all applied in light rather than in
  sRGB. A title/artist card from ID3 tags, set in a bundled OFL font, fading in
  and out on a configurable schedule.
- **`--sample 60s` / `--sample 0:45..1:45`** renders an excerpt at a reduced
  720p default, with the muxed audio seeked to the same instant and still copied.
- **`--adapter auto|gpu|software`.** One code path — wgpu → Vulkan → hardware
  driver or Mesa lavapipe. `auto` falls back to software with a warning naming
  what it costs and how to silence it; `gpu` fails rather than renders slowly.
- **Deterministic output.** All animation time is `frame_index / fps`; all
  randomness is a seeded hash. `--seed auto` (the default) is FNV-1a of the
  song's file *stem*, so the same mp3 renders the same video from any directory
  or machine. Golden-frame tests pin every preset on the software adapter.
- **Configuration.** `--config` TOML files with strict unknown-key rejection and
  "did you mean" suggestions, `--set key=value` overrides, and a fixed precedence
  chain: CLI flags > `--set` > `--config` > preset defaults > built-in defaults.
  `avz config --example` prints a documented template of every key at its
  default, which `--config` accepts back unchanged.
- **`avz probe`** — tags, duration, sample rate, and embedded cover art.
- **`avz presets`** — the shipped presets, and each one's parameter schema with
  types, defaults, ranges, and any software-rendering performance hint.
- **CLI UX.** Progress bars with phase, frame count, render fps, and ETA on a
  terminal; decile log lines in a pipe; nothing at all under `--quiet`. Warnings
  name a consequence and an action. Exit codes are contractual: 0 ok, 2 bad
  arguments or config, 3 input file problems, 4 render or encode failure — so a
  shell batch loop can tell "skip this song" from "stop, everything is wrong".
- **`--codec x264` and `--quality <CRF>`**, defaulting to CRF 18.

### Known issues

- **Two presets, not six.** `ribbons`, `particles`, `kaleido`, and `ink` are
  deferred (RFC-001 NG1) along with the spectrum-texture binding `ribbons`
  consumes. Adding one touches only `crates/avz-core/presets/`.
- **No looped background video.** `background.video` parses and is refused with
  a message saying it is not built yet (RFC-001 NG2).
- **`--codec x265` and `--codec av1` parse and are refused** with exit 2
  (RFC-001 NG3). Only x264 encodes.
- **No beat/BPM grid, lyrics, GUI, or realtime preview.** These are non-goals,
  not omissions (`VISION.md` §2).
- **Linux only in practice.** Fedora is the primary target and the only platform
  tested. Windows and macOS are neither tested nor supported.
- **Software rendering is slower, not slow.** A 5-minute song at 1080p30 takes a
  few minutes on a GPU and several more on lavapipe; see the performance table
  in `README.md`.

### Requirements

System `ffmpeg` is the only runtime dependency. A Vulkan driver is needed;
without a GPU, Mesa's lavapipe (`mesa-vulkan-drivers` on Fedora) supplies one.
