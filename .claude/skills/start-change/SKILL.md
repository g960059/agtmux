---
name: start-change
description: Start work for an Issue or PR in this repo. Use when deciding which workflow track applies and whether to create a change pack.
argument-hint: "[issue-id] [slug]"
disable-model-invocation: true
---

Start a repo change using the lightest workflow that still fits the task.

Arguments:

- `$0`: optional issue number
- `$1`: optional slug

Process:

1. Read the active Issue or PR plus `README.md`, `docs/product/`, `docs/decisions/`, and `docs/runbooks/`.
2. Classify the work using the repo workflow policy:
   - small
   - standard
   - structural
   - research
3. Apply the matching workflow:
   - small: do not create `changes/`; state that the work will proceed directly on the issue + branch + PR flow
   - standard: create `changes/<issue-id>-slug/` using the templates in `templates/`
   - structural: create `changes/<issue-id>-slug/`, check whether an ADR is needed before implementation, and prefer stacked PRs
   - research: create a dated note in `docs/research/` and do not create a `changes/` pack unless implementation starts
4. If the work is standard or structural and there is no issue id yet, recommend creating an issue first. If proceeding without one, use a temporary slug and note that the folder must be renamed once an issue is created.
5. Keep every change-pack file short and specific to the current diff.
6. Do not write finished product docs here. This is scaffolding only.
7. End with a concise summary:
   - chosen track
   - whether `changes/` was created
   - whether ADR/research is needed
   - what the first implementation step should be

Template files:

- [templates/requirements.md](templates/requirements.md)
- [templates/design.md](templates/design.md)
- [templates/plan.md](templates/plan.md)
- [templates/tasks.md](templates/tasks.md)
- [templates/README.md](templates/README.md)
