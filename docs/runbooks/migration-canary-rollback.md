# Migration / Canary / Rollback Runbook

Task ref: T-070 | User story: US-005 | Spec: FR-038, FR-039, FR-040

## Overview

This runbook covers the v4 → v5 migration process using a phased
canary rollout with instant rollback capability. The runtime selector
allows switching between v4 and v5 daemon paths without data loss.

---

## Prerequisites

| Item | Check |
|------|-------|
| v5 daemon binary built and deployed alongside v4 | `agtmux-daemon-v5 --version` |
| Supervisor startup order verified | source → gateway → daemon → UI |
| Snapshot infrastructure operational | `SnapshotManager` periodic + shutdown enabled |
| Restore dry-run passing | `RestoreDryRun::check()` returns `Ok` |
| All 6 crates pass `just verify` | fmt + clippy(strict) + 456+ tests |
| Backup taken before migration start | See [backup/restore runbook](./backup-restore.md) |

---

## Phase 1: Internal Alpha (v4 + v5 parallel)

### Steps

1. **Deploy v5 daemon alongside v4** (separate UDS socket path).
   ```
   v4: /tmp/agtmux-v4.sock
   v5: /tmp/agtmux-v5.sock
   ```

2. **Configure runtime selector** to route all sessions to v4 (default).
   ```toml
   [runtime]
   default_daemon = "v4"
   v5_enabled = false
   ```

3. **Start v5 daemon** in shadow mode (receives events, builds projection, but UI reads from v4).
   - Supervisor enforces startup order: Sources → Gateway → Daemon → UI.
   - DependencyGate must show `all_ready()` before each stage proceeds.

4. **Compare projections** between v4 and v5:
   - `list_panes` output parity check.
   - `list_sessions` output parity check.
   - Signature fields (`evidence_mode`, `pane_presence`) consistency.

5. **Monitor for 24h minimum**:
   - No supervisor hold-down events.
   - No alert routing escalations.
   - Projection version advancing normally.

### Exit Criteria
- [ ] v5 projection matches v4 for all active sessions.
- [ ] Zero supervisor HoldDown events in 24h.
- [ ] Snapshot dry-run passes at current projection version.

---

## Phase 2: Canary (limited users on v5)

### Pre-Canary Checklist

| # | Check | Evidence |
|---|-------|----------|
| 1 | Snapshot taken at current state | `snap-{id}` metadata recorded |
| 2 | Restore dry-run passes | `RestoreVerdict::Ok { age_ms }` |
| 3 | Phase 1 exit criteria met | Sign-off from Phase 1 |
| 4 | Rollback procedure tested (dry-run) | See [Rollback](#rollback-procedure) |

### Steps

1. **Enable v5 for canary sessions**:
   ```toml
   [runtime]
   default_daemon = "v4"
   v5_enabled = true
   v5_canary_sessions = ["session-alpha", "session-beta"]
   ```

2. **Monitor canary sessions**:
   - Source health: all sources `Active` (no `Stale`/`Revoked`).
   - Latency SLO: rolling p95 within threshold, no `Degraded` alerts.
   - Binding projection: CAS conflicts count = 0 (normal operation).
   - Cursor: no `invalid_cursor` streaks, watermarks advancing.

3. **Canary duration**: minimum 48h with active usage.

4. **Expand canary** if stable:
   - Add 2-3 more sessions per day.
   - Continue monitoring at each expansion.

### Exit Criteria
- [ ] Canary sessions stable for 48h+.
- [ ] No alert severity >= `Degraded`.
- [ ] All canary users confirm expected behavior.

---

## Phase 3: Gradual Default (new sessions on v5)

### Steps

1. **Switch default to v5** for new sessions:
   ```toml
   [runtime]
   default_daemon = "v5"
   v5_enabled = true
   ```

2. Existing v4 sessions continue on v4 until restart.

3. **Monitor migration rate**: track v4 vs v5 session counts daily.

4. **Keep v4 daemon running** as rollback target.

### Exit Criteria
- [ ] All new sessions running on v5 for 7+ days.
- [ ] No regression in supervisor restart counts.

---

## Phase 4: Full Cutover

### Steps

1. **Migrate remaining v4 sessions**: restart sessions to pick up v5.

2. **Verify**: zero v4 sessions remaining.

3. **Take final snapshot** before decommissioning v4 path.

4. **Decommission v4 daemon** (keep binary available for emergency rollback for 30 days).

### Exit Criteria
- [ ] Zero active v4 sessions.
- [ ] v5-only operation stable for 7 days.
- [ ] v4 binary archived (not deleted) for 30-day rollback window.

---

## Rollback Procedure

### Instant Rollback (any phase)

The runtime selector enables instant rollback without data loss.

1. **Switch runtime selector back to v4**:
   ```toml
   [runtime]
   default_daemon = "v4"
   v5_enabled = false
   ```

2. **Restart affected sessions** (v5 sessions will reconnect to v4).

3. **Verify v4 daemon is serving**:
   - `list_panes` returns expected data.
   - Source health shows `Active`.

4. **Stop v5 daemon** (optional — can leave running in shadow for debugging).

### When to Rollback

| Signal | Action |
|--------|--------|
| Supervisor enters HoldDown and doesn't recover | Rollback immediately |
| Alert severity `Escalate` persists > 5 min | Rollback immediately |
| Latency p95 breach > 3 consecutive windows | Rollback, investigate |
| Cursor `FullResync` triggered on canary | Investigate, consider rollback |
| User-reported data inconsistency | Rollback, investigate |

### Post-Rollback

1. Take a v5 snapshot before stopping (for debugging).
2. File incident report with:
   - Timeline of events.
   - Alert ledger dump.
   - Supervisor state at rollback time.
3. Do NOT retry canary without root cause analysis.

---

## Supervisor Contract Reference

| Parameter | Value | Source |
|-----------|-------|--------|
| Startup order | Sources → Gateway → Daemon → UI | ADR-20260225 |
| Backoff initial | 1,000 ms | RestartPolicy::default() |
| Backoff multiplier | 2.0x | RestartPolicy::default() |
| Backoff max | 30,000 ms | RestartPolicy::default() |
| Jitter | ±20% (applied by runtime) | RestartPolicy::default() |
| Failure budget | 5 failures / 10 min | RestartPolicy::default() |
| Hold-down | 300,000 ms (5 min) | RestartPolicy::default() |

---

## Canary Gate Checklist

Final sign-off before full cutover:

- [ ] Phase 1 exit criteria met (signed)
- [ ] Phase 2 exit criteria met (signed)
- [ ] Phase 3 exit criteria met (signed)
- [ ] Restore dry-run evidence retained
- [ ] Rollback procedure tested (dry-run)
- [ ] No unresolved `Escalate` alerts in 7 days
- [ ] Operator on-call briefed on rollback procedure
