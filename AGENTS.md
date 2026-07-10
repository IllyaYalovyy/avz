# AI Agent Instructions

These instructions apply to AI coding agents working in this repository.

`VISION.md` is the north star for `avz`. Read it before proposing architecture.
If a request does not serve the brief in VISION.md §1, it belongs in the backlog
(§12), not in the code.

## Operating Principles

- Read existing code and docs before proposing architecture.
- Prefer the repository's established patterns over new abstractions.
- Keep changes scoped to the task. Do not perform unrelated cleanup.
- Preserve user changes. Never revert files you did not intentionally modify.
- Use fast search tools such as `rg` before slower recursive commands.
- Add tests with the change unless the reason not to is explicit.
- Run `./scripts/quality.sh` before declaring implementation complete when
  practical.

## Standard Workflow

1. **Understand** - read the request, relevant docs, existing code, tests, and
   open design records.
2. **Classify** - decide whether this is a small task, bug fix, RFC-required
   change, review, or commit-prep request.
3. **Plan** - for non-trivial work, state the concrete steps and test strategy.
4. **Implement** - keep edits scoped and preserve unrelated user changes.
5. **Verify** - run focused checks first, then `./scripts/quality.sh` when
   practical.
6. **Handoff** - summarize changed behavior, files, checks, skipped checks, and
   residual risks; update and close the GitHub issue the task tracks
   (`docs/PROCESS.md`, Issue Tracking).

## Planning and Design

Use an RFC before implementation when the change:

- Touches multiple subsystems
- Adds or replaces dependencies
- Changes persistence, API, protocol, auth, or public behavior
- Is difficult to reverse

Use `designs/RFC-000-template.md` and keep the development plan updated as steps
complete.

Design review rules live in `docs/DESIGN-REVIEW.md`.

## Implementation Rules

- Keep commits and patches reviewable.
- Do not hard-code local paths, usernames, hostnames, secrets, or tokens.
- Do not commit agent working directories, task-runner state, prompts, context
  files, scratchpads, or chat logs. These are local-only and must not leak to
  the remote repository.
- Make failures explicit. Prefer errors with context over silent fallback.
- For UI work, verify real behavior, not only component existence.

## avz-Specific Rules

These encode invariants from `VISION.md`. Breaking one is a design change and
needs an RFC, not a patch.

**Architecture**

- Keep `avz-core` UI-agnostic: zero terminal I/O, no `println!`, no `indicatif`.
  Progress is reported through a callback trait. All terminal output lives in
  `avz-cli`. This is the "GUI later without refactoring" guarantee.
- `anyhow` at the CLI layer for error context chains; typed `thiserror` errors
  inside `avz-core`. Do not leak `anyhow::Error` out of core.
- Analysis completes fully before rendering starts. The two-pass design is what
  buys lookahead and global normalization — do not stream them together.

**Determinism**

- All animation time derives from `frame_index / fps`. Never wall clock, never
  frame deltas.
- Any randomness is a seeded hash of `(frame_index, preset seed)`. No
  unseeded RNG, no iteration over `HashMap` where order reaches a shader.
- Same inputs plus same config must produce the same video, modulo encoder
  nondeterminism. Golden-frame tests run on the software adapter only, because
  GPU float differences are expected across machines.

**Rendering**

- One code path: wgpu → Vulkan → (hardware driver | lavapipe). Do not add a
  second renderer or low-fi shader variants.
- Keep the 256-byte readback row-padding handling in exactly one place.
- A new preset should mean one WGSL file plus one parameter schema, and nothing
  else. If adding a preset requires touching code outside `presets/`, the
  abstraction is wrong — fix the abstraction.

**Audio and encoding**

- Never re-encode the audio. The original mp3 stream is muxed with `-c:a copy`.
- FFmpeg is a subprocess, not a crate. Preflight `ffmpeg -version` and fail with
  the Fedora install hint if missing.
- Write to `out.mp4.part` and rename on success. Never leave a half-written file.
- Propagate a clean failure if ffmpeg dies mid-render; monitor its stderr.

**CLI**

- Warnings must be actionable and say what to do next. Compare:
  "no GPU adapter found, falling back to software rendering — expect ~8 fps;
  pass `--adapter software` to silence this".
- Exit codes: 0 ok, 2 bad args/config, 3 input file problems, 4 render/encode
  failure.
- Config precedence is fixed: CLI flags > `--set` > `--config` > preset defaults
  > built-in defaults. Unknown TOML keys are rejected with "did you mean"
  suggestions.

**Non-goals — do not implement**

Lyrics of any kind, GUI, realtime preview, live audio input, beat/BPM tracking,
video editing features, bundled FFmpeg. See VISION.md §2.

## Commit Rules

- Read `docs/COMMITS.md` before preparing a commit.
- Verify `git config user.name` and `git config user.email`.
- Keep the local AI-file pre-commit guard installed by running
  `./scripts/install-git-hooks.sh` after cloning or reinitializing the project.
- Inspect the staged diff before committing.
- Keep commits coherent and reversible.
- Do not push unless explicitly instructed.

## Prompt Templates

Reusable prompts live in `docs/prompts/`:

- `task.md` - clarify and execute a task
- `rfc.md` - draft or revise an RFC
- `implement.md` - implement accepted work
- `review.md` - review a diff or branch
- `commit.md` - prepare a commit

## Review Before Handoff

Before handing work back:

- Summarize changed files and behavior.
- State which tests or checks were run.
- State any checks that were skipped and why.
- Call out remaining risks or follow-up work.
- Close the GitHub issue the task tracks, with a comment naming the behavior,
  merge commit, and tests — and tick its checkbox in the owning RFC. An issue
  left open after its work landed misleads everyone reading the milestone.
- Judge completion by the local quality gate, not remote CI. CI is advisory in
  this project; never wait for or gate on it.
