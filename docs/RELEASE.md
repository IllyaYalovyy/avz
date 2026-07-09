# Release Checklist

Use this checklist when preparing a release.

## Before Release

- [ ] Update version numbers in every package manifest.
- [ ] Update `CHANGELOG.md` or release notes.
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
- [ ] `cargo install --path crates/avz-cli` works from a clean checkout with
      only system `ffmpeg` present.
- [ ] Acceptance test: an entire album batch-renders unattended via a shell loop
      with zero interventions.

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
