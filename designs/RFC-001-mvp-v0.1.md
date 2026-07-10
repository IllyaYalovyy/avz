# RFC-001: MVP Development Plan for avz v0.1

| Field | Value |
|---|---|
| Status | Accepted (2026-07-09) |
| Author(s) | Illya Yalovyy |
| Supersedes | - |
| Superseded by | - |

## Summary

This RFC turns `VISION.md` into an executable development plan for a minimal
working version of avz: `avz render song.mp3` produces a finished, music-reactive
1080p30 mp4 with the original audio muxed in. It trims the v1 preset lineup from
six presets to two and defers the background-video layer, so the MVP ships the
complete pipeline and every architectural seam — analysis, preset system,
compositing, encoding — while more visualizations are added later as pure
`presets/` additions. Each plan step below maps to one GitHub issue and is
developed test-first per `docs/TESTING.md`.

## Goals

- **G1** - `avz render song.mp3` works end-to-end with zero configuration:
  decode → features → GPU render → ffmpeg encode → mp4 with `-c:a copy` audio.
- **G2** - Music-reactive motion that feels right: full feature set (bands, flux,
  onsets, centroid), attack/decay envelopes, global normalization.
- **G3** - The preset abstraction is proven: two shipped presets (`pulse`,
  `nebula`) exercising the uniform contract, schemas, `--set`, palettes, and
  feedback-texture plumbing, so preset #3 touches only `presets/`.
- **G4** - Works without a GPU via lavapipe; `--adapter auto|gpu|software`.
- **G5** - Deterministic output; golden-frame tests on the software adapter.
- **G6** - Title/artist text card from ID3 tags and a static background image.
- **G7** - CLI UX contract from VISION §3: `probe`, `presets`, `--sample`,
  TOML config with strict validation, actionable warnings, progress with ETA.

## Non-Goals

- **NG1** - The four remaining v1 presets (`ribbons`, `particles`, `kaleido`,
  `ink`) and the spectrum texture binding only `ribbons` consumes. Tracked as
  backlog issues; adding them later must not require core changes (that is G3).
- **NG2** - Looped background video. The layer-stack design accounts for it, but
  the second-ffmpeg-reader thread ships post-MVP.
- **NG3** - Codec matrix beyond x264 (`--codec x265|av1` deferred; `--quality`
  ships, mapping to CRF).
- **NG4** - Everything already excluded by VISION §2: lyrics, GUI, realtime,
  BPM tracking, editing, Windows/macOS CI, bundled ffmpeg.

## Background and Motivation

`VISION.md` (signed off 2026-07-08) defines the product, architecture, and a
six-milestone build order M0–M5. What it does not define is (a) the exact MVP
cut line — it plans all six presets and three background modes for v0.1 — and
(b) task-sized units of work with test-first acceptance criteria that an AI
agent can implement independently. The project owner has asked for a minimal
working version first, with more visualizations added later. This RFC records
that cut and the task breakdown; VISION.md remains the north star and is not
modified.

## User Impact

| Audience | Impact |
|---|---|
| End users | A usable `avz render` months earlier, with 2 presets instead of 6; no bg-video until post-MVP |
| Contributors | Work arrives as small, dependency-ordered GitHub issues with embedded context and TDD acceptance criteria |
| Operators / packagers | Unchanged from VISION: single binary, system ffmpeg, Fedora-first docs |

## Considered Options

### Option A - Ship the full VISION v0.1 scope as one plan

**Pros**: No re-triage later; matches VISION §9 exactly; spectrum texture and
bg-video plumbing land while the code is fresh.

**Cons**: Roughly 40% more work (4 extra presets, bg-video thread, codec matrix)
before the first publishable video exists; contradicts the explicit "minimal
working version" directive; the extra presets validate no new architecture —
`nebula` already forces the feedback-texture binding, `pulse` the uniform
contract.

### Option B - Vertical slice: full pipeline, two presets, image/text layers only

**Pros**: Every architectural seam from VISION §4–6 is built and tested (G3
guarantees preset additions are then cheap); first end-to-end video arrives at
M1; deferred items are additive, not structural.

**Cons**: The spectrum-texture and bg-video bindings ship later, so their
plumbing will be added by a future change instead of landing with the initial
render code; `avz presets` lists only two entries at v0.1.

### Option C - Analysis-first: perfect the DSP/feature layer before any rendering

