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

The VISION §5.1 performance budget — low single-digit seconds for a five-minute
song, which is what the reused FFT planner and the parallel windows buy — is a
smoke test in `crates/avz-core/tests/analysis_perf.rs`. It sits there rather than
beside the code because it reads a wall clock, and
`scripts/quality.d/90-animation-time-comes-from-the-frame-index.sh` rightly
forbids `Instant::now()` under `crates/avz-core/src`. It is `#[ignore]`d, since a
loaded machine would make it flake as a per-commit gate; run it with
`cargo test -p avz-core --test analysis_perf -- --ignored`.

**Golden frames.** Render specific `(preset, seed, synthetic-feature)` frames to
PNG with `--adapter software` and compare hashes in CI. The software adapter is
what makes this stable across machines; never run golden tests on a hardware
adapter, because GPU float differences are expected. This catches shader
regressions cheaply.

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
| Envelope follower attack/decay math wrong | Motion is twitchy or sluggish | Unit | TODO |
| Normalization divides by zero on silence | Panic or NaN frames | Unit | TODO |
| Analysis frames do not land on video frame timestamps | Cumulative audio/visual drift over a long song | Unit | `analysis_frames_never_drift_from_the_video_frame_clock`, `a_burst_lands_on_the_video_frame_nearest_it`, `one_feature_frame_per_video_frame`, `a_partial_final_video_frame_still_gets_a_feature_frame` |
| Analysis windows leave gaps between hops at low fps | A hit landing in a gap never reaches the visuals | Unit | `no_audio_falls_between_windows_when_the_hop_exceeds_the_window` |
| RMS is wrong in a way that still looks plausible | Brightness follows nothing in particular | Unit | `a_constant_sine_has_the_same_rms_on_every_frame`, `a_dc_signal_has_an_rms_equal_to_its_amplitude`, `a_loud_passage_reads_louder_than_a_quiet_one`, `silence_has_zero_rms_and_no_nans` |
| wgpu readback row padding mishandled (256-byte alignment) | Skewed or garbage frames | Unit + integration + quality hook | `readback_handles_non_multiple_of_256_row_widths` (both layers), `a_row_stride_rounds_up_to_the_256_byte_alignment`, `an_already_aligned_row_is_not_padded`, `scripts/quality.d/50-readback-padding-lives-in-one-place.sh` |
| Readback buffer size and row layout silently disagree | A sheared frame nobody notices until the video is watched | Unit | `a_buffer_that_is_not_the_padded_size_is_a_render_error` |
| `--adapter gpu` quietly renders on lavapipe | An 8 fps render the user explicitly ruled out | Unit + integration | `gpu_refuses_a_software_adapter_and_software_refuses_a_hardware_one`, `asking_for_gpu_never_yields_a_software_adapter` |
| `--adapter software` quietly renders on the GPU | Golden frames pass locally and fail everywhere else | Unit + integration | `gpu_refuses_a_software_adapter_and_software_refuses_a_hardware_one`, `the_software_adapter_is_a_cpu_adapter_and_needs_no_warning` |
| Software fallback happens without a warning, or warns when asked for | The user cannot tell a slow render from a broken one, or is nagged every render | Unit + integration + quality hook | `only_an_auto_render_that_lands_on_software_is_worth_warning_about`, `auto_always_finds_an_adapter_and_flags_a_software_fallback`, `a_gpu_less_host_falls_back_to_software_and_says_so`, `scripts/quality.d/70-gpu-less-host-falls-back-to-lavapipe.sh` |
| A second render backend creeps in (dx12/metal/gles) | Shaders run on an untested path; golden frames stop meaning anything | Quality hook | `scripts/quality.d/60-render-is-vulkan-only.sh` |
| Shader regression changes output silently | Presets drift between releases | Golden frames (software adapter) | TODO |
| Nondeterminism leaks in (wall clock, unseeded RNG) | Re-render does not reproduce; golden tests flake | Golden frames + quality hook | `scripts/quality.d/90-animation-time-comes-from-the-frame-index.sh`; golden frames TODO |
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
| Background-video decode thread stalls or deadlocks | Render hangs with no diagnostic | Integration (bounded channel + timeout) | TODO |
| Config precedence wrong (`--set` loses to file) | Reproducible renders are not reproducible | Unit | `set_override_beats_config_file_value`, `cli_flag_beats_set_override`, `a_silent_layer_does_not_erase_a_lower_one` |
| Unknown TOML key silently ignored | Typo'd param silently does nothing | Unit | `unknown_toml_key_rejected_with_suggestion`, `unknown_set_key_is_rejected_with_a_suggestion_and_the_assignment` |
| Missing ID3 tags | Crash instead of a warned-and-skipped text card | Unit | `untagged_mp3_reports_missing_tags_instead_of_failing`, `blank_and_whitespace_tag_values_count_as_missing`, `missing_tags_render_as_missing_and_missing_art_as_none` |
| Unreadable input reported as a cryptic OS error | User cannot tell a bad file from a bad disk | Unit + integration | `a_file_that_lies_about_being_an_mp3_is_an_input_error`, `a_file_of_an_unknown_format_is_an_input_error`, `probe_of_a_missing_file_exits_3` |
| Cover art picked nondeterministically from a multi-picture tag | Art-reactive presets drift between runs | Unit | `front_cover_wins_over_other_pictures_regardless_of_order`, `a_tag_without_a_front_cover_falls_back_to_the_first_picture` |
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
