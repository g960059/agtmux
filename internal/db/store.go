package db

import (
	"context"
	"database/sql"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
	"time"

	_ "modernc.org/sqlite"

	"github.com/g960059/agtmux/internal/model"
)

var (
	ErrDuplicate  = errors.New("duplicate")
	ErrNotFound   = errors.New("not found")
	ErrOutOfOrder = errors.New("out of order")
)

type Store struct {
	db *sql.DB
}

func Open(ctx context.Context, path string) (*Store, error) {
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return nil, fmt.Errorf("create db dir: %w", err)
	}
	dsn := fmt.Sprintf("file:%s?_pragma=journal_mode(WAL)&_pragma=busy_timeout(5000)&_pragma=foreign_keys(1)", path)
	db, err := sql.Open("sqlite", dsn)
	if err != nil {
		return nil, fmt.Errorf("open sqlite: %w", err)
	}
	db.SetMaxOpenConns(1)
	if err := db.PingContext(ctx); err != nil {
		return nil, fmt.Errorf("ping sqlite: %w", err)
	}
	if err := os.Chmod(path, 0o600); err != nil && !errors.Is(err, os.ErrNotExist) {
		return nil, fmt.Errorf("chmod db path: %w", err)
	}
	return &Store{db: db}, nil
}

func (s *Store) Close() error {
	if s == nil || s.db == nil {
		return nil
	}
	return s.db.Close()
}

func (s *Store) DB() *sql.DB {
	return s.db
}

func (s *Store) UpsertTarget(ctx context.Context, target model.Target) error {
	if target.UpdatedAt.IsZero() {
		target.UpdatedAt = time.Now().UTC()
	}
	if err := validateConnectionRef(target.ConnectionRef); err != nil {
		return err
	}
	var lastSeen any
	if target.LastSeenAt != nil {
		lastSeen = ts(*target.LastSeenAt)
	}
	_, err := s.db.ExecContext(ctx, `
INSERT INTO targets(target_id, target_name, kind, connection_ref, is_default, last_seen_at, health, updated_at)
VALUES (?, ?, ?, ?, ?, ?, ?, ?)
ON CONFLICT(target_id) DO UPDATE SET
	target_name=excluded.target_name,
	kind=excluded.kind,
	connection_ref=excluded.connection_ref,
	is_default=excluded.is_default,
	last_seen_at=excluded.last_seen_at,
	health=excluded.health,
	updated_at=excluded.updated_at
`, target.TargetID, target.TargetName, string(target.Kind), target.ConnectionRef, boolToInt(target.IsDefault), lastSeen, string(target.Health), ts(target.UpdatedAt))
	if err != nil {
		return fmt.Errorf("upsert target: %w", err)
	}
	return nil
}

func (s *Store) UpsertPane(ctx context.Context, pane model.Pane) error {
	if pane.UpdatedAt.IsZero() {
		pane.UpdatedAt = time.Now().UTC()
	}
	lastActivityAt := pane.LastActivityAt
	if lastActivityAt == nil {
		v := pane.UpdatedAt
		lastActivityAt = &v
	}
	_, err := s.db.ExecContext(ctx, `
INSERT INTO panes(target_id, pane_id, session_name, window_id, window_name, current_cmd, current_path, pane_title, history_bytes, last_activity_at, updated_at)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
ON CONFLICT(target_id, pane_id) DO UPDATE SET
	session_name=excluded.session_name,
	window_id=excluded.window_id,
	window_name=excluded.window_name,
	current_cmd=excluded.current_cmd,
	current_path=excluded.current_path,
	pane_title=excluded.pane_title,
	history_bytes=excluded.history_bytes,
	last_activity_at=CASE
		WHEN panes.history_bytes != excluded.history_bytes THEN excluded.last_activity_at
		WHEN panes.current_cmd != excluded.current_cmd THEN excluded.last_activity_at
		WHEN panes.current_path != excluded.current_path THEN excluded.last_activity_at
		WHEN panes.pane_title != excluded.pane_title THEN excluded.last_activity_at
		WHEN panes.last_activity_at IS NULL THEN excluded.last_activity_at
		ELSE panes.last_activity_at
	END,
	updated_at=excluded.updated_at
`, pane.TargetID, pane.PaneID, pane.SessionName, pane.WindowID, pane.WindowName, pane.CurrentCmd, pane.CurrentPath, pane.PaneTitle, pane.HistoryBytes, nullableTS(lastActivityAt), ts(pane.UpdatedAt))
	if err != nil {
		return fmt.Errorf("upsert pane: %w", err)
	}
	return nil
}

func (s *Store) UpsertAdapter(ctx context.Context, adapter model.AdapterRecord) error {
	adapterName := strings.TrimSpace(adapter.AdapterName)
	agentType := strings.ToLower(strings.TrimSpace(adapter.AgentType))
	version := strings.TrimSpace(adapter.Version)
	if adapterName == "" {
		return fmt.Errorf("adapter_name is required")
	}
	if agentType == "" {
		return fmt.Errorf("agent_type is required")
	}
	if version == "" {
		return fmt.Errorf("version is required")
	}
	if adapter.UpdatedAt.IsZero() {
		adapter.UpdatedAt = time.Now().UTC()
	}
	capabilitiesJSON, err := marshalCapabilities(adapter.Capabilities)
	if err != nil {
		return err
	}

	_, err = s.db.ExecContext(ctx, `
INSERT INTO adapters(adapter_name, agent_type, version, capabilities, enabled, updated_at)
VALUES (?, ?, ?, ?, ?, ?)
ON CONFLICT(adapter_name) DO UPDATE SET
	agent_type = excluded.agent_type,
	version = excluded.version,
	capabilities = excluded.capabilities,
	enabled = excluded.enabled,
	updated_at = excluded.updated_at
`, adapterName, agentType, version, capabilitiesJSON, boolToInt(adapter.Enabled), ts(adapter.UpdatedAt))
	if err != nil {
		return fmt.Errorf("upsert adapter: %w", err)
	}
	return nil
}

func (s *Store) ListAdapters(ctx context.Context) ([]model.AdapterRecord, error) {
	return s.ListAdaptersFiltered(ctx, nil)
}

func (s *Store) ListAdaptersFiltered(ctx context.Context, enabled *bool) ([]model.AdapterRecord, error) {
	query := `
SELECT adapter_name, agent_type, version, capabilities, enabled, updated_at
FROM adapters`
	args := make([]any, 0, 1)
	if enabled != nil {
		query += ` WHERE enabled = ?`
		args = append(args, boolToInt(*enabled))
	}
	query += ` ORDER BY adapter_name ASC`

	rows, err := s.db.QueryContext(ctx, query, args...)
	if err != nil {
		return nil, fmt.Errorf("list adapters: %w", err)
	}
	defer rows.Close()

	out := make([]model.AdapterRecord, 0)
	for rows.Next() {
		record, err := scanAdapter(rows)
		if err != nil {
			return nil, err
		}
		out = append(out, record)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("iter adapters: %w", err)
	}
	return out, nil
}

