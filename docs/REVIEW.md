# Review Checklist

Use this checklist for self-review and pull request review.

## Correctness

- Does the implementation match the user task, issue, or accepted RFC?
- Are edge cases and failure paths handled explicitly?
- Are migrations, compatibility, and rollback behavior considered?
- Are errors actionable and specific enough to debug?

## Tests

- Is there regression coverage for the behavior being changed?
- Is the test at the right layer?
- Would the test fail without the implementation?
- Are slow, flaky, or environment-dependent tests isolated and documented?

## Security and Privacy

- No secrets, local paths, real user data, or private hostnames are committed.
- Inputs from users, files, networks, and subprocesses are validated.
- Logs avoid sensitive values.
- Auth, token, and credential behavior is covered where relevant.

## Maintainability

- Does the change fit existing module boundaries?
- Is new abstraction justified by current complexity?
- Are dependencies necessary, maintained, and documented?
- Is naming consistent with product and architecture vocabulary?

## User Experience

- Does the behavior work from the user's point of view?
- Are loading, empty, error, and disabled states covered where applicable?
- Does keyboard behavior remain predictable?
- Are accessibility-visible names and states preserved?