**Pros**: The "feels musical" risk (VISION §11, likelihood High) is attacked
immediately with full attention.

**Cons**: VISION §9 explicitly puts the riskiest *plumbing* (wgpu readback +
ffmpeg piping) first as M1, and feature tuning is meaningless without a
renderer to see the result; nothing runnable for several sessions.

## Decision

**Chosen option: Option B**

Rationale: it delivers the owner's "minimal working version" while proving every
abstraction the deferred features will rely on. The two kept presets are chosen
deliberately: `pulse` (fullscreen fragment SDF) is the tuning instrument for M2,
and `nebula` (fbm + feedback trails) forces the previous-frame-texture plumbing
that is the workhorse of the deferred presets. The build order inside the MVP
follows VISION §9 unchanged — tracer bullet first — because that ordering exists
precisely to de-risk the readback/piping plumbing.

## Design

The technical design is VISION.md §4–§8 and is not restated here. Decisions
specific to this RFC:

- **Milestones become GitHub milestones M0–M5** with the same names and
  acceptance criteria as VISION §9, minus the deferred items (M3 ships `nebula`
  only; M4 ships text + bg image only; M5 ships 2 presets, not 6).
- **One plan step = one GitHub issue.** Each issue body is self-contained:
  context, expected behavior, a test-first plan naming the tests to write before
  the implementation, acceptance criteria, in/out of scope, and VISION section
  references. Dependencies are declared with "Blocked by" references.
- **TDD is the working practice.** For every task: write the failing tests
  listed in the issue first (they encode the acceptance criteria), then
  implement until green, then run `./scripts/quality.sh`. Bugs found later get a
  regression test with the fix (docs/TESTING.md regression rule).
- **Fixtures**: a tiny CC0-licensed mp3 (~5 s) plus generated synthetic signals
  (sines, clicks, silence) created in test code — never committed wav/mp3s for
  DSP tests.
- **Where the `--sample` reduced resolution lives** (decided in Step 9). VISION
  §3 wants sample renders to default to a reduced resolution, and §5.5 fixes the
  precedence chain. Rather than special-casing the flag in the renderer, `--sample`
  contributes a config layer of its own, ranked above preset defaults and below
  the `--config` file. It is therefore a default like any other: `--config` and
  `--set` still win, so previewing at the final resolution stays possible. No
  existing layer moves relative to another.
- **Where the `--sample` audio offset lives** (decided in Step 9). The picture
  starts at a frame boundary, so the audio must start at that same instant —
  `start_frame / fps`, not the seconds the user typed, which may fall between two
  frames. It reaches ffmpeg as an `-ss` in front of the mp3 input, which seeks
  and still copies: `-c:a copy` is never traded away for a sampled render.
- **Analysis windows slide inward at the song's edges** (decided in Step 11).
  Step 6 fixed each window's center on its video frame's timestamp, which leaves
  the first and last windows hanging half off the song. Zero-padding the missing
  half is the textbook answer, but it reads about 3 dB quiet across every band,
  and the fill from one padded window into the next full one is a spectral
  increase indistinguishable from an onset — the visuals would flash on a song
  that opens on a held chord. The windows within half a window of either end
  therefore slide inward to stay full, which costs at most 23 ms of timing error
  on those frames and keeps the promise `rms` already made in Step 6: a song does
  not fade in and out at its edges. A song shorter than one window has nothing to
  slide toward and is zero-padded.
- **The onset threshold carries an absolute noise floor** (decided in Step 12).
  VISION §5.1 specifies `median + k·MAD` over ±1 s, and that alone fires on
  silence: where the MAD is near zero — a held chord, a steady noise floor,
  digital silence — the threshold sits barely above the median, and under a
  Gaussian noise floor `median + 2.5·MAD` is about the 91st percentile. Roughly
  one frame in eleven would "onset" on the FFT's own numerical noise. The
  threshold is therefore `median + k·MAD + noise_floor`. The floor is an
  absolute flux rather than a fraction of the song's peak, because a
  peak-relative floor scales with whatever noise it is meant to reject and a
  steady tone would still onset. Its scale is meaningful: magnitudes are
  amplitude-normalized, so an impulse of amplitude `a` gives a flux near `2a`
  regardless of window length, and 0.05 gates out transients below roughly
  −32 dBFS. Measured: a steady 1 kHz tone peaks at 8e-5 of flux; a click at a
  tenth of full scale reaches 0.51.
