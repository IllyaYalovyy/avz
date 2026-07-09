# Design Review Rules

Design review happens before implementation for changes that require an RFC.
The purpose is to improve the decision, not to polish prose.

## Required Inputs

A design review needs:

- RFC draft in `designs/RFC-NNN-<slug>.md`
- Clear problem statement and motivation
- Goals and non-goals
- At least two considered options, unless only one is technically viable
- Chosen decision with tradeoffs
- Testing strategy tied to risks
- Development plan with reviewable steps

## Review Questions

Reviewers should ask:

- Is this solving the right user or system problem?
- Are the goals concrete enough to verify?
- Are non-goals explicit enough to prevent scope creep?
- Are alternatives represented fairly?
- Does the decision fit existing architecture and vocabulary?
- What can fail, and how will the user or operator see it?
- Does the testing strategy cover the highest-risk behavior?
- Is the rollout, migration, or rollback path clear?
- Are new dependencies justified and maintainable?

## Approval Bar

An RFC can move to `Accepted` when:

- Major tradeoffs are documented.
- Open questions are either resolved or explicitly deferred.
- The testing strategy is credible.
- The implementation plan can be reviewed in small steps.
- The design does not conflict with project principles in `CONTRIBUTING.md`.

## Changes During Implementation

If implementation invalidates the accepted design:

1. Stop broad implementation work.
2. Update the RFC with the new facts.
3. Re-review the changed decision.
4. Continue only after the RFC still supports the implementation.
