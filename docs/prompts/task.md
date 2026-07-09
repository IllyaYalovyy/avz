# Task Prompt

Use this prompt to turn an idea, bug, or feature request into a clear task for an
AI assistant.

```text
You are working in this repository. Read AGENTS.md, CONTRIBUTING.md, and the
relevant docs before making changes.

Task:
[Describe the requested change or bug.]

User / system context:
[Who is affected? What workflow, command, screen, API, or behavior matters?]

Expected behavior:
[What should be true when the task is done?]

Current behavior:
[For bugs, include reproduction steps, observed result, logs, or screenshots.
For features, describe the current gap.]

Scope:
- In scope:
  - [...]
- Out of scope:
  - [...]

Constraints:
- Preserve existing behavior unless explicitly changed.
- Follow existing project patterns.
- Add or update tests at the layer where the risk lives.
- Do not commit secrets, local paths, agent files, or unrelated cleanup.

Definition of done:
- Implementation complete.
- Relevant docs updated.
- Regression tests added or test gap explained.
- ./scripts/quality.sh run, or skipped checks explained.
- Final response summarizes changes, checks, and residual risks.
```
