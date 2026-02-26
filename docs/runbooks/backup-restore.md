# Backup / Restore Runbook

Task ref: T-071 | User story: US-005 | Spec: FR-038

## Overview

This runbook documents snapshot cadence, restore procedures, and failure
escalation for the agtmux v5 persistent state. The snapshot/restore
infrastructure is implemented in `agtmux-daemon-v5::snapshot` (T-049).

---

## Snapshot Cadence

### Automatic Snapshots

| Trigger | Interval | Description |
|---------|----------|-------------|
| `Periodic` | Every 15 min (configurable via `SnapshotPolicy.interval_ms`) | Routine checkpoint |
| `Shutdown` | On clean daemon shutdown | Captures final state before exit |
| `Manual` | Operator-initiated | On-demand before risky operations |

### Retention Policy

| Parameter | Default | Description |
|-----------|---------|-------------|
| `max_retained` | 3 | Maximum snapshots kept (oldest pruned first) |
| `max_age_ms` | 600,000 (10 min) | Restore rejects snapshots older than this |
| `interval_ms` | 900,000 (15 min) | Periodic snapshot interval |

### Snapshot Metadata

Each snapshot records:
- `snapshot_id`: Monotonically increasing ID (`snap-1`, `snap-2`, ...).
- `created_at_ms`: Epoch milliseconds at creation.
- `projection_version`: Daemon projection version at snapshot time.
- `session_count` / `pane_count`: State dimensions for quick validation.
- `trigger`: Why the snapshot was taken.
- `size_bytes`: Serialized state size.

### Persisted State Covered

| Table | Key | Contents |
|-------|-----|----------|
| `session_state_v2` | `session_key` | Session metadata, title, priority |
| `pane_state_v2` | `pane_instance_id` | Pane state, signature, binding |
| `binding_link_v2` | `pane_instance_id, bound_at` | Binding relationships |
| `cursor_state_v2` | `source_kind` | Committed cursor position |
| `source_health_v2` | `source_kind` | Source FSM state |
| `state_snapshot_v2` | `snapshot_id` | Snapshot metadata itself |

---

## Restore Procedure

### Pre-Restore Checklist

| # | Check | How |
|---|-------|-----|
| 1 | Identify target snapshot | `SnapshotManager::list()` — pick latest or specific ID |
| 2 | Verify snapshot age | `age_ms <= max_age_ms` (default 10 min) |
| 3 | Run restore dry-run | `RestoreDryRun::check()` — must return `Ok` |
| 4 | Stop daemon (clean shutdown) | Triggers `Shutdown` snapshot automatically |
| 5 | Notify operators | Alert: "restore in progress" |

### Step-by-Step Restore

1. **Select snapshot**:
   ```
   Target: snap-{id}
   Created: {created_at_ms}
   Version: {projection_version}
   Sessions: {session_count}, Panes: {pane_count}
   Size: {size_bytes} bytes
   ```

2. **Run dry-run validation**:
   ```rust
   let checker = RestoreDryRun::new(snapshot_meta, current_time_ms);
   let verdict = checker.check(current_projection_version, max_age_ms);
   ```

   Expected verdicts:

   | Verdict | Action |
   |---------|--------|
   | `Ok { age_ms }` | Proceed with restore |
   | `TooOld { age_ms, max_age_ms }` | Pick a newer snapshot or override with operator approval |
   | `VersionAhead { snapshot_version, current_version }` | **DO NOT RESTORE** — data corruption risk. Investigate. |

3. **Execute restore** (after `Ok` verdict):
   - Replace current state tables with snapshot contents.
   - Reset projection version to snapshot's `projection_version`.
   - Clear in-flight event buffers.

4. **Restart daemon**:
   - Supervisor startup order: Sources → Gateway → Daemon → UI.
   - DependencyGate enforces readiness at each stage.
   - Daemon replays events from `projection_version` forward.

5. **Post-restore verification**:
   - `list_panes` returns expected pane count.
   - `list_sessions` returns expected session count.
   - Cursor watermarks advancing (fetched > committed).
   - No `invalid_cursor` events in first 60s.
   - Source health: all sources `Active`.

---

## Cursor Recovery

When cursor state is inconsistent after restore, the cursor hardening
system (T-041) provides automatic recovery.

### Recovery Hierarchy

| Condition | Action | Reference |
|-----------|--------|-----------|
| Cursor behind (normal) | Continue from committed watermark | `CursorWatermarks::commit()` |
| Cursor slightly ahead | Safe rewind to committed | `CursorWatermarks::safe_rewind()` |
| Rewind limit exceeded | Full resync from zero | Limit: min(10 min, 10,000 events) |
| Invalid cursor (single) | Retry from committed watermark | `InvalidCursorTracker` |
| Invalid cursor streak (3+ in 60s) | Force full resync + warning | `RecoveryAction::FullResync` |

### Cursor Checkpoint Policy

| Parameter | Value |
|-----------|-------|
| Checkpoint interval | 30s or 500 events (whichever first) |
| Safe rewind limit | min(10 min, 10,000 events) |
| Dedup retention | rewind_window + 120s (≥ 12 min) |
| Invalid cursor resync threshold | 3 occurrences in 60s |

---

## Failure Escalation

### Restore Failures

| Failure | Escalation |
|---------|------------|
| Dry-run returns `TooOld` | Try next-newest snapshot. If all expired, investigate snapshot scheduling. |
| Dry-run returns `VersionAhead` | **Critical**: Do not restore. Snapshot was taken from a future state (possible corruption). File incident. |
| Restore succeeds but cursor can't resync | Wait for `InvalidCursorTracker` auto-recovery (up to 3 retries). If `FullResync` triggered, monitor for 5 min. |
| Restore succeeds but sources show `Stale` | Source registry auto-recovery: `stale` → `pending` on next `source.hello`. If stuck, manual operator `revoke` + re-register. |
| Post-restore supervisor enters HoldDown | Budget exhausted during recovery. Wait 5 min for hold-down to expire. If repeated, investigate root cause. |

### Alert Routing During Restore

| Severity | Trigger | Action |
|----------|---------|--------|
| `Info` | Restore started / completed | Log only |
| `Warn` | Cursor rewind triggered | Monitor, expect auto-recovery |
| `Degraded` | Multiple cursor resyncs or source health drops | Extend monitoring window to 15 min |
| `Escalate` | Supervisor HoldDown or VersionAhead detected | Page operator. Consider rollback to v4. |

---

## Operator Commands

### Take Manual Snapshot
```
Trigger: SnapshotTrigger::Manual
Use: Before risky operations (migration, config changes, debugging)
```

### List Snapshots
```
SnapshotManager::list() → Vec<SnapshotMetadata>
Shows: id, created_at, version, session/pane counts, trigger, size
```

### Prune Old Snapshots
```
SnapshotManager::prune() → usize (removed count)
Keeps newest max_retained (default 3)
```

### Check Restore Safety
```
RestoreDryRun::new(snapshot, now_ms).check(current_version, max_age_ms)
→ Ok / TooOld / VersionAhead
```

---

## Maintenance Schedule

| Task | Frequency | Responsible |
|------|-----------|-------------|
| Verify periodic snapshots running | Daily (automated check) | Monitoring |
| Review snapshot sizes for growth | Weekly | Operator |
| Test restore dry-run | Before each canary / migration | Operator |
| Full restore drill | Monthly | Operator |
| Review/update this runbook | Quarterly | Team |
