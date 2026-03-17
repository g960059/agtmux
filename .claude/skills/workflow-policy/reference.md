# Workflow Reference

## Classification

- `small`: typo, narrow bug fix, test fix, small rename
- `standard`: one feature or multi-file behavior change
- `structural`: boundary refactor, shared abstraction, persistence/state redesign
- `research`: comparison, investigation, benchmark, vendor or design study

## Default Flow

1. Triage into Discussion, Issue, or research.
2. Decide whether the work is small, standard, structural, or research.
3. Only create `changes/<issue-id>-slug/` for standard or structural work.
4. Keep `requirements.md`, `design.md`, `plan.md`, `tasks.md`, and `README.md` short.
5. Implement in small PRs; prefer stacked PRs for structural work.
6. Before merge, promote durable knowledge to ADRs, runbooks, tests, or code comments.
7. After merge, remove the change pack from the default branch.

## Durable Docs

- `docs/product/`: stable goals, constraints, architecture, non-goals
- `docs/decisions/`: ADRs for lasting decisions and contracts
- `docs/runbooks/`: repeatable operational steps
- `docs/research/`: dated, non-authoritative notes that can be pruned

## Agent Split

- orchestrator: read stable docs plus the active Issue, PR, and change pack, then choose the workflow track
- implementation: read related code, tests, and the active change pack only
- reviewer: read the diff, `requirements.md`, `design.md`, and tests
- research: write dated notes in `docs/research/` and avoid editing product or decision docs directly

## Cadence

- every PR: link the issue, update tests, check promotion needs, retire `changes/` if used
- weekly: clean stale issues and dangling change packs
- monthly: prune `docs/research/`, review `docs/product/` drift, add ADR index help if needed

## Failure Modes

- leaving `changes/` on the default branch
- putting feature-local detail into `docs/product/`
- treating research as current truth
- writing ADRs for every minor decision
- shipping large, all-at-once PRs
