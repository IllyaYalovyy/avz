# User Tasks

This file captures the user workflows the project must support. Treat it as a
test planning document, not marketing copy.

Each task should define:

- **Precondition** - what must be true before the user starts
- **Flow** - the sequence of user actions in the happy path
- **Outcome** - what the user observes when done
- **Interactions** - count of meaningful actions in the happy path
- **Regression coverage** - test name or reason coverage is manual

These tasks are the executable form of the UX contract in VISION.md §3. The
milestone that first delivers each task is noted; `Regression coverage` stays
`TODO` until the test exists.

## UT-001: Render a video with zero configuration

**Precondition:** A valid mp3 exists. `ffmpeg` is installed. Milestone: M1→M5.

**Flow:**

1. `avz render song.mp3`

**Outcome:** A playable `song.mp4` appears next to the input: 1080p30, H.264,
the original audio muxed in untouched, visuals that move with the music. A
progress bar showed phase, frame count, render fps, and ETA. No other setup was
required.

**Interactions:** 1

**Regression coverage:** Partial. The pipeline runs end to end and muxes the
original audio: `render_writes_a_sampled_mp4_next_to_the_input`,
`a_render_without_a_sample_covers_every_frame_of_the_song`,
`muxed_audio_stream_is_copied_not_reencoded`. The visuals are still the M1
tracer bullet, not a preset (`the_rendered_brightness_visibly_follows_the_loudness_of_the_song`),
and there is no progress bar yet — both land with RFC-001 Steps 14 and 21.

## UT-002: Iterate quickly on an excerpt

**Precondition:** A valid mp3. The user is tuning a preset and does not want to
wait for a full render. Milestone: M1.

**Flow:**

1. `avz render song.mp3 --preset ribbons --sample 0:45..1:45`
2. Watch the result, adjust, repeat.

**Outcome:** Only the 45s–1:45 excerpt renders, at reduced resolution by
default, in a fraction of the time. Audio in the output covers the same range.
`--sample 60s` is accepted as shorthand for the first 60 seconds.

**Interactions:** 1 per iteration

**Regression coverage:** Frame selection:
`a_sampled_render_writes_exactly_the_frames_of_the_requested_range`,
`a_sample_range_selects_the_frames_that_cover_it`,
`a_sample_boundary_lands_on_the_frame_whose_timestamp_it_names`. The audio
covering the same range:
`a_sampled_render_muxes_the_matching_slice_of_the_original_audio`,
`the_audio_starts_at_the_first_rendered_frames_timestamp`. The reduced default
resolution: `a_sample_render_defaults_to_a_reduced_resolution`,
`render_writes_a_sampled_mp4_next_to_the_input`. Both `--sample` spellings:
`sample_accepts_a_bare_duration_and_a_clock_range`. `--preset` is not a flag
yet (RFC-001 Step 15).

## UT-003: Render on a machine with no GPU

**Precondition:** A headless host with `mesa-vulkan-drivers` installed and no
hardware Vulkan adapter. Milestone: M1.

**Flow:**

1. `avz render song.mp3`

**Outcome:** avz warns once, actionably — that no GPU adapter was found, that it
is falling back to software rendering, roughly how slow that will be, and that
`--adapter software` silences the warning — then produces a correct video via
lavapipe. `--adapter gpu` instead fails fast with a clear error.

**Interactions:** 1

**Regression coverage:** Adapter selection and the fallback flag:
`a_gpu_less_host_falls_back_to_software_and_says_so`,
`asking_for_gpu_never_yields_a_software_adapter`,
`only_an_auto_render_that_lands_on_software_is_worth_warning_about`.
`scripts/quality.d/70-gpu-less-host-falls-back-to-lavapipe.sh` simulates the
GPU-less host by restricting Vulkan to the lavapipe ICD, so this no longer needs
one. The warning itself, once per render and only under `--adapter auto`:
`a_gpu_less_auto_render_warns_once_and_says_how_to_silence_it`,
`an_explicit_software_render_warns_about_nothing`.

## UT-004: Discover presets and their parameters

**Precondition:** avz is installed. Milestone: M3.

**Flow:**

1. `avz presets`
2. `avz presets pulse`

