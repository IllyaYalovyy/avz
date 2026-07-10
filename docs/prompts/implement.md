# Implementation Prompt

Use this prompt after a task is clear or an RFC is accepted.

```text
You are implementing a change in this repository. Read AGENTS.md,
CONTRIBUTING.md, docs/PROCESS.md, docs/TESTING.md, and any relevant RFC before
editing code.

Change to implement:
[Describe the accepted task or link the RFC.]

Implementation constraints:
- Keep the patch scoped to the accepted behavior.
- Follow existing module boundaries and local style.
- Do not perform unrelated refactors.
- Preserve user changes already present in the worktree.
- Add regression coverage at the layer where the risk lives.
- Update docs or design records if behavior or decisions changed.

Verification:
- Run focused tests first.
- Run ./scripts/quality.sh before handoff when practical. The local gate is
  authoritative; do not wait for or gate on remote CI.
- If checks cannot run, explain why and identify the residual risk.

Completion:
- Tick the task's checkbox in the owning RFC's development plan as part of the
  change.
- After the work lands on main, close the GitHub issue with a comment naming
  the behavior, merge commit, and tests (docs/PROCESS.md, Issue Tracking). If
  you cannot close it, say so in the final response.

Final response:
- Summarize changed behavior.
- List important files changed.
- List tests/checks run.
- List skipped checks and remaining risks.
- Confirm the GitHub issue was closed, or say why it was not.
```
