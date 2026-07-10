# Changelog

All notable changes to avz are recorded here. Versions follow
[semantic versioning](https://semver.org/).

Renders are reproducible per version, not across versions: a change to a shader,
to the envelope defaults, or to normalization gives an unchanged input and config
a different video. Those changes are called out under **Breaking changes** even
when no API moved, because a config checked into an album repo is an API.

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
