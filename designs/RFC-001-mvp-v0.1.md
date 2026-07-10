# RFC-001: MVP Development Plan for avz v0.1

| Field | Value |
|---|---|
| Status | Implemented (2026-07-10) |
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
  **Landed post-MVP (issues #24, #25, #26, #27):** the spectrum texture binding
  and `ribbons`, then the onset-history binding and `particles`, then `kaleido`
  on no binding at all, then `ink` on the feedback texture that already existed.
  The two bindings were core work, as this non-goal anticipated; all four presets
  were three files in `presets/`, which is G3 holding. This non-goal is now
  closed.

  The onset-history binding was *not* planned — #24 landed believing the spectrum
  was the last generic binding this design would need. `particles` proved
  otherwise, and the reason generalizes past it. A preset that spawns something on
  a hit and then lets it live must know when the hits it is still drawing
  happened; the uniform's `onset` is one number about the frame being drawn, and a
  fragment shader carries no state between frames. The alternatives were both
  worse. Integrating particle state into a ping-ponged texture would make frame
  `N` depend on how the driver rounded frames `0..N`, and golden frames would
  become a hash of the driver rather than of the shader (G5). Widening the uniform
  would put a 64-slot array into a struct VISION §6 fixes. So the hits are handed
  over as the third optional texture, and every preset stays a pure function of
  one frame's inputs. `kaleido` and `ink` are expected to need no fourth binding —
  a fold and a reaction-diffusion read the layer beneath them and the previous
  frame, both of which already exist.

  **#26 confirmed that for `kaleido`, and asked for less than expected.** VISION
  §6 calls it an "any-layer kaleidoscope post-fold", but a preset draws its own
  premultiplied layer and cannot read the layers under it (§5.3) — the only layer
  it can fold is the previous copy of its own, through `needs_feedback`. Folding
  a procedural source instead costs nothing and reads the same: a kaleidoscope is
  its symmetry, not the glass it is cut from. So `kaleido` declares none of the
  three optional bindings, and its diff is the first to touch `presets/` and the
  docs alone. Reading the layer beneath a preset would be a change to the
  compositor's contract, and belongs in an RFC rather than in a shader.

  **#27 confirmed it for `ink` too, and found a bug in the binding it reuses.** A
  reaction-diffusion reads the previous frame, and `needs_feedback` already binds
  it, so `ink` needed no fourth binding. But `Feedback::new` cleared the history
  to `wgpu::Color::BLACK`, whose alpha is 1, while every other surface in the
  renderer clears to transparent black. The history is a *premultiplied* layer
  (§5.3), and before frame 0 there is no layer, so its coverage is zero. An opaque
  clear made `nebula` open by hiding the backdrop behind a sheet of black that
  faded down over the first frames, and would have made `ink` — whose field *is*
  the alpha channel — start every render saturated. Fixed in #27; the clear is now
  `wgpu::Color::TRANSPARENT` and `nebula`'s golden hashes moved with it.

  `ink` also settles what "a couple of feedback iterations per output frame" costs.
  Iterating the *diffusion* would mean drawing the preset `steps` times per frame:
  a change to the render contract, a full-frame copy per iteration, and the 8-bit
  state quantized `steps` times instead of once. It would also buy nothing, since
  mixing toward a frozen 3×3 blur twice only gets closer to that same blur. So the
  diffusion takes one step per frame at the lattice's stability limit, and the
  *reaction* — local, stiff, and where the pattern comes from — takes `steps` of
  them inside the one fragment shader. No core change, and `steps` stays a
  schema parameter rather than a render-graph decision.

  `ink`'s `perf_hint`, like `nebula`'s, is a measurement and not a guess. Measured
  on lavapipe at 720p over 150 frames, `steps = 8` costs 2799 ms of render phase
  against 2593 ms for `steps = 1` — under 10% for eight times the reaction, because
  the nine texture samples and the frame readback dominate and the reaction loop is
  cheap ALU. `ink` at 720p (2657 ms) is also no costlier than `nebula` (2649 ms)
  and well under `particles` (4124 ms), so the hint claims neither. It names `steps`
  only to disclaim it, and sends the reader to `--sample` and the resolution, which
  are what pay: 1080p is a little over twice the work of 720p.
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
- **Normalization runs before the envelopes** (decided in Step 13). VISION §5.1
  lists envelopes first, but the follower is positively homogeneous — every step
  is a convex combination of its input and its own last value — while
  normalization is affine *and clamped*. Smoothing first and clamping second
  would let a decayed tail sit above 1.0, and would make the attack and decay
  time constants mean something different on every master. Normalizing first also
  means the follower's output needs no clamp of its own: it cannot leave the range
  of its input.
- **The onset impulse is the one feature the global pass leaves alone** (decided
  in Step 13). VISION §5.1 says every feature is normalized 0..1 by a global
  pass, but `onset` already *is* an impulse in 0..1. Stretching it to the song's
  own p5..p95 would make a record with one weak hit flash as hard as one with a
  kick drum, and would move every hit off the exactly-1.0 value that lets
  `FeatureTimeline::is_onset` recover the binary train from the impulse. `flux`
  and `centroid` are normalized but not enveloped: both are read as instants — flux
  is onset intensity, and an envelope on a hue shift smears it across the beat —
  and VISION §6's `Globals` gives an `_env` to exactly the six energy features.
  Onsets are detected on the *raw* flux, before it is rescaled, so the absolute
  noise floor above keeps the scale it was measured against.
- **A degenerate p5..p95 spread normalizes to zeros** (decided in Step 13).
  Digital silence, a held chord, and a constant all have no dynamic range worth
  mapping. Dividing by their spread is a division by zero at best and, where the
  spread is merely tiny, amplifies the FFT's own numerical noise into full-scale
  flicker. Below `envelope::NORMALIZE_EPSILON` the track becomes all zeros —
  which is also what keeps a `NaN` out of a uniform on a silent intro.
- **`visual.smoothing` scales the decay time constant, not the attack** (decided
  in Step 13). VISION §5.5 calls it the "global envelope decay scale" and
  defaults it to 0.35, and §5.1 wants a default decay of 200–400 ms. The two are
  reconciled by making the decay scale linearly with `smoothing`, anchored so
  that `smoothing = 0.35` gives exactly the 300 ms default: no change to the
  config schema, which already validates `smoothing` into `0..=1`, and
  `smoothing = 0` degenerates to an envelope that tracks its feature exactly. The
  attack is deliberately not scaled — smoothing is what happens *after* a hit,
  and slowing the rise would move the hit off the beat, which is the one thing
  VISION §4.2 spends the two-pass architecture to avoid. Per-preset overrides
  arrive with the schemas in Step 15.
- **Normalization is global over the song, never over a `--sample` excerpt**
  (decided in Step 13). `analysis::analyze_with` never sees the sample range;
  `pipeline::render` analyzes the whole song and only then computes its frame
  range. A preview therefore shows the seconds it previews exactly as the full
  render will, which is the entire point of `--sample`.
- **The `Globals` uniform is encoded by hand, not by `bytemuck`** (decided in
  Step 14). VISION §6 fixes the members and their order; WGSL's uniform address
  space then fixes their offsets — four bytes of padding after `time` so
  `resolution: vec2<f32>` lands on 8, and both arrays on 16-byte boundaries, for
  288 bytes in all. Deriving `Pod` would need `unsafe impl`, and `avz-core` is
  `#![forbid(unsafe_code)]`; it would also hide the padding that is exactly the
  thing worth reviewing. `Globals::to_bytes` writes each `f32` little-endian at a
  documented offset, `globals_layout_matches_wgsl` pins every one of them, and
  `min_binding_size` on the bind-group layout makes the driver reject a shader
  whose struct disagrees. No new runtime dependency.
- **The seed reaches WGSL as a fraction, not a `u64`** (decided in Step 14).
  VISION §6 declares `seed: f32`, and a `u64` does not survive that trip. It is
  mixed through splitmix64's finalizer and its top 23 bits are laid straight into
  a mantissa, giving a value in `0.0..1.0` that every adapter represents exactly —
  so seeding cannot become a source of the float drift golden frames exist to
  catch. Adjacent seeds avalanche, so `--seed 1` and `--seed 2` are unrelated
  videos rather than nearly the same one.
- **`seed = "auto"` is FNV-1a of the file *stem*, hand-rolled** (decided in Step
  22). VISION §5.3 wants a default seed "derived from the file name so re-renders
  match", which rules out the path: the same album on a laptop and on a homelab
  host must render the same video. It also rules out `DefaultHasher`, which
  promises nothing across Rust releases — a seed that moves with the toolchain
  would break "same inputs, same video" only on someone else's machine, months
  later. FNV-1a over the stem's bytes is ten lines, needs no dependency, and is
  pinned by `the_auto_seed_hash_is_pinned_across_toolchains`.
- **The template is generated from `Config::default()`, not written out**
  (decided in Step 22). `config::example` builds each `key = value` from the
  resolved default, and `every_declared_key_is_documented` reads the field list
  out of serde's own `unknown field` message rather than from a list a human
  maintains — so a key added to `ConfigLayer` and forgotten in the template fails
  the build. `example_parses_under_strict_validation_into_the_built_in_defaults`
  closes the loop: what avz prints, avz accepts, and it changes nothing.
- **A deferred codec is exit 2, not exit 4** (decided in Step 22). `--codec av1`
  is a thing the user typed and it fails every song in a batch identically, which
  is what VISION §8 spends exit 2 on; exit 4 would tell a retry loop the encoder
  had a bad day. `background.video`, the other NG-deferral reachable from a
  config file, already lands there. `pipeline::render` asks `encode::video_encoder`
  before the song is decoded, so the refusal costs a millisecond, not a render.
- **Golden frames hash the RGBA bytes, and their features are hand-written**
  (decided in Step 14). `crates/avz-core/tests/golden_frames.rs` renders three
  frames per preset at 320×180 on lavapipe and compares a sha256 against
  `tests/golden/<preset>.txt`, regenerated with `AVZ_UPDATE_GOLDEN=1`. The
  `FeatureFrame`s are written out in the test rather than analyzed from the
  fixture: a golden frame fed by the DSP would be rewritten by every DSP change
  and would stop saying anything about the shader. `sha2` is a dev-dependency
  only; nothing in the shipped binary hashes anything. A Mesa upgrade can move
  the hashes, which is why the harness also carries assertions that survive one —
  `same_inputs_same_hash_twice` and `every_feature_pulse_reacts_to_changes_the_frame`.
- **`pulse` drives on envelopes, and loudness is the last word** (decided in
  Step 14). `bass_env` swells the core disc, `mid_env` packs the rings,
  `low_mid_env` drifts them outward, `high_env` lights the seeded sparkle grid,
  `air_env` adds grain, `flux` glows the edge, `onset` snaps and flashes, and
  `centroid` walks the hue along the palette's accent ramp. The whole frame is
  then scaled by `rms_env`, so a quiet passage goes nearly black instead of
  sitting at half brightness — and so "brightness follows loudness" stays an
  observable property of the assembled pipeline
  (`the_rendered_brightness_visibly_follows_the_loudness_of_the_song`). The raw
  features are in the uniform and unused by `pulse`; `nebula` and `particles` are
  what they are for.
- **The preset registry lives in `presets/`, not in `src/`** (decided in Step
  15). G3 promises that adding a preset touches only `presets/`, and a `PRESETS`
  constant in `src/render/preset.rs` breaks that promise by one line — which is
  exactly the kind of erosion nobody notices until they add the fourth preset.
  `presets/registry.rs` holds the rows and is `include!`d by the module;
  `include_str!` inside an `include!`d file resolves against that file's own
  directory, so the shader and schema paths stay local to `presets/` too. A
  preset is therefore `<name>.wgsl`, `<name>.json`, one registry row, and its
  golden hashes. `scripts/quality.d/96-a-preset-is-only-files-in-presets.sh`
  fails the gate if a schema, a shader, or a registry row drifts out of the
  directory, or if a shader ships without the schema `avz presets` prints.
- **A schema declares the uniform component each parameter occupies** (decided
  in Step 15). `VISION.md` §6 gives a preset eight `vec4` slots and says the
  schema maps names onto them. Written as `"slot": [index, component]`, that
  mapping is checkable: `PresetSchema::parse` rejects two parameters claiming one
  component (the second would silently overwrite the first) and a `color` that
  does not start at component 0 (it is four floats and would run off its `vec4`).
  `every_schema_parameter_is_read_by_the_shader_that_declares_it` then greps the
  WGSL for the accessor, so a knob wired to a slot the shader ignores fails the
  suite rather than the user's expectations.
- **Every `pulse` default reproduces the constant it replaced** (decided in Step
  15). Parameterizing a shipped shader is the one change that can rewrite every
  golden hash while looking like a refactor. Choosing defaults that are exactly
  the literals Step 14 tuned — `1.0` where the parameter scales a term, `14.0`
  where it replaces `14.0` — keeps `tests/golden/pulse.txt` byte-identical, and
  `param_reaches_declared_uniform_slot` asserts that the defaults *are* what the
  committed hashes were rendered from. A future author who changes a default
  therefore has to say so in the same commit that regenerates the hashes.
- **Preset parameters are validated before the song is decoded** (decided in
  Step 15). `pipeline::render` resolves `[visual.params]` against the schema on
  the line after it resolves the preset name, so an unknown parameter or an
  out-of-range value costs a millisecond rather than a five-minute analysis pass,
  and exits 2 with the range it violated. Defaults for parameters the config never
  names are filled at that same moment, which is why `Sources::preset_defaults`
  stays empty: a preset's defaults cannot be a config *layer* without resolving
  the preset name first, and the preset name comes out of the layers.
- **A `--set` key that names no config section is a preset parameter** (decided
  in Step 15). `--set visual.params.bass_drive=1.5` is the honest path and stays
  legal, but nobody types it twice. `bass_drive=1.5` and `pulse.bass_drive=1.5`
  expand to it, because the four `ConfigLayer` tables are a closed set and a
  first segment outside it can mean nothing else. A first segment that is neither
  a section nor a shipped preset is rejected with both lists — otherwise
  `outputt.fps=30` would be filed under `visual.params` and reported as an
  unknown *parameter*, which points the user at the wrong mistake.
- **`serde_json` is a new dependency** (decided in Step 15). `VISION.md` §6 fixes
  the schema format as JSON, `toml` cannot read it, and a hand-rolled parser
  would be a parser to maintain. It is the same ecosystem, the same maintainers,
  and the same `derive` already in the tree; it adds `itoa`, `ryu`, and `memchr`.
- **`--preset` is a flag, not only a config key** (decided in Step 23). Steps 15
  and 17 shipped the registry, the schemas, and `visual.preset`, and left the
  first flag of `VISION.md` §3's typical invocation — `avz render song.mp3
  --preset nebula` — spelled `--set visual.preset=nebula`. G7 is the §3 contract,
  and a release whose README cannot print §3 verbatim has not met it. The flag is
  a `String` that clap does not validate, because the registry is what knows which
  names exist and `pipeline::render` already rejects an unknown one, before the
  song is decoded, with the list of those that do. The risk it carries is not
  parsing but plumbing: a flag that reaches `RenderArgs` and not `ConfigLayer`
  renders `pulse` and says nothing, which is what
  `render_with_an_unknown_preset_exits_2_and_names_the_known_ones` catches by
  putting a *valid* preset in a config file and a typo on the command line — a
  dropped flag makes that render succeed.
- **`nebula`'s `perf_hint` was wrong, and is now a measurement** (decided in Step
  23). It promised that `octaves = 2` "roughly halves the frame time" on software
  rendering. Measured on lavapipe (llvmpipe, Mesa 25.3.6, 16 threads), 300 frames
  at 1080p, rendering phase only, with a draining ffmpeg stand-in so the encoder's
  backpressure is not counted: `octaves` 6/4/2/1 → 40.7/45.9/53.8/59.0 fps.
  Dropping from the default 4 to 2 buys 15% of frame time, not 50% — the three
  `fbm` calls are about a third of the shader, and the readback and the fixed
  per-pixel work are the rest. What *does* scale is pixel count: the same frames at
  720p run 2.23× faster, against a pixel ratio of 2.25. The hint now says that,
  and points at `--sample` first. A hint is user-facing advice with no assertion
  behind it; `docs/RELEASE.md` requires it be re-measured per release, and
  `docs/TESTING.md` carries the risk row.
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
- [x] **Step 13** - Envelope followers + two-pass normalization *(prerequisite: Step 11)*
- [x] **Step 14** - `pulse` preset on the full Globals uniform contract + golden-frame
  harness; manual envelope-tuning pass *(prerequisite: Steps 9, 12, 13)*

**M3 — Preset system** *(accept: params adjustable via config and `--set`; preset #3 would touch only `presets/`)*

- [x] **Step 15** - Preset schemas: JSON, validation, `--set`, `avz presets`
  *(prerequisite: Step 14)*
- [x] **Step 16** - Palettes: named built-ins + inline hex *(prerequisite: Step 15)*
- [x] **Step 17** - `nebula` preset: feedback texture plumbing *(prerequisite: Step 15)*

**M4 — Layers** *(accept: config example from VISION §5.5 minus bg-video renders correctly)*

- [x] **Step 18** - Compositor pass: premultiplied-alpha layer stack *(prerequisite: Step 14)*
- [x] **Step 19** - Background image layer: fit modes, blur, darken *(prerequisite: Step 18)*
- [x] **Step 20** - Text card layer: cosmic-text, ID3 defaults, timing *(prerequisite: Step 18)*

**M5 — Polish & release v0.1** *(accept: an album batch-renders unattended)*

- [x] **Step 21** - Progress bars, actionable warnings, error-message and exit-code
  audit *(prerequisite: Step 9)*
- [x] **Step 22** - `avz config --example`, `--seed`/default seed, `--quality`
  *(prerequisite: Steps 15, 16)*
- [x] **Step 23** - Release v0.1: README usage docs, `cargo install` from clean
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
- [x] **Q2** - Exact `pulse` and `nebula` default parameter values. **Resolved in
  Steps 14, 15, and 17:** every default now lives in the preset's own JSON schema,
  which is the single place `avz presets` prints from and config validation reads.
  `param_reaches_declared_uniform_slot` pins them from the other side — the
  committed golden hashes must be what the schema's defaults render, so a default
  that drifts fails the suite rather than a video nobody re-watches. They remain
  the surface the manual listening pass moves (`docs/TESTING.md`, `docs/RELEASE.md`).

## References

- [VISION.md](../VISION.md) — product and architecture north star
- [docs/TESTING.md](../docs/TESTING.md) — risk matrix these tasks discharge
- [designs/USER-TASKS.md](./USER-TASKS.md) — UX contract as testable workflows