func (s *Store) GetAdapterByName(ctx context.Context, adapterName string) (model.AdapterRecord, error) {
	row := s.db.QueryRowContext(ctx, `
SELECT adapter_name, agent_type, version, capabilities, enabled, updated_at
FROM adapters
WHERE adapter_name = ?
`, strings.TrimSpace(adapterName))
	return scanAdapter(row)
}

func (s *Store) GetAdapterByAgentType(ctx context.Context, agentType string) (model.AdapterRecord, error) {
	row := s.db.QueryRowContext(ctx, `
SELECT adapter_name, agent_type, version, capabilities, enabled, updated_at
FROM adapters
WHERE agent_type = ?
ORDER BY enabled DESC, updated_at DESC, adapter_name ASC
LIMIT 1
`, strings.ToLower(strings.TrimSpace(agentType)))
	return scanAdapter(row)
}

func (s *Store) SetAdapterEnabledByName(ctx context.Context, adapterName string, enabled bool, updatedAt time.Time) (model.AdapterRecord, error) {
	name := strings.TrimSpace(adapterName)
	if name == "" {
		return model.AdapterRecord{}, fmt.Errorf("adapter_name is required")
	}
	if updatedAt.IsZero() {
		updatedAt = time.Now().UTC()
	}
	res, err := s.db.ExecContext(ctx, `
UPDATE adapters
SET enabled = ?, updated_at = ?
WHERE adapter_name = ?
`, boolToInt(enabled), ts(updatedAt), name)
	if err != nil {
		return model.AdapterRecord{}, fmt.Errorf("set adapter enabled: %w", err)
	}
	affected, err := res.RowsAffected()
	if err != nil {
		return model.AdapterRecord{}, fmt.Errorf("set adapter enabled rows affected: %w", err)
	}
	if affected == 0 {
		return model.AdapterRecord{}, ErrNotFound
	}
	return s.GetAdapterByName(ctx, name)
}

func scanAdapter(scanner interface{ Scan(dest ...any) error }) (model.AdapterRecord, error) {
	var (
		record           model.AdapterRecord
		capabilitiesJSON string
		enabled          int
		updatedAt        string
	)
	if err := scanner.Scan(&record.AdapterName, &record.AgentType, &record.Version, &capabilitiesJSON, &enabled, &updatedAt); err != nil {
		if errors.Is(err, sql.ErrNoRows) {
			return model.AdapterRecord{}, ErrNotFound
		}
		return model.AdapterRecord{}, fmt.Errorf("scan adapter: %w", err)
	}
	var err error
	record.Capabilities, err = unmarshalCapabilities(capabilitiesJSON)
	if err != nil {
		return model.AdapterRecord{}, fmt.Errorf("decode adapter capabilities: %w", err)
	}
	record.Enabled = enabled == 1
	record.UpdatedAt, err = parseTS(updatedAt)
	if err != nil {
		return model.AdapterRecord{}, fmt.Errorf("parse adapter updated_at: %w", err)
	}
	return record, nil
}

func (s *Store) SyncTargetPanes(ctx context.Context, targetID string, panes []model.Pane) error {
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return fmt.Errorf("begin sync panes tx: %w", err)
	}

	if len(panes) == 0 {
		if _, err := tx.ExecContext(ctx, `DELETE FROM panes WHERE target_id = ?`, targetID); err != nil {
			tx.Rollback() //nolint:errcheck
			return fmt.Errorf("delete panes for target: %w", err)
		}
		if err := tx.Commit(); err != nil {
			return fmt.Errorf("commit empty pane sync: %w", err)
		}
		return nil
	}

	placeholders := make([]string, 0, len(panes))
	args := make([]any, 0, len(panes)+1)
	args = append(args, targetID)
	for _, pane := range panes {
		placeholders = append(placeholders, "?")
		args = append(args, pane.PaneID)
	}
	query := fmt.Sprintf(`DELETE FROM panes WHERE target_id = ? AND pane_id NOT IN (%s)`, strings.Join(placeholders, ","))
	if _, err := tx.ExecContext(ctx, query, args...); err != nil {
		tx.Rollback() //nolint:errcheck
		return fmt.Errorf("delete stale panes: %w", err)
	}

	for _, pane := range panes {
		lastActivityAt := pane.LastActivityAt
		if lastActivityAt == nil {
			v := pane.UpdatedAt
			lastActivityAt = &v
		}
		if _, err := tx.ExecContext(ctx, `
INSERT INTO panes(target_id, pane_id, session_name, window_id, window_name, current_cmd, current_path, pane_title, history_bytes, last_activity_at, updated_at)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
ON CONFLICT(target_id, pane_id) DO UPDATE SET
	session_name=excluded.session_name,
	window_id=excluded.window_id,
	window_name=excluded.window_name,
	current_cmd=excluded.current_cmd,
	current_path=excluded.current_path,
	pane_title=excluded.pane_title,
	history_bytes=excluded.history_bytes,
	last_activity_at=CASE
		WHEN panes.history_bytes != excluded.history_bytes THEN excluded.last_activity_at
		WHEN panes.current_cmd != excluded.current_cmd THEN excluded.last_activity_at
		WHEN panes.current_path != excluded.current_path THEN excluded.last_activity_at
		WHEN panes.pane_title != excluded.pane_title THEN excluded.last_activity_at
		WHEN panes.last_activity_at IS NULL THEN excluded.last_activity_at
		ELSE panes.last_activity_at
	END,
	updated_at=excluded.updated_at
`, pane.TargetID, pane.PaneID, pane.SessionName, pane.WindowID, pane.WindowName, pane.CurrentCmd, pane.CurrentPath, pane.PaneTitle, pane.HistoryBytes, nullableTS(lastActivityAt), ts(pane.UpdatedAt)); err != nil {
			tx.Rollback() //nolint:errcheck
			return fmt.Errorf("upsert pane in sync: %w", err)
		}
	}
	if err := tx.Commit(); err != nil {
		return fmt.Errorf("commit pane sync: %w", err)
	}
	return nil
}

func (s *Store) InsertRuntime(ctx context.Context, rt model.Runtime) error {
	_, err := s.db.ExecContext(ctx, `
INSERT INTO runtimes(runtime_id, target_id, pane_id, tmux_server_boot_id, pane_epoch, agent_type, pid, started_at, ended_at)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
`, rt.RuntimeID, rt.TargetID, rt.PaneID, rt.TmuxServerBootID, rt.PaneEpoch, rt.AgentType, nullableI64(rt.PID), ts(rt.StartedAt), nullableTS(rt.EndedAt))
	if err != nil {
		if isUniqueErr(err) {
			return ErrDuplicate
		}
		return fmt.Errorf("insert runtime: %w", err)
	}
	return nil
}

func (s *Store) EndRuntime(ctx context.Context, runtimeID string, endedAt time.Time) error {
	res, err := s.db.ExecContext(ctx, `UPDATE runtimes SET ended_at = ? WHERE runtime_id = ? AND ended_at IS NULL`, ts(endedAt), runtimeID)
	if err != nil {
		return fmt.Errorf("end runtime: %w", err)
	}
	rows, err := res.RowsAffected()
	if err != nil {
		return fmt.Errorf("rows affected end runtime: %w", err)
	}
	if rows == 0 {
		return ErrNotFound
	}
	return nil
}

