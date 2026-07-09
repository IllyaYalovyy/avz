# Template Rationale

This template distills working practices from two active AI-assisted projects:
`axiotask` and `rttx`.

## Practices Carried Forward

- **Explicit contributor rules** - identity, secrets, agent files, and push policy
  are documented at the top level instead of relying on memory.
- **RFC-driven design** - broad or hard-to-reverse changes require a written
  decision record before implementation.
- **User-task inventory** - user workflows are written as preconditions, flows,
  outcomes, and expected regression coverage.
- **Risk-based testing** - tests are selected by the failure mode they protect,
  not by a raw coverage target.
- **Local quality gate** - one command should run the checks expected before
  review, with project-specific hooks added as the stack becomes concrete.
- **Review discipline** - reviews lead with correctness, tests, privacy, and
  maintainability before style details.
- **Agent hygiene** - AI working files stay out of version control, and agent
  handoffs must state changes, checks, skipped checks, and residual risks.

## Practices Kept Generic

The reference projects are Rust desktop applications, but this template is
stack-neutral. Rust, Node, Python, shell, and custom hooks are detected by
`scripts/quality.sh`; project-specific CI and packaging gates should be added
once the new repository chooses its technology.
