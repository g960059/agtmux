---
name: workflow-policy
description: Background repo workflow for choosing the lightest process that fits. Use when classifying work as small, standard, structural, or research, and when deciding whether to create or retire a change pack.
user-invocable: false
---

Keep the default branch focused on durable docs and behavior truth.

Choose the lightest track that fits:

1. `small`: no `changes/`; issue + branch + PR is enough
2. `standard`: short `changes/<issue-id>-slug/`
3. `structural`: short `changes/<issue-id>-slug/` plus ADR when long-lived boundaries or contracts change; prefer stacked PRs
4. `research`: dated `docs/research/YYYY-MM-DD-*.md`, non-authoritative, promote adopted decisions to ADRs

Always prefer tests over prose for behavior. Touch `docs/product/` only when global direction changes. Remove `changes/` from the default branch after merge.

See [reference.md](reference.md) for the detailed flow, cadence, and anti-patterns.
