import type { PaneInfo } from "../types";
import { stateColor } from "./PaneCard";

interface StatusBarProps {
  panes: PaneInfo[];
  connected: boolean;
}

/**
 * Builds a summary string like "2 running, 1 approval" from pane states.
 */
function buildSummary(panes: PaneInfo[]): string {
  const counts: Record<string, number> = {};
  for (const pane of panes) {
    const state = pane.activity_state;
    counts[state] = (counts[state] ?? 0) + 1;
  }

  // Display order: running first, then approval, input, error, idle, unknown
  const order = [
    "running",
    "waiting_approval",
    "waiting_input",
    "error",
    "idle",
    "unknown",
  ];
  const labels: Record<string, string> = {
    running: "running",
    waiting_approval: "approval",
    waiting_input: "input",
    error: "error",
    idle: "idle",
    unknown: "unknown",
  };

  const parts: string[] = [];
  for (const state of order) {
    const count = counts[state];
    if (count && count > 0) {
      parts.push(`${count} ${labels[state] ?? state}`);
    }
  }

  if (parts.length === 0) {
    return "No panes";
  }
  return parts.join(", ");
}

export function StatusBar({ panes, connected }: StatusBarProps) {
  const summary = buildSummary(panes);
  const connectionColor = connected ? "#4ade80" : "#f87171";
  const connectionLabel = connected ? "Connected" : "Disconnected";

  // Find the most urgent state for the summary dot color
  const urgentOrder = [
    "error",
    "waiting_approval",
    "waiting_input",
    "running",
    "idle",
    "unknown",
  ];
  let summaryColor = "#6b7280";
  for (const state of urgentOrder) {
    if (panes.some((p) => p.activity_state === state)) {
      summaryColor = stateColor(state);
      break;
    }
  }

  return (
    <div className="status-bar">
      <div className="status-bar__summary">
        <span
          className="status-bar__dot"
          style={{ backgroundColor: summaryColor }}
        />
        <span className="status-bar__text">{summary}</span>
      </div>
      <div className="status-bar__connection">
        <span
          className="status-bar__dot"
          style={{ backgroundColor: connectionColor }}
        />
        <span className="status-bar__text">{connectionLabel}</span>
      </div>
    </div>
  );
}
