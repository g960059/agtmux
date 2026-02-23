import type { PaneInfo } from "../types";

/**
 * Maps an activity_state string to a CSS color value.
 */
export function stateColor(state: string): string {
  switch (state) {
    case "running":
      return "#4ade80"; // green
    case "waiting_approval":
      return "#fb923c"; // orange
    case "waiting_input":
      return "#60a5fa"; // blue
    case "idle":
      return "#9ca3af"; // gray
    case "error":
      return "#f87171"; // red
    case "unknown":
    default:
      return "#6b7280"; // dim gray
  }
}

/**
 * Maps an activity_state string to a Unicode indicator character.
 * Matches the CLI status indicators.
 */
export function stateIcon(state: string): string {
  switch (state) {
    case "running":
      return "\u25CF"; // ●
    case "waiting_approval":
      return "\u25C9"; // ◉
    case "waiting_input":
      return "\u25C8"; // ◈
    case "idle":
      return "\u25CB"; // ○
    case "error":
      return "\u2716"; // ✖
    case "unknown":
    default:
      return "\u25CC"; // ◌
  }
}

/**
 * Converts an ISO date string to a human-readable relative time string.
 */
export function timeAgo(isoDate: string): string {
  const now = Date.now();
  const then = new Date(isoDate).getTime();
  const diffMs = now - then;

  if (isNaN(then)) {
    return "unknown";
  }

  const seconds = Math.floor(diffMs / 1000);
  if (seconds < 0) {
    return "just now";
  }
  if (seconds < 60) {
    return `${seconds}s ago`;
  }

  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) {
    return `${minutes}m ago`;
  }

  const hours = Math.floor(minutes / 60);
  if (hours < 24) {
    return `${hours}h ago`;
  }

  const days = Math.floor(hours / 24);
  return `${days}d ago`;
}

/**
 * Formats the activity_state string for display (replaces underscores, title case).
 */
function formatState(state: string): string {
  return state
    .split("_")
    .map((w) => w.charAt(0).toUpperCase() + w.slice(1))
    .join(" ");
}

interface PaneCardProps {
  pane: PaneInfo;
  selected: boolean;
  onClick: () => void;
}

export function PaneCard({ pane, selected, onClick }: PaneCardProps) {
  const color = stateColor(pane.activity_state);
  const icon = stateIcon(pane.activity_state);
  const displayName = pane.provider ?? "Unknown";
  const displayTitle = pane.pane_title || pane.current_cmd || pane.pane_id;

  return (
    <div
      className={`pane-card ${selected ? "pane-card--selected" : ""}`}
      onClick={onClick}
    >
      <div className="pane-card__header">
        <span className="pane-card__indicator" style={{ color }}>
          {icon}
        </span>
        <span className="pane-card__provider">{displayName}</span>
        <span className="pane-card__time">{timeAgo(pane.updated_at)}</span>
      </div>
      <div className="pane-card__body">
        <span className="pane-card__title">{displayTitle}</span>
      </div>
      <div className="pane-card__footer">
        <span className="pane-card__pane-id">{pane.pane_id}</span>
        <span className="pane-card__state" style={{ color }}>
          {formatState(pane.activity_state)}
        </span>
      </div>
    </div>
  );
}