**Outcome:** The first lists every preset with a one-line description. The
second pretty-prints the preset's full parameter schema: name, type, default,
valid range, and description, plus any `perf_hint` for software rendering.

**Interactions:** 1–2

**Regression coverage:** `presets_command_lists_all_registered`,
`presets_name_prints_schema_fields`,
`presets_of_an_unknown_preset_exits_2_and_names_the_known_ones` (through the
binary); `the_listing_names_every_preset_and_describes_it`,
`the_schema_print_shows_every_column_for_every_type`,
`the_schema_columns_are_aligned`,
`a_perf_hint_is_printed_when_the_schema_carries_one` (the formatter). `nebula`
lands in RFC-001 Step 17; until then `pulse` is the only preset to print.

## UT-005: Inspect an input file before rendering

**Precondition:** An mp3 of unknown provenance. Milestone: M0.

**Flow:**

1. `avz probe song.mp3`

**Outcome:** Title, artist, album, duration, sample rate, and whether cover art
is embedded (with mime type and dimensions). Missing tags are reported as
missing, not as an error.

**Interactions:** 1

**Regression coverage:** `probe_prints_tags_duration_and_cover_art`,
`probe_reports_missing_tags_as_missing_rather_than_failing`,
`probe_of_a_missing_file_exits_3`, `probe_does_not_require_ffmpeg`

## UT-006: Reproduce a render from a config file

**Precondition:** A `cold-design.toml` checked into the album repo. Milestone: M3.

**Flow:**

1. `avz render song.mp3 --config cold-design.toml`

**Outcome:** Byte-comparable video (modulo encoder nondeterminism) to the last
render from that config, because the seed and every parameter are pinned. An
unknown key in the TOML is rejected with a "did you mean" suggestion rather than
silently ignored.

**Interactions:** 1

**Regression coverage:** TODO

## UT-007: Override one parameter on top of a config

**Precondition:** A working `base.toml`. Milestone: M3.

**Flow:**

1. `avz render song.mp3 --config base.toml --set visual.intensity=1.4`

**Outcome:** Everything from `base.toml` applies except `visual.intensity`,
which is 1.4. A `--set` for a key that does not exist in the preset's schema, or
a value outside its range, fails with exit code 2 before any rendering starts.

**Interactions:** 1

**Regression coverage:** `set_override_beats_config_file_value`,
`cli_flag_beats_set_override` (precedence);
`a_set_override_beats_an_illegal_value_in_the_config_file`,
`a_config_files_preset_params_are_validated_against_the_schema`,
`out_of_range_value_fails_exit_2_before_render`,
`unknown_param_via_set_exits_2_with_a_suggestion` (through the binary);
`an_out_of_range_parameter_fails_before_the_song_is_decoded` (nothing is decoded
first); `a_preset_parameter_from_the_config_reaches_the_rendered_pixels` (it
reaches the shader).

## UT-008: Emit a documented config template

**Precondition:** avz is installed. Milestone: M5.

**Flow:**

1. `avz config --example > avz.toml`

**Outcome:** A commented TOML template covering every section with defaults, that
can be edited and passed straight back to `--config` without further changes.

**Interactions:** 1

**Regression coverage:** TODO

## UT-009: Composite a background and a title card

**Precondition:** An mp3 with ID3 title/artist, and a background image or a
loopable background video. Milestone: M4.

**Flow:**

1. `avz render song.mp3 --preset nebula --palette ember --bg art/forest.png --out video.mp4`

**Outcome:** The background sits beneath the visuals with the configured fit,
blur, and darken so the visuals still read on top. The title/artist card fades
in and out on schedule. A background video loops seamlessly regardless of its
native fps or resolution, and its audio is ignored. Missing ID3 tags warn and
skip the card rather than failing.

**Interactions:** 1

**Regression coverage:** TODO

## UT-010: Batch-render an album unattended

**Precondition:** A directory of mp3s and an `album.toml`. This is the v0.1
acceptance test. Milestone: M5.

**Flow:**

1. `for f in album/*.mp3; do avz render "$f" --config album.toml; done`

**Outcome:** Every track renders to its own mp4 with zero interventions. A
failure on one track exits non-zero with a clear reason and leaves no
half-written `.mp4` behind.

**Interactions:** 1

**Regression coverage:** TODO
