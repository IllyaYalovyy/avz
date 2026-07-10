# Abstract music video generator - Vision

**Abstract music video generator. Rust CLI. mp3 in, music-reactive video out.**

Version: 0.1 (kickoff) · Date: 2026-07-08 · Target platform: Linux (Fedora primary)

---

## 1. Project Brief (the prompt)

> Build **avz**, a single-binary Rust CLI tool that takes a valid mp3 file and renders an abstract, music-driven video (H.264/AV1 mp4) with the original audio muxed in. Visuals are GPU shader presets (WGSL via wgpu) whose motion is driven by audio features extracted offline (band energies, RMS, spectral flux/onsets), smoothed through attack/decay envelopes so motion feels musical rather than twitchy. The tool reads title/artist and cover art from ID3 tags and can render a text card overlay. Optional layers: a static background image or a looped, muted background video beneath the abstract visuals. Rendering is fully offline and deterministic: frames are generated at exact timestamps and piped as raw RGBA into an FFmpeg subprocess for encoding. The tool must work without a GPU via Mesa lavapipe (software Vulkan) — same shaders, same code path, just slower. UX is CLI-first and genuinely friendly: good `--help`, named presets with descriptions, a `--sample` flag to render a short excerpt for fast iteration, a progress bar with ETA/fps, and TOML config files for reproducible renders. No GUI, no lyrics support, no realtime playback. Architecture must keep the core library UI-agnostic (`analysis` / `render` / `encode` / `cli` separation) so a GUI or batch orchestrator could be added later without refactoring.

This paragraph is the north star. If a feature request doesn't serve it, it goes to the backlog (§12).

---

## 2. Goals and Non-Goals

### Goals (v1)

1. `avz render song.mp3 --preset X -o out.mp4` produces a finished 1080p30 video with audio, no other setup required.
2. Music-reactive motion that *feels* right: envelope-smoothed features, per-preset mapping of audio bands to visual parameters.
3. A preset system where adding a new visualizer = one WGSL file + one parameter schema. Ship 4–6 presets at v1.
4. Wide configurability without flag explosion: presets expose typed parameters (colors, intensity, smoothing, band routing) settable via `--set key=value` and TOML config.
5. Title/artist text card from ID3 metadata, with configurable font, position, timing (e.g., fade in at 0s, out at 8s), and manual override.
6. Optional background image (stretched/covered/blurred) and optional looped background video (muted), composited beneath the visuals.
7. Sample renders: `--sample 60s` or `--sample 0:45..1:45`, defaulting to reduced resolution, for fast iteration.
8. Works on machines without a GPU (lavapipe fallback), selectable via `--adapter auto|gpu|software`.
9. Deterministic output: same inputs + same config = same video (modulo encoder nondeterminism).
10. Single distributable binary (`cargo install avz` / copy to homelab hosts). Runtime dependency: system `ffmpeg` only.

### Non-Goals (v1)