func (s *Store) GetRuntime(ctx context.Context, runtimeID string) (model.Runtime, error) {
	row := s.db.QueryRowContext(ctx, `
SELECT runtime_id, target_id, pane_id, tmux_server_boot_id, pane_epoch, agent_type, pid, started_at, ended_at
FROM runtimes
WHERE runtime_id = ?
`, runtimeID)
	return scanRuntime(row)
}

func (s *Store) ListActiveRuntimesForPane(ctx context.Context, targetID, paneID string) ([]model.Runtime, error) {
	rows, err := s.db.QueryContext(ctx, `
SELECT runtime_id, target_id, pane_id, tmux_server_boot_id, pane_epoch, agent_type, pid, started_at, ended_at
FROM runtimes
WHERE target_id = ? AND pane_id = ? AND ended_at IS NULL
ORDER BY started_at DESC
`, targetID, paneID)
	if err != nil {
		return nil, fmt.Errorf("query active runtimes: %w", err)
	}
	defer rows.Close()

	out := make([]model.Runtime, 0)
	for rows.Next() {
		rt, err := scanRuntime(rows)
		if err != nil {
			return nil, err
		}
		out = append(out, rt)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("iter active runtimes: %w", err)
	}
	return out, nil
}

func (s *Store) InsertAction(ctx context.Context, action model.Action) error {
	if action.RequestedAt.IsZero() {
		action.RequestedAt = time.Now().UTC()
	}
	_, err := s.db.ExecContext(ctx, `
INSERT INTO actions(action_id, action_type, request_ref, target_id, pane_id, runtime_id, requested_at, completed_at, result_code, error_code, metadata_json)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
`, action.ActionID, string(action.ActionType), action.RequestRef, action.TargetID, action.PaneID, nullableStr(action.RuntimeID), ts(action.RequestedAt), nullableTS(action.CompletedAt), action.ResultCode, nullableStr(action.ErrorCode), nullableStr(action.MetadataJSON))
	if err != nil {
		if isUniqueErr(err) {
			return ErrDuplicate
		}
		if isForeignKeyErr(err) {
			return ErrNotFound
		}
		return fmt.Errorf("insert action: %w", err)
	}
	return nil
}

func (s *Store) GetActionByTypeRequestRef(ctx context.Context, actionType model.ActionType, requestRef string) (model.Action, error) {
	row := s.db.QueryRowContext(ctx, `
SELECT action_id, action_type, request_ref, target_id, pane_id, runtime_id, requested_at, completed_at, result_code, error_code, metadata_json
FROM actions
WHERE action_type = ? AND request_ref = ?
`, string(actionType), requestRef)
	out, err := scanAction(row)
	if err != nil {
		return model.Action{}, err
	}
	return out, nil
}

func (s *Store) GetActionByID(ctx context.Context, actionID string) (model.Action, error) {
	row := s.db.QueryRowContext(ctx, `
SELECT action_id, action_type, request_ref, target_id, pane_id, runtime_id, requested_at, completed_at, result_code, error_code, metadata_json
FROM actions
WHERE action_id = ?
`, actionID)
	out, err := scanAction(row)
	if err != nil {
		return model.Action{}, err
	}
	return out, nil
}

func (s *Store) ListEventsByActionID(ctx context.Context, actionID string) ([]model.ActionEvent, error) {
	rows, err := s.db.QueryContext(ctx, `
SELECT event_id, action_id, runtime_id, event_type, source, event_time, ingested_at, dedupe_key, raw_payload
FROM events
WHERE action_id = ?
ORDER BY ingested_at ASC, event_id ASC
`, actionID)
	if err != nil {
		return nil, fmt.Errorf("list events by action_id: %w", err)
	}
	defer rows.Close()

	out := make([]model.ActionEvent, 0)
	for rows.Next() {
		var (
			ev         model.ActionEvent
			sourceStr  string
			eventTime  string
			ingestedAt string
			rawPayload sql.NullString
		)
		if err := rows.Scan(&ev.EventID, &ev.ActionID, &ev.RuntimeID, &ev.EventType, &sourceStr, &eventTime, &ingestedAt, &ev.DedupeKey, &rawPayload); err != nil {
			return nil, fmt.Errorf("scan events by action_id: %w", err)
		}
		ev.Source = model.EventSource(sourceStr)
		ev.EventTime, err = parseTS(eventTime)
		if err != nil {
			return nil, fmt.Errorf("parse events by action_id event_time: %w", err)
		}
		ev.IngestedAt, err = parseTS(ingestedAt)
		if err != nil {
			return nil, fmt.Errorf("parse events by action_id ingested_at: %w", err)
		}
		if rawPayload.Valid {
			v := rawPayload.String
			ev.RawPayload = &v
		}
		out = append(out, ev)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("iter events by action_id: %w", err)
	}
	return out, nil
}

func (s *Store) InsertActionSnapshot(ctx context.Context, snapshot model.ActionSnapshot) error {
	_, err := s.db.ExecContext(ctx, `
INSERT INTO action_snapshots(snapshot_id, action_id, target_id, pane_id, runtime_id, state_version, observed_at, expires_at, nonce)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
`, snapshot.SnapshotID, snapshot.ActionID, snapshot.TargetID, snapshot.PaneID, snapshot.RuntimeID, snapshot.StateVersion, ts(snapshot.ObservedAt), ts(snapshot.ExpiresAt), snapshot.Nonce)
	if err != nil {
		if isUniqueErr(err) {
			return ErrDuplicate
		}
		if isForeignKeyErr(err) {
			return ErrNotFound
		}
		return fmt.Errorf("insert action snapshot: %w", err)
	}
	return nil
}

func (s *Store) GetActionSnapshotByActionID(ctx context.Context, actionID string) (model.ActionSnapshot, error) {
	row := s.db.QueryRowContext(ctx, `
SELECT snapshot_id, action_id, target_id, pane_id, runtime_id, state_version, observed_at, expires_at, nonce
FROM action_snapshots
WHERE action_id = ?
ORDER BY observed_at DESC, snapshot_id DESC
LIMIT 1
`, actionID)
	return scanActionSnapshot(row)
}

