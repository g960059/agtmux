---
name: adr
description: Write a new ADR for this repo. Use when a durable architectural or contract decision needs to be recorded.
argument-hint: "[slug]"
disable-model-invocation: true
---

Write an ADR only for decisions that are hard to reverse or that define long-lived boundaries.

Process:

1. Find the highest existing ADR number in `docs/decisions/` and use the next one.
2. Create `docs/decisions/ADR-NNNN-<slug>.md`.
3. Keep the ADR short. Include only:
   - **Context**: why this decision is being made now
   - **Decision**: what was decided
   - **Consequences**: what becomes easier, what becomes harder
4. Do not record minor or easily reversible choices as ADRs.
5. Link the ADR from the active `changes/<id>/README.md` if a change pack exists.
6. Do not restate implementation details that are already clear from code or tests.
