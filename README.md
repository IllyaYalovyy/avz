# avz

Abstract music video generator. Rust CLI. mp3 in, music-reactive video out.

`avz` takes a valid mp3 and renders an abstract, music-driven video (H.264/AV1
mp4) with the original audio muxed in untouched. Visuals are GPU shader presets
(WGSL via wgpu) whose motion is driven by audio features extracted offline —
band energies, RMS, spectral flux and onsets — smoothed through attack/decay
envelopes so motion feels musical rather than twitchy.

Rendering is fully offline and deterministic: frames are generated at exact
timestamps and piped as raw RGBA into an FFmpeg subprocess for encoding. It
works without a GPU via Mesa lavapipe (software Vulkan) — same shaders, same
code path, just slower.

See [VISION.md](VISION.md) for the full design: architecture, module breakdown,
preset system, milestones, and backlog. VISION.md is the north star; a feature
request that does not serve it goes to the backlog.

**Status:** v0.1. The whole pipeline runs: decode, full FFT analysis with
envelopes and onsets, a preset on the GPU, a background image and a title card
composited over it, ffmpeg encode with the original audio muxed untouched. Six
presets ship — `pulse`, `nebula`, `ribbons`, `particles`, `kaleido`, and `ink` —
selected with `--preset`. See [CHANGELOG.md](CHANGELOG.md) for what landed and
what is deliberately absent (the looped background video, and every codec but
x264).

**Presets.** `avz presets` lists them; `avz presets <name>` prints the
parameters, their defaults and ranges, and any note about software rendering:

```bash
avz render song.mp3 --preset nebula
avz render song.mp3 --preset nebula --set nebula.trail_decay=0.94
```

**Palettes.** `--palette` takes a built-in name — `ember`, `glacier`, `verdant`,
`mono`, or `carpathian` — or two to eight inline hex colors, which avz resamples
in Oklab onto the five slots a shader reads:

```bash
avz render song.mp3 --palette glacier
avz render song.mp3 --palette '#1a1a2e,#e94560,#ffd93d'
```

**Background image.** `--bg` composites a png or jpeg beneath the visuals. It is
fitted by `background.fit` — `cover` (the default) crops, `contain` letterboxes
onto the palette gradient, `stretch` distorts — and `background.blur` and
`background.darken` push it back so the visuals read on top:

```bash
avz render song.mp3 --bg art/forest.png \
      --set background.fit=contain --set background.blur=6 --set background.darken=0.35
```

A looped background video is planned but not built; `background.video` is
refused with a message that says so.

**Text card.** The title and artist are read from the song's ID3 tags and set in
a bundled OFL font over the visuals — fading in at `text.in_at`, holding for
`text.hold`, and fading out again. `--title` and `--artist` override the tags,
and `--no-text` draws no card at all. A song with neither tag and no override
renders without a card, and says so:

```bash
avz render song.mp3 --title 'Cold Design' --artist 'avz'
avz render song.mp3 --no-text
avz render song.mp3 --set text.position=top-right --set text.size=0.06
```

Type size and margins are fractions of the frame height, so 720p and 4k are the
same picture at different scales. `text.position` is the nine-grid from
`VISION.md` §5.3, and `text.font` may point at a font file of your own.

## Requirements

- Rust (stable) and Cargo
- System `ffmpeg` — the only runtime dependency

  ```bash
  sudo dnf install ffmpeg          # Fedora (primary target platform)
  ```

- A Vulkan driver. Without a GPU, install Mesa lavapipe for software rendering:

  ```bash
  sudo dnf install mesa-vulkan-drivers
  vulkaninfo --summary             # verify an adapter is visible
  ```

Target platform is Linux (Fedora primary). Windows and macOS are not tested or
CI'd in v1.

## Install

```bash
git clone https://github.com/IllyaYalovyy/avz && cd avz
cargo install --path crates/avz-cli
avz --version
```

That puts `avz` in `~/.cargo/bin`. Nothing else is vendored or downloaded at
runtime: the presets, their schemas, and the text-card font are compiled into the
binary, and `ffmpeg` is looked up on `PATH` before a render does any work.

## Performance

Rendering is offline, so slow is inconvenient rather than fatal. Both halves of
the pipeline matter: the shader draws frames and x264 encodes them, concurrently,
and a render moves at the speed of whichever is slower. On a busy preset the GPU
is the floor; on `pulse` at 1080p it is usually x264 `-preset slow`.

Measured on one machine — Radeon RX 6600M, 16-thread CPU, Mesa 25.3.6 — rendering
a 60-second song at 1080p30, wall clock end to end:

| Preset | `--adapter gpu` | `--adapter software` (lavapipe) |
|---|---|---|
| `pulse` | 33 s | 49 s |
| `nebula` | 53 s | 83 s |

Extrapolated to a 5-minute song: roughly 3–5 minutes on a GPU, and 4–7 on this
CPU's software rasterizer. Treat the lavapipe column as a property of the CPU,
not of avz — a two-core box will be several times slower, which is the 5–15 fps
the fallback warning quotes. Two levers when a preview is what you want:
`--sample 0:45..1:45` renders an excerpt at 720p, and frame time falls with pixel
count.

## Usage

The CLI is the UX. These invocations are the contract (see VISION.md §3):

