---
name: workflow-policy
description: Background repo workflow for choosing the lightest process that fits. Use when classifying work as small, standard, structural, or research, and when deciding whether to create or retire a change pack.
user-invocable: false
---

Keep the default branch focused on durable docs and behavior truth.

Choose the lightest track that fits:

1. `small`: no `changes/`; issue + branch + PR is enough — typo, narrow bug fix, test fix, small rename
2. `standard`: short `changes/<issue-id>-slug/` — one feature or multi-file behavior change
3. `structural`: short `changes/<issue-id>-slug/` plus ADR when long-lived boundaries or contracts change; prefer stacked PRs — boundary refactor, shared abstraction, persistence/state redesign
4. `research`: dated `docs/research/YYYY-MM-DD-*.md`, non-authoritative, promote adopted decisions to ADRs — comparison, investigation, benchmark, vendor or design study

Always prefer tests over prose for behavior. Touch `docs/product/` only when global direction changes. Remove `changes/` from the default branch after merge.

Failure modes to avoid:
- leaving `changes/` on the default branch
- putting feature-local detail into `docs/product/`
- treating research as current truth
- writing ADRs for every minor decision
- shipping large, all-at-once PRs