- **No lyrics** of any kind (USLT, SYLT, .lrc). Scrapped.
- No GUI, no realtime preview window, no live audio input.
- No beat-grid/BPM tracking (onset detection via spectral flux only; aubio bindings are a future optional feature).
- No video editing features (cuts, scenes, timelines).
- No Windows/macOS support commitments (should mostly work, but not tested/CI'd in v1).
- No bundled FFmpeg (system binary is fine on Fedora; document `dnf install ffmpeg`).

---

## 3. Primary Usage (UX contract)

The CLI *is* the UX. These invocations must all work and feel good:

```bash
# Zero-config happy path: defaults for everything
avz render song.mp3

# Typical use
avz render song.mp3 --preset nebula --palette ember \
      --bg art/forest.png --out video.mp4

# Fast iteration on a chorus, low-res
avz render song.mp3 --preset ribbons --sample 0:45..1:45

# Full control via config file (reproducible; check into the album repo)
avz render song.mp3 --config cold-design.toml

# Tweak one knob on top of a config
avz render song.mp3 --config base.toml --set visual.intensity=1.4

# Discovery
avz presets                 # list presets with one-line descriptions
avz presets nebula          # full parameter schema, defaults, ranges
avz probe song.mp3          # tags, duration, embedded art, sample rate

# Batch (an entire album) — plain shell, no special support needed
for f in album/*.mp3; do avz render "$f" --config album.toml; done
```

UX requirements: colored `--help` with examples (clap), progress bar with phase / frame count / render fps / ETA (indicatif), warnings that are actionable ("no GPU adapter found, falling back to software rendering — expect ~8 fps; pass --adapter software to silence this"), non-zero exit codes with clear errors for bad input, and `--verbose` / `--quiet` flags. Default output path: `<song-stem>.mp4` next to the input.

---

## 4. Architecture Overview

### 4.1 Crate layout (cargo workspace)

```
avz/
├── Cargo.toml                 # workspace
├── crates/
│   ├── avz-core/            # library: everything below except CLI
│   │   ├── src/
│   │   │   ├── analysis/      # decode → feature timeline
│   │   │   ├── meta/          # ID3 tags, cover art
│   │   │   ├── render/        # wgpu offscreen, presets, compositing, text
│   │   │   ├── encode/        # ffmpeg subprocess management
│   │   │   ├── config/        # TOML schema, validation, merging
│   │   │   └── pipeline.rs    # orchestrates analysis → render → encode
│   │   └── presets/           # WGSL + schema JSON, embedded via include_str!
│   └── avz-cli/             # thin binary: clap parsing → avz-core calls
└── assets/                    # default font (OFL-licensed), test fixtures
```

The core/cli split is the "GUI later without refactoring" guarantee. `avz-core` has zero terminal I/O; it reports progress via a callback trait.

### 4.2 Data flow

```
mp3 ──▶ [decode: symphonia] ──▶ PCM (f32, mono mix, 44.1/48 kHz)
                                   │
                                   ▼
        [analysis: rustfft]  ──▶ FeatureTimeline (one FeatureFrame per video frame)
                                   │
mp3 ──▶ [meta: lofty] ──▶ Tags ────┤
bg image / bg video ───────────────┤
preset WGSL + params ──────────────┤
                                   ▼
        [render: wgpu offscreen] ──▶ RGBA frames @ target fps
                                   │
                                   ▼
        [encode: ffmpeg subprocess, rawvideo stdin] ──▶ out.mp4 (+ original audio muxed)
```

Two-pass by design: analysis completes fully before rendering starts. This enables **lookahead** (visuals can anticipate an onset by N ms, which reads as "hitting the beat" instead of lagging it) and global normalization (features scaled to the song's own dynamic range, so quiet folk intros and wall-of-sound choruses both use the full visual range).

---

## 5. Module Design

### 5.1 `analysis` — mp3 → FeatureTimeline

**Decode.** `symphonia` (mp3 feature) → interleaved f32 PCM → mono mixdown for analysis. Keep the original file untouched for the final mux (never re-encode audio).

**Windowed FFT.** Hann window, size 2048 @ 44.1 kHz (~46 ms), hop chosen so analysis frames land exactly on video frame timestamps (for 30 fps: hop = sample_rate / 30). `rustfft` with a planner reused across the song. Parallelize windows with `rayon` (this pass should take low single-digit seconds for a 5-minute song).

**Features per frame** (all f32, all normalized 0..1 after a global pass):

| Feature | Definition | Typical visual use |
|---|---|---|
| `rms` | root mean square of the window | overall brightness / scale |
| `bass` | log-power sum, 20–150 Hz | pulse, camera shake, kick response |
| `low_mid` | 150–500 Hz | body movement, thickness |
| `mid` | 500–2000 Hz | detail motion, vocals-ish |
| `high` | 2–8 kHz | sparkle, particles, cymbals |
| `air` | 8–16 kHz | fine grain, shimmer |
| `flux` | positive spectral flux (half-wave rectified frame-to-frame spectrum diff) | onset intensity |
| `onset` | 1.0 on frames where `flux` exceeds adaptive threshold (median + k·MAD over ±1 s), else 0 | discrete hits: flashes, spawns, direction changes |
| `centroid` | spectral centroid, normalized | hue shift, vertical position |

**Envelopes.** Raw features are twitchy. Each feature gets an attack/decay envelope follower: `env = max(x, env·decay + x·(1−decay))` with per-preset-configurable attack (default ~10 ms) and decay (default ~200–400 ms). Both raw and enveloped values are available to shaders. This is the single most important knob for "motion feels musical" — the dev plan (§9, M2) budgets explicit tuning time for it.

**Normalization.** Two-pass: compute each feature's p5/p95 over the whole song, map to 0..1, clamp. Optionally per-section adaptive normalization later (backlog).

**Output.** `FeatureTimeline { fps, frames: Vec<FeatureFrame> }`, where `FeatureFrame` is a fixed-size struct (plain floats) — trivially uploadable as a uniform per rendered frame. Also computed once: song duration, and a coarse 512-bin averaged spectrum per frame for presets that want a full spectrum texture (spectrum ribbon preset).

### 5.2 `meta` — tags

`lofty` reads ID3v2/ID3v1: title, artist, album, embedded cover art (bytes + mime). All overridable via CLI/config (`--title`, `--artist`, `--no-text`). Missing tags → warn and skip the text card (or use overrides). Cover art is exposed to presets as an optional texture (e.g., a preset that refracts the album art — backlog, but the plumbing is v1).

### 5.3 `render` — wgpu offscreen pipeline

**Setup.** wgpu instance → adapter selection per `--adapter`:
- `auto` (default): request hardware adapter; on failure, retry with `force_fallback_adapter = true` (lavapipe) and print a performance warning.
- `gpu`: hardware only, hard error if unavailable.
- `software`: force lavapipe (useful for reproducibility tests and headless boxes).

Offscreen `Texture` (RGBA8UnormSrgb) at target resolution, rendered per frame, copied to a mapped buffer, handed to the encoder. Use 2–3 in-flight frames (ring of readback buffers) so GPU render and ffmpeg write overlap.

**Layer stack (bottom → top), composited in one final pass:**

1. **Background layer** — one of: solid color / gradient (from palette), static image (`image` crate; fit modes: `cover`, `contain`, `stretch`, plus optional gaussian blur and darken so visuals read on top), or **looped background video**: decoded by a *second* ffmpeg subprocess (`ffmpeg -stream_loop -1 -i bg.mp4 -f rawvideo -pix_fmt rgba -s WxH -r <fps> -an pipe:1`), read frame-by-frame on a dedicated thread into a small channel, uploaded as a texture each frame. Letting ffmpeg handle looping/scaling/fps-conversion keeps our side to "read W×H×4 bytes per frame." Audio ignored by construction.
2. **Visualizer layer** — the active preset's WGSL pipeline (see §6). Rendered to its own texture with premultiplied alpha so it composites cleanly over any background.
3. **Text layer** — title/artist card. Implementation: `cosmic-text` for shaping/layout + rasterize to a glyph atlas texture once (text is static), then it's just a textured quad with animated opacity/offset. Configurable: font file (default: bundled OFL font), size, position (9-grid + margins), color (from palette), show window (`in_at`, `hold`, `fade`), optional show again at the end. Avoids pulling a full GUI text stack; total scope ~1–2 days because the text never changes mid-render.

**Determinism.** All animation time derives from frame index / fps, never wall clock. Any randomness is a seeded hash of (frame_index, preset seed). `--seed` flag; default seed derived from the file name so re-renders match.

### 5.4 `encode` — ffmpeg subprocess

Spawn once per render:

```
ffmpeg -y \
  -f rawvideo -pix_fmt rgba -s 1920x1080 -r 30 -i pipe:0 \
  -i song.mp3 \
  -map 0:v -map 1:a \
  -c:v libx264 -preset slow -crf 18 -pix_fmt yuv420p \
  -c:a copy \
  -movflags +faststart -shortest out.mp4
```

- `-c:a copy` — original mp3 stream muxed untouched (no generational loss).
- Codec matrix behind `--codec x264|x265|av1` and `--quality` (maps to crf). Default x264 crf 18 — safe for YouTube upload.
- Write frames to stdin from a dedicated thread; monitor stderr for errors; propagate a clean failure if ffmpeg dies mid-render (don't leave a half-written file — write to `out.mp4.part`, rename on success).
- Preflight: check `ffmpeg -version` at startup, clear error message with the Fedora install hint if missing.

### 5.5 `config` — TOML

Precedence: **CLI flags > `--set` overrides > `--config` file > preset defaults > built-in defaults.** Example:

```toml
[output]
resolution = "1920x1080"     # or "720p", "1080p", "4k"
fps = 30
codec = "x264"
quality = 18

[visual]
preset = "nebula"
palette = "ember"            # named palette, or inline: ["#1a1a2e", "#e94560", ...]
intensity = 1.0
smoothing = 0.35             # global envelope decay scale
seed = "auto"

[visual.params]              # preset-specific, validated against schema
particle_count = 4000
bass_drive = 1.2
flow_scale = 0.8

[background]
image = "art/forest.png"     # or: video = "loops/smoke.mp4"
fit = "cover"
blur = 6.0
darken = 0.35

[text]
enabled = true
position = "bottom-left"
in_at = "1.0s"
hold = "6.0s"
font = "auto"
```

`serde` + `toml`, strict unknown-key rejection with "did you mean" suggestions, and `avz config --example > avz.toml` to emit a documented template.

---

## 6. Preset System

A preset = **WGSL shader(s) + JSON schema + defaults**, embedded in the binary.

**Uniform contract** every preset receives:

```wgsl
struct Globals {
    time: f32,            // frame_index / fps
    resolution: vec2<f32>,
    seed: f32,
    // features (raw + enveloped)
    rms: f32, rms_env: f32,
    bass: f32, bass_env: f32,
    low_mid: f32, low_mid_env: f32,
    mid: f32, mid_env: f32,
    high: f32, high_env: f32,
    air: f32, air_env: f32,
    flux: f32,
    onset: f32,           // decaying impulse: 1.0 at onset, exp decay
    centroid: f32,
    // palette (5 colors)
    pal: array<vec4<f32>, 5>,
    // preset params (flat vec of f32, indices per schema)
    params: array<vec4<f32>, 8>,
}
```

Plus optional bindings: spectrum texture (512×1), onset-history texture (64×1: the recent hits, so a preset can re-derive what an earlier beat spawned), album-art texture, previous-frame texture (for feedback/trail effects — the workhorse of good abstract visuals).

**Schema** (per preset, JSON): parameter name → type (float/int/color/enum/bool), default, range, description, and which uniform slot it maps to. `avz presets <name>` pretty-prints it; config validation uses it.

**v1 preset lineup** (deliberately spanning different techniques so the abstractions get exercised):

| Preset | Technique | Character |
|---|---|---|
| `pulse` | fullscreen fragment shader, SDF shapes | minimal, geometric; the "hello world" preset, built first |
| `nebula` | fbm noise flow + feedback trails | organic clouds; bass drives turbulence, ideal for dark folk |
| `ribbons` | spectrum texture → displaced ribbon geometry | classic reactive ribbons/waves, elegant |
| `particles` | GPU particle system (compute or vertex-pull), onset-triggered bursts | energetic; highs drive sparkle |
| `kaleido` | any-layer kaleidoscope post-fold + hue cycling | symmetric, hypnotic |
| `ink` | reaction-diffusion-ish feedback | slow, brooding; RMS drives growth rate |

**Palettes:** named built-ins (`ember`, `glacier`, `verdant`, `mono`, `carpathian` 🙂) + inline hex arrays. Palettes are the main lever for keeping output on-brand per channel.

---

## 7. GPU Fallback Strategy

- Single code path: wgpu → Vulkan → (hardware driver | lavapipe). No second renderer.
- Fedora note for docs: lavapipe ships in `mesa-vulkan-drivers`; verify with `vulkaninfo --summary`.
- Expectations to document: hardware (any iGPU+) renders 1080p at 100–300+ fps → 5-min song in ~1–2 min; lavapipe maybe 5–15 fps → same song in ~10–30 min. Fine for one-shot exports.
- If a specific preset is pathological on CPU (heavy feedback + high particle counts), the schema allows a `perf_hint` that prints a suggestion ("on software rendering, consider particle_count ≤ 1000"). No separate low-fi shader variants in v1.

---

## 8. Errors, Logging, Progress

- `anyhow` for CLI-level errors with context chains; typed errors (`thiserror`) inside `avz-core`.
- `tracing` with `--verbose` (debug: adapter chosen, ffmpeg cmdline, timing per phase) and `--quiet`.
- Progress callback trait in core; CLI implements it with `indicatif`: phase 1 "analyzing" (fast bar), phase 2 "rendering" (frames, fps, ETA), phase 3 "finalizing".
- Exit codes: 0 ok, 2 bad args/config, 3 input file problems, 4 render/encode failure.

---

## 9. Development Plan

Milestones sized for evening/weekend sessions. Each ends in something runnable.

**M0 — Skeleton & plumbing (1 session).**
Workspace scaffold, clap CLI with all subcommands stubbed, config structs + TOML parsing, `probe` command fully working (lofty), ffmpeg preflight check.
*Accept:* `avz probe song.mp3` prints tags/duration/art info; `avz render` errors politely with "not implemented".

**M1 — End-to-end tracer bullet (1–2 sessions).** *The riskiest plumbing, done first.*
symphonia decode → trivial features (RMS only) → wgpu offscreen with a hardcoded test shader (solid color pulsing with RMS) → readback → ffmpeg pipe → mp4 with muxed audio. `--sample` implemented here (it's just a time-range slice of the pipeline — cheap now, annoying later).
*Accept:* `avz render song.mp3 --sample 30s` produces a playable mp4 whose brightness visibly follows loudness, with correct audio, on both `--adapter gpu` and `--adapter software`.

**M2 — Real analysis + envelope tuning (1–2 sessions).**
Full FFT feature set (§5.1), envelopes, normalization, onset detection, spectrum texture. Build `pulse` as the first real preset and use it as the feature-tuning instrument. Budget dedicated time here for iterating attack/decay defaults against 3–4 reference tracks (a Cold Design track, a Carpathians track, something quiet, something dense) — this is where "feels musical" is won or lost.
*Accept:* `pulse` visibly distinguishes kick (bass), vocals (mid), cymbals (high); onsets read as on-beat, not late.

**M3 — Preset system + 3 more presets (2 sessions).**
Schema/validation/`--set`, `presets` command, palettes. Implement `nebula` (feedback texture plumbing), `ribbons` (spectrum texture consumption), `particles` (onset events). These three force every planned binding to exist.
*Accept:* all schema params adjustable via config and `--set`; adding a 4th preset requires touching only `presets/`.

**M4 — Layers: text, bg image, bg video (1–2 sessions).**
cosmic-text card, image background with fit/blur/darken, ffmpeg-reader thread for looped bg video, final compositor pass.
*Accept:* full config example from §5.5 renders correctly; bg video loops seamlessly regardless of its native fps/resolution.

**M5 — Polish & release 0.1 (1 session).**
`kaleido` + `ink` presets, progress bars/ETA, error message pass, `config --example`, README with Fedora install notes, `--codec/--quality`, `.part`-rename, seed handling. Tag v0.1, `cargo install` from the repo, batch-render a real album as the acceptance test.
*Accept:* an entire Cold Design album renders unattended via a shell loop with zero interventions.

**Total: roughly 7–10 sessions ≈ 3–4 weekends** to a v0.1 you'd actually publish videos from. M1 is the highest-risk milestone (wgpu readback + ffmpeg piping); everything after it is additive.

---

## 10. Testing Strategy

- **Unit:** DSP functions (windowing, band mapping, flux, envelope followers) against synthesized signals — a 60 Hz sine must light up `bass` and nothing else; a click train must produce onsets at known frames.
- **Golden frames:** render specific (preset, seed, synthetic-feature) frames to PNG with `--adapter software` (deterministic across machines) and compare hashes in CI. Catches shader regressions cheaply.
- **Integration:** tiny 5 s CC0 test mp3 in the repo; CI runs a full `--sample 2s` 320×180 software render and asserts ffprobe sees correct duration/streams.
- **Manual listening pass:** the M2 reference-track ritual, repeated before each release.

---

## 11. Risks & Mitigations

| Risk | Likelihood | Mitigation |
|---|---|---|
| wgpu offscreen readback friction (alignment, buffer mapping) | Medium | Tackled first in M1; well-documented pattern; keep 256-byte row padding handling in one place |
| lavapipe too slow on heavy presets | Medium | Acceptable by design (offline); `perf_hint`; document expectations |
| "Feels musical" tuning takes longer than planned | High | Explicitly budgeted in M2; envelopes/normalization designed as the tuning surface; raw+env both exposed so fixes are per-preset |
| bg video decode thread deadlocks/stalls pipeline | Low-Med | Bounded channel + timeout with clear error; ffmpeg does all format heavy lifting |
| Cross-machine nondeterminism (GPU float differences) | Low | Only matters for golden tests → those run on software adapter only |
| Scope creep toward GUI/realtime | Certain 🙂 | §2 Non-Goals; core/cli split means the door stays open without paying for it now |

---

## 12. Backlog (post-v1, explicitly deferred)

- Beat/BPM grid via `aubio-rs` (optional cargo feature) → bar-synced camera moves.
- Album-art-reactive preset (refraction/mosaic of embedded cover).
- Per-section adaptive normalization; manual section markers (`--sections 0:00,1:12,2:40`) to switch palettes/intensity per song section.
- `avz batch album/ --config album.toml` convenience command.
- Preset hot-reload dev mode (`--watch` re-renders a 5 s sample on WGSL save).
- Additional presets: tunnel, lissajous/oscilloscope, terrain flyover.
- GTK4/Relm4 front-end, if it ever earns its place.

---

## 13. Dependency Summary

| Concern | Crate | Notes |
|---|---|---|
| CLI | `clap` (derive) | colored help, subcommands |
| Progress | `indicatif` | |
| Decode | `symphonia` | mp3 feature |
| Tags | `lofty` | ID3v2, cover art |
| FFT | `rustfft` | + hand-rolled DSP (~300 LOC) |
| Parallelism | `rayon` | analysis pass |
| GPU | `wgpu` | Vulkan on Linux, lavapipe fallback |
| Text | `cosmic-text` + `swash` | static card rasterization |
| Images | `image` | png/jpg backgrounds, cover art |
| Config | `serde`, `toml` | strict schema |
| Errors/logs | `anyhow`, `thiserror`, `tracing` | |
| Encode | system `ffmpeg` | subprocess, not a crate |

---

*Next step after sign-off: execute M0 — scaffold the workspace and land `avz probe`.*