```bash
# Zero-config happy path
avz render song.mp3

# Typical use
avz render song.mp3 --preset nebula --palette ember \
      --bg art/forest.png --out video.mp4

# Fast iteration on a chorus, low-res
avz render song.mp3 --preset nebula --sample 0:45..1:45

# Pick the adapter: auto (default), gpu (fail without one), software (lavapipe)
avz render song.mp3 --adapter software

# Start from a documented template of every setting, at its default
avz config --example > avz.toml

# Full control via config file (reproducible; check into the album repo)
avz render song.mp3 --config cold-design.toml

# Reproduce a render exactly. `--seed auto`, the default, hashes the song's file
# name, so the same mp3 renders the same video from any directory or machine.
avz render song.mp3 --seed 1337

# Encoding: x264 only in v0.1. `--quality` is the CRF, 0 (huge) to 51 (worst).
avz render song.mp3 --quality 23

# Tweak one knob on top of a config
avz render song.mp3 --config base.toml --set visual.intensity=1.4

# Tune a preset parameter. A key that names no config section is a parameter of
# the preset being rendered, so these three spellings all mean the same thing:
avz render song.mp3 --set visual.params.bass_drive=1.5
avz render song.mp3 --set pulse.bass_drive=1.5
avz render song.mp3 --set bass_drive=1.5

# Discovery
avz presets                 # list presets with one-line descriptions
avz presets pulse           # full parameter schema, defaults, ranges
avz probe song.mp3          # tags, duration, embedded art, sample rate

# Batch an album — plain shell, no special support needed
for f in album/*.mp3; do avz render "$f" --config album.toml; done
```

Configuration precedence: CLI flags > `--set` overrides > `--config` file >
preset defaults > built-in defaults. `--sample` contributes one default of its
own — a reduced 720p resolution — which ranks just above preset defaults, so a
config file or a flag still wins.

Each render writes to `<song-stem>.mp4` beside its input and exits 0, so the
batch loop above needs nothing from avz but its exit code: 2 means the arguments
or the config are wrong and every remaining track will fail the same way, 3 means
*this* song is unreadable, and 4 means the render or the encode failed. Nothing
appears at the output path until ffmpeg has exited cleanly, so an interrupted
batch leaves no half-written mp4 to mistake for a finished one.

## Project Workflow

The default workflow is intentionally simple:

1. Write or update the user task / problem statement in `designs/USER-TASKS.md`.
2. Create an RFC for broad, irreversible, cross-cutting, or dependency-adding
   changes.
3. Implement in small reviewable steps.
4. Add tests at the layer where the risk lives.
5. Run `./scripts/quality.sh`.
6. Review for behavior, regressions, secrets, and maintainability before merge.

If you clone this repository, install the local pre-commit hook that blocks AI
task-runner files, prompts, context files, and chat logs from being committed:

```bash
./scripts/install-git-hooks.sh
```

## Repository Layout

```text
.
├── VISION.md                    # Product and architecture north star
├── AGENTS.md                    # Instructions for AI coding agents
├── CONTRIBUTING.md              # Contributor rules and quality bar
├── CHANGELOG.md                 # What each release changed
├── Cargo.toml                   # Cargo workspace
├── crates/
│   ├── avz-core/                # library: analysis / meta / render / encode /
│   │   │                        #          config / pipeline
│   │   └── presets/             # WGSL + JSON schema + registry, embedded
│   └── avz-cli/                 # thin binary `avz`: clap → avz-core calls
├── designs/
│   ├── RFC-000-template.md      # Design proposal template
│   ├── RFC-001-mvp-v0.1.md      # The v0.1 development plan
│   └── USER-TASKS.md            # User workflow inventory
├── docs/
│   ├── PROCESS.md               # How work moves from idea to merge
│   ├── COMMITS.md               # Commit identity, staging, and message rules
│   ├── DESIGN-REVIEW.md         # RFC/design review rules
│   ├── REVIEW.md                # Review checklist and expectations
│   ├── TESTING.md               # Testing strategy and risk matrix
│   ├── TEMPLATE-RATIONALE.md    # Practices carried over from source projects
│   ├── RELEASE.md               # Release checklist
│   └── prompts/                 # Copy-ready AI prompts for common workflows
├── scripts/
│   ├── album-acceptance.sh      # Batch-render an album unattended (UT-010)
│   ├── install-git-hooks.sh     # Local AI-file pre-commit guard
│   ├── make-test-fixture.sh     # Regenerate the CC0 mp3 fixtures
│   ├── quality.sh               # Local quality gate
│   └── quality.d/               # Project-specific quality checks
└── .github/workflows/quality.yml
```

A preset is three things in `crates/avz-core/presets/`: `<name>.wgsl`,
`<name>.json` declaring its parameters, and one row in `registry.rs`. Adding one
touches nothing else, which
`scripts/quality.d/96-a-preset-is-only-files-in-presets.sh` enforces.

`avz-core` has zero terminal I/O and reports progress through a callback trait.
That core/cli split is the "a GUI could be added later without refactoring"
guarantee — keep it intact. `scripts/quality.d/10-core-is-ui-agnostic.sh`
enforces it: no printing from core, no `clap` / `anyhow` / `indicatif` in its
dependency tree.

## Quality Gate

Run the local quality gate before asking for review:

```bash
./scripts/quality.sh
```

Once `Cargo.toml` exists the script runs `cargo fmt --check`,
`cargo clippy --all-targets --all-features -- -D warnings`, and
`cargo test --all-targets --all-features`. It also syntax-checks `scripts/*.sh`.

Project-specific checks belong in executable files under `scripts/quality.d/`.

## Design Documents

Use `designs/RFC-000-template.md` for changes that are hard to reverse, touch
multiple parts of the system, add dependencies, or change external behavior.

Use `designs/USER-TASKS.md` to keep user-facing workflows explicit and testable.

## AI Prompt Templates

Reusable prompts live in `docs/prompts/`:

- `task.md` - turn a request into a scoped task
- `rfc.md` - draft or revise an RFC
- `implement.md` - implement accepted work
- `review.md` - review a diff or branch
- `commit.md` - prepare a clean commit

## License

See [LICENSE](LICENSE).