func scanAction(scanner interface{ Scan(dest ...any) error }) (model.Action, error) {
	var (
		actionTypeStr string
		runtimeID     sql.NullString
		requestedAt   string
		completedAt   sql.NullString
		errorCode     sql.NullString
		metadataJSON  sql.NullString
		out           model.Action
	)
	if err := scanner.Scan(
		&out.ActionID,
		&actionTypeStr,
		&out.RequestRef,
		&out.TargetID,
		&out.PaneID,
		&runtimeID,
		&requestedAt,
		&completedAt,
		&out.ResultCode,
		&errorCode,
		&metadataJSON,
	); err != nil {
		if errors.Is(err, sql.ErrNoRows) {
			return model.Action{}, ErrNotFound
		}
		return model.Action{}, fmt.Errorf("scan action: %w", err)
	}
	out.ActionType = model.ActionType(actionTypeStr)
	if runtimeID.Valid {
		v := runtimeID.String
		out.RuntimeID = &v
	}
	if errorCode.Valid {
		v := errorCode.String
		out.ErrorCode = &v
	}
	if metadataJSON.Valid {
		v := metadataJSON.String
		out.MetadataJSON = &v
	}
	parsedRequestedAt, err := parseTS(requestedAt)
	if err != nil {
		return model.Action{}, fmt.Errorf("parse action requested_at: %w", err)
	}
	out.RequestedAt = parsedRequestedAt
	if completedAt.Valid {
		parsedCompletedAt, parseErr := parseTS(completedAt.String)
		if parseErr != nil {
			return model.Action{}, fmt.Errorf("parse action completed_at: %w", parseErr)
		}
		out.CompletedAt = &parsedCompletedAt
	}
	return out, nil
}

func scanActionSnapshot(row *sql.Row) (model.ActionSnapshot, error) {
	var (
		out        model.ActionSnapshot
		observedAt string
		expiresAt  string
	)
	if err := row.Scan(
		&out.SnapshotID,
		&out.ActionID,
		&out.TargetID,
		&out.PaneID,
		&out.RuntimeID,
		&out.StateVersion,
		&observedAt,
		&expiresAt,
		&out.Nonce,
	); err != nil {
		if errors.Is(err, sql.ErrNoRows) {
			return model.ActionSnapshot{}, ErrNotFound
		}
		return model.ActionSnapshot{}, fmt.Errorf("scan action snapshot: %w", err)
	}
	parsedObservedAt, err := parseTS(observedAt)
	if err != nil {
		return model.ActionSnapshot{}, fmt.Errorf("parse action snapshot observed_at: %w", err)
	}
	out.ObservedAt = parsedObservedAt
	parsedExpiresAt, err := parseTS(expiresAt)
	if err != nil {
		return model.ActionSnapshot{}, fmt.Errorf("parse action snapshot expires_at: %w", err)
	}
	out.ExpiresAt = parsedExpiresAt
	return out, nil
}

func (s *Store) NextPaneEpoch(ctx context.Context, targetID, paneID string) (int64, error) {
	row := s.db.QueryRowContext(ctx, `SELECT COALESCE(MAX(pane_epoch), 0) FROM runtimes WHERE target_id = ? AND pane_id = ?`, targetID, paneID)
	var maxEpoch int64
	if err := row.Scan(&maxEpoch); err != nil {
		return 0, fmt.Errorf("scan max pane_epoch: %w", err)
	}
	return maxEpoch + 1, nil
}

func (s *Store) InsertEvent(ctx context.Context, ev model.EventEnvelope, redactedPayload string) error {
	_, err := s.db.ExecContext(ctx, `
INSERT INTO events(event_id, runtime_id, event_type, source, source_event_id, source_seq, event_time, ingested_at, dedupe_key, action_id, raw_payload)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
`, ev.EventID, ev.RuntimeID, ev.EventType, string(ev.Source), nullIfEmpty(ev.SourceEventID), nullableI64(ev.SourceSeq), ts(ev.EventTime), ts(ev.IngestedAt), ev.DedupeKey, nullableStr(ev.ActionID), nullIfEmpty(redactedPayload))
	if err != nil {
		if isUniqueErr(err) {
			return ErrDuplicate
		}
		return fmt.Errorf("insert event: %w", err)
	}
	return nil
}

func (s *Store) GetEventByRuntimeSourceDedupe(
	ctx context.Context,
	runtimeID string,
	source model.EventSource,
	dedupeKey string,
) (model.EventEnvelope, error) {
	row := s.db.QueryRowContext(ctx, `
SELECT event_id, runtime_id, event_type, source, COALESCE(source_event_id, ''), source_seq, event_time, ingested_at, dedupe_key, action_id, COALESCE(raw_payload, '')
FROM events
WHERE runtime_id = ? AND source = ? AND dedupe_key = ?
`, runtimeID, string(source), dedupeKey)
	var (
		out           model.EventEnvelope
		sourceStr     string
		sourceSeq     sql.NullInt64
		eventTimeStr  string
		ingestedAtStr string
		actionID      sql.NullString
		rawPayload    string
	)
	if err := row.Scan(
		&out.EventID,
		&out.RuntimeID,
		&out.EventType,
		&sourceStr,
		&out.SourceEventID,
		&sourceSeq,
		&eventTimeStr,
		&ingestedAtStr,
		&out.DedupeKey,
		&actionID,
		&rawPayload,
	); err != nil {
		if errors.Is(err, sql.ErrNoRows) {
			return model.EventEnvelope{}, ErrNotFound
		}
		return model.EventEnvelope{}, fmt.Errorf("get event by dedupe: %w", err)
	}
	out.Source = model.EventSource(sourceStr)
	if sourceSeq.Valid {
		v := sourceSeq.Int64
		out.SourceSeq = &v
	}
	var err error
	out.EventTime, err = parseTS(eventTimeStr)
	if err != nil {
		return model.EventEnvelope{}, fmt.Errorf("parse event_time: %w", err)
	}
	out.IngestedAt, err = parseTS(ingestedAtStr)
	if err != nil {
		return model.EventEnvelope{}, fmt.Errorf("parse ingested_at: %w", err)
	}
	if actionID.Valid {
		v := actionID.String
		out.ActionID = &v
	}
	out.RawPayload = rawPayload
	return out, nil
}

func (s *Store) InsertInboxEvent(ctx context.Context, inboxID string, ev model.EventEnvelope, status model.InboxStatus, reason string, redactedPayload string) error {
	_, err := s.db.ExecContext(ctx, `
INSERT INTO event_inbox(inbox_id, target_id, pane_id, runtime_id, event_type, source, dedupe_key, event_time, ingested_at, pid, start_hint, status, reason_code, raw_payload)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
`, inboxID, ev.TargetID, ev.PaneID, nullIfEmpty(ev.RuntimeID), ev.EventType, string(ev.Source), ev.DedupeKey, ts(ev.EventTime), ts(ev.IngestedAt), nullableI64(ev.PID), nullableTS(ev.StartHint), string(status), nullIfEmpty(reason), nullIfEmpty(redactedPayload))
	if err != nil {
		if isUniqueErr(err) {
			return ErrDuplicate
		}
		return fmt.Errorf("insert inbox event: %w", err)
	}
	return nil
}

type InboxEvent struct {
	InboxID    string
	TargetID   string
	PaneID     string
	RuntimeID  string
	EventType  string
	Source     model.EventSource
	DedupeKey  string
	EventTime  time.Time
	IngestedAt time.Time
	PID        *int64
	StartHint  *time.Time
	Status     model.InboxStatus
	ReasonCode string
	RawPayload string
}