- **Onsets fire on the flux peak, not the rising edge** (decided in Step 12).
  A hit's energy builds over the analysis windows that overlap it. Taking the
  first frame above the threshold puts the onset early *and* lets the refractory
  period mask the real peak, so a candidate must also be a local maximum of the
  flux. The refractory period (100 ms) then keeps one physical hit to one onset.
- **The decaying impulse is computed at analysis time** (decided in Step 12).
  VISION §6's `onset` uniform is "1.0 at onset, exp decay", but a shader sees one
  frame at a time and would need state across draws to decay anything. The
  impulse is decayed into `FeatureFrame.onset` from a time constant, so a hit
  fades over the same 150 ms at every `fps`. The binary train stays available
  through `onset::detect` and `FeatureTimeline::is_onset`.
- **Remote CI is advisory** (owner decision, 2026-07-09). The local
  `./scripts/quality.sh` gate — tests plus the invariant hooks in
  `scripts/quality.d/` — is the authority for "done". The workflow Step 10
  added stays as a safety net, but nothing waits on it, no branch protection
  gates on it, and no further CI/CD investment is planned for v0.1.
- **Issues close with the work** (owner decision, 2026-07-09). A step's GitHub
  issue is part of its definition of done: tick the checkbox here in the same
  change, and close the issue with a comment naming the behavior, merge
  commit, and tests once it is on `origin/main` (`docs/PROCESS.md`, Issue
  Tracking). Issues #1–#10 predate this rule and were closed retroactively by
  the owner.

## Testing Strategy

The full strategy and risk matrix live in `docs/TESTING.md`; every risk row maps
into the issue that owns it. Highest-risk behaviors and where they are tested:

| Risk / invariant | Test layer | Test name / location |
|---|---|---|
| Band mapping / onset math wrong | Unit (synthetic signals) | M2 issues: `sine_at_60hz_lights_up_bass_band_only`, `click_train_produces_onsets_at_expected_frames` |
| wgpu readback row padding (256 B) | Integration | M1 renderer issue: `readback_handles_non_multiple_of_256_row_widths` |
| Audio re-encoded instead of copied | Integration (bitstream compare; an ffprobe codec assert cannot see an mp3 → mp3 re-encode) | M1 encoder issue: `muxed_audio_stream_is_copied_not_reencoded` |
| Half-written output on failure | Integration | M1 encoder issue: `ffmpeg_death_midrender_leaves_no_output_file` |
| Nondeterminism / shader drift | Golden frames, software adapter | M2 golden-harness issue |
| Config precedence and strict keys | Unit | M0 config issue: `set_override_beats_config_file_value` |
| End-to-end pipeline | Integration in CI | M1 CI issue: 2 s software render + ffprobe asserts, `crates/avz-cli/tests/render_e2e.rs` |

Not automatable: "feels musical" (manual listening pass, M2 and each release)
and lavapipe behavior on a genuinely GPU-less host (manual, documented).

## Goals Alignment

| Goal | How addressed |
|---|---|
| G1 | M1 tracer bullet (steps 5–10) |
| G2 | M2 analysis + envelope tuning (steps 11–14) |
| G3 | M3 preset system + `nebula` (steps 15–17) |
| G4 | Adapter selection in step 7; software-adapter CI throughout |
| G5 | Determinism rules in AGENTS.md; golden harness in step 14 |
| G6 | M4 layer steps 18–20 |
| G7 | M0 steps 1–4 and M5 polish steps 21–23 |

## Development Plan

One checkbox = one GitHub issue. Mark `[x]` when the issue closes.
Numbering is aligned: **Step N is GitHub issue #N** (filed 2026-07-08).
Deferred NG1–NG3 items are backlog issues #24–#29, labeled `post-mvp`.

**M0 — Skeleton & plumbing** *(accept: `avz probe song.mp3` works; `avz render` fails politely)*

- [x] **Step 1** - Cargo workspace scaffold: `avz-core` + `avz-cli`, clap CLI with
  stubbed subcommands, exit codes, tracing, CI green *(prerequisite: -)*
- [x] **Step 2** - Config module: TOML schema, strict unknown-key rejection with
  suggestions, precedence merging *(prerequisite: Step 1)*
