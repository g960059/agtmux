---
name: research-note
description: Write a dated research note for this repo. Use for comparisons, investigations, and analysis that should stay non-authoritative.
argument-hint: "[topic-or-slug]"
disable-model-invocation: true
---

Write research as a dated note, not as product truth.

Process:

1. Create `docs/research/YYYY-MM-DD-<slug>.md`.
2. Keep the note short and decision-oriented.
3. Include:
   - question or trigger
   - key findings
   - concrete conclusion
   - implications for this repo
4. State that the note is dated and non-authoritative.
5. Do not edit `docs/product/` or `docs/decisions/` directly from research alone.
6. If the investigation leads to a durable decision, propose or update an ADR separately.

Use research for exploration only. Keep current truth in code, tests, CI, runbooks, and ADRs.