func (s *Store) ListPendingInbox(ctx context.Context) ([]InboxEvent, error) {
	rows, err := s.db.QueryContext(ctx, `
SELECT inbox_id, target_id, pane_id, COALESCE(runtime_id, ''), event_type, source, dedupe_key, event_time, ingested_at, pid, start_hint, status, COALESCE(reason_code, ''), COALESCE(raw_payload, '')
FROM event_inbox
WHERE status = 'pending_bind'
ORDER BY ingested_at ASC
`)
	if err != nil {
		return nil, fmt.Errorf("query pending inbox: %w", err)
	}
	defer rows.Close()

	out := make([]InboxEvent, 0)
	for rows.Next() {
		var (
			ie            InboxEvent
			eventTimeStr  string
			ingestedAtStr string
			pid           sql.NullInt64
			startHint     sql.NullString
			sourceStr     string
			statusStr     string
		)
		if err := rows.Scan(&ie.InboxID, &ie.TargetID, &ie.PaneID, &ie.RuntimeID, &ie.EventType, &sourceStr, &ie.DedupeKey, &eventTimeStr, &ingestedAtStr, &pid, &startHint, &statusStr, &ie.ReasonCode, &ie.RawPayload); err != nil {
			return nil, fmt.Errorf("scan pending inbox: %w", err)
		}
		eventTime, err := parseTS(eventTimeStr)
		if err != nil {
			return nil, fmt.Errorf("parse event_time: %w", err)
		}
		ingestedAt, err := parseTS(ingestedAtStr)
		if err != nil {
			return nil, fmt.Errorf("parse ingested_at: %w", err)
		}
		ie.EventTime = eventTime
		ie.IngestedAt = ingestedAt
		ie.Source = model.EventSource(sourceStr)
		ie.Status = model.InboxStatus(statusStr)
		if pid.Valid {
			v := pid.Int64
			ie.PID = &v
		}
		if startHint.Valid {
			v, err := parseTS(startHint.String)
			if err != nil {
				return nil, fmt.Errorf("parse start_hint: %w", err)
			}
			ie.StartHint = &v
		}
		out = append(out, ie)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("iter pending inbox: %w", err)
	}
	return out, nil
}

func (s *Store) UpdateInboxBinding(ctx context.Context, inboxID, runtimeID string, status model.InboxStatus, reason string) error {
	_, err := s.db.ExecContext(ctx, `
UPDATE event_inbox
SET runtime_id = ?, status = ?, reason_code = ?
WHERE inbox_id = ?
`, nullIfEmpty(runtimeID), string(status), nullIfEmpty(reason), inboxID)
	if err != nil {
		return fmt.Errorf("update inbox binding: %w", err)
	}
	return nil
}

type SourceCursor struct {
	RuntimeID           string
	Source              model.EventSource
	LastSourceSeq       *int64
	LastOrderEventTime  time.Time
	LastOrderIngestedAt time.Time
	LastOrderEventID    string
}

func (s *Store) GetSourceCursor(ctx context.Context, runtimeID string, source model.EventSource) (SourceCursor, error) {
	row := s.db.QueryRowContext(ctx, `
SELECT runtime_id, source, last_source_seq, last_order_event_time, last_order_ingested_at, last_order_event_id
FROM runtime_source_cursors
WHERE runtime_id = ? AND source = ?
`, runtimeID, string(source))
	var (
		c             SourceCursor
		sourceStr     string
		sourceSeq     sql.NullInt64
		eventTimeStr  string
		ingestedAtStr string
	)
	if err := row.Scan(&c.RuntimeID, &sourceStr, &sourceSeq, &eventTimeStr, &ingestedAtStr, &c.LastOrderEventID); err != nil {
		if errors.Is(err, sql.ErrNoRows) {
			return SourceCursor{}, ErrNotFound
		}
		return SourceCursor{}, fmt.Errorf("scan source cursor: %w", err)
	}
	c.Source = model.EventSource(sourceStr)
	if sourceSeq.Valid {
		v := sourceSeq.Int64
		c.LastSourceSeq = &v
	}
	var err error
	c.LastOrderEventTime, err = parseTS(eventTimeStr)
	if err != nil {
		return SourceCursor{}, fmt.Errorf("parse last_order_event_time: %w", err)
	}
	c.LastOrderIngestedAt, err = parseTS(ingestedAtStr)
	if err != nil {
		return SourceCursor{}, fmt.Errorf("parse last_order_ingested_at: %w", err)
	}
	return c, nil
}

func (s *Store) UpsertSourceCursor(ctx context.Context, c SourceCursor) error {
	_, err := s.db.ExecContext(ctx, `
INSERT INTO runtime_source_cursors(runtime_id, source, last_source_seq, last_order_event_time, last_order_ingested_at, last_order_event_id)
VALUES (?, ?, ?, ?, ?, ?)
ON CONFLICT(runtime_id, source) DO UPDATE SET
	last_source_seq = excluded.last_source_seq,
	last_order_event_time = excluded.last_order_event_time,
	last_order_ingested_at = excluded.last_order_ingested_at,
	last_order_event_id = excluded.last_order_event_id
`, c.RuntimeID, string(c.Source), nullableI64(c.LastSourceSeq), ts(c.LastOrderEventTime), ts(c.LastOrderIngestedAt), c.LastOrderEventID)
	if err != nil {
		return fmt.Errorf("upsert source cursor: %w", err)
	}
	return nil
}

func (s *Store) GetState(ctx context.Context, targetID, paneID string) (model.StateRow, error) {
	row := s.db.QueryRowContext(ctx, `
SELECT target_id, pane_id, runtime_id, state, COALESCE(reason_code, ''), confidence, state_version, COALESCE(state_source, ''), COALESCE(last_event_type, ''), last_event_at, last_source_seq, last_seen_at, updated_at
FROM states
WHERE target_id = ? AND pane_id = ?
	`, targetID, paneID)
	var (
		st             model.StateRow
		stateStr       string
		stateSourceStr string
		lastEventType  string
		lastEventAt    sql.NullString
		lastSeenStr    string
		updatedAtStr   string
		seq            sql.NullInt64
	)
	if err := row.Scan(
		&st.TargetID,
		&st.PaneID,
		&st.RuntimeID,
		&stateStr,
		&st.ReasonCode,
		&st.Confidence,
		&st.StateVersion,
		&stateSourceStr,
		&lastEventType,
		&lastEventAt,
		&seq,
		&lastSeenStr,
		&updatedAtStr,
	); err != nil {
		if errors.Is(err, sql.ErrNoRows) {
			return model.StateRow{}, ErrNotFound
		}
		return model.StateRow{}, fmt.Errorf("scan state: %w", err)
	}
	st.State = model.CanonicalState(stateStr)
	st.StateSource = model.EventSource(stateSourceStr)
	st.LastEventType = lastEventType
	if lastEventAt.Valid {
		v, err := parseTS(lastEventAt.String)
		if err != nil {
			return model.StateRow{}, fmt.Errorf("parse state last_event_at: %w", err)
		}
		st.LastEventAt = &v
	}
	if seq.Valid {
		v := seq.Int64
		st.LastSourceSeq = &v
	}
	var err error
	st.LastSeenAt, err = parseTS(lastSeenStr)
	if err != nil {
		return model.StateRow{}, fmt.Errorf("parse state last_seen_at: %w", err)
	}
	st.UpdatedAt, err = parseTS(updatedAtStr)
	if err != nil {
		return model.StateRow{}, fmt.Errorf("parse state updated_at: %w", err)
	}
	return st, nil
}

