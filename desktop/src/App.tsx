import { useState } from "react";
import { useDaemonConnection } from "./hooks/useDaemonConnection";
import { PaneCard } from "./components/PaneCard";
import { StatusBar } from "./components/StatusBar";
import "./App.css";

const WS_URL = "ws://127.0.0.1:9780";

function App() {
  const { panes, connected, error, retry } = useDaemonConnection(WS_URL);
  const [selectedPaneId, setSelectedPaneId] = useState<string | null>(null);

  if (error && !connected && panes.length === 0) {
    return (
      <div className="sidebar">
        <div className="sidebar__header">
          <h1 className="sidebar__title">AGTMUX</h1>
        </div>
        <div className="sidebar__error">
          <div className="error-icon">!</div>
          <p className="error-title">Daemon not connected</p>
          <p className="error-message">{error}</p>
          <button className="error-retry" onClick={retry}>
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
