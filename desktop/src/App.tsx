import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { PaneInfo } from "./types";
import { PaneCard } from "./components/PaneCard";
import { StatusBar } from "./components/StatusBar";
import "./App.css";

const SOCKET_PATH = "/tmp/agtmux.sock";

function App() {
  const [panes, setPanes] = useState<PaneInfo[]>([]);
  const [selectedPaneId, setSelectedPaneId] = useState<string | null>(null);
  const [connected, setConnected] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const unlistenRefs = useRef<(() => void)[]>([]);

  const fetchPanes = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const result = await invoke<PaneInfo[]>("list_panes", {
        socketPath: SOCKET_PATH,
      });
      setPanes(result);
      setConnected(true);
    } catch (err: unknown) {
      const message =
        err instanceof Error
          ? err.message
          : typeof err === "string"
            ? err
            : "Failed to connect to daemon";
      setError(message);
      setConnected(false);
      setPanes([]);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    fetchPanes();
  }, [fetchPanes]);

  useEffect(() => {
    let cancelled = false;

    async function setupListeners() {
      try {
        // Fix 1 (wire-format): Listen for pane-update-all which contains a
        // full PaneInfo[] array fetched by the backend after state_changed.
        const unlistenUpdateAll = await listen<PaneInfo[]>(
          "pane-update-all",
          (event) => {
            if (cancelled) return;
            setPanes(event.payload);
            // Fix 5: Mark as connected when we receive pane data
            setConnected(true);
          },
        );

        // Fix 2: Listen for pane-added — trigger a full list_panes refetch
        const unlistenAdded = await listen<{ pane_id: string }>(
          "pane-added",
          (_event) => {
            if (cancelled) return;
            // Refetch the full pane list to get the new pane's PaneInfo
            invoke<PaneInfo[]>("list_panes", { socketPath: SOCKET_PATH })
              .then((result) => {
                setPanes(result);
                setConnected(true);
              })
              .catch((err) => {
                console.error("Failed to refetch panes after pane-added:", err);
              });
          },
        );

        // Fix 2: Listen for pane-removed — remove the pane from state
        const unlistenRemoved = await listen<{ pane_id: string }>(
          "pane-removed",
          (event) => {
            if (cancelled) return;
            const removedId = event.payload.pane_id;
            setPanes((prev) => prev.filter((p) => p.pane_id !== removedId));
          },
        );

        // Fix 5: Listen for daemon-status events from the backend
        const unlistenStatus = await listen<{
          connected: boolean;
          reconnecting?: boolean;
        }>("daemon-status", (event) => {
          if (cancelled) return;
          setConnected(event.payload.connected);
        });

        if (!cancelled) {
          unlistenRefs.current = [
            unlistenUpdateAll,
            unlistenAdded,
            unlistenRemoved,
            unlistenStatus,
          ];
        } else {
          // Already unmounted — clean up immediately
          unlistenUpdateAll();
          unlistenAdded();
          unlistenRemoved();
          unlistenStatus();
        }
      } catch {
        // listen may fail if Tauri runtime is not available (e.g. in browser dev)
      }
    }

    setupListeners();

    // Fix 2: Clean up all listeners on unmount
    return () => {
      cancelled = true;
      for (const unlisten of unlistenRefs.current) {
        unlisten();
      }
      unlistenRefs.current = [];
    };
  }, []);

  if (loading) {
    return (
      <div className="sidebar">
        <div className="sidebar__header">
          <h1 className="sidebar__title">AGTMUX</h1>
        </div>
        <div className="sidebar__loading">Loading...</div>
      </div>
    );
  }

  if (error) {
    return (
      <div className="sidebar">
        <div className="sidebar__header">
          <h1 className="sidebar__title">AGTMUX</h1>
        </div>
        <div className="sidebar__error">
          <div className="error-icon">!</div>
          <p className="error-title">Daemon not connected</p>
          <p className="error-message">{error}</p>
          <button className="error-retry" onClick={fetchPanes}>
            Retry
          </button>
        </div>
      </div>
    );
  }

  return (
    <div className="sidebar">
      <div className="sidebar__header">
        <h1 className="sidebar__title">AGTMUX</h1>
        <span className="sidebar__count">{panes.length} panes</span>
      </div>
      <div className="sidebar__panes">
        {panes.length === 0 ? (
          <div className="sidebar__empty">No agent panes detected</div>
        ) : (
          panes.map((pane) => (
            <PaneCard
              key={pane.pane_id}
              pane={pane}
              selected={pane.pane_id === selectedPaneId}
              onClick={() => setSelectedPaneId(pane.pane_id)}
            />
          ))
        )}
      </div>
      <StatusBar panes={panes} connected={connected} />
    </div>
  );
}

export default App;
