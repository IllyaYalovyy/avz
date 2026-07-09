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

**Status:** M0 in progress. The workspace is scaffolded and the CLI surface
exists; every subcommand still exits with "not implemented yet".

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

## Usage

The CLI is the UX. These invocations are the contract (see VISION.md §3):

```bash
# Zero-config happy path
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

# Batch an album — plain shell, no special support needed
for f in album/*.mp3; do avz render "$f" --config album.toml; done
```

Configuration precedence: CLI flags > `--set` overrides > `--config` file >
preset defaults > built-in defaults.

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
├── Cargo.toml                   # Cargo workspace
├── crates/
│   ├── avz-core/                # library: analysis / meta / render / encode /
│   │                            #          config / pipeline
│   └── avz-cli/                 # thin binary `avz`: clap → avz-core calls
├── designs/
│   ├── RFC-000-template.md      # Design proposal template
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
│   ├── install-git-hooks.sh     # Local AI-file pre-commit guard
│   ├── quality.sh               # Local quality gate
│   └── quality.d/               # Project-specific quality checks
└── .github/workflows/quality.yml
```

Still to land (VISION.md §4.1): `crates/avz-core/presets/`, holding the WGSL
shaders and schema JSON embedded via `include_str!`.

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
