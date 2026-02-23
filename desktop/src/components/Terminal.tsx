import { useEffect, useRef } from "react";
import { Terminal as XTerm } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebLinksAddon } from "@xterm/addon-web-links";
import "@xterm/xterm/css/xterm.css";

interface TerminalProps {
  paneId: string;
  wsUrl: string;
}

export function Terminal({ paneId, wsUrl }: TerminalProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<XTerm | null>(null);
  const wsRef = useRef<WebSocket | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const nextIdRef = useRef(1);

  useEffect(() => {
    const container = containerRef.current!;

    const term = new XTerm({
      fontFamily: "'SF Mono', 'Fira Code', 'Cascadia Code', 'JetBrains Mono', Menlo, Consolas, monospace",
      fontSize: 13,
      theme: {
        background: "#1a1a2e",
        foreground: "#e2e8f0",
        cursor: "#60a5fa",
        selectionBackground: "#2a3f6b",
      },
      cursorBlink: true,
    });
    termRef.current = term;

    const fit = new FitAddon();
    fitRef.current = fit;
    term.loadAddon(fit);
    term.loadAddon(new WebLinksAddon());
    term.open(container);
    fit.fit();

    const ws = new WebSocket(wsUrl);
    ws.binaryType = "arraybuffer";
    wsRef.current = ws;

    function sendRpc(method: string, params: Record<string, unknown>) {
      const id = nextIdRef.current++;
      ws.send(JSON.stringify({ jsonrpc: "2.0", id, method, params }));
    }

    ws.onopen = () => {
      sendRpc("subscribe_output", { pane_id: paneId });
      sendRpc("resize_pane", {
        pane_id: paneId,
        cols: term.cols,
        rows: term.rows,
      });
    };

    ws.onmessage = (event: MessageEvent) => {
      if (event.data instanceof ArrayBuffer) {
        const buf = new Uint8Array(event.data);
        const paneIdLen = buf[0];
        const outputData = buf.slice(1 + paneIdLen);
        term.write(outputData);
      }
      // Ignore JSON text responses (RPC acks)
    };

    term.onData((data: string) => {
      if (ws.readyState === WebSocket.OPEN) {
        sendRpc("write_input", { pane_id: paneId, data });
      }
    });

    term.onResize(({ cols, rows }) => {
      if (ws.readyState === WebSocket.OPEN) {
        sendRpc("resize_pane", { pane_id: paneId, cols, rows });
      }
    });

    const resizeObserver = new ResizeObserver(() => {
      fit.fit();
    });
    resizeObserver.observe(container);

    return () => {
      resizeObserver.disconnect();
      if (ws.readyState === WebSocket.OPEN) {
        const id = nextIdRef.current++;
        ws.send(JSON.stringify({
          jsonrpc: "2.0",
          id,
          method: "unsubscribe_output",
          params: { pane_id: paneId },
        }));
      }
      ws.close();
      wsRef.current = null;
      term.dispose();
      termRef.current = null;
      fitRef.current = null;
    };
  }, [paneId, wsUrl]);

  return <div ref={containerRef} className="terminal-container" />;
}
