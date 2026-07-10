# Testing Strategy

This project values tests that protect user behavior and system invariants over
raw coverage percentages.

`avz` is an offline batch tool with no interactive surface, so the risk
concentrates in three places: the DSP math being wrong in ways that still look
plausible, the GPU readback / ffmpeg piping plumbing, and determinism drifting
without anyone noticing.

## Test Layers

Use the lowest layer that catches the risk clearly:

- **Unit tests** - pure functions, model invariants, parsing, validation,
  reducers, state transitions
- **Integration tests** - storage, API clients, process boundaries, migrations,
  serialization compatibility, concurrency contracts
- **UI / behavioral tests** - user flows, keyboard and pointer behavior,
  accessibility-visible state, layout regressions
- **Property / fuzz tests** - parsers, protocols, tree structures, state
  machines, and untrusted input
- **Manual checks** - hardware, external providers, stores, or other cases where
  automation is not practical

## How Those Layers Land in avz

**Unit — DSP against synthesized signals.** The rule: assert on signals whose
correct answer is known analytically, not on recorded output. A 60 Hz sine must
light up `bass` and nothing else. A click train must produce onsets at known
frame indices. An envelope follower fed a step must reach its attack target in
the configured time. Silence must normalize without dividing by zero.

The FFT half of that lives in `analysis/spectrum.rs` as pure functions over a
magnitude spectrum and a bin width, which is what lets the band, flux, and
centroid math be checked against spectra written by hand — a lone bin at a known
frequency has a centroid nobody has to measure. `analysis/onset.rs` is the same
trick one level up: a pure function of a whole flux track and an `fps`, so
"a spike this far above its neighbours is a hit" can be asserted on flux written
by hand, with no FFT in the way. `analysis/features.rs` owns window placement and
the parallel drive loop, so its tests assert on whole synthesized songs instead.
`the_same_song_analyzes_to_the_same_timeline_twice` guards a decision invisible
from any single window: that rayon's reduction is index-ordered, not
thread-completion-ordered.

**The onset noise floor.** `median + k·MAD` is the threshold `VISION.md` §5.1
specifies, and on its own it fires on silence. Where the MAD is near zero — a
held chord, digital silence, a steady noise floor — the threshold sits barely
above the median, and under a Gaussian noise floor `median + 2.5·MAD` is about
the 91st percentile: roughly one frame in eleven would "onset" on the FFT's own
numerical noise. `onset::DEFAULT_NOISE_FLOOR` is the absolute gate that closes
that hole, and it is what `steady_tone_produces_no_onsets`,
`silence_produces_no_onsets`, and `a_bump_below_the_noise_floor_is_not_a_hit`
pin. Its scale is meaningful rather than arbitrary because magnitudes are
amplitude-normalized: an impulse of amplitude `a` produces a flux near `2a` at
any window length.

The adaptive half earns its keep in `quiet_section_clicks_still_detected`, which
asserts not only that the quiet clicks are found but that a *global* threshold
computed over the same song would have missed them — otherwise the test would
pass against a fixed threshold and prove nothing.

**Normalization erases the units the DSP tests assert on.** `analyze` rescales
each feature against that feature's own p5..p95 over the whole song, which is
exactly the information "a 60 Hz sine's bass energy towers over its mid" is a
statement about — and it rescales each feature *independently*, so after the pass
"bass towers over mid" is not a comparison the timeline can even express. The
windowed-FFT half therefore stays reachable as `features::raw_timeline`, and every
test whose expected value is known analytically asserts on that. `analyze` itself
is `raw_timeline` plus a pure function of a whole track, and that function is
tested as one in `analysis/envelope.rs` against a step, a ramp, and a
hand-computed hundred-value vector. What remains on `analyze` are the properties
only the composed pass has: everything in `0.0..=1.0`
(`every_feature_of_the_fixture_lands_in_the_unit_interval`), no `NaN` on silence,
the onset impulse untouched, and two masters twenty decibels apart producing the
same timeline.

**The envelope is pinned in time, not in frames.** `env = x + (env - x)·exp(-1/(τ·fps))`
means a hit swells and fades over the same milliseconds at 24, 30, and 60 fps, and
`step_input_env_reaches_90pct_within_attack_budget` and
`release_tail_matches_decay_time_constant` assert exactly that, at each of them,
against the closed form `exp(-t/τ)` rather than against recorded output. The
follower needs no output clamp because every step is a convex combination of its
input and its own last value, so it cannot leave the input's range — a property
rather than an arithmetic fact, and therefore checked as one over pseudo-random
tracks in `env_never_exceeds_input_peak_or_drops_below_zero`. The attack and decay
defaults (10 ms / 300 ms) are the M2 manual-tuning surface; the tests pin the
*math*, and the reference-track listening pass is what moves the numbers.

The VISION §5.1 performance budget — low single-digit seconds for a five-minute
song, which is what the reused FFT planner and the parallel windows buy — is a
smoke test in `crates/avz-core/tests/analysis_perf.rs`. It sits there rather than
beside the code because it reads a wall clock, and
`scripts/quality.d/90-animation-time-comes-from-the-frame-index.sh` rightly
forbids `Instant::now()` under `crates/avz-core/src`. It is `#[ignore]`d, since a
loaded machine would make it flake as a per-commit gate; run it with
`cargo test -p avz-core --test analysis_perf -- --ignored`.

**Golden frames.** `crates/avz-core/tests/golden_frames.rs` renders specific
`(preset, seed, synthetic-feature)` frames on the software adapter and compares
the sha256 of their RGBA bytes against `crates/avz-core/tests/golden/<preset>.txt`.
`crates/avz-core/tests/golden/palettes.txt` pins the same thing for the other
axis: one `pulse` frame under each built-in palette, which is what catches a
palette whose colors moved and a palette that renders no differently from its
neighbour. The software adapter is what makes this stable across machines; never
run golden tests on a hardware adapter, because GPU float differences are
expected, and
`scripts/quality.d/95-golden-frames-run-on-the-software-adapter.sh` enforces it.
This catches shader regressions cheaply.

The feature frames are *hand-written*, not analyzed from the fixture. That is
the point: a golden frame whose input came out of the DSP would be rewritten by
every change to the DSP, and would then stop saying anything about the shader.

What is hashed is the *composited* frame: the preset's premultiplied layer over
the palette backdrop, which is the stack `pipeline::render` builds and the bytes
ffmpeg receives. Hashing the visualizer layer alone would leave the compositor
and the backdrop uncovered by the only shader test in the project.

A preset whose schema declares `needs_feedback` is drawn from frame 0 up to the
golden frame, and only the last frame is hashed. Hashing `nebula`'s frame 100 in
isolation would pin a picture with no trails in it, which is to say none of what
the preset is for — and a shader that quietly stopped sampling the previous frame
would still match its hashes.

**Regenerating golden hashes.** When a preset changes on purpose:

```bash
AVZ_UPDATE_GOLDEN=1 cargo test -p avz-core --test golden_frames
```

That rewrites the hash files; commit them with the shader change and say in the
message what moved. Regenerating to turn a red test green without looking at why
it went red throws the only shader coverage in the project away. Two things can
turn it red legitimately: an intended shader edit, and a Mesa upgrade that
changes lavapipe's rounding. The `every_feature_pulse_reacts_to_changes_the_frame`
and `same_inputs_same_hash_twice` tests stay green through the second, so a
regenerate is safe when only the hashes moved and nothing else did.

**The compositor.** `crates/avz-core/tests/compositor.rs` pins the layer stack
(`VISION.md` §5.3) against layers filled with flat colors rather than against a
preset, because the `over` operator is the compositor's property and would be
invisible through a shader that also draws rings.
`premultiply_blend_math_matches_reference` computes three blends by hand — opaque,
half-covering, transparent — and demands the GPU agree; a non-premultiplied blend
passes two of the three and misses the middle one by a factor of two.
`absent_layers_skip_render_passes` probes the *alpha* channel of a lone layer,
which is the only thing that distinguishes "no background layer was drawn" from
"a black opaque one was". `visualizer_alpha_zero_shows_background_exactly` closes
the loop with the real `pulse`: on a silent frame its layer is transparent, and
the composited frame is byte-for-byte the backdrop alone.

**The background image.** The layer is built on the CPU, once per render, so
`crates/avz-core/src/render/background.rs` can test the whole of it as a pure
function from a palette, an image, and a frame size to the bytes the texture
holds — no GPU in the way. `place` is separated out because the fit modes are
integer geometry and nothing else: `cover` is asserted as a *crop* rather than as
an oversized scale, which is the decision that keeps a 1×1000 source from
allocating sixty gigabytes on its way into a 1080p frame.

The rest of the module's tests exist because every step is a place where sRGB
bytes could be mistaken for light. `darken_dims_the_light_rather_than_the_encoded_byte`
and `a_blur_averages_light_rather_than_encoded_bytes` both pin the same mistake
from opposite sides: half the photons of white is `#bc`, and any test that
accepted `#80` would accept a background a stop and a half too dark.
`a_blur_of_a_flat_field_darkens_no_edge` catches a kernel that pulls black in
from outside the frame and vignettes every render.
`a_contained_image_letterboxes_onto_the_palette_backdrop` and
`a_transparent_image_lets_the_backdrop_through` pin the one design decision in
the module — that the image is drawn *over* the backdrop rather than instead of
it, so a letterbox bar and a hole in a PNG are the same thing.

`a_background_image_reaches_the_rendered_frames` in `pipeline_render.rs` is what
says the layer is actually in the stack. It compares against the same render
without the image rather than probing one pixel, because `pulse` draws over the
background and no single pixel is guaranteed to be the image alone.

