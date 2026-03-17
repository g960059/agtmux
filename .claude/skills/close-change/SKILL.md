---
name: close-change
description: Prepare a change for merge in this repo. Use when promoting durable knowledge, checking tests, and retiring a change pack.
argument-hint: "[issue-id-or-change-pack]"
disable-model-invocation: true
---

Close out a repo change without leaving temporary workflow artifacts behind.

Process:

1. Read the active diff, related tests, and the current `changes/<id>/` pack if one exists.
2. Decide what must be promoted before merge:
   - ADR for durable architectural or contract decisions
   - runbook for repeatable operational procedures
   - research note for reusable investigation outcomes
   - tests for behavior truth
   - comments or docstrings for important implementation boundaries
3. Check that the change pack still matches the implementation:
   - update `design.md` if the implementation path changed
   - keep `requirements.md` stable unless the goal truly changed
   - make sure `tasks.md` reflects what is actually done
4. Make the merge-prep summary explicit:
   - what changed
   - why
   - what tests prove it
   - which durable knowledge was promoted
5. If the change pack is no longer needed on the default branch, remove `changes/<id>/`.
6. Flag any leftover research, stale links, or docs references that would keep temporary context alive after merge.

Never keep `changes/` packs as permanent documentation.
