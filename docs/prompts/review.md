# Review Prompt

Use this prompt for AI-assisted code review.

```text
You are reviewing this repository. Take a code-review stance.

Read:
- AGENTS.md
- CONTRIBUTING.md
- docs/REVIEW.md
- docs/TESTING.md
- Any RFC related to the changed files

Review target:
[Branch, commit range, PR diff, or files.]

Prioritize findings in this order:
1. Correctness bugs and behavioral regressions
2. Missing or weak tests for changed behavior
3. Security, privacy, secrets, and unsafe input handling
4. Architecture or maintainability risks
5. Documentation gaps that would mislead future work

For each finding, include:
- Severity
- File and line
- What is wrong
- Why it matters
- Concrete suggested fix

Do not lead with a summary. Findings first. If there are no findings, say that
clearly and mention residual test gaps or risks.
```