**The background video.** ffmpeg loops, scales, and frame-rate-converts it, so
the tests that matter are about what avz asked for and what came back — never
about the codec. Both halves are pinned, as with the optional textures.

`crates/avz-core/src/render/video.rs` owns the half that is a pure function.
`the_loop_is_requested_before_the_input_it_loops` is the whole design in one
assertion: `-stream_loop` after `-i` applies to nothing, and a loop shorter than
the song would simply run out mid-render with no test noticing.
`the_background_videos_audio_is_never_decoded` holds `-an` to the `AGENTS.md`
audio invariant from the one direction the encoder's own tests cannot see, and
`every_fit_flattens_the_sources_alpha_before_it_scales` pins the decision the
whole upload path rests on: with the source's alpha discarded before any
resampling, the only transparency left is a `contain` letterbox bar, alpha is 0
or 255 and nothing between, and premultiplied and straight alpha are the same
bytes. A chain that scaled *first* would smear the bar's edge into fractional
alpha, and the layer would fringe toward black along every letterbox — invisible
to a golden hash, and to every other test here.

`crates/avz-core/tests/background_video.rs` owns the half only a real decode can
answer. `a_one_second_loop_repeats_frame_for_frame_under_a_longer_render` is the
seam: `-stream_loop -1` re-decodes the same file, so loop *n* is byte-identical to
loop 0, which turns "seamless" from a tolerance into an equality on whole frames.
Its fixture is a flat colour per frame stepping by 8 in red, encoded losslessly,
because `testsrc2` at 64×36 hands back the same bytes for adjacent frames and a
seam that stuttered by one frame would pass. The two `assert_ne!`s at the end are
what keep the equality from being vacuous: a loop of one still frame is perfectly
periodic and proves nothing.

`a_slower_source_is_frame_rate_converted_rather_than_starving_the_render` is `-r`
as an assertion. Without it a 15 fps loop under a 30 fps render would hand avz
fifteen frames a second, and the render would block on the decoder for half of
every second — a pipeline running at half speed, and no test failing.
`a_contained_video_letterboxes_with_transparent_bars` is the `contain` bar read
in the alpha channel, which is the only channel that distinguishes "the backdrop
shows through here" from "a black bar was drawn here".

The pipeline's half is two tests. `a_background_video_reaches_the_rendered_frames`
says the layer is in the stack, compared against the same render without it for
the same reason the image test is. `each_rendered_frame_draws_the_next_frame_of_the_loop`
is the one that covers the bug the picture would hide: a reader that re-uploaded
the frame it already had, or that ran two frames ahead per rendered frame, draws a
loop that is magenta all the way through — as magenta as a correct one. So the
loop counts in red, forty per frame, and the rendered frames must count with it.
Flatten that loop to a constant and the test fails on frame 3, which is `pulse`'s
own red refusing to be monotone: the assertion is measuring the background, not
the preset.

Both tests need a real decoder *and* the raw RGBA avz piped, out of one
preflighted binary. `recording_ffmpeg_with_a_real_decoder` dispatches on the argv
— `-stream_loop` appears in the reader's invocation and in no other — and `exec`s
the system ffmpeg for it.

**A stalled decoder is `VISION.md` §11's named risk, and a fake ffmpeg is how it
gets tested.** A real one cannot be made to wedge on cue.
`a_decoder_that_stops_producing_frames_times_out_and_names_the_video` writes one
frame and then sleeps, which is the exact shape of the failure: the render begins,
the picture appears, and then nothing. `a_dropped_background_video_kills_ffmpeg_and_joins_its_threads`
covers the deadlock on the way out — the reader thread is blocked on a full queue
by then, and only dropping the receiver *before* killing the child unblocks it.

The stand-ins `exec` rather than fork, because the pid avz kills has to be the pid
holding stdout. A shell that forked `sleep 60` leaves its child holding the pipe
open, `Drop` blocks in `join`, and the test takes a minute to pass — which is what
the first draft of this suite did, and what a real ffmpeg never does.

Neither the bound on the queue nor the timeout is visible to any of these tests:
an unbounded channel delivers the same frames, and a blocking `recv()` returns
them just as fast. `scripts/quality.d/43-background-video-reader-is-bounded-and-times-out.sh`
is what says so at the source, and it also holds `-an` there — so a refactor that
dropped either the bound or the mute has to argue with the hook rather than with
nothing.

The preset side of that contract is
`every_preset_draws_a_layer_the_backdrop_shows_through` in `golden_frames.rs`. A
shader that ends `return vec4<f32>(color, 1.0)` compiles, renders, and hashes
perfectly well while hiding the background layer under an opaque rectangle in
every video anyone makes with it. Golden hashes cannot catch that — they bless
whatever they are shown — so the preset's own layer is composited with nothing
under it and required to have somewhere it did not cover.

**The preset contract.** A preset is one WGSL file in `crates/avz-core/presets/`,
one JSON parameter schema beside it, and a row in `presets/registry.rs` — which
`src/render/preset.rs` `include!`s, so all three live in one directory and
`scripts/quality.d/96-a-preset-is-only-files-in-presets.sh` can say so. That hook
is RFC-001 G3 as a check: it fails if a shader ships without a schema, if a
schema names no shader, if a registry row goes missing, or if a preset is
embedded from `src/`. `every_schema_parameter_is_read_by_the_shader_that_declares_it`
covers the other half — a schema slot the WGSL never reads is a knob wired to
nothing — and `param_reaches_declared_uniform_slot` proves the same thing in
pixels, by turning each parameter off its default and demanding the frame change.
That test also pins the *defaults*: the committed golden hashes must be what the
schema's own defaults render, so a default that drifts away from the constant it
replaced fails there rather than in a video nobody re-watches. `every_preset_declares_the_whole_globals_contract`
checks that its `struct Globals` still spells out the `VISION.md` §6 members,
because a preset that renamed one would compile against its own struct and read
the wrong feature at that offset.

**The feedback texture.** One of the two bindings beyond the uniform a preset may
ask for (`VISION.md` §6). `crates/avz-core/tests/feedback_texture.rs` pins the
renderer's half against test presets built in the test itself rather than against
`nebula`: that frame 0 samples black, that frame N samples frame N-1, and that a
preset which did not declare `needs_feedback` fails to build if it reaches for
`@binding(1)` anyway. The preset's half — that `nebula` is actually wired to it —
is `nebula_frames_depend_on_the_frames_before_them`, which renders frame 30 warm
and cold and demands they differ. Neither is redundant: the plumbing tests would
pass against a `nebula` that never sampled, and the `nebula` test would pass
against plumbing that leaked an uncleared texture into frame 0.

**The spectrum texture.** The other one, and it is tested in the same two halves
for the same reason. `crates/avz-core/tests/spectrum_texture.rs` pins the
renderer's half against a test shader that paints each column of the frame with
the bucket that column stands for, so the frame *is* the spectrum row: bucket `n`
reaches texel `n` unsmeared, the row uploaded is the frame being drawn rather
than the render's first, silence draws black, and a preset that did not declare
`needs_spectrum` fails to build if it reaches for `@binding(3)` anyway. Because
the two bindings are independent, one test asks for both at once and demands the
feedback binding did not move out from under the shader.

The preset's half is `ribbons_draws_its_light_where_the_spectrum_has_energy`,
which puts energy in the lowest buckets and then the highest and demands the
light move from the left of the frame to the right — the frequency axis is the
frame's width, and a `ribbons` wired backwards passes every hash.
`ribbons_draws_nothing_where_the_spectrum_is_silent` is the other direction: a
preset that drew its lanes whatever the texture said would satisfy the golden
hashes and ignore the music.

**The onset-history texture.** The third, tested in the same two halves.
`crates/avz-core/tests/onset_history_texture.rs` pins the renderer's half against
a test shader that paints column `n` with slot `n`'s birth time and ordinal, so
the frame *is* the history row: both channels survive in the right order, an
unfilled slot reads the `NO_ONSET` sentinel rather than a hit at time zero, the
row uploaded is the frame being drawn, and a preset that did not declare
`needs_onsets` fails to build if it reaches for `@binding(4)`.

The preset's half is eight tests on `particles`, and each covers a way the preset
could pass its golden hashes while being wrong.
`particles_throws_a_burst_that_expands_as_the_hit_that_threw_it_recedes` reads the
history as *time*: a `particles` that dropped its `textureLoad` and drew a still
cloud would be blessed without the music in it.
`particles_draws_nothing_without_a_live_hit` is the other direction, and covers
the sentinel from the preset's side — a burst older than its lifetime, and a frame
no hit has reached yet, must both be black.
`a_burst_is_hashed_from_the_hit_that_threw_it_and_not_from_its_slot` covers the
one bug the picture would hide: a slot is a place in a sliding window, and a
preset that hashed it would tear every live particle across the frame on every
kick — a shimmer that reads as an effect rather than as a bug, and that two
blessed hashes would never disagree about.

`the_burst_cull_takes_no_light_off_the_burst_it_skips` covers the optimization.
`particles` skips a whole burst when the pixel it is shading lies outside the
shell between the burst's slowest and fastest particle, widened by how far a
particle's halo reaches. Widened by too little, the cull shaves the halo off both
faces of that shell, and *every other test still passes*: the burst expands, it
fades, it twinkles, and its golden hashes are blessed with the clipping in them.
So the test asserts the shape rather than the speed — a ring of pixels that is
dark while the ring beside it is bright is a cut, not a falloff. Narrowing the
cull's `reach` from `size * GLOW_REACH` to `size` still leaves six of the other
seven `particles` tests passing: the burst expands, fades, twinkles, and stays a
pure function of its frame.

