---
name: review-change
description: Review an active diff or PR against this repo's lightweight workflow. Use before merge to check requirement drift, boundary damage, test coverage, promotion gaps, and whether a change pack should be removed.
argument-hint: "[issue-id-or-branch]"
disable-model-invocation: true
---

Review a change with the repo's durable-docs rules in mind.

Process:

1. Read the diff, touched tests, and the active `changes/<id>/` pack if one exists.
2. Check:
   - scope still matches `requirements.md`
   - `design.md` reflects boundary or contract changes
   - tests prove behavior changes
   - durable knowledge is not trapped in `changes/`
   - the change pack can be removed after merge
3. Report findings first:
   - bugs or regressions
   - requirement drift or missing tests
   - missing ADR/runbook/research/comment promotion
4. End with a short merge-readiness summary.
