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
  const unlistenRef = useRef<(() => void) | null>(null);

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

    async function setupListener() {
      try {
        const unlisten = await listen<PaneInfo>("pane-update", (event) => {
          if (cancelled) return;
          const updated = event.payload;
          setPanes((prev) => {
            const idx = prev.findIndex((p) => p.pane_id === updated.pane_id);
            if (idx >= 0) {
              const next = [...prev];
              next[idx] = updated;
              return next;
            }
            return [...prev, updated];
          });
          setConnected(true);
        });
        if (!cancelled) {
          unlistenRef.current = unlisten;
        } else {
          unlisten();
        }
      } catch {
        // listen may fail if Tauri runtime is not available (e.g. in browser dev)
      }
    }

    setupListener();

    return () => {
      cancelled = true;
      if (unlistenRef.current) {
        unlistenRef.current();
        unlistenRef.current = null;
      }
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