`a_burst_lights_a_disc_on_the_frame_it_is_thrown` is the seventh, and the other
half of that same cull. At age zero every particle of a burst is still at the
origin, and what the frame shows is the disc their overlapping glows make. A cull
that only admitted pixels *on* the burst's shell would shrink that disc to a
single pixel, and the expansion test above would not notice — a one-pixel burst
still expands.

`particles_renders_a_frame_the_same_whether_or_not_the_frames_before_it_were_drawn`
is the determinism claim the whole design rests on. A burst is re-derived from the
hit that threw it on every frame, never integrated forward, so frame `N` is a pure
function of frame `N`'s inputs — which is what lets `--sample 1:00..1:03` render
the same seconds a full render would, and what keeps a golden hash from becoming a
hash of the driver's rounding on the frames before it.

**A preset on no binding at all.** `kaleido` asks the renderer for nothing beyond
the uniform, so there is no plumbing half to test and the whole risk lives in the
shader. Four tests carry it, and each covers a way the preset could pass its
golden hashes while being wrong.

`kaleido_folds_the_frame_into_the_wedges_it_declares` is the preset itself as an
assertion. A shader that drew the same petals and rings *without folding them*
would be blessed by every hash, would react to every feature, and would leave the
backdrop showing through — nothing else here would say a word. So the frame is
compared against itself turned by one wedge, which a fold into `segments` wedges
is symmetric under by construction, and then against itself turned by half a
wedge, which it must not be: otherwise the first assertion would pass on
concentric rings with no angular structure at all. The profile is read around a
circle with bilinear sampling, because a rotation lands between pixel centers and
nearest-pixel quantization would sit an order of magnitude above the symmetry
being measured.

`a_mirrored_fold_reflects_the_frame_across_its_axis` pins the `mirror` parameter,
which `param_reaches_declared_uniform_slot` cannot: a bool that changed *a* pixel
through any use at all would satisfy that test. With `spin = 0` the fold's axis is
the frame's own horizontal, and a 180-row frame puts its center on a row boundary,
so rows `j` and `179 - j` sample points exactly `±d` from it. Mirrored, the frame
must be its own reflection; rotated, it must not be.

`the_only_clocks_kaleido_reads_are_the_three_knobs_that_name_one` is `AGENTS.md`
determinism stated somewhere it can be checked. `time` reaches the picture through
`spin`, `drift`, and `hue_cycle` and nowhere else, so with all three at zero the
same features must render the same frame three seconds apart. A stray `sin(time)`
wobble is invisible to a golden hash, which pins one frame, and invisible to
`param_reaches_declared_uniform_slot`, which never moves `time`. It is also what
forces the grain to be sampled in the *folded* coordinates rather than at the
fragment's own position — per-pixel grain would break the symmetry the preset
exists for, and `a_mirrored_fold_reflects_the_frame_across_its_axis` is what
would notice.

`the_hue_cycles_with_time_under_features_that_do_not_move` is the other half of
that: turn the other two clocks off and the frame must still change, or the hue
was wired to `centroid` alone and the preset is a still picture under a held
chord.

**A preset whose picture is its own history.** `ink` is the only preset whose
state at frame `N` is a hundred rounds of feedback deep, so a golden hash of one
frame says almost nothing: any shader that boils differently every frame would
satisfy it. Five tests carry the risk, and each names a way `ink` could hash green
and be wrong.

`the_loudness_of_the_song_is_the_growth_rate` renders two fields whose features
differ in `rms_env` and nothing else. Under the loud one the drop takes hold and
the reaction runs away; under the quiet one the same drop, fed the same blooms and
stirred the same way, never reaches the density it needs to survive its own
dissolving. `VISION.md` §6 writes one sentence about how `ink` should move, and
this is it. A golden hash would be satisfied by `rms_env` moving the picture *at
all*, and `param_reaches_declared_uniform_slot` never moves a feature.

`the_ink_dissolves_back_to_the_backdrop_when_the_song_stops` is the sharpest
feedback test in the suite, because of the control it renders. Sixty loud frames
grow a dense field, then the music stops. `never_played` is fed *the same silent
features on the same frame* as the frame after the last note, and differs only in
the sixty frames before it, which it spent in silence. A memoryless shader cannot
tell the two apart and must draw them alike; `ink` draws a dense field and clear
water. That is "the picture at frame `N` is a function of the frames before it",
stated so it can fail. The test measures peak density rather than counting dense
pixels: the plateau a blob settles on sits just above any fixed threshold, so a
count falls off a cliff on the first frame of decay while the ink is plainly still
there.

`diffusion_is_the_only_way_the_ink_leaves_the_drop_that_threw_it` turns off the
blooms and the stirring, leaving one drop bounded inside a known radius. A shader
that read only its *own* texel of the previous frame would still grow, still react
to every feature, still dissolve into silence, and still hash green — it would
simply never spread, and `ink` would be a preset about a circle. Only crossing
that radius proves the diffusion stencil reads its neighbours.

`more_reaction_sub_steps_advance_the_reaction_further_in_the_same_frames` pins
`steps` to the growth term rather than to *a* pixel, which is all
`param_reaches_declared_uniform_slot` can see — a `steps` wired to the hue would
pass that. The measure is density, not spread: more sub-steps grow *less* area,
because the reaction is bistable and every extra step also eats the thin ink below
its threshold. It is checked against the shipped `steps = 4` and not the maximum 8
on purpose. Past the point where fronts meet, more steps grow *fewer* dense pixels,
since `crowd` starves a blob's interior and hollows it out — that hollowing is the
reaction-diffusion look, so the honest claim is monotone only up to it.

`ink_is_reproducible_from_its_seed_and_its_frames` is `AGENTS.md` determinism
through the one preset that could plausibly lose it. Two renders of the same frames
must agree exactly, so no wall clock and no unseeded randomness survives a hundred
rounds of feedback; two seeds must disagree, so `--seed` reaches the blooms rather
than being plumbed through every layer and dropped by the last.

`the_feedback_texture_is_transparent_on_the_first_frame` guards the binding rather
than the preset. Its sibling `the_feedback_texture_is_black_on_the_first_frame`
reads only RGB, and the two clear colors it cannot distinguish —
`wgpu::Color::BLACK` and `wgpu::Color::TRANSPARENT` — differ in the one channel
`ink` stores its whole field in. That gap is exactly where the bug in CHANGELOG's
Fixed section lived: an opaque history made every `nebula` render open behind a
sheet of black, and no test in the suite said so.

The Rust side of that boundary is
`globals_layout_matches_wgsl`, which pins every member's byte offset — the
uniform is encoded by hand, field by field, since `avz-core` is
`forbid(unsafe_code)` and cannot transmute a `#[repr(C)]` struct into bytes.
The two tests meet in the middle: `min_binding_size` on the bind-group layout
makes the driver reject a `Globals` whose size disagrees with the WGSL.

**"Pulse reacts to the music" is an assertion, not a vibe.**
`every_feature_pulse_reacts_to_changes_the_frame` turns each driving feature up,
one at a time, and demands the frame change. A uniform field that reaches the
GPU and no pixel — dropped from the shader, or sitting at the wrong offset —
fails there rather than in a render nobody looks at closely. The M2 criterion
that kick, vocals, and cymbals read as *distinct* is still the manual listening
pass; what is automated is that each of them reaches the picture at all.

**Adapter selection.** `--adapter auto|gpu|software` behaves differently
depending on what the host has, so a developer machine with a GPU never walks
the fallback path. `scripts/quality.d/70-gpu-less-host-falls-back-to-lavapipe.sh`
points `VK_DRIVER_FILES` at the lavapipe ICD, which makes any host look GPU-less,
and sets `AVZ_TEST_EXPECT_NO_GPU=1` so the render tests demand the fallback
rather than tolerating either adapter. That turns "needs a GPU-less host" from a
manual check into a local one.

**Integration.** A tiny CC0 test mp3 (about 5 s) lives in the repo at
`assets/fixtures/tone-tagged.mp3`, synthesized by `./scripts/make-test-fixture.sh`
and described in `assets/fixtures/README.md`.

**End to end, through the binary.** `crates/avz-cli/tests/render_e2e.rs` runs
`avz render song.mp3 --sample 2s --adapter software` the way a user would, and
asserts `ffprobe` sees a container with exactly one video stream and one audio
stream, 60 frames at 30 fps, and a two-second duration. It is the only test that
covers the assembled binary rather than a seam of it, so it is deliberately
shallow: pixels belong to `pipeline_render.rs` and bitstreams to
`encode_ffmpeg.rs`, both of which can see things `ffprobe` cannot. The excerpt
renders at the 720p `--sample` default, because no CLI flag reaches
`output.resolution` until the preset system lands (RFC-001 Step 15). CI installs
`ffmpeg` and `mesa-vulkan-drivers` for it; a GPU is never involved.

**The batch loop.** `an_album_batch_renders_every_song_to_its_own_mp4_unattended`
renders three songs from one directory the way `for f in album/*.mp3` does. It is
not a third rendering test: what it pins is the *shape* of a render seen from a
shell — that the default output path is derived from each input rather than
shared, so two tracks do not overwrite each other, that every iteration exits 0,
and that no `.part` survives. A default `--out` that collided would pass every
single-song test in this suite and destroy an album on the first run.

