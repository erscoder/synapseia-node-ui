import { listen } from "@tauri-apps/api/event";

// Event contract emitted by the Rust backend (see install_python_deps).
// Kept in sync with `python-install-progress` payload on the Rust side.
export type PythonInstallPhase =
  | "venv"
  | "torch"
  | "lora-stack"
  | "cuda-probe"
  | "bitsandbytes"
  | "docking"
  | "complete"
  | "skipped";

export type PythonInstallStatus = "running" | "done" | "error" | "skip";

export interface PythonInstallProgress {
  phase: PythonInstallPhase;
  status: PythonInstallStatus;
  message: string;
}

// Human-readable labels for every phase. `complete` and `skipped` are
// terminal signals — not rendered as rows — but we keep labels here so an
// out-of-order event still produces something sensible.
const PHASE_LABELS: Record<PythonInstallPhase, string> = {
  venv: "Creating Python venv",
  torch: "Installing PyTorch",
  "lora-stack": "Installing LoRA training stack",
  "cuda-probe": "Probing CUDA",
  bitsandbytes: "Installing bitsandbytes (DiLoCo full-mode)",
  docking: "Installing AutoDock Vina + Open Babel (docking)",
  complete: "Complete",
  skipped: "Python environment ready",
};

const STATUS_ICON: Record<PythonInstallStatus, string> = {
  running: "⟳",
  done: "✓",
  error: "✗",
  skip: "↷",
};

const STATUS_COLOR: Record<PythonInstallStatus, string> = {
  running: "text-emerald-400",
  done: "text-emerald-400",
  error: "text-red-400",
  skip: "text-slate-400",
};

interface Props {
  phases: PythonInstallProgress[];
  // Terminal status carried by the final `complete` event. `null` while
  // we're still waiting for it. When `'skip'` and `phases` is empty the
  // backend short-circuited the whole install — render a compact one-liner.
  terminal: PythonInstallStatus | null;
}

/**
 * Renders the "Python environment" sub-section of the boot loading screen.
 * Mirrors the CLI install section's visual language (slate card, status
 * icon + message per row). State is owned by the parent so subscription
 * lifecycle and the "all done → route forward" decision live in one place.
 */
export function PythonDepsProgress({ phases, terminal }: Props) {
  const isInstantSkip = terminal === "skip" && phases.length === 0;

  if (isInstantSkip) {
    return (
      <div className="w-full max-w-md mt-4 rounded border border-slate-700/60 bg-slate-800/40 px-4 py-3">
        <p className="text-xs text-slate-300">
          <span className="text-emerald-400 mr-2">{STATUS_ICON.skip}</span>
          Python environment ready
        </p>
      </div>
    );
  }

  // Hide while no events have arrived — avoids a flash of an empty section
  // between the CLI install completing and the first python event.
  if (phases.length === 0) return null;

  return (
    <div className="w-full max-w-md mt-4 rounded border border-slate-700/60 bg-slate-800/40 px-4 py-3">
      <p className="text-xs font-semibold text-slate-200 mb-2">Python environment</p>
      <ul className="space-y-1.5">
        {phases.map((p) => (
          <li key={p.phase} className="flex items-start gap-2 text-xs">
            <span
              className={`${STATUS_COLOR[p.status]} ${
                p.status === "running" ? "animate-spin inline-block" : ""
              }`}
              aria-hidden="true"
            >
              {STATUS_ICON[p.status]}
            </span>
            <span className="flex-1">
              <span className="text-slate-200">{PHASE_LABELS[p.phase] ?? p.phase}</span>
              {p.message && <span className="text-slate-400"> — {p.message}</span>}
            </span>
          </li>
        ))}
      </ul>
    </div>
  );
}

/**
 * Subscribe to the Rust `python-install-progress` event stream. Returns
 * the same unlisten promise shape Tauri exposes so the caller can clean
 * up in a `useEffect` return path.
 */
export function subscribePythonInstall(
  onEvent: (e: PythonInstallProgress) => void,
): Promise<() => void> {
  return listen<PythonInstallProgress>("python-install-progress", (event) => {
    onEvent(event.payload);
  });
}

/**
 * Upsert helper. Keyed by phase so the same phase never renders twice
 * even if the backend emits `running` followed by `done` for the same
 * step.
 */
export function upsertPhase(
  prev: PythonInstallProgress[],
  next: PythonInstallProgress,
): PythonInstallProgress[] {
  const idx = prev.findIndex((p) => p.phase === next.phase);
  if (idx === -1) return [...prev, next];
  const clone = prev.slice();
  clone[idx] = next;
  return clone;
}
