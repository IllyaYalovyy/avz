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

**Golden frames.** Render specific `(preset, seed, synthetic-feature)` frames to
PNG with `--adapter software` and compare hashes in CI. The software adapter is
what makes this stable across machines; never run golden tests on a hardware
adapter, because GPU float differences are expected. This catches shader
regressions cheaply.

**Integration.** A tiny CC0 test mp3 (about 5 s) lives in the repo at
`assets/fixtures/tone-tagged.mp3`, synthesized by `./scripts/make-test-fixture.sh`
and described in `assets/fixtures/README.md`. CI runs a
full `--sample 2s` render at 320×180 on the software adapter and asserts
`ffprobe` sees the expected duration, one video stream, one audio stream, and an
audio codec of `mp3` — which proves `-c:a copy` was not silently replaced by a
re-encode.

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
| Band energies map to wrong frequency ranges | Visuals react to the wrong instruments; subtly off, never obviously broken | Unit | TODO |
| Onset detection fires late or misses hits | Motion lags the beat — the core promise fails | Unit + manual | TODO |
| Envelope follower attack/decay math wrong | Motion is twitchy or sluggish | Unit | TODO |
| Normalization divides by zero on silence | Panic or NaN frames | Unit | TODO |
| Analysis frames do not land on video frame timestamps | Cumulative audio/visual drift over a long song | Unit | TODO |
| wgpu readback row padding mishandled (256-byte alignment) | Skewed or garbage frames | Integration | TODO |
| Shader regression changes output silently | Presets drift between releases | Golden frames (software adapter) | TODO |
| Nondeterminism leaks in (wall clock, unseeded RNG) | Re-render does not reproduce; golden tests flake | Golden frames | TODO |
| ffmpeg missing at runtime | Tool fails late with a cryptic error | Integration (preflight) | `missing_ffmpeg_fails_with_the_fedora_install_hint`, `render_without_ffmpeg_fails_with_the_fedora_install_hint`, `render_checks_for_ffmpeg_before_doing_any_work` |
| `ffmpeg` on PATH is not really ffmpeg | Cryptic subprocess failure mid-render | Integration (preflight) | `a_binary_that_is_not_ffmpeg_is_rejected`, `an_ffmpeg_that_exits_nonzero_is_rejected` |
| ffmpeg dies mid-render | Half-written `.mp4` left on disk | Integration | TODO |
| Audio re-encoded instead of `-c:a copy` | Generational quality loss, silently | Integration (ffprobe codec assert) | TODO |
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