The rest of UT-010 is `scripts/album-acceptance.sh`, which renders a real album at
full resolution and fails on the first intervention. Minutes of encoding is not a
per-commit gate, so it is a release-checklist step (`docs/RELEASE.md`) rather than
a `scripts/quality.d/` hook.

**Proving `-c:a copy`.** An audio codec of `mp3` in the output does *not* prove
the stream was copied: re-encoding an mp3 with `libmp3lame` also reports
`codec_name=mp3`, so a codec assertion passes straight through a generation of
quality loss. The bitstream is what tells the truth.
`muxed_audio_stream_is_copied_not_reencoded` extracts the raw audio packet
payloads from both the source mp3 and the rendered mp4 (`-c copy -f data`) and
asserts the muxed bytes are a byte-exact prefix of the original — a prefix, not
the whole thing, because `-shortest` truncates the audio to the rendered frames.
`scripts/quality.d/80-audio-is-never-reencoded.sh` guards the same invariant in
the source, for hosts where ffmpeg cannot run.

**The ffmpeg subprocess.** A shell stand-in for ffmpeg is what makes the failure
paths testable: a real encoder cannot be made to die on cue, or to hold stdin
open forever. `crates/avz-core/tests/encode_ffmpeg.rs` uses one to prove the
output appears only after a clean exit, that a mid-render death removes the
`.part` file, and that a dropped `Encoder` kills ffmpeg and cleans up. The mux
test in the same file drives the real system ffmpeg.

**The whole pipeline.** `crates/avz-core/tests/pipeline_render.rs` renders the
fixture on lavapipe through the same stand-in, which turns the "mp4" into the raw
RGBA avz actually piped. That is the only vantage point from which *which* frames
were rendered and *how bright* each one was are both observable, so it is where
`--sample` frame selection and the tracer bullet's brightness-follows-loudness
mapping are pinned. Expected brightness is derived from the sRGB transfer
function written out in the test, not read back from the renderer — an
independent implementation is the point, and two bytes of slack covers lavapipe's
rounding.

**`--sample` audio.** The picture starts at a frame boundary, so the audio must
too. `EncodeSettings::audio_start` becomes an ffmpeg `-ss` in front of the mp3
input, which seeks *and still copies*. `a_sampled_render_muxes_the_matching_slice_of_the_original_audio`
proves it by finding the muxed bitstream verbatim inside the original at a
non-zero offset: a re-encode would not appear at all, and a missing seek would
appear at offset zero.

**The progress bars.** `crates/avz-cli/src/progress.rs` turns the `Progress`
callbacks into one of three presentations, and which one it picks is a pure
function of `--quiet` and whether stderr is a terminal — so
`a_bar_is_drawn_only_on_a_terminal_and_never_when_quiet` can pin the decision
without a terminal in sight.

The bar's *template* is the part that fails silently. `indicatif` rejects a
malformed template at build time (a panic on the first frame of the first render,
which `every_template_is_one_indicatif_accepts` moves into a test), but it renders
an **unregistered** key as the empty string and reports nothing.  `{fps}` is ours
rather than `indicatif`'s — its own `{per_sec}` prints `39.1114/s` — so a `style()`
that forgot to bind it would drop the render rate out of every bar and every
assertion about the template *string* would still pass.
`the_rendering_bar_draws_its_frame_count_render_fps_and_eta` draws a real bar into
a `TermLike` that keeps what was written on it, and reads the frame count, the
rate, and the ETA back out of the pixels.

The line-based fallback exists because a bar's carriage returns pile up into a
wall of redraw garbage in a CI log or a pipe.
`a_piped_render_reports_progress_as_lines_rather_than_a_bar` asserts both halves:
that the deciles are there, and that no `\r` reached the pipe.
`the_line_fallback_reports_once_per_decile_of_progress` keeps a 9000-frame render
from writing 9000 lines, and `the_line_fallback_always_ends_at_a_hundred_percent`
pins the arithmetic at frame counts where `done * 100 / total` skips a decile.

`--verbose`'s log lines and the bars share one stderr. `LogWriter` suspends the
draw before letting a record through; without it they overwrite each other, which
is a thing no assertion sees and every user does. That one is verified by hand
against a pty (below).

**Warnings are named, and their shape is checked twice.**
`every_pipeline_warning_names_a_consequence_and_an_action` holds every warning
`avz-core` can emit to the `AGENTS.md` shape: an em dash separating what happened
from what to do, and a backticked flag or key in the second half. But a test can
only assert about warnings someone remembered to add to its list, so
`scripts/quality.d/97-warnings-are-actionable.sh` closes the gap from the other
side: a `warn()` call must pass a *named* warning — a `*_WARNING` const or a
`*_warning()` function — never an inline string, and every such name must carry
both halves. A new warning therefore cannot reach a user without passing through
the test that enumerates them.

The em dash is not decoration. `upscale_warning` names the background image in
backticks too, so "contains a backtick" would pass on a warning that quoted the
path and offered no way out; the hook checks the half *after* the dash.

**The exit-code contract.** `crates/avz-cli/tests/exit_codes.rs` is `VISION.md`
§8 as a test matrix, driven through the assembled binary the way a shell drives
it. It exists because `for f in album/*.mp3; do avz render "$f" || break; done` is
the batch story avz ships instead of a `batch` subcommand, and it can only tell
"this song has no tags" (3) from "the disk is full" (4) from "my `--config` path
is wrong" (2) by the number. Each row also asserts the message names the thing the
user handed avz and never leaks the errno: `os error 2` tells nobody which file.

Writing that matrix found two bugs. A missing `--config` file exited 3 and printed
`No such file or directory (os error 2)`; it is the user's *argument*, so it is
now an `Error::Config` (exit 2) whose message is a sentence — exit 3 belongs to
the song, and a batch loop has to tell "skip this one" from "every one will fail".

**Manual: the bars against a real terminal.** Three things have no assertion,
because they are about what a terminal *looks* like: that the bar redraws in
place rather than scrolling, that `--verbose`'s log lines land above it rather
than through it, and that the ETA and fps are legible while they move. Verified
by running a full 1080p software render under a pty:

```bash
cargo build --release -p avz-cli
python3 -c 'import pty,sys; pty.spawn(["./target/release/avz","render","song.mp3","--adapter","software"])'
python3 -c 'import pty,sys; pty.spawn(["./target/release/avz","render","song.mp3","--adapter","software","--verbose"])'
```

Expect `rendering  [========>     ] 81/150 frames  44.8 fps  eta 2s` redrawing in
place, a spinner on either side of it, and — under `--verbose` — debug records on
their own lines with no bar text glued to them. `--quiet` must write nothing at
all to the terminal.

**Manual listening pass.** The M2 reference-track ritual: render 3–4 reference
tracks (something quiet, something dense, a Cold Design track, a Carpathians
track) and confirm onsets read as on-beat rather than late. Repeat before each
release. This cannot be automated — "feels musical" has no assertion.

## Regression Rule

Every fixed bug should leave behind a test that fails without the fix.

Every new user-facing behavior should have a test that would fail if the behavior
disappeared.

## Risk Matrix

Maintain this as the architecture settles. `Coverage` is a test name once one
exists, or `TODO` / `manual` with a reason.

