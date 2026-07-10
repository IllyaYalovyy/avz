# Release Checklist

Use this checklist when preparing a release.

## Before Release

- [ ] Update version numbers in every package manifest.
- [ ] Update `CHANGELOG.md`, including a **Known issues** section naming what is
      deferred and where it is tracked.
- [ ] Run `./scripts/quality.sh`.
- [ ] Run any project-specific packaging, installer, or smoke tests.
- [ ] Verify docs describe the released behavior.
- [ ] Confirm no secrets, local paths, or agent files are staged.

## avz-Specific Checks

- [ ] Manual listening pass: render the reference tracks and confirm onsets read
      as on-beat, not late (see `docs/TESTING.md`). "Feels musical" has no
      automated assertion — this gate is a human.
- [ ] Golden frames pass on the software adapter.
- [ ] Full render succeeds on both `--adapter gpu` and `--adapter software`.
- [ ] `ffprobe` on a released render confirms the audio stream is `mp3` and was
      copied, not re-encoded.
- [ ] Every shipped preset renders without a panic at 1080p and at `--sample`
      resolution, and its `perf_hint` is accurate on software rendering.
- [ ] `avz config --example` output can be fed straight back into `--config`.
- [ ] `avz presets` and `avz presets <name>` reflect the shipped schemas.
- [ ] Every `perf_hint` is *re-measured*, not re-read. A hint is advice with no
      assertion behind it, and a shader change can make yesterday's number a lie.
      Time the rendering phase alone, with an ffmpeg stand-in that drains stdin,
      or x264's backpressure is what you will measure.
- [ ] `cargo install --path crates/avz-cli` works from a clean checkout with
      only system `ffmpeg` present.
- [ ] Acceptance test: an entire album batch-renders unattended via a shell loop
      with zero interventions:

      ```bash
      cargo build --release -p avz-cli
      ./scripts/album-acceptance.sh path/to/album album.toml   # or no args, for a synthetic stand-in
      ADAPTER=software ./scripts/album-acceptance.sh path/to/album album.toml
      ```

      Record its track count, wall time, adapter, and warnings in the release
      notes. The script fails on the first intervention; a real album is the gate,
      and the synthetic stand-in is only a smoke test of the loop.

## Release Notes

Include:

- User-visible changes
- Bug fixes
- Breaking changes or migrations
- Known issues
- Upgrade or rollback notes

For avz, also call out any change that alters rendered output for an unchanged
input and config — a new preset version, a shader fix, or a change to envelope
defaults or normalization. Those break reproducibility of existing configs and
belong under "Breaking changes".

## After Release

- [ ] Tag the release.
- [ ] Publish artifacts.
- [ ] Verify install / upgrade from the published artifact.
- [ ] Open follow-up issues for deferred work.
