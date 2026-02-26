# Review Pack: T-071 Backup / Restore Runbook

## Objective
- Task: T-071
- User story: US-005
- Acceptance criteria: FR-038 (snapshot/restore cadence, dry-run mandatory)

## Summary (3-7 lines)
- Created `docs/runbooks/backup-restore.md` covering snapshot cadence, restore procedures, and failure escalation.
- Documents automatic snapshot triggers (Periodic/Shutdown/Manual) and retention policy.
- Step-by-step restore procedure with mandatory dry-run validation gate.
- Cursor recovery hierarchy: committed watermark → safe rewind → full resync.
- Failure escalation matrix covering TooOld, VersionAhead, cursor issues, and source health recovery.
- Alert routing severity guide during restore operations.
- Maintenance schedule for ongoing operational readiness.

## Change scope
- `docs/runbooks/backup-restore.md` (NEW)
- `docs/85_reviews/RP-T071-backup-restore.md` (NEW, this file)

## Verification evidence
- Snapshot cadence matches `SnapshotPolicy` defaults (interval=15m, retention=3, max_age=10m)
- Restore dry-run procedure matches `RestoreDryRun::check()` API (Ok/TooOld/VersionAhead)
- Cursor recovery parameters match `CursorWatermarks` and `InvalidCursorTracker` implementations (T-041)
- Source registry lifecycle matches `SourceRegistry` 4-state FSM (T-048)
- Alert severity levels match `AlertSeverity` enum ordering (T-051)
- Cross-referenced against ADR-20260225-operational-guards.md cursor numeric contract

## Risk declaration
- Breaking change: no (documentation only)
- Fallbacks: N/A (operator procedure)
- Known gaps: Actual CLI commands for operator actions TBD (depends on runtime CLI implementation)

## Reviewer request
- Provide verdict: GO / GO_WITH_CONDITIONS / NO_GO / NEED_INFO