| Risk / failure mode | User impact | Test layer | Coverage |
|---|---|---|---|
| Stereo mixdown takes one channel or sums instead of averaging | Analysis sees the wrong loudness; hard-panned material drives the visuals twice as hard | Unit | `stereo_downmix_is_channel_average`, `channel_average_is_exact_for_interleaved_frames`, `mono_source_passes_through_unmixed` |
| Decoded sample count disagrees with the reported duration | Cumulative audio/visual drift over a long song | Unit | `decodes_fixture_to_expected_duration`, `mono_output_length_matches_duration_times_rate`, `a_stereo_source_decodes_to_one_channel` |
| Truncated or corrupt mp3 panics, or is silently analyzed short | Crash, or visuals rendered against audio that keeps playing | Unit | `truncated_mp3_yields_input_error_not_panic`, `non_mp3_bytes_rejected` |
| Decode resamples, or grows beyond mp3 | Hop math drifts off video frame timestamps; the binary carries codecs avz can never mux | Unit + quality hook | `a_stereo_source_decodes_to_one_channel`, `scripts/quality.d/40-decode-stays-mp3-only.sh` |
| Band energies map to wrong frequency ranges | Visuals react to the wrong instruments; subtly off, never obviously broken | Unit | `sine_at_60hz_lights_up_bass_band_only`, `sine_at_1khz_dominates_mid`, `sine_at_12khz_dominates_air`, `two_tone_signal_lights_both_bands`, `a_bin_belongs_to_the_band_holding_its_center_frequency`, `dc_and_ultrasound_belong_to_no_band` |
| Spectral flux counts energy leaving as well as arriving | Every note *ending* reads as a hit; onsets fire between the beats | Unit | `flux_is_half_wave_rectified`, `steady_tone_has_near_zero_flux`, `tone_switch_spikes_flux_at_switch_frame` |
| Spectral centroid divides by a silent spectrum | A `NaN` reaches a shader uniform and the frame paints black | Unit | `silence_centroid_is_zero_not_nan`, `the_centroid_of_a_silent_spectrum_is_zero_not_nan`, `a_single_bin_spectrum_has_no_centroid_rather_than_an_infinity` |
| Edge analysis windows are zero-padded | The song reads ~3 dB quiet on its first frame and then "grows" into the second — an onset the music never played | Unit | `the_first_and_last_frames_read_as_loud_as_the_middle_of_the_song`, `a_song_shorter_than_one_window_still_analyzes` |
| Parallel window analysis reduces in thread-completion order | A re-render of the same song is a different video | Unit | `the_same_song_analyzes_to_the_same_timeline_twice` |
| FFT magnitudes scale with the window length | Band and onset thresholds silently become a function of `fps` | Unit | `a_full_scale_sine_reads_unit_amplitude_at_its_bin`, `the_hann_window_is_periodic_and_sums_to_half_its_length` |
| Onset detection fires late or misses hits | Motion lags the beat — the core promise fails | Unit + manual | `click_train_produces_onsets_at_expected_frames`, `a_spike_above_the_local_median_and_mad_is_a_hit`, `a_hit_lands_on_the_peak_not_the_rising_edge`, `the_fixtures_kicks_are_the_only_onsets`; manual listening pass |
| Onset threshold is global, so a chorus sets the bar a quiet verse can never clear | Half the song never onsets — the visuals go dead exactly where the music got intimate | Unit | `quiet_section_clicks_still_detected` (which also asserts a global threshold *would* miss them), `the_threshold_follows_the_passage_it_sits_in` |
| A held chord or digital silence onsets on the FFT's own noise, because its MAD is ~0 | The visuals strobe through a sustained note or a silent intro | Unit | `steady_tone_produces_no_onsets`, `silence_produces_no_onsets`, `a_flat_flux_track_has_no_onsets`, `a_bump_below_the_noise_floor_is_not_a_hit` |
| One physical hit fires two onsets, one per analysis window that overlaps it | Every kick double-flashes | Unit | `refractory_period_merges_double_triggers`, `two_clicks_inside_the_refractory_period_are_one_onset`, `hits_a_refractory_period_apart_are_both_kept` |
| The onset impulse's decay is a per-frame factor rather than a time constant | A hit fades over twice as long at 60 fps as at 30 | Unit | `onset_impulse_decays_exponentially` (asserted in time, at both fps), `the_onset_impulse_decays_from_each_hit`, `every_hit_restarts_the_impulse_at_one` |
| The threshold window is zero-padded at the song's edges | The opening chord reads as an onset; the last second stops detecting | Unit | `the_threshold_window_clamps_at_the_song_edges`, `first_and_last_second_do_not_panic` |
| Envelope follower attack/decay math wrong | Motion is twitchy or sluggish | Unit + manual | `step_input_env_reaches_90pct_within_attack_budget`, `release_tail_matches_decay_time_constant`, `the_envelope_rises_faster_than_it_falls`, `the_envelope_holds_a_feature_after_it_has_fallen_away`; manual listening pass |
| The envelope's attack or decay is a per-frame factor rather than a time constant | A hit swells and fades over twice as long at 60 fps as at 30 | Unit | `step_input_env_reaches_90pct_within_attack_budget`, `release_tail_matches_decay_time_constant` (both asserted in time, across three frame rates) |
| The envelope overshoots the feature it follows | A normalized feature drives a uniform past 1.0 and the shader clips | Property | `env_never_exceeds_input_peak_or_drops_below_zero` |
| `visual.smoothing` reaches nothing | The one global tuning knob VISION §5.5 promises silently does nothing | Unit | `smoothing_config_scales_decay`, `a_larger_smoothing_holds_every_envelope_longer`, `the_default_smoothing_yields_the_default_decay` |
| Normalization divides by zero on silence | Panic or NaN frames | Unit | `silence_normalizes_without_nan`, `constant_signal_normalizes_to_zeros_not_nan`, `silence_has_no_nans_in_any_feature`, `every_feature_of_the_fixture_lands_in_the_unit_interval` |
| Normalization is computed per `--sample` excerpt rather than per song | The same second looks different in the preview and in the full render | Unit | `a_quiet_master_and_a_loud_one_analyze_to_the_same_timeline`; analysis never sees the sample range, and `pipeline::render` normalizes before `frame_range` |
| The global pass rescales the onset impulse | A hit stops reading exactly 1.0 and `is_onset` finds nothing | Unit | `the_onset_impulse_passes_through_the_global_normalization_unchanged` |
| Analysis frames do not land on video frame timestamps | Cumulative audio/visual drift over a long song | Unit | `analysis_frames_never_drift_from_the_video_frame_clock`, `a_burst_lands_on_the_video_frame_nearest_it`, `one_feature_frame_per_video_frame`, `a_partial_final_video_frame_still_gets_a_feature_frame` |
| Analysis windows leave gaps between hops at low fps | A hit landing in a gap never reaches the visuals | Unit | `no_audio_falls_between_windows_when_the_hop_exceeds_the_window` |
| RMS is wrong in a way that still looks plausible | Brightness follows nothing in particular | Unit | `a_constant_sine_has_the_same_rms_on_every_frame`, `a_dc_signal_has_an_rms_equal_to_its_amplitude`, `a_loud_passage_reads_louder_than_a_quiet_one`, `silence_has_zero_rms_and_no_nans` |
| wgpu readback row padding mishandled (256-byte alignment) | Skewed or garbage frames | Unit + integration + quality hook | `readback_handles_non_multiple_of_256_row_widths` (both layers), `a_row_stride_rounds_up_to_the_256_byte_alignment`, `an_already_aligned_row_is_not_padded`, `scripts/quality.d/50-readback-padding-lives-in-one-place.sh` |
| Readback buffer size and row layout silently disagree | A sheared frame nobody notices until the video is watched | Unit | `a_buffer_that_is_not_the_padded_size_is_a_render_error` |
| `--adapter gpu` quietly renders on lavapipe | An 8 fps render the user explicitly ruled out | Unit + integration | `gpu_refuses_a_software_adapter_and_software_refuses_a_hardware_one`, `asking_for_gpu_never_yields_a_software_adapter` |
| `--adapter software` quietly renders on the GPU | Golden frames pass locally and fail everywhere else | Unit + integration | `gpu_refuses_a_software_adapter_and_software_refuses_a_hardware_one`, `the_software_adapter_is_a_cpu_adapter_and_needs_no_warning` |
| Software fallback happens without a warning, or warns when asked for | The user cannot tell a slow render from a broken one, or is nagged every render | Unit + integration + quality hook | `only_an_auto_render_that_lands_on_software_is_worth_warning_about`, `auto_always_finds_an_adapter_and_flags_a_software_fallback`, `a_gpu_less_host_falls_back_to_software_and_says_so`, `scripts/quality.d/70-gpu-less-host-falls-back-to-lavapipe.sh` |
| A second render backend creeps in (dx12/metal/gles) | Shaders run on an untested path; golden frames stop meaning anything | Quality hook | `scripts/quality.d/60-render-is-vulkan-only.sh` |
| Shader regression changes output silently | Presets drift between releases | Golden frames (software adapter) | `every_preset_renders_its_golden_frames`, `a_loud_frame_and_a_quiet_one_are_different_pictures` |
| The layer blend is not premultiplied | Half-transparent layers composite at half strength; every background is wrong and nothing errors | Integration (pixels) | `premultiply_blend_math_matches_reference`, `layers_composite_bottom_to_top` |
| A preset returns a hardcoded opaque alpha | Its layer covers the background layer entirely; `--bg` and the palette backdrop are decoration | Integration (pixels) | `every_preset_draws_a_layer_the_backdrop_shows_through`, `visualizer_alpha_zero_shows_background_exactly` |
| An absent layer is drawn as a black quad | The bottom of the stack is black rather than empty, and `absent_layers_skip_render_passes` is the only place it shows | Integration (pixels) | `absent_layers_skip_render_passes` |
| Nondeterminism leaks in (wall clock, unseeded RNG) | Re-render does not reproduce; golden tests flake | Golden frames + quality hook | `same_inputs_same_hash_twice`, `scripts/quality.d/90-animation-time-comes-from-the-frame-index.sh` |
| The `Globals` uniform drifts between the Rust struct and the WGSL | Every preset silently reads the wrong feature at that offset, and nothing crashes | Unit + integration | `globals_layout_matches_wgsl`, `the_palette_and_param_arrays_sit_on_sixteen_byte_boundaries`, `every_preset_declares_the_whole_globals_contract`, `every_feature_pulse_reacts_to_changes_the_frame` |
| A preset ignores a feature it claims to be driven by | The kick, or the cymbals, drive nothing; the video looks alive and reacts to half the song | Integration (pixels) | `every_feature_pulse_reacts_to_changes_the_frame` |
| The coarse spectrum is bucketed linearly, or off by a factor of two | The whole kick lands in two texels and `ribbons` reads hiss as loud as a snare — plausible, and wrong at every frequency | Unit | `a_tone_lights_the_coarse_bucket_that_holds_its_frequency`, `the_buckets_are_log_spaced_across_the_declared_range`, `a_bucket_averages_its_bins_rather_than_summing_them`, `a_bucket_narrower_than_one_fft_bin_reads_the_bin_nearest_it` |
| The spectrum texture is uploaded once, or a row short, or byte-swapped | A preset draws a still ribbon over a moving song, or reads off the end of its own texture | Unit + integration (pixels) | `every_video_frame_carries_a_512_bucket_spectrum`, `a_hot_bucket_lights_the_column_that_reads_it_and_no_other`, `the_texture_carries_the_spectrum_of_the_frame_being_drawn` |
| A preset reaches for an optional binding it never declared, or declares one it never reads | A shader samples a texture nobody bound, or every frame pays for an upload that shows nothing | Unit + integration | `a_preset_that_does_not_ask_for_the_spectrum_gets_no_binding`, `a_preset_asks_for_the_spectrum_texture_exactly_when_its_shader_samples_it`, `a_preset_may_ask_for_the_spectrum_and_the_previous_frame_together`, `a_preset_that_does_not_ask_for_the_onsets_gets_no_binding`, `a_preset_asks_for_the_onset_history_exactly_when_its_shader_samples_it`, `a_preset_may_ask_for_the_onsets_and_the_previous_frame_together` |
| `ribbons` stops reading the spectrum, or reads its frequency axis backwards | The golden hashes are blessed over a preset that draws flat lines and ignores the music | Integration (pixels) | `ribbons_draws_its_light_where_the_spectrum_has_energy`, `ribbons_draws_nothing_where_the_spectrum_is_silent` |
| The onset history is uploaded once, byte-swapped, or with its channels crossed | A burst preset re-derives every particle from the wrong hit, and the bursts land off the beat | Unit + integration (pixels) | `the_onset_history_holds_the_recent_hits_newest_first`, `every_slot_reaches_the_texel_that_reads_it_with_both_channels_intact`, `the_texture_carries_the_history_of_the_frame_being_drawn` |
| An unfilled onset slot reads as a hit at time zero | Every render opens with a burst the song never played | Unit + integration (pixels) | `a_song_with_no_hits_yet_hands_back_an_empty_history`, `an_unfilled_slot_reads_the_sentinel_and_not_a_hit_at_time_zero`, `particles_draws_nothing_without_a_live_hit` |
| A burst is hashed from its sliding slot rather than from the hit that threw it | Every live particle tears across the frame on every kick; both frames still hash, so no golden test disagrees | Integration (pixels) | `a_burst_is_hashed_from_the_hit_that_threw_it_and_not_from_its_slot` |
| `particles`' burst cull is narrower than a particle's glow | The cull stops being an optimization and becomes the shape: bursts get a hard edge and a hollow middle, and the golden hashes bless it | Integration (pixels) | `the_burst_cull_takes_no_light_off_the_burst_it_skips`, `a_burst_lights_a_disc_on_the_frame_it_is_thrown` |
| A preset carries state between draws | Frame `N` depends on the driver's rounding on frames `0..N`; `--sample` renders different pixels than a full render | Integration (pixels) | `particles_renders_a_frame_the_same_whether_or_not_the_frames_before_it_were_drawn` |
| `kaleido` stops folding, or folds into a different number of wedges than its schema declares | The golden hashes bless a preset that draws petals and rings with no symmetry in them, and `segments` is decoration | Integration (pixels) | `kaleido_folds_the_frame_into_the_wedges_it_declares`, `a_mirrored_fold_reflects_the_frame_across_its_axis` |
| A shader reads the clock somewhere other than the knobs that name one | A golden hash pins one frame and cannot see it; the render still looks right and stops being reproducible from its config | Integration (pixels) | `the_only_clocks_kaleido_reads_are_the_three_knobs_that_name_one`, `the_hue_cycles_with_time_under_features_that_do_not_move` |
| The feedback texture is cleared to an opaque color | The history is a premultiplied layer with zero coverage; an opaque clear hides the backdrop behind black for the first frames of every feedback render, and saturates a preset that stores its state in alpha | Integration (pixels) | `the_feedback_texture_is_transparent_on_the_first_frame` |
| `ink` recomputes its field from the current frame's features instead of growing it from the previous frame | A memoryless boil that reacts to every feature and hashes green; the reaction-diffusion is decoration | Integration (pixels) | `the_ink_dissolves_back_to_the_backdrop_when_the_song_stops` |
| `ink`'s diffusion stencil reads only its own texel | The field grows and dissolves correctly but never spreads; every golden hash still passes and the preset is a circle | Integration (pixels) | `diffusion_is_the_only_way_the_ink_leaves_the_drop_that_threw_it` |
| `ink`'s `steps` is wired to something other than the reaction | `param_reaches_declared_uniform_slot` passes on a `steps` that changes the hue; the knob that buys the reaction-diffusion look buys nothing | Integration (pixels) | `more_reaction_sub_steps_advance_the_reaction_further_in_the_same_frames` |
| `rms_env` stops being `ink`'s growth rate | The one sentence VISION §6 writes about how `ink` moves; any feature moving the picture satisfies a golden hash | Integration (pixels) | `the_loudness_of_the_song_is_the_growth_rate` |
| A hundred rounds of feedback let a wall clock or an unseeded hash into the field | `--seed` stops reproducing a render, and only `ink` is deep enough for it to show | Integration (pixels) | `ink_is_reproducible_from_its_seed_and_its_frames` |
| The spectrogram's global normalization divides by a silent song's zero spread | A `NaN` reaches the texture and every ribbon paints black | Unit | `a_silent_song_has_a_zero_spectrum_and_no_nans`, `the_spectrum_is_normalized_into_the_unit_interval` |
| `--seed` never reaches the shader's noise | Two seeds render the same video; `--seed` is decoration | Integration (pixels) | `different_seed_different_hash` |
| A golden test renders on a hardware adapter | The committed hashes are a hash of one laptop; the test fails everywhere else for reasons that look like shader bugs | Quality hook | `scripts/quality.d/95-golden-frames-run-on-the-software-adapter.sh` |
| A preset ships with no golden hashes | The one thing that catches shader drift does not cover the new preset | Quality hook | `scripts/quality.d/95-golden-frames-run-on-the-software-adapter.sh` |
| `--sample` previews pixels the full render will not draw | The preview is a different video from the one that ships; the preset's clock is the excerpt's, not the song's | Integration (pixels) | `a_sampled_render_writes_exactly_the_frames_of_the_requested_range` |
| A typo'd `visual.preset` renders something, or fails late | A five-minute decode before a one-word error | Unit + integration | `an_unknown_preset_is_a_config_error_that_names_the_known_ones`, `an_unknown_preset_fails_before_the_song_is_even_decoded`, `render_with_an_unknown_preset_exits_2_and_names_the_known_ones` |
| `--preset` parses and never reaches the config layer | Every render draws `pulse`; the flag VISION §3 opens with is decoration | Unit + CLI | `the_preset_flag_reaches_the_cli_config_layer`, `preset_names_the_visualizer_to_render`, `render_with_an_unknown_preset_exits_2_and_names_the_known_ones` (which a silently dropped flag turns green→red by *succeeding*) |
| Two songs in a directory render to one output path | A batch loop overwrites every track with the last one, silently | Integration (through the binary) | `an_album_batch_renders_every_song_to_its_own_mp4_unattended` |
| A batch loop needs a human: a prompt, a retry, a leftover `.part` | The v0.1 acceptance criterion (VISION §9 M5) fails, and only on a real album | Integration + release script | `an_album_batch_renders_every_song_to_its_own_mp4_unattended`; `scripts/album-acceptance.sh` per release |
| A preset's `perf_hint` promises a speedup it does not deliver | The one tuning advice avz gives for software rendering sends the user to the wrong knob | Manual (measured per release) | manual — `docs/RELEASE.md`; the v0.1 measurement is recorded in RFC-001 Step 23 |
| Rendering starts before analysis has finished | No lookahead, no global normalization — the two-pass design silently becomes one pass | Integration | `progress_reports_the_three_phases_in_order_with_a_frame_total` |
| Visuals do not react to the audio at all | The one thing avz exists for, and a static video still looks like a successful render | Integration (pixels) | `the_rendered_brightness_visibly_follows_the_loudness_of_the_song` |
| The assembled binary writes an mp4 no player will open: a stream missing, a frame short, the wrong length | Every seam passes its own tests and the one artifact avz exists to produce is broken | Integration in CI (ffprobe, through the binary) | `a_two_second_software_render_is_a_playable_mp4_with_one_video_and_one_audio_stream` |
| `--sample` renders the wrong frames | The picture runs against the wrong second of the song, for the whole excerpt | Unit + integration | `a_sample_range_selects_the_frames_that_cover_it`, `a_sample_boundary_lands_on_the_frame_whose_timestamp_it_names`, `a_sampled_render_writes_exactly_the_frames_of_the_requested_range` |
| `--sample` picture and muxed audio start at different instants | Sound sits a fraction of a second off the visuals for the whole excerpt | Unit + integration (bitstream compare) | `the_audio_starts_at_the_first_rendered_frames_timestamp`, `a_sampled_render_seeks_the_audio_input_and_still_copies_the_stream`, `a_sampled_render_muxes_the_matching_slice_of_the_original_audio` |
| A sample the song cannot satisfy reaches ffmpeg as an empty video | A cryptic encoder failure, or a zero-frame mp4 | Unit + integration | `a_sample_that_starts_after_the_song_ends_is_a_config_error`, `a_sample_shorter_than_one_frame_is_a_config_error`, `render_of_a_sample_past_the_end_of_the_song_exits_2` |
| `--sample` renders at full resolution | "Fast iteration" costs as much as a full render | Unit + integration | `a_sample_render_defaults_to_a_reduced_resolution`, `an_explicit_resolution_beats_the_sample_default`, `render_writes_a_sampled_mp4_next_to_the_input` |
| `--out` points back at the input | The song is destroyed by its own render, after ffmpeg has read it | Unit + integration | `an_output_that_is_the_input_is_refused_however_it_is_spelled`, `render_refuses_to_write_over_its_own_input` |
| ffmpeg missing at runtime | Tool fails late with a cryptic error | Integration (preflight) | `missing_ffmpeg_fails_with_the_fedora_install_hint`, `render_without_ffmpeg_fails_with_the_fedora_install_hint`, `render_checks_for_ffmpeg_before_doing_any_work` |
| `ffmpeg` on PATH is not really ffmpeg | Cryptic subprocess failure mid-render | Integration (preflight) | `a_binary_that_is_not_ffmpeg_is_rejected`, `an_ffmpeg_that_exits_nonzero_is_rejected` |
| ffmpeg dies mid-render, or the finished file cannot be moved into place | Half-written `.mp4` left on disk | Integration | `ffmpeg_death_midrender_leaves_no_output_file`, `a_dropped_encoder_kills_ffmpeg_and_removes_the_part_file`, `a_render_that_cannot_be_moved_into_place_leaves_no_part_file`, `the_output_appears_only_after_a_successful_finish` |
| ffmpeg's stderr pipe fills while avz waits to write a frame | Render deadlocks with no diagnostic | Integration | `ffmpeg_death_midrender_leaves_no_output_file` (surfaces the drained stderr) |
| Audio re-encoded instead of `-c:a copy` | Generational quality loss, silently | Integration (bitstream compare) + unit + quality hook | `muxed_audio_stream_is_copied_not_reencoded`, `the_audio_stream_is_copied_and_never_reencoded`, `scripts/quality.d/80-audio-is-never-reencoded.sh` |
| `background.image` reaches the config and not the frame | `--bg` is decoration; every render draws the palette gradient | Integration (pixels) | `a_background_image_reaches_the_rendered_frames`, `the_bg_flag_reaches_the_cli_config_layer` |
| A background image is blurred or darkened in sRGB rather than in light | Every background is a stop and a half too dark, and nothing errors | Unit | `darken_dims_the_light_rather_than_the_encoded_byte`, `a_blur_averages_light_rather_than_encoded_bytes`, `darken_of_one_leaves_black`, `darkening_the_background_dims_the_rendered_frames` |
| `fit = "cover"` scales instead of cropping | A 1×1000 source needs a 1920× enlargement; the render dies allocating the intermediate | Unit | `cover_crops_the_overhanging_axis_and_centers_what_is_left`, `a_sliver_of_an_image_still_occupies_at_least_one_pixel` |
| A fit mode distorts, crops, or letterboxes where it should not | The user's artwork is silently the wrong shape in every video | Unit | `a_stretched_image_covers_every_pixel_of_the_frame`, `a_covering_image_leaves_no_backdrop_showing`, `a_contained_image_letterboxes_onto_the_palette_backdrop`, `an_image_shaped_like_the_frame_is_neither_cropped_nor_letterboxed`, `contain_fits_the_binding_axis_and_centers_the_rest` |
| A `contain` letterbox or a transparent PNG shows black rather than the backdrop | The palette stops reaching the frame the moment `--bg` is passed | Unit | `a_contained_image_letterboxes_onto_the_palette_backdrop`, `a_transparent_image_lets_the_backdrop_through`, `a_half_transparent_image_blends_with_the_backdrop` |
| The blur's edge handling pulls black in from outside the frame | Every blurred background is vignetted | Unit | `a_blur_of_a_flat_field_darkens_no_edge`, `a_blur_of_zero_leaves_the_image_untouched`, `a_blur_spreads_light_beyond_the_shape_that_emitted_it` |
| A missing or corrupt `--bg` fails after the song is decoded, or not at all | A five-minute decode before a one-line error | Unit + integration + CLI | `a_missing_background_image_is_an_input_error_naming_the_path`, `a_file_that_is_not_an_image_is_an_input_error`, `a_missing_background_image_fails_before_the_song_is_even_decoded`, `render_with_a_missing_background_image_exits_3_and_names_the_path` |
| `background.video` reaches the config and not the frame | The layer the user asked for is missing from every video | Integration (pixels) | `a_background_video_reaches_the_rendered_frames` |
| The background loop freezes, runs ahead, or draws out of order | A still or stuttering loop that every static-picture assertion blesses | Integration (pixels) + integration (real decode) | `each_rendered_frame_draws_the_next_frame_of_the_loop`, `a_one_second_loop_repeats_frame_for_frame_under_a_longer_render` |
| A slower background video starves the render | The pipeline crawls behind a decoder that has nothing to say | Integration (real decode) | `a_slower_source_is_frame_rate_converted_rather_than_starving_the_render` |
| A `contain` background video letterboxes onto black rather than the backdrop | The palette stops reaching the frame the moment `background.video` is set | Integration (real decode) | `a_contained_video_letterboxes_with_transparent_bars` |
| A background video is blurred or darkened in sRGB rather than in light | Every loop is a stop and a half too dark, and nothing errors | Unit | `darken_dims_the_light_rather_than_the_encoded_byte`, `a_blur_averages_light_rather_than_encoded_bytes`, `a_blurred_frame_is_darkened_in_light_too` (all in `render/video.rs`) |
| The background video's audio is decoded, or reaches the mux | The song is not the only sound in the video | Unit + quality hook | `the_background_videos_audio_is_never_decoded`, `scripts/quality.d/43-background-video-reader-is-bounded-and-times-out.sh` |
| A missing `background.video` fails after the song is decoded, or as exit 2 | A five-minute decode before a one-line error; a batch loop cannot tell a bad config from a missing file | Unit + integration + CLI | `a_missing_background_video_is_an_input_error_naming_the_path`, `a_missing_background_video_fails_before_the_song_is_even_decoded`, `a_missing_background_video_exits_3_and_names_the_path` |
| `image` grows past png/jpg | A dozen untrusted-input parsers in the binary, half-supporting formats `--bg` documents away | Quality hook | `scripts/quality.d/41-background-images-stay-png-and-jpeg.sh` |
| Background-video decode thread stalls or deadlocks | Render hangs with no diagnostic | Unit (fake decoder) + quality hook | `a_decoder_that_stops_producing_frames_times_out_and_names_the_video`, `a_decoder_that_exits_ends_the_render_with_ffmpegs_own_complaint`, `a_dropped_background_video_kills_ffmpeg_and_joins_its_threads`, `scripts/quality.d/43-background-video-reader-is-bounded-and-times-out.sh` |
| The background video's frame queue is unbounded | A decoder that outruns lavapipe reads the whole loop into memory, on the host chosen for having no GPU | Quality hook | `scripts/quality.d/43-background-video-reader-is-bounded-and-times-out.sh` |
| A preset schema declares a parameter the shader never reads | The knob does nothing; `avz presets` documents a lie | Unit + golden frames | `every_schema_parameter_is_read_by_the_shader_that_declares_it`, `param_reaches_declared_uniform_slot` |
| Two schema parameters claim one uniform component | The second silently overwrites the first; one knob does nothing | Unit | `two_parameters_may_not_claim_the_same_uniform_component`, `a_color_cannot_start_partway_through_its_slot`, `a_slot_beyond_the_uniform_is_rejected` |
| A schema default drifts from the constant the shader used before it | Every golden hash is rewritten by a change that looks like a refactor | Golden frames | `param_reaches_declared_uniform_slot` (the defaults must render the committed hashes) |
| A schema default sits outside its own declared range | `avz presets` prints a range the default violates; every default render is illegal | Unit (meta over every shipped schema) | `schema_defaults_all_within_declared_ranges`, `a_schema_whose_default_is_outside_its_own_range_is_rejected` |
| A preset parameter reaches the config but not the uniform | Every `[visual.params]` value silently does nothing | Integration (pixels) | `a_preset_parameter_from_the_config_reaches_the_rendered_pixels` |
| An unknown or out-of-range parameter is caught after analysis, or not at all | A five-minute decode before a one-word error, or a shader clamping in silence | Unit + integration + CLI | `unknown_param_rejected_with_suggestion`, `out_of_range_value_names_the_allowed_range`, `an_out_of_range_parameter_fails_before_the_song_is_decoded`, `an_unknown_parameter_fails_before_the_song_is_decoded_and_suggests_a_name`, `out_of_range_value_fails_exit_2_before_render` |
| A `--set` shorthand swallows a mistyped config section | `outputt.fps=30` is reported as an unknown preset *parameter*, pointing at the wrong mistake | Unit | `a_set_key_under_an_unknown_section_names_the_sections_and_the_presets`, `a_bare_set_key_is_a_parameter_of_the_active_preset`, `a_preset_prefixed_set_key_is_a_parameter_of_that_preset`, `the_shorthand_and_the_long_form_set_the_same_parameter` |
| An int parameter silently accepts a float, or a bool a string | `ring_count = 4.5` renders 4 rings nobody asked for | Unit | `an_int_parameter_rejects_a_float`, `a_bool_parameter_rejects_the_string_true`, `a_float_parameter_accepts_a_bare_integer` |
| A `color` parameter reaches the shader in sRGB rather than linear | The tint is a stop and a half off, exactly like a mis-linearized palette | Unit | `a_color_parameter_is_linearized_across_its_whole_slot` |
| The palette reaches the shader in sRGB rather than linear | Every palette washes out by a stop and a half, in every preset at once | Unit | `srgb_to_linear_round_trip_within_epsilon`, `named_palette_resolves_to_five_linear_colors`, `the_resolved_palette_reaches_the_shader_unaltered` |
| A built-in palette's colors drift | Every video anyone rendered under that name is silently rewritten | Golden frames | `every_built_in_palette_renders_a_distinct_stable_frame` |
| Two built-in palettes render one picture | `--palette` offers five choices and delivers fewer | Golden frames | `every_built_in_palette_renders_a_distinct_stable_frame`, `no_two_built_ins_resolve_to_the_same_colors` |
| `--palette` resolves but never reaches a pixel | The flag is decoration; every render draws the default palette | Unit + golden frames | `the_palette_flag_reaches_the_cli_config_layer`, `an_inline_palette_reaches_the_pixels` |
| An inline palette is resampled by a blend that muddies the middle slots | A two-color palette's midpoint is olive; resampling is worse than not offering it | Unit | `inline_two_colors_interpolate_to_five`, `oklab_round_trips_through_linear_rgb`, `a_resampled_palette_never_leaves_the_gamut` |
| A typo'd `--palette` renders something, or fails after the decode | A five-minute wait for a one-word error | Unit + integration + CLI | `unknown_palette_name_lists_valid_names`, `an_unknown_palette_fails_before_the_song_is_even_decoded`, `render_with_an_unknown_palette_exits_2_and_names_the_known_ones` |
| A malformed inline hex color is rejected without saying which one | The user counts commas to find the typo | Unit + CLI | `bad_hex_rejected_with_position`, `render_with_a_malformed_inline_palette_exits_2_and_names_the_entry` |
| Adding a preset requires touching code outside `presets/` | The abstraction VISION §6 promises is wrong, and the four deferred presets get expensive | Quality hook | `scripts/quality.d/96-a-preset-is-only-files-in-presets.sh` |
| `avz presets` omits a preset, a column, or a `perf_hint` | UT-004 discovery fails; a parameter exists that nobody can find | Unit (CLI formatter) + CLI | `the_listing_names_every_preset_and_describes_it`, `the_schema_print_shows_every_column_for_every_type`, `the_schema_columns_are_aligned`, `a_perf_hint_is_printed_when_the_schema_carries_one`, `presets_command_lists_all_registered`, `presets_name_prints_schema_fields` |
| `--config` or `--set` never reaches the pipeline | The reproducible-render promise of UT-007 is decoration | CLI | `a_config_files_preset_params_are_validated_against_the_schema`, `a_set_override_beats_an_illegal_value_in_the_config_file`, `unknown_param_via_set_exits_2_with_a_suggestion` |
| Config precedence wrong (`--set` loses to file) | Reproducible renders are not reproducible | Unit | `set_override_beats_config_file_value`, `cli_flag_beats_set_override`, `a_silent_layer_does_not_erase_a_lower_one` |
| Unknown TOML key silently ignored | Typo'd param silently does nothing | Unit | `unknown_toml_key_rejected_with_suggestion`, `unknown_set_key_is_rejected_with_a_suggestion_and_the_assignment` |
| Missing ID3 tags | Crash instead of a warned-and-skipped text card | Unit + integration | `untagged_mp3_reports_missing_tags_instead_of_failing`, `blank_and_whitespace_tag_values_count_as_missing`, `missing_tags_render_as_missing_and_missing_art_as_none`, `missing_tags_warns_and_skips_card` |
| `--title`/`--artist` lose to the ID3 tags they override | The one escape hatch for a badly tagged file does nothing | Unit + integration | `overrides_beat_id3_values`, `the_text_flags_reach_the_cli_config_layer`, `overrides_put_a_card_on_an_untagged_song_without_a_warning` |
| The text card resolves, rasterizes, and never reaches the compositor | `[text]` is decoration; no video ever carries a card | Integration + golden | `the_text_card_from_id3_reaches_the_rendered_frames`, `the_text_card_renders_its_golden_frames` |
| The card ignores its opacity envelope | The type is burned over every frame of the song, or over none | Unit + golden | `opacity_envelope_matches_in_hold_fade_windows`, `the_text_card_is_invisible_before_it_fades_in` |
| The card is set in a host font | The same render draws different glyphs on two machines | Quality hook | `scripts/quality.d/42-text-rasterizes-from-the-bundled-font.sh`, `the_same_card_rasterizes_to_the_same_bytes_twice` |
| Unreadable input reported as a cryptic OS error | User cannot tell a bad file from a bad disk | Unit + integration | `a_file_that_lies_about_being_an_mp3_is_an_input_error`, `a_file_of_an_unknown_format_is_an_input_error`, `probe_of_a_missing_file_exits_3` |
| Cover art picked nondeterministically from a multi-picture tag | Art-reactive presets drift between runs | Unit | `front_cover_wins_over_other_pictures_regardless_of_order`, `a_tag_without_a_front_cover_falls_back_to_the_first_picture` |
| A progress-bar template key is never registered | The render fps, or the ETA, silently renders as the empty string in every bar; the template string still reads correctly | Unit (renders a real bar) | `the_rendering_bar_draws_its_frame_count_render_fps_and_eta`, `every_template_is_one_indicatif_accepts` |
| A malformed bar template | A panic on the first frame of the first render, after every test has passed | Unit | `every_template_is_one_indicatif_accepts`, `both_styles_build` |
| The render fps is computed before any frame has landed | `inf fps` or `NaN fps` in the bar | Unit | `a_render_rate_that_cannot_be_computed_yet_reads_as_unknown` |
| A bar is drawn into a pipe or a CI log | Thousands of redraws pile up as garbage; the error at the end is buried | Unit + integration | `a_bar_is_drawn_only_on_a_terminal_and_never_when_quiet`, `a_piped_render_reports_progress_as_lines_rather_than_a_bar` (asserts no `\r` reached the pipe) |
| The line fallback reports once per frame, or never reaches 100% | A 9000-frame render writes 9000 log lines, or looks stalled at 90% | Unit | `the_line_fallback_reports_once_per_decile_of_progress`, `the_line_fallback_always_ends_at_a_hundred_percent`, `progress_beyond_the_total_still_reads_a_hundred_percent`, `a_phase_of_unknown_length_reports_no_percentage` |
| `--quiet` still prints progress, a warning, or the adapter | The flag `VISION.md` §3 promises is decoration | Unit + integration | `a_silent_ui_draws_nothing_and_still_accepts_every_callback`, `quiet_emits_nothing_on_success` (stdout *and* stderr) |
| `--verbose` omits the adapter, the ffmpeg command line, or the phase timings | The one flag that exists to explain a wrong render explains nothing | Integration | `verbose_logs_adapter_and_ffmpeg_cmdline` |
| A `--verbose` log line is drawn through the progress bar | Both are unreadable, and no assertion sees it | Manual (pty) | manual — see "the bars against a real terminal" |
| A warning says what happened and not what to do | The user cannot act on it; `AGENTS.md`'s CLI invariant is folklore | Unit + quality hook | `warning_text_for_software_fallback_matches_contract`, `every_pipeline_warning_names_a_consequence_and_an_action`, `the_sample_resolution_warning_names_the_size_and_the_way_out`, `scripts/quality.d/97-warnings-are-actionable.sh` |
| A new warning is added as an inline string, bypassing the tests that enumerate warnings | The shape is checked for four warnings and the fifth says "failed" | Quality hook | `scripts/quality.d/97-warnings-are-actionable.sh` |
| A background image smaller than the frame is upscaled in silence | Every video is soft and nothing errors; the user blames the preset | Unit + integration | `a_background_smaller_than_the_frame_warns_that_it_will_be_upscaled`, `only_an_image_short_of_the_frame_on_some_axis_is_upscaled`, `a_background_smaller_than_the_frame_warns_once_before_any_frame_is_drawn`, `a_background_as_large_as_the_frame_warns_about_nothing` |
| `--sample` drops to 720p in silence | The user judges a preview they believe is full size, and ships the wrong picture | Unit + integration | `a_sample_render_that_names_no_resolution_warns_that_it_was_reduced`, `a_configured_resolution_silences_the_sample_resolution_warning`, `a_sample_render_says_it_dropped_to_a_reduced_resolution`, `a_sample_render_at_a_configured_resolution_warns_about_nothing` |
| An error lands in the wrong exit-code bucket | A batch loop cannot tell "skip this song" from "stop, everything is misconfigured" | Integration (the whole matrix, through the binary) | `crates/avz-cli/tests/exit_codes.rs` |
| A bare `io::Error` reaches the user | "No such file or directory (os error 2)" names no file; the user cannot tell a bad path from a bad disk | Unit + integration | `an_unreadable_config_file_is_named_in_a_sentence_not_an_errno`, `a_config_file_that_cannot_be_opened_says_why_without_an_errno`, `every_failure_prefixes_its_message_and_names_what_the_user_gave_avz`, `a_missing_config_file_exits_2_and_names_the_path` |
| A missing `--config` file is classified as an input-file problem | Exit 3 says "this song is broken"; a batch loop skips every song instead of stopping | Unit + integration | `a_missing_config_file_is_a_config_problem_not_an_input_problem`, `a_missing_config_file_exits_2_and_names_the_path` |
| ffmpeg's own last words never reach the user | "encode failed" and nothing else; the disk was full and nobody knows | Integration (through the binary) | `an_ffmpeg_that_dies_midrender_exits_4_and_reports_its_last_words` |
| lavapipe unavailable under `--adapter software` | Hard failure on headless boxes | Manual (documented) | manual |

## Local Quality Gate

Run:

```bash
./scripts/quality.sh
```

Add project-specific checks as executable files in `scripts/quality.d/`.

## Test Naming

Prefer names that describe the requirement:

```text
sine_at_60hz_lights_up_bass_band_only
click_train_produces_onsets_at_expected_frames
readback_handles_non_multiple_of_256_row_widths
ffmpeg_death_midrender_leaves_no_output_file
muxed_audio_stream_is_copied_not_reencoded
set_override_beats_config_file_value
```

Avoid names that only describe the implementation:

```text
test_update
manager_returns_true
component_renders
```
