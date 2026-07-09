# RFC Prompt

Use this prompt when asking an AI assistant to draft or revise an RFC.

```text
You are drafting an RFC for this repository. Read AGENTS.md, CONTRIBUTING.md,
docs/PROCESS.md, docs/DESIGN-REVIEW.md, and designs/RFC-000-template.md first.

Goal:
Draft an RFC for:
[Describe the change.]

Context:
[Link or describe existing code paths, user symptoms, issues, prior decisions,
or operational constraints.]

Requirements:
- Use the RFC template exactly.
- State goals and non-goals clearly.
- Present at least two real options unless only one is viable.
- Make tradeoffs explicit.
- Include user impact.
- Include testing strategy mapped to risks.
- Include a development plan that can be implemented in small reviewable steps.
- Mark unknowns as open questions instead of inventing certainty.

Do not implement the RFC yet. Stop after creating or updating the RFC and
summarizing the decision points that need review.
```
