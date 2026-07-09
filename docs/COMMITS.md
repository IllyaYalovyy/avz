# Commit Rules

Commits should make review and rollback straightforward.

## Identity

Use the project identity from `CONTRIBUTING.md`:

```bash
git config user.name
git config user.email
```

Do not commit with an unrelated corporate, noreply, or machine-specific identity.

## Local Hook Guard

Run this after cloning or initializing the project:

```bash
./scripts/install-git-hooks.sh
```

The hook rejects staged AI/task-runner files, prompts, context files, and chat
logs even if they were added with `git add -f`. It is local to `.git/hooks/`,
so every checkout needs it installed.

## When to Commit

Commit only after:

- The change is coherent and reviewable.
- Tests or quality checks were run, or skipped checks are understood.
- No secrets, local paths, agent files, or unrelated artifacts are staged.
- The AI-file pre-commit guard is installed.
- Generated files are intentionally included.

## Commit Shape

Prefer small commits that each answer one reason for change:

- One bug fix with its regression test
- One feature slice with its tests and docs
- One mechanical refactor with no behavior change
- One dependency update with rationale and lockfile changes

Avoid commits that mix formatting, refactoring, behavior changes, and generated
files unless the project explicitly requires it.

## Message Format

Default format:

```text
scope: imperative summary

Why this change is needed.
What changed at a high level.
Important test or migration notes.
```

Examples:

```text
auth: reject expired sessions before refresh

Avoid starting authenticated requests with an already expired access token.
This keeps retry behavior explicit and leaves refresh token ownership in the
session manager.

Tests: cargo test -p auth
```

```text
docs: add RFC template for storage migrations
```

## Staging Checklist

Before `git commit`:

```bash
git status --short
git diff --check
git diff --cached --stat
git diff --cached
```

Review the staged diff as if someone else wrote it.

## Prohibited Commit Content

Never commit:

- Secrets or credentials
- Local absolute paths with usernames or hostnames
- AI agent scratch directories, task-runner state, prompts, context files, or
  chat logs
- Debug-only print spam
- Unexplained binary files
- Large generated artifacts unless they are release assets or required fixtures