- [x] **Step 3** - ffmpeg preflight check *(prerequisite: Step 1)*
- [x] **Step 4** - `avz probe`: lofty tags, duration, cover art; CC0 fixture lands
  *(prerequisite: Step 1)*

**M1 — End-to-end tracer bullet** *(accept: `--sample 30s` renders on both adapters, brightness follows loudness, correct audio)*

- [x] **Step 5** - Audio decode: symphonia → mono f32 PCM *(prerequisite: Step 4)*
- [x] **Step 6** - Minimal FeatureTimeline: RMS only, hop aligned to video frames
  *(prerequisite: Step 5)*
- [x] **Step 7** - wgpu offscreen renderer: adapter selection, readback with row
  padding *(prerequisite: Step 1)*
- [x] **Step 8** - ffmpeg encoder subprocess: rawvideo stdin, `-c:a copy`, `.part`
  rename, stderr monitoring *(prerequisite: Step 3)*
- [x] **Step 9** - Pipeline orchestration: progress callback trait, hardcoded RMS
  test shader, `--sample` *(prerequisite: Steps 6, 7, 8)*
- [x] **Step 10** - CI integration test: 2 s software render, ffprobe asserts
  *(prerequisite: Step 9)*

**M2 — Real analysis + envelope tuning** *(accept: `pulse` distinguishes kick/vocals/cymbals; onsets on-beat)*

- [x] **Step 11** - Full FFT features: bands, flux, centroid *(prerequisite: Step 6)*
- [x] **Step 12** - Onset detection: adaptive median+MAD threshold *(prerequisite: Step 11)*
- [ ] **Step 13** - Envelope followers + two-pass normalization *(prerequisite: Step 11)*
- [ ] **Step 14** - `pulse` preset on the full Globals uniform contract + golden-frame
  harness; manual envelope-tuning pass *(prerequisite: Steps 9, 12, 13)*

**M3 — Preset system** *(accept: params adjustable via config and `--set`; preset #3 would touch only `presets/`)*

- [ ] **Step 15** - Preset schemas: JSON, validation, `--set`, `avz presets`
  *(prerequisite: Step 14)*
- [ ] **Step 16** - Palettes: named built-ins + inline hex *(prerequisite: Step 15)*
- [ ] **Step 17** - `nebula` preset: feedback texture plumbing *(prerequisite: Step 15)*

**M4 — Layers** *(accept: config example from VISION §5.5 minus bg-video renders correctly)*

- [ ] **Step 18** - Compositor pass: premultiplied-alpha layer stack *(prerequisite: Step 14)*
- [ ] **Step 19** - Background image layer: fit modes, blur, darken *(prerequisite: Step 18)*
- [ ] **Step 20** - Text card layer: cosmic-text, ID3 defaults, timing *(prerequisite: Step 18)*

**M5 — Polish & release v0.1** *(accept: an album batch-renders unattended)*

- [ ] **Step 21** - Progress bars, actionable warnings, error-message and exit-code
  audit *(prerequisite: Step 9)*
- [ ] **Step 22** - `avz config --example`, `--seed`/default seed, `--quality`
  *(prerequisite: Steps 15, 16)*
- [ ] **Step 23** - Release v0.1: README usage docs, `cargo install` from clean
  checkout, album batch acceptance run, tag *(prerequisite: all)*

## Open Questions

- [x] **Q1** - Which CC0 mp3 becomes the repo fixture? **Resolved in Step 4: the
  project authors its own.** No existing CC0 track carried both ID3v2 tags and
  embedded cover art, and vendoring one would have imported a licence to audit.
  `scripts/make-test-fixture.sh` synthesizes `assets/fixtures/tone-tagged.mp3`
  (5 s, 44.1 kHz stereo, ID3v2.3 tags, 256×256 PNG cover) and its untagged twin
  from `sin`/`exp` expressions and a generated gradient — nothing sampled,
  everything CC0. The audio is a decaying 60 Hz kick under a 1 kHz tone, so it
  also serves the Step 10 render test: loudness visibly moves and the bass band
  is separable from the mid.
- [ ] **Q2** - Exact `pulse` and `nebula` default parameter values — resolved
  during the M2 manual tuning pass against reference tracks.

## References

- [VISION.md](../VISION.md) — product and architecture north star
- [docs/TESTING.md](../docs/TESTING.md) — risk matrix these tasks discharge
- [designs/USER-TASKS.md](./USER-TASKS.md) — UX contract as testable workflows
