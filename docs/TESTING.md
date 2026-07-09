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

**Integration.** A tiny CC0 test mp3 (about 5 s) lives in the repo. CI runs a
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
| Band energies map to wrong frequency ranges | Visuals react to the wrong instruments; subtly off, never obviously broken | Unit | TODO |
| Onset detection fires late or misses hits | Motion lags the beat — the core promise fails | Unit + manual | TODO |
| Envelope follower attack/decay math wrong | Motion is twitchy or sluggish | Unit | TODO |
| Normalization divides by zero on silence | Panic or NaN frames | Unit | TODO |
| Analysis frames do not land on video frame timestamps | Cumulative audio/visual drift over a long song | Unit | TODO |
| wgpu readback row padding mishandled (256-byte alignment) | Skewed or garbage frames | Integration | TODO |
| Shader regression changes output silently | Presets drift between releases | Golden frames (software adapter) | TODO |
| Nondeterminism leaks in (wall clock, unseeded RNG) | Re-render does not reproduce; golden tests flake | Golden frames | TODO |
| ffmpeg missing at runtime | Tool fails late with a cryptic error | Integration (preflight) | TODO |
| ffmpeg dies mid-render | Half-written `.mp4` left on disk | Integration | TODO |
| Audio re-encoded instead of `-c:a copy` | Generational quality loss, silently | Integration (ffprobe codec assert) | TODO |
| Background-video decode thread stalls or deadlocks | Render hangs with no diagnostic | Integration (bounded channel + timeout) | TODO |
| Config precedence wrong (`--set` loses to file) | Reproducible renders are not reproducible | Unit | TODO |
| Unknown TOML key silently ignored | Typo'd param silently does nothing | Unit | TODO |
| Missing ID3 tags | Crash instead of a warned-and-skipped text card | Unit | TODO |
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