func (s *Store) UpsertState(ctx context.Context, state model.StateRow) error {
	if state.UpdatedAt.IsZero() {
		state.UpdatedAt = time.Now().UTC()
	}
	if state.LastSeenAt.IsZero() {
		state.LastSeenAt = state.UpdatedAt
	}
	_, err := s.db.ExecContext(ctx, `
INSERT INTO states(target_id, pane_id, runtime_id, state, reason_code, confidence, state_version, state_source, last_event_type, last_event_at, last_source_seq, last_seen_at, updated_at)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
ON CONFLICT(target_id, pane_id) DO UPDATE SET
	runtime_id = excluded.runtime_id,
	state = excluded.state,
	reason_code = excluded.reason_code,
	confidence = excluded.confidence,
	state_version = excluded.state_version,
	state_source = excluded.state_source,
	last_event_type = excluded.last_event_type,
	last_event_at = excluded.last_event_at,
	last_source_seq = excluded.last_source_seq,
	last_seen_at = excluded.last_seen_at,
	updated_at = excluded.updated_at
	`,
		state.TargetID,
		state.PaneID,
		state.RuntimeID,
		string(state.State),
		nullIfEmpty(state.ReasonCode),
		state.Confidence,
		state.StateVersion,
		nullableEventSource(state.StateSource),
		nullIfEmpty(state.LastEventType),
		nullableTS(state.LastEventAt),
		nullableI64(state.LastSourceSeq),
		ts(state.LastSeenAt),
		ts(state.UpdatedAt),
	)
	if err != nil {
		return fmt.Errorf("upsert state: %w", err)
	}
	return nil
}

func (s *Store) PurgeRetention(ctx context.Context, payloadCutoff, metadataCutoff time.Time) error {
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return fmt.Errorf("begin retention tx: %w", err)
	}
	if _, err := tx.ExecContext(ctx, `UPDATE events SET raw_payload = NULL WHERE ingested_at < ?`, ts(payloadCutoff)); err != nil {
		tx.Rollback() //nolint:errcheck
		return fmt.Errorf("clear payloads: %w", err)
	}
	if _, err := tx.ExecContext(ctx, `UPDATE event_inbox SET raw_payload = NULL WHERE ingested_at < ?`, ts(payloadCutoff)); err != nil {
		tx.Rollback() //nolint:errcheck
		return fmt.Errorf("clear inbox payloads: %w", err)
	}
	if _, err := tx.ExecContext(ctx, `DELETE FROM events WHERE ingested_at < ?`, ts(metadataCutoff)); err != nil {
		tx.Rollback() //nolint:errcheck
		return fmt.Errorf("delete old events: %w", err)
	}
	if _, err := tx.ExecContext(ctx, `DELETE FROM event_inbox WHERE ingested_at < ? AND status != 'pending_bind'`, ts(metadataCutoff)); err != nil {
		tx.Rollback() //nolint:errcheck
		return fmt.Errorf("delete old inbox: %w", err)
	}
	if err := tx.Commit(); err != nil {
		return fmt.Errorf("commit retention tx: %w", err)
	}
	return nil
}

func (s *Store) CountRows(ctx context.Context, table string) (int64, error) {
	row := s.db.QueryRowContext(ctx, fmt.Sprintf(`SELECT COUNT(*) FROM %s`, table))
	var count int64
	if err := row.Scan(&count); err != nil {
		return 0, fmt.Errorf("count rows %s: %w", table, err)
	}
	return count, nil
}

func (s *Store) ReadEventPayload(ctx context.Context, eventID string) (*string, error) {
	row := s.db.QueryRowContext(ctx, `SELECT raw_payload FROM events WHERE event_id = ?`, eventID)
	var payload sql.NullString
	if err := row.Scan(&payload); err != nil {
		if errors.Is(err, sql.ErrNoRows) {
			return nil, ErrNotFound
		}
		return nil, fmt.Errorf("read event payload: %w", err)
	}
	if !payload.Valid {
		return nil, nil
	}
	v := payload.String
	return &v, nil
}

func (s *Store) GetTargetHealth(ctx context.Context, targetID string) (model.TargetHealth, error) {
	row := s.db.QueryRowContext(ctx, `SELECT health FROM targets WHERE target_id = ?`, targetID)
	var health string
	if err := row.Scan(&health); err != nil {
		if errors.Is(err, sql.ErrNoRows) {
			return "", ErrNotFound
		}
		return "", fmt.Errorf("get target health: %w", err)
	}
	return model.TargetHealth(health), nil
}

func (s *Store) ListTargets(ctx context.Context) ([]model.Target, error) {
	rows, err := s.db.QueryContext(ctx, `
SELECT target_id, target_name, kind, connection_ref, is_default, last_seen_at, health, updated_at
FROM targets
ORDER BY target_name ASC
`)
	if err != nil {
		return nil, fmt.Errorf("list targets: %w", err)
	}
	defer rows.Close()

	out := make([]model.Target, 0)
	for rows.Next() {
		var (
			t          model.Target
			kind       string
			health     string
			lastSeenAt sql.NullString
			updatedAt  string
			isDefault  int
		)
		if err := rows.Scan(&t.TargetID, &t.TargetName, &kind, &t.ConnectionRef, &isDefault, &lastSeenAt, &health, &updatedAt); err != nil {
			return nil, fmt.Errorf("scan target: %w", err)
		}
		t.Kind = model.TargetKind(kind)
		t.Health = model.TargetHealth(health)
		t.IsDefault = isDefault == 1
		if lastSeenAt.Valid {
			v, err := parseTS(lastSeenAt.String)
			if err != nil {
				return nil, fmt.Errorf("parse target last_seen_at: %w", err)
			}
			t.LastSeenAt = &v
		}
		t.UpdatedAt, err = parseTS(updatedAt)
		if err != nil {
			return nil, fmt.Errorf("parse target updated_at: %w", err)
		}
		out = append(out, t)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("iter targets: %w", err)
	}
	return out, nil
}

func (s *Store) GetTargetByName(ctx context.Context, targetName string) (model.Target, error) {
	row := s.db.QueryRowContext(ctx, `
SELECT target_id, target_name, kind, connection_ref, is_default, last_seen_at, health, updated_at
FROM targets
WHERE target_name = ?
`, targetName)
	var (
		t          model.Target
		kind       string
		health     string
		lastSeenAt sql.NullString
		updatedAt  string
		isDefault  int
	)
	if err := row.Scan(&t.TargetID, &t.TargetName, &kind, &t.ConnectionRef, &isDefault, &lastSeenAt, &health, &updatedAt); err != nil {
		if errors.Is(err, sql.ErrNoRows) {
			return model.Target{}, ErrNotFound
		}
		return model.Target{}, fmt.Errorf("get target by name: %w", err)
	}
	t.Kind = model.TargetKind(kind)
	t.Health = model.TargetHealth(health)
	t.IsDefault = isDefault == 1
	if lastSeenAt.Valid {
		v, err := parseTS(lastSeenAt.String)
		if err != nil {
			return model.Target{}, fmt.Errorf("parse target last_seen_at: %w", err)
		}
		t.LastSeenAt = &v
	}
	var err error
	t.UpdatedAt, err = parseTS(updatedAt)
	if err != nil {
		return model.Target{}, fmt.Errorf("parse target updated_at: %w", err)
	}
	return t, nil
}

