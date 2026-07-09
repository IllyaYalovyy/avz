# Commit Prompt

Use this prompt when asking an AI assistant to prepare a commit.

```text
Prepare a commit for the current repository changes.

Read CONTRIBUTING.md and docs/COMMITS.md first.

Before committing:
- Check git config user.name and user.email.
- Inspect git status --short.
- Inspect staged and unstaged diffs.
- Stage only files that belong to this task.
- Do not stage secrets, local paths, agent files, logs, or unrelated changes.
- Run git diff --check.
- Run relevant tests or explain why they are not run.

Commit rules:
- Use a small, coherent commit.
- Use an imperative summary.
- Mention tests in the body when useful.
- Do not push unless explicitly instructed.

After committing:
- Report commit hash and subject.
- Summarize files included.
- Report checks run and any remaining risk.
```
