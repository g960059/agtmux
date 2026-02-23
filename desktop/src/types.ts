export interface PaneInfo {
  pane_id: string;
  session_name: string;
  window_id: string;
  pane_title: string;
  current_cmd: string;
  provider: string | null;
  provider_confidence: number;
  activity_state: string;
  activity_confidence: number;
  activity_source: string;
  attention_state: string;
  attention_reason: string;
  attention_since: string | null;
  updated_at: string;
}

export type ActivityState =
  | "running"
  | "waiting_approval"
  | "waiting_input"
  | "idle"
  | "error"
  | "unknown";
