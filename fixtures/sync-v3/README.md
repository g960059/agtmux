# sync-v3 Fixtures

Canonical daemon-owned `ui.bootstrap.v3` examples for the v3 rollout.

Rules for these fixtures:

- Parse them as the bootstrap wire contract, not as ad hoc test blobs.
- Treat them as source-of-truth for consumer decode tests.
- Keep exact identity strict:
  - `session_name`
  - `window_id`
  - `session_key`
  - `pane_id`
  - `pane_instance_id`
- `binding_epoch_id` is required for managed rows and absent for unmanaged rows.
- `runtime_ref` is optional and appears only when the daemon knows a
  provider-native runtime ID.
- `attention` is derived summary only.
- `pending_requests[].request_id` is the request identity truth.
- `provider_raw` is opaque/debuggable and may evolve additively.

Scenario files:

- `codex-running.json`
- `codex-waiting-approval.json`
- `codex-completed-idle.json`
- `claude-approval.json`
- `claude-stop-idle.json`
- `unmanaged-demotion.json`
- `error.json`
- `freshness-degraded.json`

The contract freeze is documented in:

- `docs/decisions/ADR-20260309-sync-v3-contract-freeze.md`
- `docs/decisions/ADR-20260318-sync-v3-binding-epoch-extension.md`
