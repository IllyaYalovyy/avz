# RFC-002: The Effects Stage — Transforming the Finished Picture

| Field | Value |
|---|---|
| Status | Accepted (2026-07-12) |
| Author(s) | Illya Yalovyy |
| Supersedes | - |
| Superseded by | - |

## Summary

A post-processing stage that transforms the *composited* frame — after
background, visualizer, and text are flattened — with music-drivable color,
brightness, zoom, and rotation, freely combined. Configured as a new
`[effects]` section whose defaults are all identity; when nothing is asked
for, the pass is skipped entirely and the render is byte-identical to today's,
which keeps every shipped golden hash valid. Owner-requested (2026-07-12).

## Goals

- **G1** - Color (hue, saturation, contrast), brightness, zoom, and rotation
  over the whole picture, each individually and in combination.
- **G2** - Music-drivable: the kick can pulse the zoom, the bass can sway the
  rotation, a hit can lift the brightness, the hue can drift on the song's
  clock.
- **G3** - Identity by default: an absent or default `[effects]` section
  changes nothing — not one byte of output, not one microsecond of GPU work.
- **G4** - Deterministic and golden-testable, like everything else avz draws.

## Non-Goals

- **NG1** - Per-layer effects (transforming only the background or only the
  visualizer). The stage sees the finished picture; per-layer grading is a
  future RFC if it earns one.
- **NG2** - Temporal effects (motion blur, echo) — they need frame history at
  the post stage and a determinism story of their own.
- **NG3** - An effect plugin system. This is one fixed, parametric pass, not a
  second preset registry.

## Background and Motivation

Presets draw the visualizer layer; nothing today can grade or move the *whole
picture*. The compositor's contract — layers in, one flattened frame out — is
fixed by RFC-001's design notes, and its own kaleido finding says changes to
that contract belong in an RFC. This is that RFC: one new stage between the
compositor and the readback.

## User Impact

| Audience | Impact |
|---|---|
| End users | `--set effects.spin=0.05 --set effects.pulse=0.08` breathes and turns any render; config files gain a `[effects]` section |
| Contributors | One new render module + config section; presets and layers unchanged |
| Operators / packagers | None: same binary, same dependencies |

## Considered Options

### Option A - Chained single-effect passes

**Pros**: each effect is its own tiny shader; arbitrary ordering.

**Cons**: one texture round-trip per enabled effect; N× the contract surface;
ordering becomes user-visible configuration nobody asked for.

### Option B - One combined pass, matrices built on the CPU

The shader applies a 2×2 UV transform (zoom ∘ rotation, about the center, in
aspect-true coordinates) and a 3×3+offset color matrix (contrast ∘ saturation
∘ hue ∘ brightness, in linear light). Both matrices are composed **in Rust,
per frame**, from the config and that frame's features.

**Pros**: one round-trip regardless of combination; the math lives in Rust
where it is unit-testable against hand-computed values; a fixed, documented
order (geometry, then color).

**Cons**: adding a fundamentally new effect kind later means touching the one
pass rather than dropping in a file.

### Option C - ffmpeg video filters

**Pros**: zero GPU work.

**Cons**: ffmpeg sees only finished frames — no feature timeline, so nothing
can follow the music; filter graphs are also where cross-platform
reproducibility goes to die. Rejected outright.

## Decision

**Chosen option: Option B.** One pass, CPU-built matrices, fixed order.
Skipped when identity (G3) — the compositor then writes directly to the
readback target exactly as today.

## Design

- **Pipeline**: when `[effects]` is non-identity, the compositor flattens into
  an intermediate sampleable texture; the effects pass samples it with the
  frame's UV transform (clamp-to-edge for the uncovered fringe a zoom-out or
  rotation exposes) and applies the color matrix into the readback target.
- **Config** (`[effects]`, all defaults identity, strict validation):
  `zoom` (0.5..3, default 1), `pulse` (kick→zoom, 0..0.5, default 0),
  `spin` (turns/s, −2..2, default 0), `sway` (bass→tilt in turns, −0.25..0.25,
  default 0), `hue` (turns, 0..1, default 0), `hue_drift` (turns/s, −2..2,
  default 0), `saturation` (0..3, default 1), `contrast` (0.2..3, default 1),
  `brightness` (0..3, default 1), `flash` (hit→brightness, 0..2, default 0).
- **Determinism**: angle = `spin·time + sway·bass_env`, zoom =
  `zoom·(1 + pulse·bass_env)`, hue = `hue + hue_drift·time`, brightness lift =
  `1 + flash·onset` — all instantaneous functions of frame time and that
  frame's features, nothing integrated (AGENTS.md).
- **Color math**: linear light, Rec. 709 luma weights, hue rotation about the
  gray axis; matrices composed contrast → saturation → hue → brightness and
  uploaded as one 3×3 + offset.

## Testing Strategy

| Risk / invariant | Test layer | Test |
|---|---|---|
| Identity config changes output | Integration (lavapipe) | default `[effects]` renders byte-identical to no effects stage |
| Matrix math wrong | Unit (Rust) | hand-computed vectors: brightness scales, contrast pivots at mid-gray, saturation 0 is luma, hue 1/3 turn cycles R→G→B, zoom/rotation compose |
| Pass samples wrong UVs | Integration (lavapipe) | zoom leaves the center pixel, moves a known off-center marker inward; a quarter-turn relocates a corner marker |
| Config drift | Existing meta-tests | `[effects]` keys in `config --example` and CONFIGURATION.md (docs_reference) |
| End to end | e2e | `--set effects.spin=0.1 --set effects.brightness=1.3` renders through the binary |

## Goals Alignment

| Goal | How addressed |
|---|---|
| G1/G2 | The ten keys above, each feature-coupled where it should be |
| G3 | Pass skipped at identity; goldens prove byte-equality |
| G4 | CPU matrices + clamp sampling; no hash, no clock but frame time |

## Development Plan

- [ ] **Step 1** - The effects stage: config section, CPU matrices with unit
  tests, the pass, pipeline wiring, pixel-relation integration tests, docs,
  example config *(one GitHub issue)*

## Open Questions

- [x] **Q1** - Edge fill for zoom-out/rotation? **Clamp-to-edge.** Black bars
  advertise the transform; clamped smear reads as camera movement. Revisit if
  a `fill` option is ever asked for.

## References

- [VISION.md](../VISION.md) §5.3 — the layer stack this stage sits after
- [RFC-001](./RFC-001-mvp-v0.1.md) — the compositor-contract precedent