func (s *Store) DeleteTargetByName(ctx context.Context, targetName string) error {
	res, err := s.db.ExecContext(ctx, `DELETE FROM targets WHERE target_name = ?`, targetName)
	if err != nil {
		return fmt.Errorf("delete target by name: %w", err)
	}
	affected, err := res.RowsAffected()
	if err != nil {
		return fmt.Errorf("delete target by name rows affected: %w", err)
	}
	if affected == 0 {
		return ErrNotFound
	}
	return nil
}

func (s *Store) ListPanes(ctx context.Context) ([]model.Pane, error) {
	rows, err := s.db.QueryContext(ctx, `
SELECT target_id, pane_id, session_name, window_id, window_name, current_cmd, current_path, pane_title, history_bytes, last_activity_at, updated_at
FROM panes
ORDER BY target_id ASC, session_name ASC, window_id ASC, pane_id ASC
`)
	if err != nil {
		return nil, fmt.Errorf("list panes: %w", err)
	}
	defer rows.Close()

	out := make([]model.Pane, 0)
	for rows.Next() {
		var (
			p              model.Pane
			updatedAt      string
			lastActivityAt sql.NullString
		)
		if err := rows.Scan(&p.TargetID, &p.PaneID, &p.SessionName, &p.WindowID, &p.WindowName, &p.CurrentCmd, &p.CurrentPath, &p.PaneTitle, &p.HistoryBytes, &lastActivityAt, &updatedAt); err != nil {
			return nil, fmt.Errorf("scan pane: %w", err)
		}
		p.UpdatedAt, err = parseTS(updatedAt)
		if err != nil {
			return nil, fmt.Errorf("parse pane updated_at: %w", err)
		}
		if lastActivityAt.Valid {
			v, parseErr := parseTS(lastActivityAt.String)
			if parseErr != nil {
				return nil, fmt.Errorf("parse pane last_activity_at: %w", parseErr)
			}
			p.LastActivityAt = &v
		}
		out = append(out, p)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("iter panes: %w", err)
	}
	return out, nil
}

func (s *Store) ListSendActionsForPanes(ctx context.Context, targetIDs, paneIDs []string) ([]model.Action, error) {
	targets := dedupeNonEmpty(targetIDs)
	panes := dedupeNonEmpty(paneIDs)
	if len(targets) == 0 || len(panes) == 0 {
		return []model.Action{}, nil
	}

	targetPlaceholders := strings.TrimRight(strings.Repeat("?,", len(targets)), ",")
	panePlaceholders := strings.TrimRight(strings.Repeat("?,", len(panes)), ",")
	query := fmt.Sprintf(`
SELECT action_id, action_type, request_ref, target_id, pane_id, runtime_id, requested_at, completed_at, result_code, error_code, metadata_json
FROM actions
WHERE action_type = ?
  AND target_id IN (%s)
  AND pane_id IN (%s)
ORDER BY requested_at ASC, action_id ASC
`, targetPlaceholders, panePlaceholders)

	args := make([]any, 0, 1+len(targets)+len(panes))
	args = append(args, string(model.ActionTypeSend))
	for _, targetID := range targets {
		args = append(args, targetID)
	}
	for _, paneID := range panes {
		args = append(args, paneID)
	}

	rows, err := s.db.QueryContext(ctx, query, args...)
	if err != nil {
		return nil, fmt.Errorf("list send actions for panes: %w", err)
	}
	defer rows.Close()

	out := make([]model.Action, 0)
	for rows.Next() {
		action, scanErr := scanAction(rows)
		if scanErr != nil {
			return nil, scanErr
		}
		out = append(out, action)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("iter send actions for panes: %w", err)
	}
	return out, nil
}

type RuntimeLatestEvent struct {
	RuntimeID  string
	EventType  string
	Source     model.EventSource
	EventTime  time.Time
	IngestedAt time.Time
	RawPayload string
}

func (s *Store) ListLatestRuntimeEvents(ctx context.Context, runtimeIDs []string) ([]RuntimeLatestEvent, error) {
	runtimes := dedupeNonEmpty(runtimeIDs)
	if len(runtimes) == 0 {
		return []RuntimeLatestEvent{}, nil
	}

	runtimePlaceholders := strings.TrimRight(strings.Repeat("?,", len(runtimes)), ",")
	query := fmt.Sprintf(`
SELECT runtime_id, event_type, source, event_time, ingested_at, COALESCE(raw_payload, ''), event_id
FROM events
WHERE runtime_id IN (%s)
  AND source IN ('hook','notify','wrapper')
ORDER BY runtime_id ASC, ingested_at DESC, event_id DESC
`, runtimePlaceholders)

	args := make([]any, 0, len(runtimes))
	for _, runtimeID := range runtimes {
		args = append(args, runtimeID)
	}

	rows, err := s.db.QueryContext(ctx, query, args...)
	if err != nil {
		return nil, fmt.Errorf("list latest runtime events: %w", err)
	}
	defer rows.Close()

	seen := map[string]struct{}{}
	out := make([]RuntimeLatestEvent, 0, len(runtimes))
	for rows.Next() {
		var (
			ev          RuntimeLatestEvent
			sourceStr   string
			eventAtStr  string
			ingestedStr string
			eventID     string
		)
		if err := rows.Scan(&ev.RuntimeID, &ev.EventType, &sourceStr, &eventAtStr, &ingestedStr, &ev.RawPayload, &eventID); err != nil {
			return nil, fmt.Errorf("scan latest runtime event: %w", err)
		}
		if _, ok := seen[ev.RuntimeID]; ok {
			continue
		}
		seen[ev.RuntimeID] = struct{}{}
		ev.Source = model.EventSource(sourceStr)
		ev.EventTime, err = parseTS(eventAtStr)
		if err != nil {
			return nil, fmt.Errorf("parse latest runtime event event_time: %w", err)
		}
		ev.IngestedAt, err = parseTS(ingestedStr)
		if err != nil {
			return nil, fmt.Errorf("parse latest runtime event ingested_at: %w", err)
		}
		out = append(out, ev)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("iter latest runtime events: %w", err)
	}
	return out, nil
}

func (s *Store) ListActiveRuntimes(ctx context.Context) ([]model.Runtime, error) {
	rows, err := s.db.QueryContext(ctx, `
SELECT runtime_id, target_id, pane_id, tmux_server_boot_id, pane_epoch, agent_type, pid, started_at, ended_at
FROM runtimes
WHERE ended_at IS NULL
ORDER BY started_at DESC
`)
	if err != nil {
		return nil, fmt.Errorf("list active runtimes: %w", err)
	}
	defer rows.Close()

	out := make([]model.Runtime, 0)
	for rows.Next() {
		rt, err := scanRuntime(rows)
		if err != nil {
			return nil, err
		}
		out = append(out, rt)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("iter active runtimes: %w", err)
	}
	return out, nil
}

