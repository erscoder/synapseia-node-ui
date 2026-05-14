import { useEffect, useRef } from "react";
import { LogLine, NodeStatus } from "../App";
import { Download, Play, Square } from "lucide-react";

interface Props {
  logs: LogLine[];
  // Same shape MyNodePanel uses so we keep a single source-of-truth (the
  // App-level NodeStatus + password + onStart/onStop handlers). Avoids
  // duplicating local state here.
  status: NodeStatus;
  password: string | null;
  onStart: () => void;
  onStop: () => void;
}

export function LogViewer({ logs, status, password, onStart, onStop }: Props) {
  const containerRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (containerRef.current) {
      containerRef.current.scrollTop = containerRef.current.scrollHeight;
    }
  }, [logs]);

  const handleExport = () => {
    const content = logs
      .map((l) => `[${l.timestamp}] [${l.level.toUpperCase()}] ${l.message}`)
      .join("\n");
    const blob = new Blob([content], { type: "text/plain" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = `synapseia-logs-${new Date().toISOString().slice(0, 10)}.txt`;
    a.click();
    URL.revokeObjectURL(url);
  };

  const levelColors: Record<string, string> = {
    stdout: "text-green-400",
    stderr: "text-red-400",
    info: "text-blue-400",
    warn: "text-yellow-400",
    error: "text-red-500",
  };

  return (
    <div className="flex flex-col h-full gap-4">
      <div className="flex items-center justify-between shrink-0">
        <h1 className="text-2xl font-bold text-slate-100">Logs</h1>
        <div className="flex gap-2">
          {status.running ? (
            <button
              onClick={onStop}
              aria-label="Stop node"
              className="flex items-center gap-2 px-4 py-2 bg-[var(--accent-red)]/90 hover:bg-[var(--accent-red)] text-white rounded-lg text-sm font-medium transition-all shadow-lg shadow-red-500/20 border border-[var(--accent-red)]/40"
            >
              <Square className="w-4 h-4" />
              Stop Node
            </button>
          ) : (
            <button
              onClick={onStart}
              disabled={!password}
              aria-label="Start node"
              className="flex items-center gap-2 px-4 py-2 bg-[var(--accent-cyan)]/90 hover:bg-[var(--accent-cyan)] disabled:opacity-40 disabled:cursor-not-allowed text-[var(--bg-primary)] rounded-lg text-sm font-medium transition-all shadow-lg shadow-cyan-500/20 border border-[var(--accent-cyan)]/40"
            >
              <Play className="w-4 h-4" />
              Start Node
            </button>
          )}
          <button
            onClick={handleExport}
            className="flex items-center gap-2 px-4 py-2 bg-[var(--bg-surface)] hover:opacity-90 text-slate-300 rounded-lg text-sm font-medium transition-all"
          >
            <Download className="w-4 h-4" />
            Export
          </button>
        </div>
      </div>

      <div
        ref={containerRef}
        className="bg-[var(--bg-surface)] rounded-xl border border-[var(--bg-elevated)] p-4 flex-1 min-h-0 overflow-y-auto font-mono text-sm"
      >
        {logs.length === 0 ? (
          <div className="flex items-center justify-center h-full text-slate-500">
            No logs yet. Start the node to see output.
          </div>
        ) : (
          <div className="space-y-0.5">
            {logs.map((log, i) => (
              <div key={i} className="flex gap-3 py-0.5 hover:bg-slate-800/50 px-2 rounded">
                <span className="text-slate-500 shrink-0">{log.timestamp}</span>
                <span className={`shrink-0 uppercase font-medium ${levelColors[log.level] || "text-slate-400"}`}>
                  {log.level}
                </span>
                <span className="text-slate-300 break-all">{log.message}</span>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
