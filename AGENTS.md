# AGENTS

Read in order: `README.md`, `docs/README.md`, `docs/product/`, `docs/decisions/`, `docs/runbooks/`, then the active GitHub Issue/PR.

Source of truth is code, tests, schemas, CI, and ADRs. Treat dated research, active `changes/`, and `docs/archive/` as working notes or history, not permanent truth.

Use `changes/<issue-id>-slug/` only for active multi-step work. Keep `requirements.md`, `design.md`, `plan.md`, and `tasks.md` short.

Prefer small, reversible diffs. Update tests when behavior changes.

Before merge, promote durable knowledge to `docs/decisions/`, `docs/runbooks/`, tests, or code comments. After merge, remove the change pack from the default branch.