func (s *Store) ListStates(ctx context.Context) ([]model.StateRow, error) {
	rows, err := s.db.QueryContext(ctx, `
SELECT target_id, pane_id, runtime_id, state, COALESCE(reason_code, ''), confidence, state_version, COALESCE(state_source, ''), COALESCE(last_event_type, ''), last_event_at, last_source_seq, last_seen_at, updated_at
FROM states
ORDER BY updated_at ASC
`)
	if err != nil {
		return nil, fmt.Errorf("list states: %w", err)
	}
	defer rows.Close()

	out := make([]model.StateRow, 0)
	for rows.Next() {
		var (
			st             model.StateRow
			state          string
			stateSourceStr string
			lastEventType  string
			lastEventAt    sql.NullString
			seq            sql.NullInt64
			lastSeenAt     string
			updatedAt      string
		)
		if err := rows.Scan(
			&st.TargetID,
			&st.PaneID,
			&st.RuntimeID,
			&state,
			&st.ReasonCode,
			&st.Confidence,
			&st.StateVersion,
			&stateSourceStr,
			&lastEventType,
			&lastEventAt,
			&seq,
			&lastSeenAt,
			&updatedAt,
		); err != nil {
			return nil, fmt.Errorf("scan state row: %w", err)
		}
		st.State = model.CanonicalState(state)
		st.StateSource = model.EventSource(stateSourceStr)
		st.LastEventType = lastEventType
		if lastEventAt.Valid {
			v, parseErr := parseTS(lastEventAt.String)
			if parseErr != nil {
				return nil, fmt.Errorf("parse list state last_event_at: %w", parseErr)
			}
			st.LastEventAt = &v
		}
		if seq.Valid {
			v := seq.Int64
			st.LastSourceSeq = &v
		}
		st.LastSeenAt, err = parseTS(lastSeenAt)
		if err != nil {
			return nil, fmt.Errorf("parse list state last_seen_at: %w", err)
		}
		st.UpdatedAt, err = parseTS(updatedAt)
		if err != nil {
			return nil, fmt.Errorf("parse list state updated_at: %w", err)
		}
		out = append(out, st)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("iter states: %w", err)
	}
	return out, nil
}

func scanRuntime(scanner interface{ Scan(dest ...any) error }) (model.Runtime, error) {
	var (
		rt        model.Runtime
		pid       sql.NullInt64
		startedAt string
		endedAt   sql.NullString
	)
	if err := scanner.Scan(&rt.RuntimeID, &rt.TargetID, &rt.PaneID, &rt.TmuxServerBootID, &rt.PaneEpoch, &rt.AgentType, &pid, &startedAt, &endedAt); err != nil {
		if errors.Is(err, sql.ErrNoRows) {
			return model.Runtime{}, ErrNotFound
		}
		return model.Runtime{}, fmt.Errorf("scan runtime: %w", err)
	}
	if pid.Valid {
		v := pid.Int64
		rt.PID = &v
	}
	var err error
	rt.StartedAt, err = parseTS(startedAt)
	if err != nil {
		return model.Runtime{}, fmt.Errorf("parse started_at: %w", err)
	}
	if endedAt.Valid {
		v, err := parseTS(endedAt.String)
		if err != nil {
			return model.Runtime{}, fmt.Errorf("parse ended_at: %w", err)
		}
		rt.EndedAt = &v
	}
	return rt, nil
}

func boolToInt(v bool) int {
	if v {
		return 1
	}
	return 0
}

func nullableI64(v *int64) any {
	if v == nil {
		return nil
	}
	return *v
}

func nullableEventSource(v model.EventSource) any {
	source := strings.TrimSpace(string(v))
	if source == "" {
		return nil
	}
	return source
}

func nullableTS(v *time.Time) any {
	if v == nil {
		return nil
	}
	return ts(*v)
}

func nullableStr(v *string) any {
	if v == nil {
		return nil
	}
	return *v
}

func nullIfEmpty(v string) any {
	if v == "" {
		return nil
	}
	return v
}

func dedupeNonEmpty(values []string) []string {
	seen := map[string]struct{}{}
	out := make([]string, 0, len(values))
	for _, value := range values {
		v := strings.TrimSpace(value)
		if v == "" {
			continue
		}
		if _, ok := seen[v]; ok {
			continue
		}
		seen[v] = struct{}{}
		out = append(out, v)
	}
	return out
}

func ts(t time.Time) string {
	return t.UTC().Format(time.RFC3339Nano)
}

func parseTS(s string) (time.Time, error) {
	return time.Parse(time.RFC3339Nano, s)
}

func isUniqueErr(err error) bool {
	if err == nil {
		return false
	}
	msg := err.Error()
	return containsAny(msg,
		"UNIQUE constraint failed",
		"constraint failed: UNIQUE",
	)
}

func isForeignKeyErr(err error) bool {
	if err == nil {
		return false
	}
	msg := err.Error()
	return containsAny(msg,
		"FOREIGN KEY constraint failed",
		"constraint failed: FOREIGN KEY",
	)
}

func containsAny(s string, patterns ...string) bool {
	for _, p := range patterns {
		if p != "" && strings.Contains(s, p) {
			return true
		}
	}
	return false
}

func marshalCapabilities(capabilities []string) (string, error) {
	normalized := make([]string, 0, len(capabilities))
	seen := map[string]struct{}{}
	for _, capability := range capabilities {
		v := strings.ToLower(strings.TrimSpace(capability))
		if v == "" {
			continue
		}
		if _, ok := seen[v]; ok {
			continue
		}
		seen[v] = struct{}{}
		normalized = append(normalized, v)
	}
	sort.Strings(normalized)
	if len(normalized) == 0 {
		return "[]", nil
	}
	buf, err := json.Marshal(normalized)
	if err != nil {
		return "", fmt.Errorf("marshal capabilities: %w", err)
	}
	return string(buf), nil
}

func unmarshalCapabilities(raw string) ([]string, error) {
	text := strings.TrimSpace(raw)
	if text == "" {
		return nil, nil
	}
	var values []string
	if err := json.Unmarshal([]byte(text), &values); err != nil {
		return nil, fmt.Errorf("unmarshal capabilities: %w", err)
	}
	out := make([]string, 0, len(values))
	for _, value := range values {
		v := strings.ToLower(strings.TrimSpace(value))
		if v == "" {
			continue
		}
		out = append(out, v)
	}
	sort.Strings(out)
	return out, nil
}

var connectionRefAliasPattern = regexp.MustCompile(`^[A-Za-z0-9._-]{1,128}$`)

func validateConnectionRef(ref string) error {
	v := strings.TrimSpace(ref)
	if v == "" {
		return nil
	}
	if !connectionRefAliasPattern.MatchString(v) {
		return fmt.Errorf("connection_ref must match alias pattern [A-Za-z0-9._-]{1,128}")
	}
	return nil
}
