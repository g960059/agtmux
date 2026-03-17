# Poller Quality Gate

## Purpose

- Keep heuristic fallback quality explicit and reproducible before release.

## Acceptance

- `weighted F1 >= 0.85`
- `waiting recall >= 0.85`

Both thresholds must pass on the same run.

## Dataset

- Use the fixed labeled fixture set in `fixtures/poller-baseline/dataset.json`.
- Keep the benchmark stable across runs; refresh it only as an intentional new evaluation cycle.

## Command

```bash
just poller-gate
```

## Notes

- Treat this gate as release-blocking when poller detection logic changes.
- Prefer tests and fixture truth over prose when the two diverge.
