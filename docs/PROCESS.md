# Project Process

avz uses a lightweight design-first process. The goal is not more
documents; the goal is fewer unclear implementation decisions.

## Work Intake

Every non-trivial change starts from one of:

- A user task in `designs/USER-TASKS.md`
- A bug report with reproduction steps and expected behavior
- An RFC in `designs/`
- A small maintenance task with a clear definition of done

Use `docs/prompts/task.md` when asking an AI assistant to turn an idea into an
actionable implementation task.

## Decision Levels

Small changes can go straight to implementation when they are local, reversible,
and covered by tests.

Use an RFC when the change affects architecture, dependencies, storage, API,
security, UI workflow, or multiple subsystems.

Use `docs/prompts/rfc.md` to draft the RFC and `docs/DESIGN-REVIEW.md` to review
it before implementation.

## Execution Modes

### Small Task

Use for local, reversible changes.

1. Confirm expected behavior.
2. Implement the smallest coherent patch.
3. Add regression coverage.
4. Run focused checks and `./scripts/quality.sh`.
5. Hand off with changed behavior, checks, and risks.

### RFC-Led Change

Use for broad, irreversible, or architecture-affecting changes.

1. Draft RFC from `designs/RFC-000-template.md`.
2. Review using `docs/DESIGN-REVIEW.md`.
3. Mark the RFC accepted.
4. Implement in the RFC's development-plan steps.
5. Keep the RFC updated if the design changes.

### Review

Use `docs/prompts/review.md`. Findings should lead, ordered by severity, with
file and line references.

### Commit Prep

Use `docs/prompts/commit.md` and `docs/COMMITS.md`. Do not push unless
explicitly instructed.

## Done Means

A change is done when:

- Behavior is implemented
- Relevant docs are updated
- Regression tests exist at the right layer
- `./scripts/quality.sh` passes or failures are explained
- Review feedback is resolved
- Known follow-up work is tracked

## Design Discipline

RFCs should compare real options. Avoid using them to justify a decision already
made unless the document records the tradeoffs honestly.

Implementation should follow the accepted RFC. If implementation reveals a
better design, update the RFC before continuing.

## Prompt Index

- Task intake: `docs/prompts/task.md`
- RFC drafting: `docs/prompts/rfc.md`
- Implementation: `docs/prompts/implement.md`
- Review: `docs/prompts/review.md`
- Commit prep: `docs/prompts/commit.md`
