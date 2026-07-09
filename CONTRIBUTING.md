# Contributing to avz

This document defines the working rules for avz. Keep it current as
the project learns.

## Git Identity

All commits for this project should use:

```text
Name:  Illya Yalovyy
Email: yalovoy@gmail.com
```

Before committing, verify:

```bash
git config user.name
git config user.email
```

Do not commit with a corporate, noreply, or unrelated identity unless the
project explicitly changes this rule.

## Sensitive Data

Never commit:

- API keys, tokens, cookies, private certificates, or credentials
- Personal filesystem paths, usernames, hostnames, or machine-specific state
- Corporate email addresses, internal URLs, VPN names, or private hostnames
- Real user data, production database dumps, or unredacted logs

If a secret is accidentally committed, treat it as compromised and rotate it.

## AI / Agent Files

The following must stay ignored and must not be committed:

```text
.claude/
.claude_*
.kiro/
.aim/
.cursor/
.copilot/
.aider*
.codex/
.tabnine/
.continue/
.windsurf/
*.prompt
*.prompt.md
.chat_history/
.ai/
.agent/
```

## Design Process

An RFC is required for:

- Changes touching more than three files or introducing new abstractions
- New external dependencies
- Data model, persistence, API, protocol, or auth changes
- User-visible behavior that changes established workflows
- Architectural decisions that are expensive to reverse

Use `designs/RFC-000-template.md`. Do not begin implementation until the RFC is
accepted, except for small discovery spikes that are explicitly throwaway.

## Code Standards

- Prefer simple, local changes over broad refactors.
- Preserve existing style unless changing it is part of the accepted work.
- Keep boundaries clear: domain logic should not leak into UI, transport, or
  persistence glue without a deliberate reason.
- Dependencies need a documented purpose, maintenance status, and test impact.
- Avoid hidden global state. If unavoidable, document lifecycle and test impact.

## Testing Requirements

Every behavior change needs regression coverage unless the RFC or PR explains why
that is impractical.

Choose the layer based on the risk:

- Unit tests for pure logic and invariants
- Integration tests for persistence, API, process, or module boundaries
- UI / behavioral tests for user workflows, layout, keyboard behavior, and
  accessibility-visible state
- Property or fuzz tests for parsers, protocols, state machines, and tree-like
  data structures

Run before review:

```bash
./scripts/quality.sh
```

## Review Practice

Reviews should prioritize:

1. Correctness and user-visible behavior
2. Regression coverage at the right layer
3. Security, privacy, and secret handling
4. Maintainability and architectural fit
5. Naming, formatting, and polish

See `docs/REVIEW.md` for the checklist.
Use `docs/DESIGN-REVIEW.md` for RFC review before implementation.

## Commit Practice

Follow `docs/COMMITS.md`. Keep commits coherent, inspect staged diffs before
committing, and do not mix unrelated refactors with behavior changes.

## Push Policy

Do not push to a remote unless explicitly instructed or unless this project has
adopted a different team policy.
