# Quality Gates

## State Engine Accuracy

| Metric | Dev Gate | Beta Gate | Release Gate |
|--------|----------|-----------|--------------|
| activity_weighted_f1 | >= 0.88 | >= 0.92 | >= 0.95 |
| running_precision | >= 0.92 | >= 0.95 | >= 0.97 |
| waiting_input_recall | >= 0.75 | >= 0.85 | >= 0.90 |
| waiting_approval_recall | >= 0.70 | >= 0.82 | >= 0.90 |

## Attention Accuracy

| Metric | Dev Gate | Beta Gate | Release Gate |
|--------|----------|-----------|--------------|
| attention_precision | >= 0.78 | >= 0.85 | >= 0.90 |
| attention_recall | >= 0.70 | >= 0.80 | >= 0.85 |
| false_positive_rate | <= 0.20 | <= 0.14 | <= 0.10 |

## Performance

| Metric | Target |
|--------|--------|
| State update latency (state change → CLI display) | < 3s (p95) |
| Daemon memory usage | < 50MB |
| Daemon CPU usage (idle, 10 panes) | < 2% |
| `agtmux status` response time | < 500ms |
| tmux-status output time | < 200ms |
