# ADR 20260318: sync-v3 Binding Epoch Additive Extension

## Status
- Accepted

## Context
- `ADR-20260309` froze `sync-v3` as the daemon-owned canonical semantic truth.
- The frozen contract still lacks a first-class occupancy discriminator for
  same-pane runtime changes when visible pane identity stays constant.
- `agtmux-term` currently treats `sync-v3` as required metadata and its v3
  change decoder rejects unknown `field_groups`, so adding a new enum value
  would be a breaking change.

## Decision
- Extend `ui.bootstrap.v3` and `ui.changes.v3` additively with:
  - `binding_epoch_id?: string`
  - `runtime_ref?: { provider, native_id }`
- Validation rules:
  - `presence = managed` requires a non-empty `binding_epoch_id`
  - `presence != managed` must not emit `binding_epoch_id` or `runtime_ref`
  - `runtime_ref.provider` must match the pane `provider`
  - `runtime_ref.native_id` must be non-empty
- `session_key` remains the existing link / dedup / metadata key for this slice.
- Epoch rotation is daemon-owned and occurs when:
  - a pane becomes managed from unmanaged
  - the pane instance changes
  - the top-level provider changes
  - the provider-native runtime ID changes
- Epoch rotation does **not** occur for freshness-only, attention-only,
  pending-request-only, execution-only, or `provider_raw` changes.
- `SyncV3FieldGroupV3` is unchanged in this slice.
  Binding/runtime updates are emitted under the existing `provider` field group
  so current `agtmux-term` consumers remain compatible.
- `runtime_ref` is optional and must not be guessed when a source does not have
  a stable provider-native runtime ID.

## Consequences
- Positive:
  - managed row consumers can distinguish occupancy changes without a new wire
    version
  - current term decoders keep working because version and field group enums
    stay stable
  - the daemon remains the only canonical semantic reducer
- Negative / risks:
  - `binding_epoch_id` is daemon-local state and may be reissued on full daemon
    restart or resync
  - heuristic-only rows still cannot expose a provider-native runtime identity

## Alternatives
- A: add a parallel `binding.v1` wire contract
  - Rejected because it creates dual truth systems beside the just-frozen
    `sync-v3` contract.
- B: add a new `SyncV3FieldGroupV3::Binding`
  - Rejected because current `agtmux-term` decoders would fail on an unknown
    change enum.
- C: move canonical reduction into `agtmux-term`
  - Rejected because it reverses the `ADR-20260309` daemon-owned truth
    decision.

## Links
- Related docs:
  - `docs/decisions/ADR-20260309-sync-v3-contract-freeze.md`
  - `fixtures/sync-v3/README.md`
