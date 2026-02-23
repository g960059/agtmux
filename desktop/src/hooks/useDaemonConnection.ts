import { useCallback, useEffect, useRef, useState } from "react";
import type { PaneInfo } from "../types";

// ---------------------------------------------------------------------------
// JSON-RPC types (send/receive over WebSocket)
// ---------------------------------------------------------------------------

interface JsonRpcRequest {
  jsonrpc: "2.0";
  id: number;
  method: string;
  params: Record<string, unknown>;
}

interface JsonRpcResponse {
  jsonrpc: "2.0";
  id?: number;
  result?: unknown;
  error?: { code: number; message: string };
  method?: string;
  params?: Record<string, unknown>;
}

interface ListPanesResult {
  panes: PaneInfo[];
}

// ---------------------------------------------------------------------------
// Hook
// ---------------------------------------------------------------------------

const RECONNECT_DELAY_MS = 3000;

export interface DaemonConnection {
  panes: PaneInfo[];
  connected: boolean;
  error: string | null;
  retry: () => void;
}

export function useDaemonConnection(wsUrl: string): DaemonConnection {
  const [panes, setPanes] = useState<PaneInfo[]>([]);
  const [connected, setConnected] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const wsRef = useRef<WebSocket | null>(null);
  const nextIdRef = useRef(1);
  const reconnectTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  // Track whether the component is still mounted to avoid state updates
  // after unmount.
  const mountedRef = useRef(true);

  // Helper: send a JSON-RPC request over the WebSocket.
  const sendRequest = useCallback(
    (method: string, params: Record<string, unknown> = {}): number | null => {
      const ws = wsRef.current;
      if (!ws || ws.readyState !== WebSocket.OPEN) return null;
      const id = nextIdRef.current++;
      const req: JsonRpcRequest = { jsonrpc: "2.0", id, method, params };
      ws.send(JSON.stringify(req));
      return id;
    },
    [],
  );

  // Connect to the daemon WebSocket.
  const connect = useCallback(() => {
    // Clean up any pending reconnect timer.
    if (reconnectTimerRef.current !== null) {
      clearTimeout(reconnectTimerRef.current);
      reconnectTimerRef.current = null;
    }

    // Close existing connection if any.
    if (wsRef.current) {
      wsRef.current.close();
      wsRef.current = null;
    }

    let listPanesId: number | null = null;
    let subscribeId: number | null = null;

    const ws = new WebSocket(wsUrl);
    wsRef.current = ws;

    ws.onopen = () => {
      if (!mountedRef.current) return;
      setError(null);
      // Step 1: request the current pane list.
      const id = nextIdRef.current++;
      listPanesId = id;
      const req: JsonRpcRequest = {
        jsonrpc: "2.0",
        id,
        method: "list_panes",
        params: {},
      };
      ws.send(JSON.stringify(req));
    };

    ws.onmessage = (event: MessageEvent) => {
      if (!mountedRef.current) return;

      let msg: JsonRpcResponse;
      try {
        msg = JSON.parse(event.data as string) as JsonRpcResponse;
      } catch {
        console.error("Failed to parse daemon message:", event.data);
        return;
      }

      // --- Handle RPC responses (have `id`) ---
      if (msg.id !== undefined && msg.id !== null) {
        if (msg.error) {
          console.error(
            `JSON-RPC error (id=${msg.id}):`,
            msg.error.message,
          );
          return;
        }

        // Response to list_panes
        if (msg.id === listPanesId) {
          const result = msg.result as ListPanesResult | undefined;
          if (result?.panes) {
            setPanes(result.panes);
          }
          setConnected(true);

          // Step 2: subscribe to state + topology events.
          const subId = nextIdRef.current++;
          subscribeId = subId;
          const subReq: JsonRpcRequest = {
            jsonrpc: "2.0",
            id: subId,
            method: "subscribe",
            params: { events: ["state", "topology"] },
          };
          ws.send(JSON.stringify(subReq));
          return;
        }

        // Response to subscribe
        if (msg.id === subscribeId) {
          // Subscription acknowledged — nothing else to do.
          return;
        }

        // Response to a re-fetch list_panes (triggered by notifications).
        // Any other response with a result containing panes is a list_panes
        // response.
        const result = msg.result as Record<string, unknown> | undefined;
        if (result && Array.isArray(result.panes)) {
          setPanes(result.panes as PaneInfo[]);
        }
        return;
      }

      // --- Handle push notifications (no `id`, have `method`) ---
      if (msg.method) {
        switch (msg.method) {
          case "state_changed": {
            // The state_changed params have a nested PaneState structure
            // that differs from the flat PaneInfo. The simplest and most
            // reliable approach: re-fetch the full pane list.
            sendRequest("list_panes");
            break;
          }
          case "pane_added": {
            // New pane detected — re-fetch full list to get complete info.
            sendRequest("list_panes");
            break;
          }
          case "pane_removed": {
            const params = msg.params as { pane_id?: string } | undefined;
            if (params?.pane_id) {
              const removedId = params.pane_id;
              setPanes((prev) =>
                prev.filter((p) => p.pane_id !== removedId),
              );
            }
            break;
          }
          default:
            // Ignore unknown notifications (e.g. "summary").
            break;
        }
      }
    };

    ws.onerror = () => {
      if (!mountedRef.current) return;
      // The error event itself carries no useful info in browsers; the
      // close event will fire next and trigger reconnect.
    };

    ws.onclose = () => {
      if (!mountedRef.current) return;
      wsRef.current = null;
      setConnected(false);
      setError("Connection to daemon lost");

      // Auto-reconnect after delay.
      reconnectTimerRef.current = setTimeout(() => {
        if (mountedRef.current) {
          connect();
        }
      }, RECONNECT_DELAY_MS);
    };
  }, [wsUrl, sendRequest]);

  // Manual retry exposed to the UI.
  const retry = useCallback(() => {
    setError(null);
    connect();
  }, [connect]);

  // Effect: connect on mount, disconnect on unmount.
  useEffect(() => {
    mountedRef.current = true;
    connect();

    return () => {
      mountedRef.current = false;
      if (reconnectTimerRef.current !== null) {
        clearTimeout(reconnectTimerRef.current);
        reconnectTimerRef.current = null;
      }
      if (wsRef.current) {
        wsRef.current.close();
        wsRef.current = null;
      }
    };
  }, [connect]);

  return { panes, connected, error, retry };
}
