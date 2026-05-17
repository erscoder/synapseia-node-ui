import { useState } from "react";
import { AlertTriangle, XCircle, Loader2 } from "lucide-react";
import type { ExternalLockInfo } from "../hooks/useExternalLock";

interface Props {
  lock: ExternalLockInfo;
  /** Kill the lock owner (alive case). Called for cli AND ui sources. */
  onTakeOver: () => Promise<void>;
  /** Clean up a stale (dead-PID) lock file. No process to kill. */
  onCleanStale: () => Promise<void>;
}

/**
 * Format a duration in seconds as a short human label: "12s", "4m", "1h 4m".
 * Caps at hours — no operator is going to leave a node running for days
 * without noticing this banner.
 */
function humaniseAge(seconds: number): string {
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  const remMin = minutes % 60;
  return remMin > 0 ? `${hours}h ${remMin}m` : `${hours}h`;
}

/**
 * Persistent banner shown above the Start button when `~/.synapseia/node.lock`
 * exists. Three semantic states:
 *
 *   1. Alive + source=cli  → amber  "Node already running from CLI"
 *   2. Alive + source=ui   → amber  "Node running in another window"
 *   3. Dead (zombie)       → red    "Stale lock detected"
 *
 * The banner is `role="alert"` so screen readers announce it. The age string
 * is wrapped in `<time>` carrying the ISO timestamp for precise AT output.
 */
export function LockBanner({ lock, onTakeOver, onCleanStale }: Props) {
  // Action-in-flight guard: while a take-over or clean is running we disable
  // the button + show a spinner. Avoids double-fire if the user clicks twice.
  const [busy, setBusy] = useState(false);

  const isStale = !lock.isAlive;
  const isUi = lock.source === "ui";

  // Source-driven copy. We deliberately surface "unknown" alongside "cli"
  // because operators reading the banner think in terms of "did I start
  // this from a terminal or from a window?" — and unknown values almost
  // always come from older CLI versions.
  let title: string;
  let subtext: string;
  let actionLabel: string;
  if (isStale) {
    title = "Stale lock detected";
    subtext = `Previous node (PID ${lock.pid}) crashed without cleanup`;
    actionLabel = "Clean lock";
  } else if (isUi) {
    title = "Node running in another window";
    subtext = `PID ${lock.pid} · running for ${humaniseAge(lock.ageSeconds)}`;
    actionLabel = "Stop other window";
  } else {
    title = "Node already running from CLI";
    subtext = `PID ${lock.pid} · running for ${humaniseAge(lock.ageSeconds)}`;
    actionLabel = "Take over";
  }

  const handleClick = async () => {
    if (busy) return;
    setBusy(true);
    try {
      if (isStale) {
        await onCleanStale();
      } else {
        await onTakeOver();
      }
    } finally {
      setBusy(false);
    }
  };

  // Color tokens — amber for alive (advisory), red for stale (broken state).
  const containerCls = isStale
    ? "border-red-500/40 bg-red-500/10 text-red-200"
    : "border-amber-500/40 bg-amber-500/10 text-amber-200";
  const iconCls = isStale ? "text-red-300" : "text-amber-300";
  const Icon = isStale ? XCircle : AlertTriangle;
  const buttonCls = isStale
    ? "bg-red-500/20 hover:bg-red-500/30 text-red-100 border border-red-500/50"
    : "bg-amber-500/20 hover:bg-amber-500/30 text-amber-100 border border-amber-500/50";

  return (
    <div
      role="alert"
      aria-live="polite"
      className={`flex items-start gap-3 rounded-md border px-4 py-3 ${containerCls}`}
    >
      <Icon className={`mt-0.5 h-5 w-5 flex-shrink-0 ${iconCls}`} aria-hidden="true" />
      <div className="flex-1 min-w-0">
        <p className="text-sm font-semibold">{title}</p>
        <p className="mt-0.5 text-xs opacity-80">
          {/* When alive, the readable age has a precise ISO timestamp under it
              so assistive tech reads the exact start time. For stale locks we
              just keep the plain crashed copy — the timestamp is less useful. */}
          {isStale ? (
            subtext
          ) : (
            <>
              PID {lock.pid} · running for{" "}
              <time dateTime={lock.startedAt}>{humaniseAge(lock.ageSeconds)}</time>
            </>
          )}
        </p>
      </div>
      <button
        type="button"
        onClick={handleClick}
        disabled={busy}
        className={`inline-flex items-center gap-2 rounded px-3 py-1.5 text-xs font-medium transition-colors disabled:opacity-50 ${buttonCls}`}
      >
        {busy && <Loader2 className="h-3.5 w-3.5 animate-spin" aria-hidden="true" />}
        {actionLabel}
      </button>
    </div>
  );
}
